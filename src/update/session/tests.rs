use super::lease::*;
use super::pane_events::*;
use super::status::*;
use crate::Msg;
use crate::session::client::SessionClient;
use crate::session::protocol::{ClientInfo, PaneRuntimeState};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tui_lipan::TestBackend;

fn agent_pane(status: &str) -> crate::pane::TerminalPane {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
        agent: crate::session::protocol::AgentIdentity::new("claude", "Claude Code").into(),
        state: crate::session::protocol::DetectedAgentState::Idle,
    });
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: status.to_string(),
        reason: None,
        set_at: 1,
    });
    pane
}

fn install_search_scan(
    backend: &mut TestBackend<crate::AppRoot>,
    target: crate::state::PaneId,
    query: &str,
) -> u64 {
    let pane_end = crate::pane::lifecycle::find_pane(backend.state(), target)
        .expect("search target")
        .terminal
        .search_line_count();
    let state = backend.state_mut();
    let epoch = state.search_scan_epoch.wrapping_add(1);
    state.search_scan_epoch = epoch;
    state.search_scan_scheduled_epoch = Some(epoch);
    let mut search = crate::state::ScrollbackSearchState::new(target);
    search.input.set_text(query);
    search.scan = Some(crate::state::ScrollbackSearchScan {
        epoch,
        query: Arc::from(query),
        panes: Arc::from([target]),
        pane_ends: Arc::from([pane_end]),
        pane_index: 0,
        line_cursor: 0,
        first_jump_done: false,
    });
    search.refresh_match_status();
    state.search = Some(search);
    epoch
}

fn search_output_lines(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| format!("needle-{index}\r\n"))
        .collect::<String>()
        .into_bytes()
}

fn settle_command_link(backend: &mut TestBackend<crate::AppRoot>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while backend.state().command_link.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the mount never delivered the command link"
        );
        backend.pump().expect("settle the mount");
        std::thread::yield_now();
    }
}

#[test]
fn animated_titles_update_live_labels_and_preserve_full_refresh_fallbacks() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        backend.state_mut().sidebar.hide_all();
        backend.state().sidebar_slide.set(0.0);
        let epoch = backend.state().runtime_epoch;
        let target = backend.state().current().focused_pane.unwrap();
        let generation = 42;
        let source = {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(target, generation);
            pane.terminal.process_server_output(b"\x1b]2;old\x07");
            pane.logging = true;
            pane.live_title
                .bind("icon ", "old [log]", None)
                .source
                .unwrap()
        };
        let message = |title: &str| Msg::SessionOutput {
            media: None,
            metadata: crate::session::protocol::OutputMetadata::default(),
            epoch,
            pane_id: target,
            local: false,
            generation,
            bytes: format!("\x1b]2;{title}\x07").into_bytes(),
        };
        assert_eq!(
            backend.update_level(message("new title")).unwrap(),
            tui_lipan::UpdateLevel::TerminalPaint
        );
        let title: String = source
            .snapshot()
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(title.starts_with("icon new title"), "{title}");
        assert!(title.ends_with(" [log]"), "{title}");
        backend.state_mut().sidebar.shown = [true, false];
        assert_eq!(
            backend.update_level(message("sidebar sees this")).unwrap(),
            tui_lipan::UpdateLevel::Full
        );
        backend.state_mut().sidebar.hide_all();
        backend.state().sidebar_slide.set(0.5);
        assert_eq!(
            backend.update_level(message("closing sidebar")).unwrap(),
            tui_lipan::UpdateLevel::Full
        );
        backend.state().sidebar_slide.set(0.0);
        crate::pane::lifecycle::find_pane(backend.state(), target)
            .unwrap()
            .live_title
            .bound
            .set(false);
        assert_eq!(
            backend.update_level(message("border title")).unwrap(),
            tui_lipan::UpdateLevel::Full
        );
    });
}

#[test]
fn pane_reset_starts_the_pane_over_at_the_snapshot_geometry() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let target = backend
            .state()
            .current()
            .focused_pane
            .expect("focused pane");
        let generation = 7;
        {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(target, generation);
            pane.terminal.process_server_output(b"stale transcript");
        }

        let session_cell = tui_lipan::TerminalCellSize::new(14, 32);
        for stale_generation in [generation + 1, generation] {
            backend
                .dispatch(Msg::SessionPaneReset {
                    epoch,
                    pane_id: target,
                    generation: stale_generation,
                    cols: 41,
                    rows: 13,
                    cell: Some(session_cell),
                })
                .expect("dispatch pane reset");
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            let reset = stale_generation == generation;
            assert_eq!(
                pane.terminal
                    .search_scrollback("stale transcript")
                    .is_empty(),
                reset,
                "only the pane's own generation is reset"
            );
            assert_eq!((pane.terminal.cols, pane.terminal.rows) == (41, 13), reset);
            assert_eq!(pane.terminal.cell_size() == session_cell, reset);
        }

        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id: target,
                local: false,
                generation,
                bytes: b"replayed".to_vec(),
            })
            .expect("dispatch replay");
        let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
        assert!(!pane.terminal.search_scrollback("replayed").is_empty());
    });
}

/// A follower's cells can differ from the controller's, whose cell size the child's PTY reports. An
/// image the child sized for the controller must cover the same cells in the follower, not its
/// pixel size measured in the follower's cells.
#[test]
fn a_pane_lays_images_out_in_the_cells_the_server_reports() {
    use base64::Engine as _;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let target = backend
            .state()
            .current()
            .focused_pane
            .expect("focused pane");
        let generation = 4;
        let host_cell = {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(target, generation);
            pane.terminal.cell_size()
        };
        let session_cell = tui_lipan::TerminalCellSize::new(14, 32);
        assert_ne!(
            host_cell, session_cell,
            "the test needs a follower's own cells"
        );

        backend
            .dispatch(Msg::SessionResized {
                epoch,
                pane_id: target,
                local: false,
                generation,
                cols: 40,
                rows: 12,
                cell: Some(session_cell),
            })
            .expect("dispatch resize");
        // 224x256 pixels is 16x8 of the controller's 14x32 cells.
        let payload =
            base64::engine::general_purpose::STANDARD.encode([10u8, 20, 30, 255].repeat(224 * 256));
        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id: target,
                local: false,
                generation,
                bytes: format!("\x1b[H\x1b_Ga=T,f=32,s=224,v=256,i=9,q=2;{payload}\x1b\\")
                    .into_bytes(),
            })
            .expect("dispatch image");

        let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
        assert_eq!(pane.terminal.cell_size(), session_cell);
        let snapshot = pane.terminal.snapshot();
        let placement = snapshot.images.first().expect("image placement");
        assert_eq!((placement.cols, placement.rows), (16, 8));
    });
}

/// What `clear; icat` writes: the clear, then a 40x40 RGBA image in 4096-byte Kitty chunks.
fn clear_then_image() -> (Vec<u8>, Vec<u8>) {
    use base64::Engine as _;

    let payload =
        base64::engine::general_purpose::STANDARD.encode([255u8, 128, 0, 200].repeat(40 * 40));
    let chunks: Vec<_> = payload.as_bytes().chunks(4096).collect();
    let mut image = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            image.extend_from_slice(
                format!("\x1b_Ga=T,f=32,s=40,v=40,i=7,q=2,m={more};").as_bytes(),
            );
        } else {
            image.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        image.extend_from_slice(chunk);
        image.extend_from_slice(b"\x1b\\");
    }
    image.extend_from_slice(b"\r\nimage-done\r\n");
    (b"\x1b[H\x1b[2J\x1b[3J".to_vec(), image)
}

fn png_capture(pane: &crate::pane::TerminalPane) -> String {
    let content = pane
        .with_screen_mut(|screen| {
            crate::pane::capture_screen(
                screen,
                None,
                crate::control::CaptureRender::Png,
                None,
                false,
            )
        })
        .expect("png capture");
    match content {
        crate::control::CaptureContent::Png { png_base64 } => png_base64,
        other => panic!("expected a png, got {other:?}"),
    }
}

/// The session server reads `clear` and the image a program writes after it from the PTY, usually
/// in separate reads, and relays them to the UI as they queue: often as one message. The UI's copy
/// of the pane must keep the image either way, or `capture-pane` through the UI and through the
/// session disagree and the UI never draws it.
#[test]
fn an_image_relayed_in_one_message_with_the_clear_before_it_stays_on_screen() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let target = backend
            .state()
            .current()
            .focused_pane
            .expect("focused pane");
        let generation = 3;
        let (cols, rows, palette) = {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(target, generation);
            (
                pane.terminal.cols,
                pane.terminal.rows,
                pane.terminal.last_palette,
            )
        };
        // Enough earlier output to leave history for `clear` to erase.
        let history = "$ earlier output\r\n".repeat(usize::from(rows) * 2);
        let (clear, image) = clear_then_image();
        let relayed = [history.as_bytes(), &[clear.as_slice(), &image].concat()];
        for bytes in relayed {
            backend
                .dispatch(Msg::SessionOutput {
                    media: None,
                    metadata: crate::session::protocol::OutputMetadata::default(),
                    epoch,
                    pane_id: target,
                    local: false,
                    generation,
                    bytes: bytes.to_vec(),
                })
                .expect("dispatch pane output");
        }

        // The same bytes as the server read them.
        let mut read_apart = crate::pane::TerminalPane::new(100);
        read_apart.apply_server_resize(cols, rows);
        if let Some(palette) = palette {
            read_apart.set_palette(palette);
        }
        for bytes in [history.as_bytes(), &clear, &image] {
            read_apart.process_server_output(bytes);
        }
        let placements = |pane: &crate::pane::TerminalPane| -> Vec<_> {
            let snapshot = pane.snapshot();
            snapshot
                .images
                .iter()
                .map(|image| (image.image_id, image.row, image.col, image.rows, image.cols))
                .collect()
        };
        assert_eq!(placements(&read_apart).len(), 1);

        let pane = crate::pane::lifecycle::find_pane(backend.state(), target).unwrap();
        assert_eq!(
            placements(&pane.terminal),
            placements(&read_apart),
            "the UI's screen lost the image placement"
        );
        assert!(
            png_capture(&pane.terminal) == png_capture(&read_apart),
            "the UI's capture differs from the screen that read the output apart"
        );
    });
}

#[test]
fn background_output_arms_the_alert_pulse_without_the_global_sweep() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        settle_command_link(&mut backend);
        let epoch = backend.state().runtime_epoch;
        let pane_id = 77;
        let generation = 9;
        let mut pane =
            crate::state::Pane::new(pane_id, 100, tui_lipan::prelude::FloatRect::default());
        pane.pty_generation = generation;
        pane.terminal.bind_session(pane_id, generation);
        backend.state_mut().current_mut().workspaces[1]
            .panes
            .push(pane);

        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id,
                local: false,
                generation,
                bytes: vec![7],
            })
            .expect("deliver background bell");

        assert!(
            backend.state().alert_pulse_armed,
            "the output edge must arm the pulse before its reduced epilogue"
        );
    });
}

#[test]
fn pane_output_restarts_partial_search_and_rejects_its_stale_chunk() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let target = backend
            .state()
            .current()
            .focused_pane
            .expect("focused pane");
        let generation = 7;
        {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(target, generation);
            pane.terminal
                .process_server_output(&search_output_lines(700));
        }
        let old_epoch = install_search_scan(&mut backend, target, "needle");
        assert!(matches!(
            crate::ops::search::advance_search_scan(backend.state_mut(), old_epoch, 100),
            crate::ops::search::SearchScanAdvance::Running { .. }
        ));
        assert!(
            !backend
                .state()
                .search
                .as_ref()
                .expect("partial search")
                .matches
                .is_empty()
        );

        let level = backend
            .update_level(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id: target,
                local: false,
                generation,
                bytes: b"live needle\r\n".to_vec(),
            })
            .expect("apply live output");
        assert_eq!(level, tui_lipan::UpdateLevel::Full);
        let restarted_epoch = backend.state().search_scan_epoch;
        assert_ne!(old_epoch, restarted_epoch);
        let search = backend.state().search.as_ref().expect("restarted search");
        assert!(!search.matches.is_empty());
        assert!(search.refresh_matches.as_ref().is_some_and(Vec::is_empty));
        assert_eq!(search.scan.as_ref().expect("scan").epoch, restarted_epoch);
        assert_eq!(
            search.scan.as_ref().expect("scan").panes.as_ref(),
            &[target]
        );
        assert_eq!(
            crate::ops::search::advance_search_scan(backend.state_mut(), old_epoch, 1),
            crate::ops::search::SearchScanAdvance::Stale
        );

        let stale_level = backend
            .update_level(Msg::SearchScanChunk { epoch: old_epoch })
            .expect("deliver stale queued chunk");
        assert_eq!(stale_level, tui_lipan::UpdateLevel::None);
        assert_eq!(
            backend.state().search_scan_scheduled_epoch,
            Some(restarted_epoch)
        );
        backend
            .update_level(Msg::SearchScanChunk { epoch: old_epoch })
            .expect("reject duplicate stale chunk");
        assert_eq!(
            backend.state().search_scan_scheduled_epoch,
            Some(restarted_epoch)
        );

        let offset_before_activation = crate::pane::lifecycle::find_pane(backend.state(), target)
            .unwrap()
            .terminal
            .scrollback_offset();
        backend
            .update_level(Msg::SearchActivate(0))
            .expect("refreshing result is not actionable");
        assert!(backend.state().search.is_some());
        assert_eq!(
            crate::pane::lifecycle::find_pane(backend.state(), target)
                .unwrap()
                .terminal
                .scrollback_offset(),
            offset_before_activation
        );
    });
}

#[test]
fn pane_output_restarts_a_completed_search() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let target = backend
            .state()
            .current()
            .focused_pane
            .expect("focused pane");
        let generation = 9;
        {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(target, generation);
            pane.terminal
                .process_server_output(&search_output_lines(40));
        }
        let old_epoch = install_search_scan(&mut backend, target, "needle");
        loop {
            if matches!(
                crate::ops::search::advance_search_scan(backend.state_mut(), old_epoch, 17),
                crate::ops::search::SearchScanAdvance::Complete { .. }
            ) {
                break;
            }
        }
        backend.state_mut().search_scan_scheduled_epoch = None;
        assert!(
            !backend
                .state()
                .search
                .as_ref()
                .expect("completed search")
                .matches
                .is_empty()
        );
        {
            let search = backend.state_mut().search.as_mut().unwrap();
            search.current = 5;
            search.refresh_match_status();
        }
        let visible_items = Arc::clone(&backend.state().search.as_ref().unwrap().items);

        backend
            .update_level(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id: target,
                local: false,
                generation,
                bytes: b"post-completion\r\n".to_vec(),
            })
            .expect("apply output after completion");
        let restarted_epoch = backend.state().search_scan_epoch;
        assert_ne!(old_epoch, restarted_epoch);
        let search = backend.state().search.as_ref().expect("restarted search");
        assert!(!search.matches.is_empty());
        assert!(search.refresh_matches.as_ref().is_some_and(Vec::is_empty));
        assert_eq!(search.current, 5);
        assert!(Arc::ptr_eq(&search.items, &visible_items));
        assert_eq!(search.scan.as_ref().expect("scan").epoch, restarted_epoch);
        assert_eq!(
            backend.state().search_scan_scheduled_epoch,
            Some(restarted_epoch)
        );

        while backend
            .state()
            .search
            .as_ref()
            .is_some_and(|search| search.scan.is_some())
        {
            let _ = crate::ops::search::advance_search_scan(
                backend.state_mut(),
                restarted_epoch,
                crate::ops::search::SEARCH_LINES_PER_CHUNK,
            );
        }
        assert_eq!(backend.state().search.as_ref().unwrap().current, 5);
    });
}

#[test]
fn bell_events_include_focused_for_current_panes() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let pane_id = backend.state().current().focused_pane.unwrap();
        let events = backend
            .state()
            .event_hub
            .subscribe(Some(HashSet::from([crate::events::EventKind::Bell])));
        let bell = |pane_id: crate::state::PaneId| {
            let mut data = std::collections::BTreeMap::new();
            data.insert("pane".to_string(), pane_id.to_string());
            crate::session::server::events::SessionEvent {
                session_instance: crate::session::protocol::SessionInstanceId::for_test("test"),
                seq: 1,
                kind: crate::events::EventKind::Bell,
                data,
            }
        };
        backend
            .update_level(Msg::SessionEvent {
                epoch,
                event: bell(pane_id),
            })
            .unwrap();
        assert!(events.recv().unwrap().contains("\"focused\":\"true\""));
        backend.state_mut().current_mut().focused_pane = None;
        backend
            .update_level(Msg::SessionEvent {
                epoch,
                event: bell(pane_id),
            })
            .unwrap();
        assert!(events.recv().unwrap().contains("\"focused\":\"false\""));
    });
}

#[test]
fn focused_pane_bell_raises_background_attention_until_a_key_clears_it() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let pane_id = backend
            .state()
            .current()
            .focused_pane
            .expect("fresh pane focus");
        let generation = 42;
        {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), pane_id).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(pane_id, generation);
        }

        backend
            .set_window_focused(false)
            .expect("lose host-window focus");
        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id,
                local: false,
                generation,
                bytes: vec![7],
            })
            .expect("deliver BEL while unfocused");
        let pane = crate::pane::lifecycle::find_pane(backend.state(), pane_id).unwrap();
        assert!(pane.activity.has_unseen_output);
        assert!(pane.activity.bell);

        backend
            .set_window_focused(true)
            .expect("gain host-window focus");
        let pane = crate::pane::lifecycle::find_pane(backend.state(), pane_id).unwrap();
        assert!(pane.activity.has_unseen_output);
        assert!(pane.activity.bell);

        backend
            .dispatch(Msg::PaneKey(
                pane_id,
                tui_lipan::KeyEvent {
                    code: tui_lipan::KeyCode::Char('x'),
                    mods: tui_lipan::KeyMods::NONE,
                },
            ))
            .expect("type into the focused pane");
        let pane = crate::pane::lifecycle::find_pane(backend.state(), pane_id).unwrap();
        assert!(!pane.activity.has_unseen_output);
        assert!(!pane.activity.bell);
    });
}

#[test]
fn agent_bell_leaves_the_mark_to_the_agent_state() {
    assert!(!unwatched_bell_marks(|terminal| {
        terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
            agent: crate::session::protocol::AgentIdentity::new("codex", "Codex").into(),
            state: crate::session::protocol::DetectedAgentState::Idle,
        });
    }));
}

#[test]
fn generic_activity_bell_still_marks_the_pane() {
    assert!(unwatched_bell_marks(|terminal| {
        terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: crate::session::protocol::pane_status::WORKING.into(),
            reason: Some("running tests".into()),
            set_at: 1,
        });
    }));
}

/// Rings BEL in a pane nobody is watching, after `configure` sets up its terminal, and returns
/// whether the bell marked the pane. The bell event and unseen output fire either way.
fn unwatched_bell_marks(
    configure: impl FnOnce(&mut crate::pane::TerminalPane) + Send + 'static,
) -> bool {
    crate::test_support::on_large_stack(move || {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let pane_id = backend
            .state()
            .current()
            .focused_pane
            .expect("fresh pane focus");
        let generation = 42;
        {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), pane_id).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(pane_id, generation);
            configure(&mut pane.terminal);
        }
        backend
            .set_window_focused(false)
            .expect("lose host-window focus");
        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id,
                local: false,
                generation,
                bytes: vec![7],
            })
            .expect("deliver BEL while unfocused");
        let pane = crate::pane::lifecycle::find_pane(backend.state(), pane_id).unwrap();
        assert!(pane.activity.has_unseen_output);
        pane.activity.bell
    })
}

fn finished_record(
    pane_id: crate::state::PaneId,
    generation: u64,
    completions: u64,
) -> crate::session::protocol::AgentRecord {
    crate::session::protocol::AgentRecord {
        reference: crate::session::protocol::AgentRef::for_test(pane_id, generation, None),
        completions,
    }
}

#[test]
fn finished_unseen_survives_background_updates_and_the_return_to_the_window() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let pane_id = backend
            .state()
            .current()
            .focused_pane
            .expect("fresh pane focus");
        let generation = 42;
        {
            let pane = crate::pane::lifecycle::find_pane_mut(backend.state_mut(), pane_id).unwrap();
            pane.pty_generation = generation;
            pane.terminal.bind_session(pane_id, generation);
            pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
                value: crate::session::protocol::pane_status::WORKING.into(),
                reason: None,
                set_at: 1,
            });
            pane.terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
                agent: crate::session::protocol::AgentIdentity::new("claude", "Claude Code").into(),
                state: crate::session::protocol::DetectedAgentState::Idle,
            });
            pane.agents = vec![finished_record(pane_id, generation, 0)];
        }
        let agents = backend.state().current().workspaces[0].panes[0]
            .agents
            .clone();
        backend
            .state_mut()
            .agent_attention
            .observe_records(&agents, crate::state::FirstSighting::Baseline);

        backend
            .set_window_focused(false)
            .expect("lose host-window focus");
        backend
            .dispatch(Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id,
                local: false,
                generation,
                agents: vec![finished_record(pane_id, generation, 1)],
                state: PaneRuntimeState {
                    sequence: 1,
                    status: Some(crate::session::protocol::PaneStatus {
                        value: crate::session::protocol::pane_status::IDLE.into(),
                        reason: None,
                        set_at: 2,
                    }),
                    detected_agent: Some(crate::session::protocol::DetectedAgent {
                        agent: crate::session::protocol::AgentIdentity::new(
                            "claude",
                            "Claude Code",
                        )
                        .into(),
                        state: crate::session::protocol::DetectedAgentState::Idle,
                    }),
                    ..PaneRuntimeState::default()
                },
            })
            .expect("deliver finished edge while unfocused");
        backend
            .dispatch(Msg::WorkbarTick)
            .expect("ordinary post-update pass");
        assert!(
            crate::pane::lifecycle::find_pane(backend.state(), pane_id)
                .expect("finished pane")
                .finished_unseen(&backend.state().agent_attention)
        );

        // The run finished while the user was away, in the pane they were already sitting in.
        // Coming back must not silently swallow that: the mark waits for an actual keystroke.
        backend
            .set_window_focused(true)
            .expect("gain host-window focus");
        backend
            .dispatch(Msg::WorkbarTick)
            .expect("ordinary post-update pass");
        assert!(
            crate::pane::lifecycle::find_pane(backend.state(), pane_id)
                .expect("finished pane")
                .finished_unseen(&backend.state().agent_attention)
        );

        backend
            .dispatch(Msg::PaneKey(
                pane_id,
                tui_lipan::KeyEvent {
                    code: tui_lipan::KeyCode::Char('x'),
                    mods: tui_lipan::KeyMods::NONE,
                },
            ))
            .expect("type into the focused pane");
        assert!(
            !crate::pane::lifecycle::find_pane(backend.state(), pane_id)
                .expect("finished pane")
                .finished_unseen(&backend.state().agent_attention)
        );
    });
}

#[test]
fn parked_disconnect_preserves_identity_and_marks_attachment_offline() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, _outbound) = SessionClient::test_channel();
        let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
        {
            let state = backend.state_mut();
            state.runtime_epoch = 4;
            state.current_mut().epoch = 4;
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().session_client = Some(client);
            state.current_mut().session_attached = true;
            state.current_mut().pending_session_attach = None;
            state.current_mut().connection = crate::state::ConnectionState::Connected;
            state.current_mut().remote_host = Some("workbox".to_string());
            state.current_mut().remote_target = Some(target.clone());
            state.current_mut().remote_drag = Some(crate::state::RemoteDrag {
                pane_id: 4,
                rect: crate::layout::shared::FracRect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
            });
            state.current().remote_drag_snap.set(Some(4));
            state.park_current(4, crate::state::Attachment::new());
            state.runtime_epoch = 5;
        }

        assert_eq!(backend.state().runtime_epoch, 5);
        let before = backend
            .state()
            .background
            .get(&4)
            .expect("parked before drop");
        assert_eq!(before.session_name.as_deref(), Some("dev"));
        assert!(before.pending_session_attach.is_none());

        backend
            .dispatch(Msg::SessionDisconnected {
                epoch: 4,
                name: "dev".to_string(),
            })
            .expect("dispatch parked disconnect");

        let parked = backend
            .state()
            .background
            .get(&4)
            .expect("retained session");
        assert_eq!(
            parked.connection,
            crate::state::ConnectionState::Disconnected
        );
        assert!(!parked.session_attached);
        assert!(parked.session_client.is_none());
        assert_eq!(parked.remote_drag, None);
        assert_eq!(parked.remote_drag_snap.get(), None);
        assert_eq!(parked.remote_target.as_ref(), Some(&target));
        assert_eq!(parked.session_name.as_deref(), Some("dev"));
    });
}

#[test]
fn parked_rename_updates_retained_identity() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let state = backend.state_mut();
            state.runtime_epoch = 8;
            state.current_mut().session_name = Some("before".to_string());
            state.park_current(8, crate::state::Attachment::new());
            state.runtime_epoch = 9;
        }

        backend
            .dispatch(Msg::SessionRenamed {
                epoch: 8,
                session: "after".to_string(),
            })
            .expect("dispatch parked rename");

        assert_eq!(
            backend
                .state()
                .background
                .get(&8)
                .and_then(|attachment| attachment.session_name.as_deref()),
            Some("after")
        );
    });
}

/// Naming the current temporary session from the picker reopens the picker before the server
/// confirms the rename. The confirmation relabels its row right away, highlight included, rather
/// than leaving the temporary name up until the next discovery sweep.
#[test]
fn current_rename_relabels_open_session_picker_row() {
    crate::test_support::on_large_stack(|| {
        let _persisted = crate::test_support::lock_persisted_state();
        let ephemeral = crate::state::ephemeral_session_name();
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let state = backend.state_mut();
            state.runtime_epoch = 3;
            state.current_mut().session_name = Some(ephemeral.clone());
            let current = crate::ops::session::discovery::attachment_session_row(state.current())
                .expect("current session row");
            let other = crate::session::discovery::DiscoveredSession {
                name: "alpha".to_string(),
                ephemeral: false,
                ..current.clone()
            };
            state.sidebar.sessions = vec![other.clone(), current.clone()];
            let mut picker = crate::state::SessionPickerState::new(vec![other, current]);
            picker.selected = 1;
            state.session_picker = Some(picker);
            state.show_session_picker = true;
        }

        backend
            .dispatch(Msg::SessionRenamed {
                epoch: 3,
                session: "work".to_string(),
            })
            .expect("dispatch current rename");

        let state = backend.state();
        let picker = state.session_picker.as_ref().expect("picker stays open");
        let names: Vec<_> = picker.entries.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["work", "alpha"]);
        assert!(!picker.entries[0].ephemeral);
        assert_eq!(picker.selected, 0);
        let sidebar: Vec<_> = state
            .sidebar
            .sessions
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(sidebar, ["alpha", "work"]);
    });
}

/// The rule a host monitor's summaries are graded against, where there is no pane to consult.
/// The two edges it must never invent are the ones that would fire on every poll of a machine
/// that has simply been sitting there: a standing prompt, and an agent that was already idle.
#[test]
fn summary_edges_report_transitions_rather_than_standing_states() {
    use crate::session::protocol::{AgentRef, AgentState, AgentSummary};
    let summary = |state, completions| AgentSummary {
        session: "dev".into(),
        reference: AgentRef::for_test(1, 1, None),
        agent: "codex".into(),
        label: "Codex".into(),
        state,
        reason: None,
        task: None,
        completions,
        changed_at: 0,
    };
    let edges = |previous: Option<(AgentState, u64)>, current: (AgentState, u64)| {
        let previous = previous.map(|(state, completions)| summary(state, completions));
        summary_edges(previous.as_ref(), &summary(current.0, current.1))
    };

    let blocked = edges(Some((AgentState::Working, 0)), (AgentState::Blocked, 0));
    assert!(blocked.became_blocked);
    assert!(!blocked.finished);
    // Still blocked on the next poll. The prompt is not new, so neither is the news.
    assert!(!edges(Some((AgentState::Blocked, 0)), (AgentState::Blocked, 0)).became_blocked);
    // A first sighting of an agent that is already blocked *is* an edge by this rule; the caller
    // is what keeps a freshly connected host quiet, by seeding its baseline without alerting.
    assert!(edges(None, (AgentState::Blocked, 0)).became_blocked);

    // A finish is a rise in the server's count, whether or not a poll ever saw the agent working.
    assert!(edges(Some((AgentState::Idle, 2)), (AgentState::Idle, 3)).finished);
    assert!(edges(Some((AgentState::Working, 0)), (AgentState::Done, 1)).finished);
    // An agent that was already quiet has not just finished anything.
    assert!(!edges(Some((AgentState::Idle, 1)), (AgentState::Idle, 1)).finished);
    assert!(!edges(None, (AgentState::Done, 4)).finished);
    // Blocking is not finishing, and neither is carrying on working.
    assert!(!edges(Some((AgentState::Working, 0)), (AgentState::Blocked, 0)).finished);

    // A failure is news once, like a prompt: when it happens, not on every poll it lasts.
    let failed = edges(Some((AgentState::Working, 0)), (AgentState::Error, 0));
    assert!(failed.failed);
    assert!(!failed.finished);
    assert!(!edges(Some((AgentState::Error, 0)), (AgentState::Error, 0)).failed);
    assert!(!edges(Some((AgentState::Working, 0)), (AgentState::Done, 1)).failed);
}

/// An agent pane reports the finish the server counted and keeps no flag of its own: whether
/// anyone has looked is `AgentAttention`'s.
#[test]
fn an_agent_pane_reports_the_counted_finish_and_keeps_no_flag() {
    let mut pane = agent_pane("idle");
    let edges = update_agent_status_edge(&mut pane, Some("working"), None, false, true);
    assert!(edges.finished);
    assert!(!pane.finished_unseen);
    let quiet = update_agent_status_edge(&mut pane, Some("idle"), None, false, false);
    assert!(!quiet.finished);
}

/// The duration column is only honest if the stamp moves on a real state change and holds
/// still across the repeated polls that report the same state.
#[test]
fn status_since_stamps_transitions_and_survives_unchanged_polls() {
    let mut pane = agent_pane("working");
    assert!(pane.status_since.is_none());
    update_agent_status_edge(&mut pane, Some("idle"), None, false, false);
    let stamped = pane.status_since.expect("a transition stamps the pane");

    update_agent_status_edge(&mut pane, Some("working"), None, false, false);
    assert_eq!(pane.status_since, Some(stamped));

    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: "idle".into(),
        reason: None,
        set_at: 2,
    });
    update_agent_status_edge(&mut pane, Some("working"), None, false, false);
    assert!(pane.status_since.expect("re-stamped") > stamped);
}

/// A finished run reports what it cost. The number is banked as the run ends and never moves
/// again, so it cannot drift into meaning "how long ago it stopped".
#[test]
fn finishing_a_run_banks_its_length_and_freezes_it() {
    let run = std::time::Duration::from_secs(12 * 60);
    let mut pane = agent_pane("idle");
    update_agent_status_edge(&mut pane, Some("working"), Some(run), false, false);
    assert_eq!(pane.last_run, Some(run));

    // Repeated idle polls afterwards leave the banked run alone.
    update_agent_status_edge(
        &mut pane,
        Some("idle"),
        Some(std::time::Duration::from_secs(1)),
        false,
        false,
    );
    assert_eq!(pane.last_run, Some(run));

    // Only a `working` stretch is banked; leaving any other state does not overwrite it.
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: "working".into(),
        reason: None,
        set_at: 2,
    });
    update_agent_status_edge(
        &mut pane,
        Some("blocked"),
        Some(std::time::Duration::from_secs(3)),
        false,
        false,
    );
    assert_eq!(pane.last_run, Some(run));
}

#[test]
fn agent_edges_report_detected_only_blocked() {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
        agent: crate::session::protocol::AgentIdentity::new("claude", "Claude Code").into(),
        state: crate::session::protocol::DetectedAgentState::Blocked,
    });

    let edges = update_agent_status_edge(&mut pane, Some("idle"), None, false, false);

    assert!(edges.became_blocked);
    assert!(!edges.finished);
    assert!(pane.reported_status.is_none());
}

#[test]
fn agent_edges_report_reported_only_blocked_once() {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: crate::session::protocol::pane_status::BLOCKED.into(),
        reason: None,
        set_at: 1,
    });

    let first = update_agent_status_edge(&mut pane, None, None, false, false);
    let repeated = update_agent_status_edge(&mut pane, None, None, true, false);

    assert!(first.became_blocked);
    assert!(!repeated.became_blocked);
    assert!(pane.detected_agent.is_none());
}

/// The hooks follow one conversation; another one the publisher reports blocked is the same news,
/// so its `working -> blocked` transition must raise the edge that notifies and plays the sound.
#[test]
fn a_conversation_the_hooks_do_not_follow_raises_the_blocked_edge() {
    use crate::session::protocol::{
        AgentIdentity, AgentIntegrationReport, AgentRecord, AgentRef, AgentState, PublishedRow,
        PublisherKind, RowKey,
    };
    let row = |id: &str, status: &str, active: bool| PublishedRow {
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: id.into(),
        title: id.into(),
        status: status.into(),
        reason: None,
        active,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some(format!("native-{id}")),
    };
    let runtime = |behind: &str| PaneRuntimeState {
        rows: vec![row("shown", "idle", true), row("behind", behind, false)],
        integration: Some(Box::new(AgentIntegrationReport {
            integration: "hook".into(),
            identity: AgentIdentity::new("claude", "Claude Code"),
            reference: AgentRef::for_test(1, 1, None),
            state: AgentState::Done,
            reason: None,
            native_session: Some("native-shown".into()),
            seq: 1,
            reported_at_unix_ms: 1,
        })),
        ..PaneRuntimeState::default()
    };
    let records = || -> Vec<AgentRecord> {
        ["shown", "behind"]
            .into_iter()
            .map(|id| AgentRef::for_test(1, 1, Some(RowKey::new("sessions", id))).into())
            .collect()
    };
    let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());

    let working = apply_pane_runtime_state(&mut pane, records(), runtime("working"));
    assert!(!working.edges.became_blocked);
    let blocked = apply_pane_runtime_state(&mut pane, records(), runtime("blocked"));
    assert!(!pane.terminal.is_blocked(), "the hooks' row is done");
    assert!(blocked.edges.became_blocked);
    let standing = apply_pane_runtime_state(&mut pane, records(), runtime("blocked"));
    assert!(
        !standing.edges.became_blocked,
        "a standing prompt is not new"
    );
}

/// A conversation that fails behind the one on screen marks its pane once. Looking at the pane
/// answers it for as long as that failure lasts, so the mark does not come back every time focus
/// leaves; another failure, or the same conversation failing again later, raises it again.
#[test]
fn a_failure_marks_its_pane_until_seen_and_again_only_when_new() {
    use crate::session::protocol::{AgentRecord, AgentRef, PublishedRow, PublisherKind, RowKey};
    let row = |id: &str, status: &str, active: bool| PublishedRow {
        publisher: "threads".into(),
        kind: PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: id.into(),
        title: id.into(),
        status: status.into(),
        reason: None,
        active,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some(format!("native-{id}")),
    };
    let runtime = |first: &str, second: &str| PaneRuntimeState {
        rows: vec![
            row("shown", "idle", true),
            row("first", first, false),
            row("second", second, false),
        ],
        ..PaneRuntimeState::default()
    };
    let records = || -> Vec<AgentRecord> {
        ["shown", "first", "second"]
            .into_iter()
            .map(|id| AgentRef::for_test(1, 1, Some(RowKey::new("threads", id))).into())
            .collect()
    };
    let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());
    let apply = |pane: &mut crate::state::Pane, first: &str, second: &str| {
        apply_pane_runtime_state(pane, records(), runtime(first, second));
        pane.terminal.error_unseen
    };

    assert!(!apply(&mut pane, "working", "idle"));
    assert!(apply(&mut pane, "error", "idle"), "a failure is news");
    // Looked at.
    pane.terminal.error_unseen = false;
    assert!(
        !apply(&mut pane, "error", "idle"),
        "the same failure is not news again"
    );
    assert!(
        apply(&mut pane, "error", "error"),
        "another conversation failing is"
    );
    pane.terminal.error_unseen = false;
    assert!(!apply(&mut pane, "working", "idle"));
    assert!(
        apply(&mut pane, "error", "idle"),
        "failing again after recovering is news"
    );
    assert!(!apply(&mut pane, "done", "idle"), "nothing failed any more");
}

/// The hooks and the publisher speak for one conversation independently, and either can be first.
/// A hook report confirming a failure the pane already showed and the user already saw is the same
/// failure, not a new one.
#[test]
fn a_hook_report_of_a_seen_failure_does_not_raise_it_again() {
    use crate::session::protocol::{
        AgentIdentity, AgentIntegrationReport, AgentRecord, AgentRef, AgentState, PublishedRow,
        PublisherKind, RowKey,
    };
    let row = |status: &str| PublishedRow {
        publisher: "threads".into(),
        kind: PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: "conversation".into(),
        title: "conversation".into(),
        status: status.into(),
        reason: None,
        active: false,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some("native-conversation".into()),
    };
    let runtime = |status: &str, hooks: Option<AgentState>| PaneRuntimeState {
        rows: vec![row(status)],
        integration: hooks.map(|state| {
            Box::new(AgentIntegrationReport {
                integration: "hook".into(),
                identity: AgentIdentity::new("codex", "Codex"),
                reference: AgentRef::for_test(1, 1, None),
                state,
                reason: None,
                native_session: Some("native-conversation".into()),
                seq: 1,
                reported_at_unix_ms: 1,
            })
        }),
        ..PaneRuntimeState::default()
    };
    let records = || -> Vec<AgentRecord> {
        vec![AgentRef::for_test(1, 1, Some(RowKey::new("threads", "conversation"))).into()]
    };
    let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());

    apply_pane_runtime_state(&mut pane, records(), runtime("working", None));
    apply_pane_runtime_state(&mut pane, records(), runtime("error", None));
    assert!(pane.terminal.error_unseen);
    // Looked at.
    pane.terminal.error_unseen = false;
    let reported = apply_pane_runtime_state(
        &mut pane,
        records(),
        runtime("error", Some(AgentState::Error)),
    );
    assert!(
        reported.failed_rows.is_empty() && !reported.edges.failed,
        "nothing new to announce"
    );
    assert_eq!(
        pane.failures()
            .into_iter()
            .map(|failure| failure.slot)
            .collect::<Vec<_>>(),
        [Some(RowKey::new("threads", "conversation"))]
    );
    assert!(
        !pane.terminal.error_unseen,
        "the same failure, reported twice"
    );
}

/// A failure is announced once, as the alert is raised: a conversation behind the one on screen by
/// its own name, whatever the pane's attendance, and the one on screen as the pane's edge.
#[test]
fn a_new_failure_is_announced_once_where_it_happened() {
    use crate::session::protocol::{AgentRecord, AgentRef, PublishedRow, PublisherKind, RowKey};
    let row = |id: &str, status: &str, active: bool| PublishedRow {
        publisher: "threads".into(),
        kind: PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: id.into(),
        title: id.into(),
        status: status.into(),
        reason: None,
        active,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some(format!("native-{id}")),
    };
    let runtime = |shown: &str, behind: &str| PaneRuntimeState {
        rows: vec![row("shown", shown, true), row("behind", behind, false)],
        ..PaneRuntimeState::default()
    };
    let records = || -> Vec<AgentRecord> {
        ["shown", "behind"]
            .into_iter()
            .map(|id| AgentRef::for_test(1, 1, Some(RowKey::new("threads", id))).into())
            .collect()
    };
    let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());
    let mut apply = |shown: &str, behind: &str| {
        let applied = apply_pane_runtime_state(&mut pane, records(), runtime(shown, behind));
        (applied.failed_rows, applied.edges.failed)
    };

    assert_eq!(apply("working", "working"), (vec![], false));
    assert_eq!(
        apply("working", "error"),
        (vec![RowKey::new("threads", "behind")], false),
        "the conversation behind, by name"
    );
    assert_eq!(apply("working", "error"), (vec![], false), "announced once");
    assert_eq!(
        apply("error", "error"),
        (vec![], true),
        "the one on screen fails the pane"
    );
}

/// The hooks speak for the pane while no row lists their conversation, and for its row once one
/// does. A standing failure is the same failure on either side of that, whichever way it moves.
#[test]
fn a_standing_hook_failure_is_not_new_when_its_row_comes_or_goes() {
    use crate::session::protocol::{
        AgentIdentity, AgentIntegrationReport, AgentRecord, AgentRef, AgentState, PublishedRow,
        PublisherKind, RowKey,
    };
    let runtime = |listed: bool| PaneRuntimeState {
        rows: listed
            .then(|| PublishedRow {
                publisher: "threads".into(),
                kind: PublisherKind::Agent,
                live_session: None,
                reported: false,
                id: "conversation".into(),
                title: "conversation".into(),
                status: "error".into(),
                reason: None,
                active: true,
                work_started_at: None,
                cwd: None,
                project: None,
                native_session: Some("abc".into()),
            })
            .into_iter()
            .collect(),
        integration: Some(Box::new(AgentIntegrationReport {
            integration: "hook".into(),
            identity: AgentIdentity::new("codex", "Codex"),
            reference: AgentRef::for_test(1, 1, None),
            state: AgentState::Error,
            reason: None,
            native_session: Some("abc".into()),
            seq: 1,
            reported_at_unix_ms: 1,
        })),
        ..PaneRuntimeState::default()
    };
    let records = |listed: bool| -> Vec<AgentRecord> {
        let slot = listed.then(|| RowKey::new("threads", "conversation"));
        vec![AgentRef::for_test(1, 1, slot).into()]
    };
    for (first, then) in [(false, true), (true, false)] {
        let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());
        apply_pane_runtime_state(&mut pane, records(first), runtime(first));
        assert!(pane.terminal.error_unseen);
        // Looked at.
        pane.terminal.error_unseen = false;
        let moved = apply_pane_runtime_state(&mut pane, records(then), runtime(then));
        assert_eq!(pane.failures().len(), 1);
        assert!(
            !pane.terminal.error_unseen && !moved.edges.failed && moved.failed_rows.is_empty(),
            "listed {first} then {then}: the same failure"
        );
    }
}

/// A conversation that moves to another process keeps its row and first ID, and names the new
/// process as `live_session`. A standing failure is the same failure across that move, whether the
/// row or a hook report under the new process carries it.
#[test]
fn a_standing_failure_is_not_new_when_its_conversation_moves_process() {
    use crate::session::protocol::{
        AgentIdentity, AgentIntegrationReport, AgentRecord, AgentRef, AgentState, PublishedRow,
        PublisherKind, RowKey,
    };
    let row = |live: Option<&str>| PublishedRow {
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        live_session: live.map(str::to_string),
        reported: false,
        id: "origin".into(),
        title: "origin".into(),
        status: "error".into(),
        reason: None,
        active: false,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some("origin".into()),
    };
    let hook = || {
        Box::new(AgentIntegrationReport {
            integration: "hook".into(),
            identity: AgentIdentity::new("claude", "Claude Code"),
            reference: AgentRef::for_test(1, 1, None),
            state: AgentState::Error,
            reason: None,
            native_session: Some("fork".into()),
            seq: 1,
            reported_at_unix_ms: 1,
        })
    };
    let listed = || -> Vec<AgentRecord> {
        vec![AgentRef::for_test(1, 1, Some(RowKey::new("sessions", "origin"))).into()]
    };
    let alone = || -> Vec<AgentRecord> { vec![AgentRef::for_test(1, 1, None).into()] };
    let steps: [(Vec<AgentRecord>, PaneRuntimeState); 3] = [
        (
            listed(),
            PaneRuntimeState {
                rows: vec![row(Some("fork"))],
                ..PaneRuntimeState::default()
            },
        ),
        (
            listed(),
            PaneRuntimeState {
                rows: vec![row(Some("fork"))],
                integration: Some(hook()),
                ..PaneRuntimeState::default()
            },
        ),
        (
            alone(),
            PaneRuntimeState {
                integration: Some(hook()),
                ..PaneRuntimeState::default()
            },
        ),
    ];
    let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());
    apply_pane_runtime_state(
        &mut pane,
        listed(),
        PaneRuntimeState {
            rows: vec![row(None)],
            ..PaneRuntimeState::default()
        },
    );
    assert!(pane.terminal.error_unseen);
    // Looked at.
    pane.terminal.error_unseen = false;
    for (step, (records, runtime)) in steps.into_iter().enumerate() {
        let applied = apply_pane_runtime_state(&mut pane, records, runtime);
        assert!(
            !pane.terminal.error_unseen && !applied.edges.failed && applied.failed_rows.is_empty(),
            "step {step}: the same failure"
        );
    }
}

/// Two publishers can list one conversation, one showing it and one not. When it fails, that is
/// one failure, announced once, as the one on screen.
#[test]
fn a_conversation_two_publishers_list_fails_once() {
    use crate::session::protocol::{AgentRecord, AgentRef, PublishedRow, PublisherKind, RowKey};
    let row = |publisher: &str, status: &str, active: bool| PublishedRow {
        publisher: publisher.into(),
        kind: PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: "conversation".into(),
        title: "conversation".into(),
        status: status.into(),
        reason: None,
        active,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some("abc".into()),
    };
    let runtime = |status: &str| PaneRuntimeState {
        rows: vec![row("first", status, false), row("second", status, true)],
        ..PaneRuntimeState::default()
    };
    let records = || -> Vec<AgentRecord> {
        ["first", "second"]
            .into_iter()
            .map(|publisher| {
                AgentRef::for_test(1, 1, Some(RowKey::new(publisher, "conversation"))).into()
            })
            .collect()
    };
    let mut pane = crate::state::Pane::new(1, 100, tui_lipan::prelude::FloatRect::default());
    apply_pane_runtime_state(&mut pane, records(), runtime("working"));
    let failed = apply_pane_runtime_state(&mut pane, records(), runtime("error"));
    assert_eq!(pane.failures().len(), 2, "it shows in both places");
    assert!(pane.terminal.error_unseen);
    assert!(
        failed.edges.failed && failed.failed_rows.is_empty(),
        "announced once, as the row on screen"
    );
}

#[test]
fn hold_on_exit_excludes_disabled_and_closing_panes() {
    assert!(should_hold_on_exit(true, false));
    assert!(!should_hold_on_exit(false, false));
    assert!(
        !should_hold_on_exit(true, true),
        "a pane the user closed must not hold, or its own close would keep it alive"
    );
}

#[test]
fn parked_non_controller_defers_exit_until_control_returns() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let mut attachment = crate::state::Attachment::new();
        attachment.epoch = 9;
        let mut pane = crate::state::Pane::new(4, 100, tui_lipan::prelude::FloatRect::default());
        pane.pty_generation = 7;
        attachment.workspaces[0].panes.push(pane);
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.controller = Some(2);
        attachment.shared = Some(shared);
        backend.state_mut().background.insert(9, attachment);

        backend
            .dispatch(Msg::SessionExited {
                epoch: 9,
                pane_id: 4,
                local: false,
                generation: 7,
                code: 0,
            })
            .expect("parked exit");

        assert_eq!(
            backend.state().background[&9].pending_background_closes,
            vec![(4, 7)]
        );
    });
}

/// A follower attached to a session whose lease belongs to client 2.
fn follower_backend() -> TestBackend<crate::AppRoot> {
    let mut backend = TestBackend::new(crate::AppRoot::default());
    let state = backend.state_mut();
    state.current_mut().session_attached = true;
    let mut shared = crate::state::SharedSessionState::new(1);
    shared.controller = Some(2);
    state.current_mut().shared = Some(shared);
    backend
}

fn drag_at(x: f32) -> crate::state::RemoteDrag {
    crate::state::RemoteDrag {
        pane_id: 4,
        rect: crate::layout::shared::FracRect {
            x,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        },
    }
}

#[test]
fn a_relayed_drag_is_mirrored_and_cleared_by_its_own_end() {
    crate::test_support::on_large_stack(|| {
        let mut backend = follower_backend();

        assert_eq!(
            backend
                .update_level(Msg::SessionDragChanged {
                    epoch: 0,
                    author: 2,
                    drag: Some(drag_at(0.1)),
                })
                .expect("mirror the controller's lift"),
            tui_lipan::UpdateLevel::Full,
            "a foreground drag update must redraw the current attachment"
        );
        assert_eq!(backend.state().current().remote_drag, Some(drag_at(0.1)));

        assert_eq!(
            backend
                .update_level(Msg::SessionDragChanged {
                    epoch: 0,
                    author: 2,
                    drag: None,
                })
                .expect("the gesture ends"),
            tui_lipan::UpdateLevel::Full,
            "a foreground drag clear must redraw the current attachment"
        );
        assert_eq!(backend.state().current().remote_drag, None);
    });
}

/// The relay is one broadcast to everyone, author included. Adopting its own echo would make the
/// controller carry a copy of its pane one round trip behind the pointer.
#[test]
fn a_client_ignores_the_echo_of_its_own_drag() {
    crate::test_support::on_large_stack(|| {
        let mut backend = follower_backend();
        backend
            .state_mut()
            .current_mut()
            .shared
            .as_mut()
            .unwrap()
            .controller = Some(1);

        backend
            .dispatch(Msg::SessionDragChanged {
                epoch: 0,
                author: 1,
                drag: Some(drag_at(0.1)),
            })
            .expect("own echo");

        assert_eq!(backend.state().current().remote_drag, None);
    });
}

/// The backstop for a `DragEnd` that never arrives - a controller killed with a pane still lifted.
/// Every disconnect, takeover, and release already broadcasts `ControllerChanged`, so the pane
/// falls back into the authoritative layout instead of hanging over it for the rest of the session.
#[test]
fn losing_the_controller_stops_lifting_whatever_it_was_carrying() {
    crate::test_support::on_large_stack(|| {
        let mut backend = follower_backend();
        backend.state_mut().current_mut().remote_drag = Some(drag_at(0.1));

        backend
            .dispatch(Msg::SessionControllerChanged {
                epoch: 0,
                controller: Some(3),
                reason: crate::session::protocol::ControllerChangeReason::Expired,
            })
            .expect("the controller dropped mid-gesture");

        assert_eq!(
            backend.state().current().remote_drag,
            None,
            "a lift with nobody behind it must not outlive its controller"
        );
    });
}

#[test]
fn remote_drag_presence_stays_with_the_parked_attachment_and_clears_there() {
    crate::test_support::on_large_stack(|| {
        let mut backend = follower_backend();
        let mut parked = crate::state::Attachment::new();
        parked.epoch = 9;
        parked.session_attached = true;
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.controller = Some(2);
        parked.shared = Some(shared);
        backend.state_mut().background.insert(9, parked);

        assert_eq!(
            backend
                .update_level(Msg::SessionDragChanged {
                    epoch: 9,
                    author: 2,
                    drag: Some(drag_at(0.1)),
                })
                .expect("drag for another attachment"),
            tui_lipan::UpdateLevel::None,
            "a background drag update must not redraw the foreground"
        );

        assert_eq!(backend.state().current().remote_drag, None);
        assert_eq!(
            backend.state().background[&9].remote_drag,
            Some(drag_at(0.1))
        );
        assert_eq!(
            backend
                .update_level(Msg::SessionDragChanged {
                    epoch: 9,
                    author: 2,
                    drag: None,
                })
                .expect("clear the parked attachment's drag"),
            tui_lipan::UpdateLevel::None,
            "clearing a background drag must not redraw the foreground"
        );
        assert_eq!(backend.state().background[&9].remote_drag, None);

        backend.state_mut().current_mut().remote_drag = Some(drag_at(0.2));
        backend
            .state_mut()
            .park_current(0, crate::state::Attachment::new());
        backend.state_mut().runtime_epoch = 1;
        assert_eq!(
            backend.state().background[&0].remote_drag,
            Some(drag_at(0.2))
        );
        backend
            .dispatch(Msg::SessionControllerChanged {
                epoch: 0,
                controller: Some(3),
                reason: crate::session::protocol::ControllerChangeReason::Expired,
            })
            .expect("clear the parked attachment's stale drag");
        assert_eq!(backend.state().background[&0].remote_drag, None);
        let restored = backend
            .state_mut()
            .unpark(0, 1)
            .expect("switch back to the parked attachment");
        backend.state_mut().runtime_epoch = restored;
        assert_eq!(backend.state().current().remote_drag, None);
    });
}

#[test]
fn first_relayed_lift_arms_tile_animation_and_abnormal_clear_snaps_the_pane() {
    crate::test_support::on_large_stack(|| {
        let mut backend = follower_backend();
        {
            let workspace = backend.state_mut().current_mut().active_workspace_mut();
            workspace.panes.clear();
            workspace.panes.extend((1..=2).map(|id| {
                crate::state::Pane::new(id, 100, tui_lipan::prelude::FloatRect::default())
            }));
        }
        let mut drag = drag_at(0.1);
        drag.pane_id = 1;
        backend
            .dispatch(Msg::SessionDragChanged {
                epoch: 0,
                author: 2,
                drag: Some(drag),
            })
            .expect("relay the first lift");
        assert_eq!(
            backend.state().animation,
            crate::layout::anim::GeometryAnimation::TileFloat
        );
        let carried = &backend.state().current().workspaces[0].panes[0];
        let neighbour = &backend.state().current().workspaces[0].panes[1];
        assert!(!crate::layout::anim::geometry_animation_enabled(
            backend.state(),
            carried,
            false
        ));
        assert!(crate::layout::anim::geometry_animation_enabled(
            backend.state(),
            neighbour,
            false
        ));

        backend
            .update_level(Msg::SessionControllerChanged {
                epoch: 0,
                controller: Some(3),
                reason: crate::session::protocol::ControllerChangeReason::Expired,
            })
            .expect("clear the abandoned lift");
        assert_eq!(backend.state().current().remote_drag, None);
        assert_eq!(
            backend.state().animation,
            crate::layout::anim::GeometryAnimation::TileFloat
        );
        let carried = &backend.state().current().workspaces[0].panes[0];
        let neighbour = &backend.state().current().workspaces[0].panes[1];
        assert!(!crate::layout::anim::geometry_animation_enabled(
            backend.state(),
            carried,
            false
        ));
        assert!(crate::layout::anim::geometry_animation_enabled(
            backend.state(),
            neighbour,
            false
        ));
        backend.render();
        assert_eq!(
            backend.state().current().remote_drag_snap.get(),
            None,
            "the abnormal-clear snap marker lasts for one rendered frame"
        );
    });
}

#[test]
fn disconnect_cancels_pointer_gestures_and_reconnect_flushes_pending_resizes() {
    // Renders the whole app, a view tree deeper than the default test stack holds.
    crate::test_support::on_large_stack(
        disconnect_cancels_pointer_gestures_and_reconnect_flushes_pending_resizes_body,
    );
}

fn disconnect_cancels_pointer_gestures_and_reconnect_flushes_pending_resizes_body() {
    let mut backend = follower_backend();
    let (old_client, _old_rx) = SessionClient::test_channel();
    {
        let state = backend.state_mut();
        state.current_mut().session_name = Some("dev".to_string());
        state.current_mut().session_client = Some(old_client);
        state.current_mut().session_attached = true;
        state.current_mut().pending_session_attach = None;
        state.current_mut().shared.as_mut().unwrap().controller = Some(1);
        state.current_mut().workspaces[0]
            .panes
            .push(crate::state::Pane::new(
                2,
                100,
                tui_lipan::prelude::FloatRect::default(),
            ));
        state.moving_pane = Some(crate::state::MoveSession {
            id: 1,
            was_floating: false,
            drag_rect: tui_lipan::prelude::FloatRect::default(),
            pointer_x: 0,
            pointer_y: 0,
        });
    }
    backend
        .update_level(Msg::PaneResize(1, 80, 20))
        .expect("record a resize reported mid-drag");
    backend
        .update_level(Msg::PaneResize(2, 90, 22))
        .expect("record a neighbour's resize");
    backend.state_mut().resizing_pane = Some(crate::state::ResizeSession {
        id: 1,
        corner: crate::state::ResizeCorner::LowerRight,
        workspace: crate::state::LayoutTarget::Workspace(0),
        start_x: 0,
        start_y: 0,
        start_tile_tree: None,
        start_split_ratios: Vec::new(),
        start_floating_rect: None,
        start_scrollable_width: None,
        start_scratch_height: None,
    });
    backend.state_mut().split_drag = Some(crate::state::SplitDragSession {
        kind: crate::state::SplitDragKind::Single {
            pane_id: 1,
            horizontal_split: true,
        },
        workspace: crate::state::LayoutTarget::Workspace(0),
        start_x: 0,
        start_y: 0,
        start_tile_tree: None,
        start_split_ratios: Vec::new(),
    });

    let disconnect_epoch = backend.state().runtime_epoch;
    assert_eq!(
        backend.state().current().session_name.as_deref(),
        Some("dev")
    );
    assert!(backend.state().current().pending_session_attach.is_none());
    backend
        .update_level(Msg::SessionDisconnected {
            epoch: disconnect_epoch,
            name: "dev".to_string(),
        })
        .expect("disconnect and begin reconnect");

    let reconnect_epoch = backend.state().runtime_epoch;
    assert_ne!(reconnect_epoch, disconnect_epoch);
    assert!(backend.state().moving_pane.is_none());
    assert!(backend.state().resizing_pane.is_none());
    assert!(backend.state().split_drag.is_none());
    assert_eq!(
        backend.state().current().pending_resizes.len(),
        2,
        "pending resizes survive the reconnect"
    );

    let (new_client, reconnect_rx) = SessionClient::test_channel();
    backend
        .state_mut()
        .current_mut()
        .pending_session_attach
        .as_mut()
        .expect("reconnect attach")
        .client = Some(new_client);
    backend
        .update_level(Msg::SessionAttached {
            epoch: reconnect_epoch,
            session_instance: crate::session::protocol::SessionInstanceId::for_test("reconnect"),
            session: "dev".to_string(),
            client_id: 1,
            panes: Vec::new(),
            layout_rev: 0,
            layout: None,
            controller: Some(1),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: false,
            read_only: false,
            origin: crate::session::origin::SessionOrigin::default(),
        })
        .expect("flush pending geometry after reconnect");
    let mut resized: Vec<_> = reconnect_rx
        .try_iter()
        .filter_map(|outbound| match outbound {
            crate::session::client::ClientOutbound::Control(
                crate::session::protocol::ClientMessage::Resize {
                    pane_id,
                    local,
                    cols,
                    rows,
                    ..
                },
            ) => Some((pane_id, local, cols, rows)),
            _ => None,
        })
        .collect();
    resized.sort_unstable();
    assert_eq!(resized, vec![(1, false, 80, 20), (2, false, 90, 22)]);
}

#[test]
fn roster_diff_emits_joins_and_leaves_with_the_new_count() {
    let client = |id, label: &str| ClientInfo {
        id,
        label: label.to_string(),
        read_only: false,
        requesting_control: false,
        parked: false,
    };
    let events = roster_diff_events(
        &[client(1, "one"), client(2, "two")],
        &[client(2, "renamed"), client(3, "three")],
    );

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, crate::events::EventKind::ClientJoined);
    assert_eq!(
        events[0].fields,
        vec![
            ("client_id", "3".into()),
            ("client_name", "three".into()),
            ("count", "2".into()),
        ]
    );
    assert_eq!(events[1].kind, crate::events::EventKind::ClientLeft);
    assert_eq!(
        events[1].fields,
        vec![
            ("client_id", "1".into()),
            ("client_name", "one".into()),
            ("count", "2".into()),
        ]
    );
}

#[test]
fn runtime_status_transitions_emit_once_and_stale_updates_are_ignored() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
        pane.pty_generation = 7;
        pane.terminal.bind_session(pane.id, 7);
        let events = backend.state().event_hub.subscribe(Some(HashSet::from([
            crate::events::EventKind::PaneStatusChanged,
        ])));
        let status = crate::session::protocol::PaneStatus {
            value: "blocked".into(),
            reason: Some("needs approval".into()),
            set_at: 1,
        };
        let runtime = PaneRuntimeState {
            status: Some(status.clone()),
            detected_agent: Some(crate::session::protocol::DetectedAgent {
                agent: crate::session::protocol::AgentIdentity::new("opencode", "OpenCode").into(),
                state: crate::session::protocol::DetectedAgentState::Blocked,
            }),
            sequence: 1,
            ..PaneRuntimeState::default()
        };

        backend
            .dispatch(Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id: 1,
                local: false,
                generation: 7,
                agents: Vec::new(),
                state: runtime.clone(),
            })
            .expect("dispatch status transition");
        assert!(
            events.try_recv().is_err(),
            "runtime receipts do not emit shared events"
        );
        backend
            .update_level(Msg::SessionEvent {
                epoch,
                event: crate::session::server::events::SessionEvent {
                    session_instance: crate::session::protocol::SessionInstanceId::for_test("test"),
                    seq: 1,
                    kind: crate::events::EventKind::PaneStatusChanged,
                    data: std::collections::BTreeMap::from([
                        ("pane".into(), "1".into()),
                        ("status".into(), "blocked".into()),
                        ("reason".into(), "needs approval".into()),
                        ("previous_status".into(), "".into()),
                    ]),
                },
            })
            .unwrap();
        let event: serde_json::Value =
            serde_json::from_str(&events.try_recv().expect("server event")).unwrap();
        assert_eq!(event["event"], "pane-status-changed");
        assert_eq!(event["data"]["pane"], "1");
        assert_eq!(event["data"]["status"], "blocked");
        assert_eq!(event["data"]["reason"], "needs approval");
        assert_eq!(event["data"]["previous_status"], "");
        assert_eq!(
            backend.state().current().workspaces[0].panes[0]
                .terminal
                .detected_agent
                .as_ref()
                .map(|agent| agent.agent.id.clone()),
            Some("opencode".to_string())
        );

        backend
            .dispatch(Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id: 1,
                local: false,
                generation: 7,
                agents: Vec::new(),
                state: runtime,
            })
            .expect("dispatch duplicate status");
        assert!(events.try_recv().is_err());

        backend
            .dispatch(Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id: 1,
                local: false,
                generation: 7,
                agents: Vec::new(),
                state: PaneRuntimeState {
                    status: None,
                    sequence: 0,
                    ..PaneRuntimeState::default()
                },
            })
            .expect("dispatch stale status");
        assert_eq!(
            backend.state().current().workspaces[0].panes[0]
                .terminal
                .reported_status,
            Some(status)
        );
        assert!(
            backend.state().current().workspaces[0].panes[0]
                .terminal
                .detected_agent
                .is_some()
        );
        assert!(events.try_recv().is_err());
    });
}

#[test]
fn parked_runtime_updates_keep_background_metadata_current() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let state = backend.state_mut();
            state.runtime_epoch = 4;
            let pane = &mut state.current_mut().workspaces[0].panes[0];
            pane.pty_generation = 7;
            pane.terminal.bind_session(pane.id, 7);
            state.park_current(4, crate::state::Attachment::new());
            state.runtime_epoch = 5;
        }

        backend
            .dispatch(Msg::SessionPaneRuntimeChanged {
                epoch: 4,
                pane_id: 1,
                local: false,
                generation: 7,
                agents: Vec::new(),
                state: PaneRuntimeState {
                    cwd: Some("/remote/project".to_string()),
                    foreground_program: Some("cargo".to_string()),
                    sequence: 1,
                    ..PaneRuntimeState::default()
                },
            })
            .expect("dispatch parked runtime update");

        let pane = &backend.state().background[&4].workspaces[0].panes[0];
        assert_eq!(pane.terminal.cwd.as_deref(), Some("/remote/project"));
        assert_eq!(pane.terminal.foreground_program.as_deref(), Some("cargo"));
    });
}

/// A failed *local* attach must not install another pending attach: a local ephemeral is
/// itself the fallback, so retrying it on failure would spin forever (fail → fall back → fail →
/// …). Only a remote failure falls back, which is verified live. Side-effect-free: the local
/// path returns no command, so nothing is spawned.
#[test]
fn local_attach_failure_does_not_retry_into_an_ephemeral_loop() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        backend.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach::new(
                42,
                crate::state::AttachRequest::new(
                    "eph-local",
                    crate::state::AttachMode::JoinOrStart,
                )
                .with_access(crate::state::AttachAccess::Writable {
                    on_attach: Some(crate::config::SessionOnAttach::Take),
                }),
            ));
        backend
            .dispatch(Msg::SessionAttachFailed {
                epoch: 42,
                message: "no local server".into(),
            })
            .expect("dispatch local attach failure");
        assert!(
            backend.state().current().pending_session_attach.is_none(),
            "a local ephemeral failure must clear the pending attach and not re-arm one"
        );
    });
}

#[test]
fn retained_remote_reconnect_failure_stays_offline_and_remote() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().remote_host = Some("workbox".to_string());
            state.current_mut().remote_target = Some(target.clone());
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: 42,
                request: crate::state::AttachRequest::new(
                    "dev",
                    crate::state::AttachMode::Reconnect,
                )
                .on_host(Some(crate::session::remote::RemoteTarget::Alias(
                    "workbox".into(),
                ))),
                client: None,
                left: None,
                parked_epoch: None,
            });
        }

        backend
            .dispatch(Msg::SessionAttachFailed {
                epoch: 42,
                message: "offline".to_string(),
            })
            .expect("dispatch reconnect failure");

        assert!(backend.state().current().pending_session_attach.is_none());
        assert_eq!(
            backend.state().current().connection,
            crate::state::ConnectionState::Unreachable
        );
        assert_eq!(
            backend.state().current().remote_target.as_ref(),
            Some(&target)
        );
        assert_eq!(
            backend.state().current().session_name.as_deref(),
            Some("dev")
        );
    });
}

#[test]
fn lost_remote_session_requires_explicit_recreation() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
        let epoch = backend.state().runtime_epoch;
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().remote_host = Some("workbox".to_string());
            state.current_mut().remote_target = Some(target);
            state.current_mut().connection = crate::state::ConnectionState::Reconnecting;
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch,
                request: crate::state::AttachRequest::new(
                    "dev",
                    crate::state::AttachMode::Reconnect,
                )
                .on_host(Some(crate::session::remote::RemoteTarget::Alias(
                    "workbox".into(),
                ))),
                client: None,
                left: None,
                parked_epoch: None,
            });
            state.askpass = Some(crate::state::AskpassState::new(
                crate::state::AskpassPrompt {
                    id: u64::MAX - 9,
                    session: "reconnect".to_string(),
                    attach_epoch: Some(epoch),
                    kind: crate::session::remote::AskpassKind::Secret,
                    prompt: "Password:".to_string(),
                    error: None,
                },
            ));
        }

        backend
            .dispatch(Msg::SessionLost {
                epoch,
                message: "the original remote session `dev` is gone".to_string(),
            })
            .expect("mark session lost");
        assert!(backend.state().current().remote_session_lost);
        assert_eq!(
            backend.state().current().connection,
            crate::state::ConnectionState::Unreachable
        );
        assert!(backend.state().current().pending_session_attach.is_none());
        assert!(
            backend.state().askpass.is_none(),
            "a prompt owned by the lost attach must not outlive it"
        );

        backend
            .dispatch(Msg::RetrySessionReconnect)
            .expect("ordinary retry is refused for a lost session");
        assert!(backend.state().current().pending_session_attach.is_none());

        backend.render();
        backend
            .send_key(tui_lipan::prelude::KeyEvent {
                code: tui_lipan::prelude::KeyCode::Enter,
                mods: tui_lipan::prelude::KeyMods::NONE,
            })
            .expect("Enter on the modal starts explicit recreation");
        assert!(
            backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .is_some_and(|pending| pending.request.is_reconnect())
        );
        assert_eq!(
            backend.state().current().connection,
            crate::state::ConnectionState::Reconnecting
        );
    });
}

#[test]
fn retry_session_reconnect_is_a_noop_unless_the_session_is_unreachable() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().pending_session_attach = None;
            state.current_mut().connection = crate::state::ConnectionState::Connected;
        }
        backend
            .dispatch(Msg::RetrySessionReconnect)
            .expect("dispatch ignored retry");
        assert_eq!(
            backend.state().current().connection,
            crate::state::ConnectionState::Connected
        );
        assert!(backend.state().current().pending_session_attach.is_none());
    });
}

#[test]
fn abandoning_a_remote_reconnect_stays_offline_and_opens_sessions() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
        let epoch_before;
        {
            let state = backend.state_mut();
            state.runtime_epoch = u64::MAX - 8;
            state.show_session_picker = false;
            state.session_picker = None;
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().remote_host = Some("workbox".to_string());
            state.current_mut().remote_target = Some(target.clone());
            state.current_mut().connection = crate::state::ConnectionState::Reconnecting;
            epoch_before = state.runtime_epoch;
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: epoch_before,
                request: crate::state::AttachRequest::new(
                    "dev",
                    crate::state::AttachMode::Reconnect,
                )
                .on_host(Some(crate::session::remote::RemoteTarget::Alias(
                    "workbox".into(),
                ))),
                client: None,
                left: None,
                parked_epoch: None,
            });
        }

        backend
            .dispatch(Msg::AbandonSessionReconnect)
            .expect("dispatch abandon reconnect");

        let state = backend.state();
        assert!(state.current().pending_session_attach.is_none());
        assert_eq!(
            state.current().connection,
            crate::state::ConnectionState::Unreachable
        );
        assert_eq!(state.current().session_name.as_deref(), Some("dev"));
        assert_eq!(state.current().remote_target.as_ref(), Some(&target));
        assert!(state.show_session_picker, "Esc must open Sessions");
        assert_ne!(
            state.runtime_epoch, epoch_before,
            "in-flight attach results must be invalidated"
        );
        crate::session::bootstrap::finish_cancelled_remote_attach(epoch_before);
    });
}

/// A local reconnect that fails leaves the session instead of rendering its dead panes as though
/// the client were still inside it.
#[test]
fn local_reconnect_failure_leaves_the_session() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().mark_disconnected();
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: 42,
                request: crate::state::AttachRequest::new(
                    "dev",
                    crate::state::AttachMode::Reconnect,
                ),
                client: None,
                left: None,
                parked_epoch: None,
            });
        }

        backend
            .dispatch(Msg::SessionAttachFailed {
                epoch: 42,
                message: "Session `dev` is busy or not accepting clients".to_string(),
            })
            .expect("dispatch local reconnect failure");

        let current = backend.state().current();
        assert!(current.pending_session_attach.is_none());
        assert_eq!(current.session_name, None);
        assert!(!current.session_attached);
        assert!(
            !backend
                .state()
                .background
                .values()
                .any(|attachment| { attachment.session_name.as_deref() == Some("dev") })
        );
    });
}

/// A failed *remote* connect that had parked a live session restores that session rather than
/// falling back to a fresh local ephemeral. The ephemeral fallback would re-attach to this
/// process's own `eph-<pid>` server — still controlled by the parked client — and come back as a
/// follower of itself. Restoring the parked attachment keeps the user on their real session, and
/// the dead empty connect attachment is discarded.
#[test]
fn failed_remote_connect_restores_the_parked_session_not_a_follower_ephemeral() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let target = crate::session::remote::RemoteTarget::Alias("windev".to_string());
        {
            let state = backend.state_mut();
            // The live local session, parked into the background under epoch 1 when the
            // connect started.
            let mut parked = crate::state::Attachment::new();
            parked.epoch = 1;
            parked.session_name = Some("eph-local".to_string());
            parked.session_attached = true;
            parked.connection = crate::state::ConnectionState::Connected;
            state.background.insert(1, parked);

            // The current attachment is the fresh empty one the connect installed; it never
            // attached.
            state.runtime_epoch = 2;
            state.current_mut().epoch = 2;
            state.current_mut().remote_host = Some("windev".to_string());
            state.current_mut().remote_target = Some(target.clone());
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: 2,
                request: crate::state::AttachRequest::new(
                    "eph-windev",
                    crate::state::AttachMode::JoinOrStart,
                )
                .on_host(Some(crate::session::remote::RemoteTarget::Alias(
                    "windev".into(),
                ))),
                client: None,
                left: None,
                parked_epoch: Some(1),
            });
        }

        backend
            .dispatch(Msg::SessionAttachFailed {
                epoch: 2,
                message: "could not resolve hostname windev".to_string(),
            })
            .expect("dispatch remote connect failure");

        // Restored onto the parked local session, not a fresh ephemeral, and no longer
        // pointed at the remote host.
        assert_eq!(
            backend.state().current().session_name.as_deref(),
            Some("eph-local")
        );
        assert!(backend.state().current().session_attached);
        assert_eq!(backend.state().current().remote_target, None);
        assert!(backend.state().current().pending_session_attach.is_none());
        // The parked entry is gone (now current) and the dead connect attachment was not
        // retained in its place.
        assert!(!backend.state().background.contains_key(&1));
        assert!(!backend.state().background.contains_key(&2));
    });
}

/// A failed *local* create also restores the parked session — creating now parks the current
/// session like a switch, so a create that can't start its server must not strand the user on
/// the broken empty attachment either. (The remote-only ephemeral fallback stays remote-only.)
#[test]
fn failed_local_create_restores_the_parked_session() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let state = backend.state_mut();
            let mut parked = crate::state::Attachment::new();
            parked.epoch = 1;
            parked.session_name = Some("eph-local".to_string());
            parked.session_attached = true;
            parked.connection = crate::state::ConnectionState::Connected;
            state.background.insert(1, parked);

            state.runtime_epoch = 2;
            state.current_mut().epoch = 2;
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                parked_epoch: Some(1),
                ..crate::state::PendingSessionAttach::new(
                    2,
                    crate::state::AttachRequest::new("work", crate::state::AttachMode::Create),
                )
            });
        }

        backend
            .dispatch(Msg::SessionAttachFailed {
                epoch: 2,
                message: "could not start session server".to_string(),
            })
            .expect("dispatch local create failure");

        assert_eq!(
            backend.state().current().session_name.as_deref(),
            Some("eph-local")
        );
        assert!(backend.state().current().session_attached);
        assert!(backend.state().current().pending_session_attach.is_none());
        assert!(!backend.state().background.contains_key(&1));
        assert!(!backend.state().background.contains_key(&2));
    });
}

/// Attaching where another client already holds the lease: the client must ask what to do
/// instead of quietly becoming a follower. `controller` is who the server says holds it.
fn attach_with_controller(controller: crate::layout::shared::ClientId, reconnect: bool) -> bool {
    attach_policy_result(
        Some(controller),
        reconnect,
        false,
        false,
        crate::config::SessionOnAttach::Ask,
        None,
    )
    .0
}

fn attach_policy_result(
    controller: Option<crate::layout::shared::ClientId>,
    reconnect: bool,
    read_only: bool,
    allow_takeover: bool,
    mode: crate::config::SessionOnAttach,
    mode_override: Option<crate::config::SessionOnAttach>,
) -> (bool, bool) {
    let (tx, rx) = std::sync::mpsc::channel();
    crate::test_support::on_large_stack(move || {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, outbound) = SessionClient::test_channel();
        backend.state_mut().config.session.on_attach = mode;
        let access = if read_only {
            crate::state::AttachAccess::ReadOnly
        } else {
            crate::state::AttachAccess::Writable {
                on_attach: mode_override,
            }
        };
        let attach_mode = if reconnect {
            crate::state::AttachMode::Reconnect
        } else {
            crate::state::AttachMode::Join
        };
        let mut pending = crate::state::PendingSessionAttach::new(
            1,
            crate::state::AttachRequest::new("dev", attach_mode).with_access(access),
        );
        pending.client = Some(client);
        backend.state_mut().current_mut().pending_session_attach = Some(pending);
        backend
            .dispatch(Msg::SessionAttached {
                epoch: 1,
                session_instance: crate::session::protocol::SessionInstanceId::for_test("dev"),
                session: "dev".into(),
                client_id: 2,
                panes: Vec::new(),
                layout_rev: 0,
                layout: None,
                controller,
                clients: Vec::new(),
                input_locked: false,
                allow_takeover,
                read_only,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .expect("dispatch attach");
        let requested = outbound.try_iter().any(|message| {
            matches!(
                message,
                crate::session::client::ClientOutbound::Control(
                    crate::session::protocol::ClientMessage::RequestControl
                )
            )
        });
        assert_eq!(
            backend
                .state()
                .current()
                .shared
                .as_ref()
                .unwrap()
                .controller,
            controller,
            "only the server can transfer control"
        );
        assert!(
            backend.state().current().pending_session_attach.is_none(),
            "the attempt and its override end with the landing"
        );
        tx.send((backend.state().follow_prompt.is_some(), requested))
            .expect("report result");
    });
    rx.recv().expect("test result")
}

/// Lands attempt `epoch` on a session another client controls, and reports whether the follow
/// prompt opened and whether control was requested.
fn land_on_occupied_session(
    backend: &mut TestBackend<crate::AppRoot>,
    epoch: u64,
    outbound: &std::sync::mpsc::Receiver<crate::session::client::ClientOutbound>,
) -> (bool, bool) {
    backend
        .dispatch(Msg::SessionAttached {
            epoch,
            session_instance: crate::session::protocol::SessionInstanceId::for_test("dev"),
            session: "dev".into(),
            client_id: 2,
            panes: Vec::new(),
            layout_rev: 0,
            layout: None,
            controller: Some(1),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: true,
            read_only: false,
            origin: crate::session::origin::SessionOrigin::default(),
        })
        .expect("dispatch attach");
    let requested = outbound.try_iter().any(|message| {
        matches!(
            message,
            crate::session::client::ClientOutbound::Control(
                crate::session::protocol::ClientMessage::RequestControl
            )
        )
    });
    (backend.state().follow_prompt.is_some(), requested)
}

fn pending_attach(
    epoch: u64,
    on_attach: Option<crate::config::SessionOnAttach>,
    client: Option<SessionClient>,
) -> crate::state::PendingSessionAttach {
    let mut pending = crate::state::PendingSessionAttach::new(
        epoch,
        crate::state::AttachRequest::new("dev", crate::state::AttachMode::Join)
            .with_access(crate::state::AttachAccess::Writable { on_attach }),
    );
    pending.client = client;
    pending
}

/// `--take-control` belongs to the attempt it was given with. An attempt that fails, or that a
/// newer attach replaces, takes its override with it, so the next landing follows the config.
#[test]
fn an_attach_override_ends_with_its_attempt() {
    crate::test_support::on_large_stack(|| {
        use crate::config::SessionOnAttach::{Ask, Take};

        let mut backend = TestBackend::new(crate::AppRoot::default());
        backend.state_mut().config.session.on_attach = Ask;
        backend.state_mut().current_mut().pending_session_attach =
            Some(pending_attach(1, Some(Take), None));
        backend
            .dispatch(Msg::SessionAttachFailed {
                epoch: 1,
                message: "no such session".into(),
            })
            .expect("dispatch failure");
        assert!(backend.state().current().pending_session_attach.is_none());

        let (stale, _stale_rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().pending_session_attach =
            Some(pending_attach(2, Some(Take), Some(stale)));
        let (client, outbound) = SessionClient::test_channel();
        backend.state_mut().runtime_epoch = 3;
        backend.state_mut().current_mut().epoch = 3;
        backend.state_mut().current_mut().pending_session_attach =
            Some(pending_attach(3, None, Some(client)));

        assert_eq!(
            land_on_occupied_session(&mut backend, 3, &outbound),
            (true, false),
            "the replaced attempt's --take-control must not reach this landing"
        );
    });
}

/// The configured policy is read when the attach lands, so a config reload during the connect
/// applies to it, while an override given with the attach still wins.
#[test]
fn attach_policy_reads_the_config_at_landing_and_the_override_wins() {
    crate::test_support::on_large_stack(|| {
        use crate::config::SessionOnAttach::{Ask, Follow, Take};

        for (override_mode, expected) in [(None, (false, true)), (Some(Follow), (false, false))] {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            backend.state_mut().config.session.on_attach = Ask;
            let (client, outbound) = SessionClient::test_channel();
            backend.state_mut().current_mut().pending_session_attach =
                Some(pending_attach(1, override_mode, Some(client)));
            backend.state_mut().config.session.on_attach = Take;
            assert_eq!(
                land_on_occupied_session(&mut backend, 1, &outbound),
                expected,
                "override {override_mode:?}"
            );
        }
    });
}

/// How far an attach landing moves the session view revision that drives the switch reveal.
fn session_view_revision_step(reconnect: bool) -> u64 {
    crate::test_support::on_large_stack(move || {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, _rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch: 1,
                request: crate::state::AttachRequest::new(
                    "dev",
                    if reconnect {
                        crate::state::AttachMode::Reconnect
                    } else {
                        crate::state::AttachMode::Join
                    },
                ),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
        let before = backend.state().session_view_revision;
        backend
            .dispatch(Msg::SessionAttached {
                epoch: 1,
                session_instance: crate::session::protocol::SessionInstanceId::for_test("dev"),
                session: "dev".into(),
                client_id: 1,
                panes: Vec::new(),
                layout_rev: 0,
                layout: None,
                controller: Some(1),
                clients: Vec::new(),
                input_locked: false,
                allow_takeover: false,
                read_only: false,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .expect("dispatch attach");
        backend.state().session_view_revision - before
    })
}

/// Attach onto an empty server that seeds pane 7 from client state, and report how that pane
/// arrived: `(opening, has_open_snapshot, terminal_active)` and whether a spawn reflow began.
fn seeded_pane_arrival(reconnect: bool) -> ((bool, bool, bool), bool) {
    crate::test_support::on_large_stack(move || {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, _rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes.push(crate::state::Pane::new(
                7,
                0,
                tui_lipan::prelude::FloatRect::default(),
            ));
            crate::layout::tiling::append_tiled_window(workspace, 7);
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: 1,
                request: crate::state::AttachRequest::new(
                    "dev",
                    if reconnect {
                        crate::state::AttachMode::Reconnect
                    } else {
                        crate::state::AttachMode::JoinOrStart
                    },
                ),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
        }
        backend
            .update_level(Msg::SessionAttached {
                epoch: 1,
                session_instance: crate::session::protocol::SessionInstanceId::for_test("dev"),
                session: "dev".into(),
                client_id: 1,
                panes: Vec::new(),
                layout_rev: 0,
                layout: None,
                controller: Some(1),
                clients: Vec::new(),
                input_locked: false,
                allow_takeover: false,
                read_only: false,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .expect("dispatch attach");
        let state = backend.state();
        let pane = crate::pane::lifecycle::find_pane(state, 7).expect("seeded pane");
        (
            (
                pane.opening,
                pane.opening_animation.is_some(),
                pane.terminal_active,
            ),
            state.animation == crate::layout::anim::GeometryAnimation::Spawn,
        )
    })
}

/// A new session's first panes arrive with the session, under its reveal: open and live at once,
/// with no pane-open effect or spawn reflow of their own. Reconnecting to a session whose server
/// came back empty reveals nothing new, so its re-seeded panes still open the usual way.
#[test]
fn a_new_sessions_first_panes_arrive_with_it_instead_of_animating_open() {
    assert_eq!(seeded_pane_arrival(false), ((false, false, true), false));
    let ((opening, animating, active), reflow) = seeded_pane_arrival(true);
    assert!(
        opening && animating && !active && reflow,
        "reconnect keeps the pane-open path"
    );
}

/// The Connecting scene giving way to a session is a new view; reconnecting to the session that
/// was already on screen is not, and its reconnect chrome already owns that moment.
#[test]
fn only_a_fresh_attach_reveals_a_new_session_view() {
    assert_eq!(session_view_revision_step(false), 1);
    assert_eq!(session_view_revision_step(true), 0);
}

#[test]
fn shared_attach_policies_and_one_shot_overrides() {
    use crate::config::SessionOnAttach::{Ask, Follow, Take};
    for allow_takeover in [false, true] {
        assert_eq!(
            attach_policy_result(Some(1), false, false, allow_takeover, Ask, None),
            (true, false)
        );
        assert_eq!(
            attach_policy_result(Some(1), false, false, allow_takeover, Follow, None),
            (false, false)
        );
        assert_eq!(
            attach_policy_result(Some(1), false, false, allow_takeover, Take, None),
            (false, true)
        );
        assert_eq!(
            attach_policy_result(Some(1), false, false, allow_takeover, Take, Some(Follow)),
            (false, false)
        );
        assert_eq!(
            attach_policy_result(Some(1), false, false, allow_takeover, Follow, Some(Take)),
            (false, true)
        );
        for mode in [Ask, Follow, Take] {
            assert_eq!(
                attach_policy_result(None, false, false, allow_takeover, mode, Some(Take)),
                (false, false)
            );
            assert_eq!(
                attach_policy_result(Some(1), false, true, allow_takeover, mode, None),
                (false, false)
            );
            assert_eq!(
                attach_policy_result(Some(1), true, false, allow_takeover, mode, None),
                (false, false)
            );
            assert_eq!(
                attach_policy_result(Some(2), false, false, allow_takeover, mode, Some(Take)),
                (false, false)
            );
        }
    }
}

#[test]
fn attaching_to_an_occupied_session_asks_before_following() {
    assert!(attach_with_controller(1, false));
}

/// Getting the lease on attach — which is what happens when the only other client is parked —
/// is not a fork in the road, so nothing is asked.
#[test]
fn attaching_with_the_lease_in_hand_asks_nothing() {
    assert!(!attach_with_controller(2, false));
}

#[test]
fn reconnecting_to_an_occupied_session_does_not_interrupt_with_follow_prompt() {
    assert!(!attach_with_controller(1, true));
}

#[test]
fn empty_ephemeral_profile_seed_emits_profile_loaded_after_attach() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, rx) = SessionClient::test_channel();
        let path = PathBuf::from("legacy-profile.toml");
        backend.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch: 1,
                request: crate::state::AttachRequest::new(
                    "eph-test",
                    crate::state::AttachMode::JoinOrStart,
                )
                .with_intent(crate::state::AttachIntent::ProfileSeed {
                    profile: "legacy-profile".into(),
                    path: path.clone(),
                }),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
        backend.state_mut().show_profile_picker = true;
        backend.state_mut().profile_picker =
            Some(crate::state::ProfilePickerState::new(Vec::new()));
        let events = backend.state().event_hub.subscribe(Some(HashSet::from([
            crate::events::EventKind::ProfileLoaded,
        ])));

        backend
            .dispatch(Msg::SessionAttached {
                epoch: 1,
                session_instance: crate::session::protocol::SessionInstanceId::for_test(
                    "ephemeral",
                ),
                session: "eph-test".into(),
                client_id: 1,
                panes: Vec::new(),
                layout_rev: 0,
                layout: None,
                controller: Some(1),
                clients: Vec::new(),
                input_locked: false,
                allow_takeover: false,
                read_only: false,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .expect("dispatch attach");

        assert_eq!(backend.state().current().origin.profile, None);
        assert!(!backend.state().show_profile_picker);
        assert!(backend.state().profile_picker.is_none());
        assert!(rx.try_iter().any(|message| matches!(
            message,
            crate::session::client::ClientOutbound::Control(
                crate::session::protocol::ClientMessage::SetSessionOrigin { origin }
            ) if origin.profile.as_deref() == Some("legacy-profile")
        )));
        assert!(events.try_recv().is_err());
        backend
            .dispatch(Msg::SessionOriginSet {
                epoch: 1,
                origin: crate::session::origin::SessionOrigin {
                    profile: Some("legacy-profile".to_string()),
                    ..Default::default()
                },
            })
            .expect("acknowledge session origin");
        assert_eq!(
            backend.state().current().origin.profile.as_deref(),
            Some("legacy-profile")
        );

        let event: serde_json::Value =
            serde_json::from_str(&events.try_recv().expect("profile-loaded event"))
                .expect("event json");
        assert_eq!(
            event,
            serde_json::json!({
                "event": "profile-loaded",
                "data": {
                    "profile": "legacy-profile",
                    "path": path.display().to_string(),
                    "session": "eph-test"
                }
            })
        );
    });
}

fn colliding_namespace_backend() -> TestBackend<crate::AppRoot> {
    let mut backend = TestBackend::new(crate::AppRoot::default());
    let rect = tui_lipan::prelude::FloatRect::default();
    let mut shared = crate::state::Pane::new(7, 100, rect);
    shared.pty_generation = 1;
    shared.terminal.bind_session(7, 1);
    backend.state_mut().current_mut().workspaces[0]
        .panes
        .push(shared);
    let mut local = crate::state::Pane::new(7, 100, rect);
    local.pty_generation = 1;
    local.terminal.bind_session(7, 1);
    backend.state_mut().scratch.panes.push(local);
    backend.state_mut().scratch.focused_pane = Some(7);
    backend.state_mut().scratch_visible = true;
    backend
}

fn namespace_text(backend: &TestBackend<crate::AppRoot>, local: bool) -> String {
    crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, local)
        .expect("namespaced pane")
        .terminal
        .capture_text()
}

#[test]
fn colliding_pane_ids_route_session_events_by_namespace() {
    crate::test_support::on_large_stack(|| {
        let mut backend = colliding_namespace_backend();
        let epoch = backend.state().runtime_epoch;

        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id: 7,
                local: false,
                generation: 1,
                bytes: b"shared-out\r\n".to_vec(),
            })
            .expect("shared output");
        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                metadata: crate::session::protocol::OutputMetadata::default(),
                epoch,
                pane_id: 7,
                local: true,
                generation: 1,
                bytes: b"local-out\r\n".to_vec(),
            })
            .expect("local output");
        assert!(
            namespace_text(&backend, false).contains("shared-out"),
            "shared output must land on the attachment pane"
        );
        assert!(
            !namespace_text(&backend, false).contains("local-out"),
            "local output must not land on the attachment pane"
        );
        assert!(
            namespace_text(&backend, true).contains("local-out"),
            "local output must land on the scratch pane"
        );
        assert!(
            !namespace_text(&backend, true).contains("shared-out"),
            "shared output must not land on the scratch pane"
        );

        backend
            .dispatch(Msg::SessionResized {
                epoch,
                pane_id: 7,
                local: false,
                generation: 1,
                cols: 40,
                rows: 12,
                cell: None,
            })
            .expect("shared resize");
        backend
            .dispatch(Msg::SessionResized {
                epoch,
                pane_id: 7,
                local: true,
                generation: 1,
                cols: 20,
                rows: 8,
                cell: None,
            })
            .expect("local resize");
        assert_eq!(
            crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, false)
                .unwrap()
                .terminal
                .cols,
            40
        );
        assert_eq!(
            crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, true)
                .unwrap()
                .terminal
                .rows,
            8
        );

        backend.state_mut().config.pane.hold_on_exit = false;
        backend
            .dispatch(Msg::SessionExited {
                epoch,
                pane_id: 7,
                local: false,
                generation: 1,
                code: 0,
            })
            .expect("shared exit");
        assert!(
            crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, false)
                .is_some_and(|pane| pane.closing),
            "shared exit must close the attachment pane"
        );
        assert!(
            crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, true)
                .is_some_and(|pane| !pane.closing),
            "shared exit must not close the scratch pane"
        );

        backend
            .dispatch(Msg::SessionExited {
                epoch,
                pane_id: 7,
                local: true,
                generation: 1,
                code: 0,
            })
            .expect("local exit");
        assert!(
            crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, true)
                .is_some_and(|pane| pane.closing),
            "local exit must close the scratch pane"
        );
    });
}

#[test]
fn attach_metadata_targets_shared_namespace_when_scratch_id_collides() {
    crate::test_support::on_large_stack(|| {
        let mut backend = colliding_namespace_backend();
        {
            let state = backend.state_mut();
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes.retain(|pane| pane.id == 7);
            workspace.tile_tree = Some(crate::layout::tiling::DwindleTree::Leaf(7));
            workspace.focused_pane = Some(7);
            state.current_mut().focused_pane = Some(7);
            state.scratch.panes[0].terminal.title = Some("scratch-title".into());
        }
        let layout = crate::layout::shared::shared_layout_from_state(backend.state(), (80, 24));
        let (client, _rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch: 2,
                request: crate::state::AttachRequest::new("dev", crate::state::AttachMode::Join),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
        let mut runtime = PaneRuntimeState::default();
        runtime.rows.push(crate::session::protocol::PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: "shared-row".into(),
            title: "Shared row".into(),
            status: "working".into(),
            reason: None,
            active: true,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
        });
        let session_instance = crate::session::protocol::SessionInstanceId::for_test("shared");
        let agent_ref = crate::session::protocol::AgentRef {
            pane: crate::session::protocol::PaneRef {
                session_instance: session_instance.clone(),
                pane_id: 7,
                generation: 1,
            },
            slot: None,
            incarnation: 4,
        };
        runtime.integration = Some(Box::new(crate::session::protocol::AgentIntegrationReport {
            integration: "hook-before-attach".into(),
            identity: crate::session::protocol::AgentIdentity::new("claude", "Claude Code"),
            reference: agent_ref.clone(),
            state: crate::session::protocol::AgentState::Blocked,
            reason: Some("approval".into()),
            native_session: Some("native-123".into()),
            seq: 8,
            reported_at_unix_ms: 42,
        }));

        backend
            .dispatch(Msg::SessionAttached {
                epoch: 2,
                session_instance,
                session: "dev".into(),
                client_id: 1,
                panes: vec![crate::session::protocol::PaneMeta {
                    scrollback: 100,
                    pane_id: 7,
                    generation: 1,
                    agents: vec![agent_ref.into()],
                    cols: 80,
                    rows: 24,
                    pid: Some(42),
                    title: Some("shared-title".into()),
                    original_user: None,
                    exited: None,
                    logging: false,
                    runtime,
                }],
                layout_rev: 1,
                layout: Some(layout),
                controller: Some(1),
                clients: Vec::new(),
                input_locked: false,
                allow_takeover: false,
                read_only: false,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .expect("attach shared pane");

        let shared = crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, false)
            .expect("shared pane");
        assert_eq!(shared.terminal.title.as_deref(), Some("shared-title"));
        assert_eq!(shared.terminal.published_rows[0].id, "shared-row");
        assert_eq!(
            shared
                .terminal
                .agent_integration
                .as_ref()
                .map(|integration| integration.state),
            Some(crate::session::protocol::AgentState::Blocked)
        );
        let scratch = crate::pane::lifecycle::find_pane_in_namespace(backend.state(), 7, true)
            .expect("scratch pane");
        assert_eq!(scratch.terminal.title.as_deref(), Some("scratch-title"));
        assert!(scratch.terminal.published_rows.is_empty());
    });
}

/// A popup lives outside every workspace, so the generic teardown cannot find it: `local` with
/// no scratch membership falls straight through and marks nothing. Without an explicit
/// interception a failed popup spawn left a dead pane on screen for the rest of the session.
#[test]
fn a_failed_popup_spawn_tears_the_popup_down() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let generation = {
            let state = backend.state_mut();
            let mut pane = crate::state::Pane::new(
                crate::state::POPUP_PANE_ID,
                100,
                tui_lipan::prelude::FloatRect::default(),
            );
            pane.opening = false;
            // A `keep_open` popup must still go: there is no process to keep open.
            pane.identity.keep_open = true;
            let generation = pane.pty_generation;
            state.popup = Some(pane);
            generation
        };
        backend.render();

        let epoch = backend.state().runtime_epoch;
        backend
            .dispatch(Msg::SessionSpawnResult {
                epoch,
                pane_id: crate::state::POPUP_PANE_ID,
                local: true,
                generation,
                pid: None,
                ok: false,
                error: Some("no such command".to_string()),
            })
            .expect("failed popup spawn");

        assert!(
            backend
                .state()
                .popup
                .as_ref()
                .is_some_and(|pane| pane.closing),
            "the popup must be marked closing so its prune can drop it"
        );

        backend
            .dispatch(Msg::PruneClosed(
                epoch,
                crate::state::POPUP_PANE_ID,
                generation,
            ))
            .expect("prune the closed popup");
        assert!(backend.state().popup.is_none(), "the popup must be dropped");
    });
}

#[test]
fn scratch_agent_acknowledgement_survives_an_attachment_epoch_change() {
    // Renders the whole app, a view tree deeper than the default test stack holds.
    crate::test_support::on_large_stack(
        scratch_agent_acknowledgement_survives_an_attachment_epoch_change_body,
    );
}

fn scratch_agent_acknowledgement_survives_an_attachment_epoch_change_body() {
    let mut backend = TestBackend::new(crate::AppRoot::default());
    let (reply, response) = std::sync::mpsc::channel();
    backend.state_mut().pending_agent_report_replies.insert(
        17,
        crate::state::PendingAgentReportReply {
            origin_epoch: None,
            reply,
        },
    );
    backend.state_mut().runtime_epoch = 9;

    backend
        .dispatch(Msg::SessionAgentReportResult {
            epoch: 9,
            request_id: 17,
            response: crate::control::ControlResponse::empty(),
        })
        .expect("dispatch acknowledgement");

    assert!(response.recv().expect("pending reply").ok);
    assert!(backend.state().pending_agent_report_replies.is_empty());
}

#[test]
fn attached_agent_acknowledgement_rejects_another_epoch() {
    // Renders the whole app, a view tree deeper than the default test stack holds.
    crate::test_support::on_large_stack(attached_agent_acknowledgement_rejects_another_epoch_body);
}

fn attached_agent_acknowledgement_rejects_another_epoch_body() {
    let mut backend = TestBackend::new(crate::AppRoot::default());
    let (reply, response) = std::sync::mpsc::channel();
    backend.state_mut().pending_agent_report_replies.insert(
        23,
        crate::state::PendingAgentReportReply {
            origin_epoch: Some(4),
            reply,
        },
    );

    backend
        .dispatch(Msg::SessionAgentReportResult {
            epoch: 9,
            request_id: 23,
            response: crate::control::ControlResponse::empty(),
        })
        .expect("dispatch stale acknowledgement");
    assert_eq!(
        response.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    );
    assert!(
        backend
            .state()
            .pending_agent_report_replies
            .contains_key(&23)
    );

    backend
        .dispatch(Msg::SessionAgentReportResult {
            epoch: 4,
            request_id: 23,
            response: crate::control::ControlResponse::empty(),
        })
        .expect("dispatch owning acknowledgement");
    assert!(response.recv().expect("pending reply").ok);
}

/// The update popup runs the updater and stays open. Success leaves the old build running, so it
/// earns a toast naming the step that is left; failure brings **Update rozi** back to retry.
#[test]
fn the_update_popup_exit_reports_what_is_left() {
    fn exit_update_popup(code: i32) -> TestBackend<crate::AppRoot> {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let generation = {
            let state = backend.state_mut();
            state.available_update = Some(crate::ops::update_check::AvailableUpdate::for_test(
                "9.9.9",
                crate::platform::install_source::InstallSource::Managed,
                None,
            ));
            state.update_started = true;
            let mut pane = crate::state::Pane::new(
                crate::state::POPUP_PANE_ID,
                100,
                tui_lipan::prelude::FloatRect::default(),
            );
            pane.opening = false;
            pane.identity.keep_open = true;
            pane.identity.launch = Some(crate::pane::launch::PaneLaunch::shell("rozi update"));
            let generation = pane.pty_generation;
            state.popup = Some(pane);
            generation
        };
        backend.render();
        let epoch = backend.state().runtime_epoch;
        backend
            .dispatch(Msg::SessionExited {
                epoch,
                pane_id: crate::state::POPUP_PANE_ID,
                local: true,
                generation,
                code,
            })
            .expect("update popup exits");
        backend
    }

    crate::test_support::on_large_stack(|| {
        let installed = exit_update_popup(0);
        let toast = installed
            .state()
            .replaceable_toasts
            .values()
            .next()
            .expect("a successful update is followed up")
            .content()
            .replace('\u{0}', " ");
        assert!(toast.starts_with("rozi v9.9.9 installed"), "{toast}");
        assert!(toast.contains("quit and start rozi again"), "{toast}");
        assert!(installed.state().popup.is_some(), "the popup stays open");
        assert!(installed.state().update_started);

        let failed = exit_update_popup(1);
        assert!(failed.state().replaceable_toasts.is_empty());
        assert!(
            !failed.state().update_started,
            "a failed update is offered again"
        );
    });
}

/// Attaching adopts panes that are already live, so no open/activate timer ever runs to hand one
/// the keyboard - and the framework's `OnDemand` focus policy has no first-widget fallback. Without
/// an explicit request the session draws its focused pane as focused while input goes nowhere until
/// the pane is clicked.
#[test]
fn attaching_to_a_populated_session_hands_the_keyboard_to_the_focused_pane() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        backend.render();
        let epoch = backend.state().runtime_epoch;
        let (client, _rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch,
                request: crate::state::AttachRequest::new("dev", crate::state::AttachMode::Join),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });

        backend
            .dispatch(Msg::SessionAttached {
                epoch,
                session_instance: crate::session::protocol::SessionInstanceId::for_test("dev"),
                session: "dev".to_string(),
                client_id: 1,
                panes: vec![
                    crate::session::protocol::PaneMeta::for_test(7, 0, 100),
                    crate::session::protocol::PaneMeta::for_test(9, 0, 100),
                ],
                layout_rev: 1,
                layout: Some(attached_layout(&[7, 9])),
                controller: Some(1),
                clients: Vec::new(),
                input_locked: false,
                allow_takeover: true,
                read_only: false,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .expect("attach to a populated session");
        backend.render();

        assert_eq!(backend.state().current().focused_pane, Some(7));
        assert_eq!(
            backend.focused_key().map(|key| key.as_ref()),
            Some(crate::view::pane_terminal_key(7).as_str()),
            "the adopted focused pane must own the keyboard without a click"
        );
    });
}

/// A minimal authoritative layout: one workspace of live tiled panes, the shape a server sends on
/// attach.
fn attached_layout(panes: &[crate::state::PaneId]) -> crate::layout::shared::SharedLayout {
    use crate::layout::shared::{
        SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis,
        SharedWorkspace,
    };
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
        canvas_cols: 100,
        canvas_rows: 28,
        workspaces: vec![SharedWorkspace {
            index: 0,
            name: None,
            synchronized: false,
            layout: SharedLayoutKind::Dwindle,
            start_axis: SharedSplitAxis::Horizontal,
            split_ratios: Vec::new(),
            tree: None,
            panes: panes
                .iter()
                .map(|id| SharedPane {
                    pane_id: *id,
                    generation: 0,
                    title: None,
                    profile_name: None,
                    cwd: None,
                    launch: None,
                    replay: false,
                    keep_open: false,
                    floating: false,
                    pinned: false,
                    fullscreen: false,
                    rect: None,
                    scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                })
                .collect(),
        }],
    }
}

#[test]
fn layout_revisions_do_not_duplicate_server_events() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        backend.state_mut().current_mut().shared = Some(crate::state::SharedSessionState::new(1));
        let layout = crate::layout::shared::shared_layout_from_state(backend.state(), (80, 24));
        let events = backend.state().event_hub.subscribe(Some(HashSet::from([
            crate::events::EventKind::LayoutChanged,
        ])));
        for (rev, author, expected) in [(5, 1, "self"), (6, 0, "server"), (7, 2, "client")] {
            backend
                .update_level(Msg::SessionLayoutCommitted {
                    epoch,
                    rev,
                    author,
                    layout: layout.clone(),
                    live: false,
                })
                .unwrap();
            assert!(
                events.try_recv().is_err(),
                "layout receipts must not invent events"
            );
            backend
                .update_level(Msg::SessionEvent {
                    epoch,
                    event: crate::session::server::events::SessionEvent {
                        session_instance: crate::session::protocol::SessionInstanceId::for_test(
                            "test",
                        ),
                        seq: rev,
                        kind: crate::events::EventKind::LayoutChanged,
                        data: std::collections::BTreeMap::from([
                            ("revision".into(), rev.to_string()),
                            ("author".into(), expected.into()),
                        ]),
                    },
                })
                .unwrap();
            let event: serde_json::Value = serde_json::from_str(
                &events
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(event["data"]["revision"], rev.to_string());
            assert_eq!(event["data"]["author"], expected);
        }
    });
}

/// A live revision is a step of the controller's gesture: the follower draws it where it lands
/// instead of easing toward it. An ordinary revision keeps the default transition.
#[test]
fn a_live_layout_revision_is_applied_without_easing() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        backend.state_mut().current_mut().shared = Some(crate::state::SharedSessionState::new(1));
        let layout = crate::layout::shared::shared_layout_from_state(backend.state(), (80, 24));
        for (rev, live, expected) in [
            (5, true, crate::layout::anim::GeometryAnimation::None),
            (6, false, crate::layout::anim::GeometryAnimation::TileFloat),
        ] {
            backend
                .update_level(Msg::SessionLayoutCommitted {
                    epoch,
                    rev,
                    author: 2,
                    layout: layout.clone(),
                    live,
                })
                .unwrap();
            assert_eq!(backend.state().animation, expected, "live: {live}");
        }
    });
}

#[test]
fn pane_output_uses_completed_parser_mutations_for_frame_scope() {
    use crate::pane::OutputFrame;
    let mut pane = crate::pane::TerminalPane::new(100);
    assert_eq!(
        pane.process_server_output(b"ready").frame,
        OutputFrame::Rebuild
    );
    assert_eq!(
        pane.process_server_output(b"\x1b[c").frame,
        OutputFrame::None
    );
    assert_eq!(
        pane.process_server_output(b"\x1b_Ga=T,i=7,f=24,s=1,v=1,q=2,C=1;YW")
            .frame,
        OutputFrame::None
    );
    let completed = pane.process_server_output(b"Jj\x1b\\");
    assert_eq!(completed.frame, OutputFrame::Graphics);
    assert_eq!(completed.mutation.cells, tui_lipan::TerminalDamage::None);
    assert_eq!(
        pane.process_server_output(b"\x1b_Ga=t,i=7,f=24,s=1,v=1,q=2;ZGVm\x1b\\")
            .frame,
        OutputFrame::Graphics
    );
    assert_eq!(
        pane.process_server_output(b"\x1b_Ga=d,d=I,i=7,q=2;\x1b\\")
            .frame,
        OutputFrame::Graphics
    );
    assert_eq!(
        pane.process_server_output(b"text").frame,
        OutputFrame::Repaint
    );
    assert_eq!(
        pane.process_server_output(b"\x1b]2;new title\x07").frame,
        OutputFrame::Rebuild
    );
    assert_eq!(
        pane.process_server_output(b"\x1b_Ga=q,i=8,f=24,s=1,v=1;YWJj\x1b\\")
            .frame,
        OutputFrame::None
    );
}

#[test]
fn mixed_graphics_and_cells_keep_the_broader_terminal_frame() {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.process_server_output(b"ready");
    let output = pane.process_server_output(b"\x1b_Ga=T,i=7,f=24,s=1,v=1,q=2,C=1;YWJj\x1b\\text");
    assert_eq!(output.frame, crate::pane::OutputFrame::Repaint);
    assert_ne!(output.mutation.graphics, tui_lipan::GraphicsDamage::None);
    assert_ne!(output.mutation.cells, tui_lipan::TerminalDamage::None);
}

#[test]
fn parsed_output_is_acknowledged_only_by_successful_presentation_reports() {
    crate::test_support::on_large_stack(|| {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, outbound) = SessionClient::test_channel();
        backend.state_mut().current_mut().session_client = Some(client);
        let pane_id = backend.state().current().focused_pane.unwrap();
        let generation = crate::pane::lifecycle::find_pane(backend.state(), pane_id)
            .unwrap()
            .pty_generation;
        let epoch = backend.state().runtime_epoch;
        backend
            .dispatch(Msg::SessionOutput {
                media: None,
                epoch,
                pane_id,
                local: false,
                generation,
                metadata: crate::session::protocol::OutputMetadata {
                    sequence: 7,
                    ..Default::default()
                },
                bytes: b"visible output".to_vec(),
            })
            .unwrap();
        assert!(!outbound.try_iter().any(|message| matches!(
            message,
            crate::session::client::ClientOutbound::PresentationAcks(_)
        )));
        let terminal_id = crate::pane::lifecycle::find_pane(backend.state(), pane_id)
            .unwrap()
            .terminal
            .stable_terminal_id();
        let report = tui_lipan::PresentedTerminal {
            stable_terminal_id: terminal_id,
            token: 7,
        };
        backend
            .dispatch(Msg::TerminalPresented(vec![report]))
            .unwrap();
        let sent: Vec<_> = outbound.try_iter().collect();
        assert!(sent.iter().any(|message| matches!(message, crate::session::client::ClientOutbound::PresentationAcks(acks)
            if acks == &[crate::session::protocol::PresentationAck { local: false, pane_id, generation, sequence: 7 }])));
        backend
            .dispatch(Msg::TerminalPresented(vec![report]))
            .unwrap();
        assert!(!outbound.try_iter().any(|message| matches!(
            message,
            crate::session::client::ClientOutbound::PresentationAcks(_)
        )));
        backend
            .dispatch(Msg::TerminalPresented(vec![tui_lipan::PresentedTerminal {
                stable_terminal_id: u64::MAX,
                token: 8,
            }]))
            .unwrap();
        assert!(!outbound.try_iter().any(|message| matches!(
            message,
            crate::session::client::ClientOutbound::PresentationAcks(_)
        )));
    });
}

/// The renderer keeps its startup cadence across config reloads, so the cadence it reports to the
/// session server must too: a reloaded `frame_rate` would otherwise throttle graphics to a rate the
/// UI is not presenting at.
#[test]
fn presentation_interest_reports_the_runtime_frame_rate_not_a_reloaded_one() {
    crate::test_support::on_large_stack(|| {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let (client, outbound) = SessionClient::test_channel();
        backend.state_mut().current_mut().session_client = Some(client);
        let runtime = backend.state().runtime_frame_rate();
        let reloaded = if runtime == 30 { 60 } else { 30 };
        backend.state_mut().config.frame_rate = reloaded;
        let pane_id = backend.state().current().focused_pane.unwrap();
        backend.dispatch(Msg::FocusPane(pane_id)).unwrap();

        let interest = &backend.state().current().presentation_interest;
        assert!(!interest.is_empty());
        assert!(
            interest
                .values()
                .all(|pane| pane.frames_per_second == runtime),
            "interest must follow the runtime cadence {runtime}, not {reloaded}: {interest:?}"
        );
        assert!(outbound.try_iter().any(|message| matches!(message,
                crate::session::client::ClientOutbound::PresentationInterest(panes)
                    if !panes.is_empty()
                        && panes.iter().all(|pane| pane.frames_per_second == runtime))));
    });
}

#[test]
fn attachment_keeps_the_remote_path_family_across_disconnect() {
    crate::test_support::on_large_stack(|| {
        use crate::platform::paths::PathFamily;
        let mut backend = TestBackend::new(crate::AppRoot::default());
        for family in [PathFamily::Windows, PathFamily::Posix] {
            let (mut client, _rx) = SessionClient::test_channel();
            client.path_family = family;
            backend.state_mut().current_mut().pending_session_attach =
                Some(crate::state::PendingSessionAttach {
                    epoch: 1,
                    request: crate::state::AttachRequest::new(
                        "remote-path-family",
                        crate::state::AttachMode::Join,
                    )
                    .on_host(Some(
                        crate::session::remote::RemoteTarget::Alias("session-host".into()),
                    )),
                    client: Some(client),
                    left: None,
                    parked_epoch: None,
                });
            backend
                .dispatch(Msg::SessionAttached {
                    epoch: 1,
                    session_instance: crate::session::protocol::SessionInstanceId::for_test(
                        "remote-path-family",
                    ),
                    session: "remote-path-family".into(),
                    client_id: 1,
                    panes: Vec::new(),
                    layout_rev: 0,
                    layout: None,
                    controller: Some(1),
                    clients: Vec::new(),
                    input_locked: false,
                    allow_takeover: false,
                    read_only: false,
                    origin: crate::session::origin::SessionOrigin::default(),
                })
                .expect("attach remote session");
            assert_eq!(backend.state().current().path_family, family);
            backend.state_mut().current_mut().mark_disconnected();
            assert_eq!(backend.state().current().path_family, family);
        }
    });
}

#[test]
fn restored_replay_waits_for_input_readiness_in_current_and_background_attachments() {
    crate::test_support::on_large_stack(|| {
        use crate::session::client::ClientOutbound;
        use crate::session::protocol::PaneCommandPhase;

        for background in [false, true] {
            for use_fallback in [false, true] {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (client, outbound) = SessionClient::test_channel();
                let epoch = backend.state().runtime_epoch;
                let pane_id = backend.state().current().workspaces[0].panes[0].id;
                let generation = 7;
                {
                    let attachment = backend.state_mut().current_mut();
                    attachment.workspaces[0].panes[0].pty_generation = generation;
                    attachment.session_client = Some(client);
                    attachment
                        .pending_replay_inputs
                        .insert((pane_id, generation), "printf restored-command".into());
                }
                if background {
                    backend
                        .state_mut()
                        .park_current(epoch, crate::state::Attachment::new());
                    backend.state_mut().runtime_epoch = epoch + 1;
                }
                let runtime = |sequence, command_phase| Msg::SessionPaneRuntimeChanged {
                    epoch,
                    pane_id,
                    local: false,
                    generation,
                    agents: Vec::new(),
                    state: PaneRuntimeState {
                        sequence,
                        command_phase,
                        ..PaneRuntimeState::default()
                    },
                };
                backend
                    .dispatch(runtime(1, PaneCommandPhase::Prompt))
                    .unwrap();
                let attachment = backend.state().attachment_for_epoch(epoch).unwrap();
                assert!(
                    attachment
                        .pending_replay_inputs
                        .contains_key(&(pane_id, generation))
                );
                assert!(
                    !outbound
                        .try_iter()
                        .any(|message| matches!(message, ClientOutbound::PaneInput { .. }))
                );

                if use_fallback {
                    backend
                        .dispatch(Msg::ReplayInputDeadline {
                            epoch,
                            pane_id,
                            generation,
                        })
                        .unwrap();
                } else {
                    backend
                        .dispatch(runtime(2, PaneCommandPhase::Input))
                        .unwrap();
                }
                // A subsequent readiness report or an already scheduled deadline must not
                // deliver the command a second time.
                backend
                    .dispatch(runtime(3, PaneCommandPhase::Input))
                    .unwrap();
                backend
                    .dispatch(Msg::ReplayInputDeadline {
                        epoch,
                        pane_id,
                        generation,
                    })
                    .unwrap();
                assert!(
                    backend
                        .state()
                        .attachment_for_epoch(epoch)
                        .unwrap()
                        .pending_replay_inputs
                        .is_empty()
                );
                let inputs: Vec<_> = outbound
                    .try_iter()
                    .filter_map(|message| match message {
                        ClientOutbound::PaneInput {
                            pane_id: target,
                            generation: actual_generation,
                            local,
                            bytes,
                            ..
                        } => {
                            assert_eq!(
                                (target, actual_generation, local),
                                (pane_id, generation, false)
                            );
                            Some(bytes)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(inputs, vec![b"printf restored-command\r".to_vec()]);
            }
        }
    });
}

/// Dispatch one runtime update for pane 1, generation 7, of the backend's on-screen session.
fn dispatch_runtime(
    backend: &mut TestBackend<crate::AppRoot>,
    agents: Vec<crate::session::protocol::AgentRecord>,
    state: PaneRuntimeState,
) {
    let epoch = backend.state().runtime_epoch;
    backend
        .dispatch(Msg::SessionPaneRuntimeChanged {
            epoch,
            pane_id: 1,
            local: true,
            generation: 7,
            agents,
            state,
        })
        .expect("dispatch runtime");
}

fn bind_first_pane(backend: &mut TestBackend<crate::AppRoot>) {
    let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
    pane.pty_generation = 7;
    pane.terminal.bind_session(pane.id, 7);
}

fn next_event(events: &std::sync::mpsc::Receiver<String>) -> serde_json::Value {
    serde_json::from_str(&events.try_recv().expect("an event")).unwrap()
}

/// An agent is reported as it appears, at each change of the state `agents list` gives it, and as
/// it goes; an update that leaves its state alone says nothing.
#[test]
fn agent_state_changes_emit_with_the_agents_list_state_and_ref() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let mut local = crate::state::Pane::new(1, 100, Default::default());
        local.pty_generation = 7;
        local.terminal.bind_session(1, 7);
        backend.state_mut().scratch.panes.push(local);
        let events = backend.state().event_hub.subscribe(Some(HashSet::from([
            crate::events::EventKind::AgentStateChanged,
        ])));
        let reference = crate::session::protocol::AgentRef::for_test(1, 7, None);
        let records = || vec![reference.clone().into()];
        let detected = |state, sequence| PaneRuntimeState {
            detected_agent: Some(crate::session::protocol::DetectedAgent {
                agent: crate::session::protocol::AgentIdentity::new("opencode", "OpenCode").into(),
                state,
            }),
            sequence,
            ..PaneRuntimeState::default()
        };
        use crate::session::protocol::DetectedAgentState::{Blocked, Working};

        dispatch_runtime(&mut backend, records(), detected(Working, 1));
        let appeared = next_event(&events);
        assert_eq!(appeared["event"], "agent-state-changed");
        assert_eq!(appeared["data"]["pane"], "1");
        assert_eq!(appeared["data"]["agent"], "opencode");
        assert_eq!(appeared["data"]["state"], "working");
        assert_eq!(appeared["data"]["previous_state"], "");
        let named: crate::session::protocol::AgentRef =
            serde_json::from_str(appeared["data"]["ref"].as_str().unwrap()).unwrap();
        assert_eq!(named, reference, "`ref` is what `agents get --ref` takes");

        dispatch_runtime(&mut backend, records(), detected(Blocked, 2));
        let blocked = next_event(&events);
        assert_eq!(blocked["data"]["state"], "blocked");
        assert_eq!(blocked["data"]["previous_state"], "working");

        // Another field changed; the agent's state did not.
        dispatch_runtime(
            &mut backend,
            records(),
            PaneRuntimeState {
                git_branch: Some("main".into()),
                ..detected(Blocked, 3)
            },
        );
        assert!(events.try_recv().is_err());

        dispatch_runtime(
            &mut backend,
            records(),
            PaneRuntimeState {
                sequence: 4,
                ..PaneRuntimeState::default()
            },
        );
        let gone = next_event(&events);
        assert_eq!(gone["data"]["state"], "");
        assert_eq!(gone["data"]["previous_state"], "blocked");
        assert!(events.try_recv().is_err());
    });
}

/// The running program is reported, not the field that keeps naming the last command at a
/// prompt; and the pane's shell in the foreground is the pane at its prompt.
#[test]
fn foreground_changes_report_the_running_program() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let mut local = crate::state::Pane::new(1, 100, Default::default());
        local.pty_generation = 7;
        local.terminal.bind_session(1, 7);
        backend.state_mut().scratch.panes.push(local);
        let events = backend.state().event_hub.subscribe(Some(HashSet::from([
            crate::events::EventKind::ForegroundChanged,
        ])));
        use crate::session::protocol::PaneCommandPhase;
        let foreground = |phase, program: &str, pid, sequence| PaneRuntimeState {
            command_phase: phase,
            foreground_program: Some(program.to_string()),
            foreground_pid: pid,
            sequence,
            ..PaneRuntimeState::default()
        };

        dispatch_runtime(
            &mut backend,
            Vec::new(),
            foreground(PaneCommandPhase::Executing, "cargo", Some(42), 1),
        );
        let started = next_event(&events);
        assert_eq!(started["event"], "foreground-changed");
        assert_eq!(started["data"]["pane"], "1");
        assert_eq!(started["data"]["program"], "cargo");
        assert_eq!(started["data"]["previous_program"], "");
        assert_eq!(started["data"]["pid"], "42");

        // Shell integration still names `cargo` at the prompt; nothing is running.
        dispatch_runtime(
            &mut backend,
            Vec::new(),
            foreground(PaneCommandPhase::Prompt, "cargo", None, 2),
        );
        let ended = next_event(&events);
        assert_eq!(ended["data"]["program"], "");
        assert_eq!(ended["data"]["previous_program"], "cargo");
        assert_eq!(ended["data"]["pid"], "");

        // Without integration the shell itself is in the foreground: still nothing running. The
        // server says which program is a shell, so a custom one on a remote host - `xonsh`, which
        // this client's configuration never names - is a shell all the same.
        for (shell, sequence) in [("bash", 3), ("xonsh", 4)] {
            dispatch_runtime(
                &mut backend,
                Vec::new(),
                PaneRuntimeState {
                    foreground_shell: true,
                    ..foreground(PaneCommandPhase::Unknown, shell, Some(7), sequence)
                },
            );
            assert!(events.try_recv().is_err(), "{shell} is the prompt");
        }

        // The same program in a new process group is a new foreground job.
        dispatch_runtime(
            &mut backend,
            Vec::new(),
            foreground(PaneCommandPhase::Unknown, "claude", Some(50), 5),
        );
        assert_eq!(next_event(&events)["data"]["program"], "claude");
        dispatch_runtime(
            &mut backend,
            Vec::new(),
            foreground(PaneCommandPhase::Unknown, "claude", Some(51), 6),
        );
        let restarted = next_event(&events);
        assert_eq!(restarted["data"]["previous_program"], "claude");
        assert_eq!(restarted["data"]["pid"], "51");
    });
}

/// A scratch pane and a session pane can share an id. `focused` names the one actually in focus:
/// the visible scratch pane, not the hidden session pane its id happens to match.
#[test]
fn runtime_events_say_focused_only_for_the_pane_in_focus_across_namespaces() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        bind_first_pane(&mut backend);
        assert_eq!(backend.state().current().focused_pane, Some(1));
        let mut scratch = crate::state::Pane::new(1, 100, Default::default());
        scratch.pty_generation = 7;
        scratch.terminal.bind_session(1, 7);
        let state = backend.state_mut();
        state.scratch.panes.push(scratch);
        state.scratch.focused_pane = Some(1);
        state.scratch_visible = true;
        let events = backend.state().event_hub.subscribe(Some(HashSet::from([
            crate::events::EventKind::ForegroundChanged,
        ])));
        let epoch = backend.state().runtime_epoch;
        let mut focused = |local| {
            backend
                .dispatch(Msg::SessionPaneRuntimeChanged {
                    epoch,
                    pane_id: 1,
                    local,
                    generation: 7,
                    agents: Vec::new(),
                    state: PaneRuntimeState {
                        command_phase: crate::session::protocol::PaneCommandPhase::Executing,
                        foreground_program: Some("cargo".into()),
                        sequence: 1,
                        ..PaneRuntimeState::default()
                    },
                })
                .expect("dispatch runtime");
            if !local {
                assert!(events.try_recv().is_err());
                backend
                    .update_level(Msg::SessionEvent {
                        epoch,
                        event: crate::session::server::events::SessionEvent {
                            session_instance: crate::session::protocol::SessionInstanceId::for_test(
                                "test",
                            ),
                            seq: 1,
                            kind: crate::events::EventKind::ForegroundChanged,
                            data: std::collections::BTreeMap::from([
                                ("pane".into(), "1".into()),
                                ("program".into(), "cargo".into()),
                            ]),
                        },
                    })
                    .unwrap();
            }
            next_event(&events)["data"]["focused"].clone()
        };

        assert_eq!(focused(true), "true", "the visible scratch pane");
        assert_eq!(
            focused(false),
            "false",
            "the hidden session pane with its id"
        );
    });
}

#[test]
fn background_session_events_keep_their_scope_and_reject_stale_sources() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let instance = crate::session::protocol::SessionInstanceId::for_test("background");
        let state = backend.state_mut();
        state.runtime_epoch = 9;
        state.current_mut().session_instance = Some(instance.clone());
        state.park_current(9, crate::state::Attachment::new());
        state.runtime_epoch = 10;
        let events = backend.state().event_hub.subscribe_bound(
            None,
            Some(crate::state::WorkerBinding {
                host: crate::state::HostKey::Local,
                session: Some(instance.clone()),
            }),
        );
        let event = crate::session::server::events::SessionEvent {
            session_instance: instance,
            seq: 1,
            kind: crate::events::EventKind::ForegroundChanged,
            data: std::collections::BTreeMap::from([("pane".into(), "1".into())]),
        };
        backend
            .update_level(Msg::SessionEvent {
                epoch: 9,
                event: event.clone(),
            })
            .unwrap();
        assert_eq!(next_event(&events)["data"]["focused"], "false");
        backend
            .update_level(Msg::SessionEvent {
                epoch: 99,
                event: event.clone(),
            })
            .unwrap();
        let mut wrong_instance = event;
        wrong_instance.session_instance =
            crate::session::protocol::SessionInstanceId::for_test("other");
        backend
            .update_level(Msg::SessionEvent {
                epoch: 9,
                event: wrong_instance,
            })
            .unwrap();
        assert!(events.try_recv().is_err());
    });
}

#[test]
fn notification_read_state_is_local_to_each_ui_and_session_instance() {
    crate::test_support::on_large_stack(|| {
        let mut first = TestBackend::new(crate::AppRoot::default());
        let mut second = TestBackend::new(crate::AppRoot::default());
        let instance = crate::session::protocol::SessionInstanceId::for_test("notifications");
        for backend in [&mut first, &mut second] {
            backend.state_mut().current_mut().session_instance = Some(instance.clone());
            for _ in 0..2 {
                backend
                    .update_level(Msg::SessionNotifications {
                        epoch: backend.state().runtime_epoch,
                        session_instance: instance.clone(),
                        session: "work".into(),
                        notifications: vec![crate::control::SessionNotification {
                            id: "notification-5".into(),
                            seq: 5,
                            level: crate::control::NotifyLevel::Info,
                            title: None,
                            message: "finished".into(),
                        }],
                    })
                    .unwrap();
                assert_eq!(backend.state().session_notification_cursors[&instance], 5);
                assert_eq!(backend.state().replaceable_toasts.len(), 1);
            }
        }
        first
            .state_mut()
            .session_notification_cursors
            .insert(instance.clone(), 8);
        assert_eq!(second.state().session_notification_cursors[&instance], 5);
    });
}

#[test]
fn deployed_actions_reach_the_palette_only_for_a_client_that_may_call_them() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let epoch = backend.state().runtime_epoch;
        backend
            .update_level(Msg::SessionActions {
                epoch,
                actions: vec![crate::control::PaletteAction {
                    id: "demo.plain".into(),
                    title: "Plain".into(),
                }],
            })
            .unwrap();
        assert_eq!(backend.state().current().session_actions.len(), 1);
        assert_eq!(
            crate::commands::offered_session_actions(backend.state().current()).len(),
            1
        );
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.read_only = true;
        backend.state_mut().current_mut().shared = Some(shared);
        assert!(crate::commands::offered_session_actions(backend.state().current()).is_empty());
        backend.state_mut().current_mut().shared = None;

        // Nothing to run it on: the person hears why rather than nothing happening.
        backend
            .update_level(Msg::RunPaletteAction("demo.plain".into()))
            .unwrap();
        assert!(
            backend
                .state()
                .replaceable_toasts
                .values()
                .any(|toast| toast.content().contains("not attached")),
        );

        backend
            .update_level(Msg::SessionActions {
                epoch,
                actions: Vec::new(),
            })
            .unwrap();
        assert!(backend.state().current().session_actions.is_empty());
    });
}
