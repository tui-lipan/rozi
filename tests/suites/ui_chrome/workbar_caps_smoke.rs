//! Production workbar caps and workspace tabs: edge glyphs, powerline chaining, pointer hit-testing
//! through the decorative overlay, and dragging one tab onto another. The cases render `AppRoot`,
//! not a hand-built stand-in for `view::workbar`.

use rozi::AppRoot;
use rozi::config::{WorkbarItem, WorkbarSegment};
use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::{CapStyle, KeyMods, Rect};
use tui_lipan::{CapturedCell, PointerShape, TestBackend};

const LEFT_CAP: &str = "\u{e0b6}";
const RIGHT_CAP: &str = "\u{e0b4}";

fn backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 8,
    });
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.config.sidebar.startup = false;
        state.sidebar.hide_all();
        state.config.nerd_icons = true;
        state.config.workbar.right.clear();
    }
    backend
}

fn row(backend: &mut TestBackend<AppRoot>) -> Vec<CapturedCell> {
    backend.render();
    let frame = backend.capture_frame();
    frame.cells[..frame.width as usize].to_vec()
}

fn click(backend: &mut TestBackend<AppRoot>, x: u16) {
    for kind in [
        MouseKind::Down(MouseButton::Left),
        MouseKind::Up(MouseButton::Left),
    ] {
        backend
            .send_mouse(MouseEvent {
                x,
                y: 0,
                kind,
                mods: KeyMods::NONE,
            })
            .expect("workbar click");
    }
}

fn find_run(row: &[CapturedCell], needle: &str) -> Option<usize> {
    let chars: Vec<char> = needle.chars().collect();
    row.windows(chars.len()).position(|window| {
        window
            .iter()
            .zip(&chars)
            .all(|(cell, ch)| cell.symbol == ch.to_string())
    })
}

#[test]
fn edge_caps_take_the_end_segment_colors_and_leave_the_title_in_place() {
    rozi::test_support::on_deeper_stack(|| {
        let mut capped = backend();
        capped.state_mut().config.pane.workbar_style = CapStyle::Round;
        let theme = capped.state().theme.clone();
        let cells = row(&mut capped);
        let left = &cells[0];
        let right = cells.last().expect("workbar row");
        assert_eq!(
            left.symbol, LEFT_CAP,
            "left cap occupies the title's padding cell"
        );
        assert_eq!(
            left.fg, theme.border_active,
            "left cap uses the title badge"
        );
        assert_eq!(
            left.bg, theme.surface.backdrop,
            "left cap sits on the backdrop"
        );
        assert_eq!(cells[1].symbol, "r", "the title stays in the next cell");
        assert_eq!(
            right.symbol, RIGHT_CAP,
            "right cap occupies the trailing cell"
        );
        assert_eq!(
            right.fg, theme.surface.panel,
            "a bar with no trailing badge keeps a panel-colored right cap"
        );
        assert_eq!(right.bg, theme.surface.backdrop);

        let mut flush = backend();
        flush.state_mut().config.pane.workbar_style = CapStyle::Padded;
        let cells = row(&mut flush);
        assert_eq!(cells[0].symbol, " ", "a padded bar keeps its leading space");
        assert_eq!(cells[1].symbol, "r");
        assert_ne!(cells[0].symbol, LEFT_CAP);
        assert_ne!(cells.last().expect("row").symbol, RIGHT_CAP);
    });
}

#[test]
fn trailing_badges_chain_only_while_powerline_is_on() {
    rozi::test_support::on_deeper_stack(|| {
        let prepare = |powerline: bool| {
            let mut backend = backend();
            {
                let state = backend.state_mut();
                state.config.pane.workbar_style = CapStyle::Round;
                state.config.pane.workbar_badge_style = CapStyle::Round;
                state.config.pane.workbar_powerline = powerline;
                state.do_not_disturb = true;
                state.config.workbar.right = vec![WorkbarItem {
                    segment: WorkbarSegment::Session,
                    color: None,
                    priority: None,
                }];
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().session_attached = true;
            }
            let theme = backend.state().theme.clone();
            let cells = row(&mut backend);
            (theme, cells)
        };

        let (theme, chained) = prepare(true);
        let dnd = find_run(&chained, "DND").expect("DND badge");
        let session = find_run(&chained, "dev").expect("session badge");
        let between = &chained[dnd + 3..session];
        let seam = between
            .iter()
            .find(|cell| cell.symbol == LEFT_CAP)
            .expect("powerline seam cap");
        assert_eq!(
            seam.fg, theme.border_active,
            "seam cap is the session color"
        );
        assert_eq!(
            seam.bg, theme.status.warning,
            "seam cap blends over the DND neighbor"
        );
        assert!(
            between.iter().all(|cell| cell.symbol != RIGHT_CAP),
            "chained badges do not close into separate pills"
        );

        let (theme, apart) = prepare(false);
        let dnd = find_run(&apart, "DND").expect("DND badge");
        let session = find_run(&apart, "dev").expect("session badge");
        let between = &apart[dnd + 3..session];
        let closing = between
            .iter()
            .find(|cell| cell.symbol == RIGHT_CAP)
            .expect("standalone DND chip rounds off");
        assert_eq!(closing.fg, theme.status.warning);
        assert_eq!(closing.bg, theme.surface.panel, "pill cap sits on the bar");
        assert!(
            between
                .iter()
                .any(|cell| cell.symbol == " " && cell.bg == theme.surface.panel),
            "powerline off leaves a gap between badges"
        );
    });
}

#[test]
fn cap_overlay_lets_workspace_tabs_and_the_title_receive_pointer_events() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend();
        backend.state_mut().config.pane.workbar_style = CapStyle::Round;
        let cells = row(&mut backend);
        let tab = find_run(&cells, "2").expect("second workspace tab") as u16;

        backend
            .send_mouse(MouseEvent {
                x: tab,
                y: 0,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("hover tab");
        assert_eq!(
            backend.pointer_shape_at(tab, 0),
            Some(PointerShape::Pointer),
            "the cap overlay must not hide the tab's hit target"
        );
        click(&mut backend, tab);
        assert_eq!(backend.state().current().active_workspace, 1);

        click(&mut backend, 0);
        assert!(
            backend.state().show_palette,
            "the left cap must pass the click through to the title badge"
        );
    });
}

#[test]
fn dropping_a_workspace_tab_moves_it_there_as_previewed_and_the_view_follows() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend();
        {
            let workspaces = &mut backend.state_mut().current_mut().workspaces;
            workspaces[0].name = Some("code".to_string());
            workspaces[2].name = Some("docs".to_string());
        }
        let cells = row(&mut backend);
        let source = find_run(&cells, "1:code").expect("first workspace tab") as u16;
        let target = find_run(&cells, "3:docs").expect("third workspace tab") as u16;

        for (x, kind) in [
            (source, MouseKind::Down(MouseButton::Left)),
            (source + 2, MouseKind::Drag(MouseButton::Left)),
            (target, MouseKind::Drag(MouseButton::Left)),
        ] {
            backend
                .send_mouse(MouseEvent {
                    x,
                    y: 0,
                    kind,
                    mods: KeyMods::NONE,
                })
                .expect("workbar drag");
        }

        // Before mouse release, workspaces have not moved in state, but the workbar previews the
        // dragged tab in the target slot with the tabs between shifted back to make room.
        assert_eq!(
            backend.state().current().workspaces[0].name.as_deref(),
            Some("code")
        );
        // Each previewed tab already carries the number it will have once dropped.
        let preview = row(&mut backend);
        let preview_one = find_run(&preview, "1").expect("emptied first slot in preview");
        let preview_docs = find_run(&preview, "2:docs").expect("docs renumbered in preview");
        let preview_code = find_run(&preview, "3:code").expect("dragged tab in previewed slot");
        assert!(
            preview_one < preview_docs && preview_docs < preview_code,
            "OnDrop drag previews the moved and renumbered tabs before mouse release"
        );
        for stale in ["1:code", "3:docs"] {
            assert!(
                find_run(&preview, stale).is_none(),
                "{stale} keeps its old number in the preview"
            );
        }

        backend
            .send_mouse(MouseEvent {
                x: target,
                y: 0,
                kind: MouseKind::Up(MouseButton::Left),
                mods: KeyMods::NONE,
            })
            .expect("workbar drop");

        // The drop lands where the preview put it: the dragged workspace third, the two it passed
        // one slot earlier each, in the same order the preview drew them.
        let workspaces = &backend.state().current().workspaces;
        assert_eq!(workspaces[0].name, None);
        assert_eq!(workspaces[1].name.as_deref(), Some("docs"));
        assert_eq!(workspaces[2].name.as_deref(), Some("code"));
        assert_eq!(
            backend.state().current().active_workspace,
            2,
            "the view follows the dragged workspace to its new number"
        );
        let cells = row(&mut backend);
        let text = |cells: &[CapturedCell]| -> String {
            cells.iter().map(|cell| cell.symbol.as_str()).collect()
        };
        assert_eq!(
            text(&cells),
            text(&preview),
            "the drop reads exactly as the preview did"
        );
        let one = find_run(&cells, "1").expect("the empty workspace now holds the first slot");
        let docs = find_run(&cells, "2:docs").expect("docs now holds the second slot");
        let code = find_run(&cells, "3:code").expect("code now holds the third slot");
        assert!(
            one < docs && docs < code,
            "tabs keep their numeric order and the previewed arrangement"
        );
    });
}

/// A workspace recording off screen keeps its dot ahead of its number, and keeps it there while
/// another tab is dragged past and the preview renumbers it.
#[test]
fn a_recording_dot_stays_ahead_of_the_number_a_preview_gives_its_workspace() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend();
        {
            let state = backend.state_mut();
            state.current_mut().workspaces[0].panes[0]
                .terminal
                .recording = true;
            state.current_mut().active_workspace = 1;
        }
        let dot = backend.state().config.recording_icon().to_string();
        let text = |cells: &[CapturedCell]| -> String {
            cells.iter().map(|cell| cell.symbol.as_str()).collect()
        };
        let cells = row(&mut backend);
        let recorded = find_run(&cells, &format!("{dot} 1 ·1"))
            .unwrap_or_else(|| panic!("dot ahead of the number in {:?}", text(&cells)));
        let source = find_run(&cells, "3").expect("third workspace tab") as u16;

        for (x, kind) in [
            (source, MouseKind::Down(MouseButton::Left)),
            (source - 2, MouseKind::Drag(MouseButton::Left)),
            (recorded as u16, MouseKind::Drag(MouseButton::Left)),
        ] {
            backend
                .send_mouse(MouseEvent {
                    x,
                    y: 0,
                    kind,
                    mods: KeyMods::NONE,
                })
                .expect("workbar drag");
        }
        let preview = row(&mut backend);
        assert!(
            find_run(&preview, &format!("{dot} 2 ·1")).is_some(),
            "the recorded workspace makes room and keeps its dot ahead of its new number, got {:?}",
            text(&preview)
        );
    });
}

#[test]
fn center_segments_stay_centered_as_the_sides_change() {
    rozi::test_support::on_deeper_stack(|| {
        let item = |segment| WorkbarItem {
            segment,
            color: None,
            priority: None,
        };
        let prepare = |right: Vec<WorkbarItem>, dnd: bool| {
            let mut backend = backend();
            {
                let state = backend.state_mut();
                state.config.pane.workbar_style = CapStyle::Round;
                state.config.pane.workbar_badge_style = CapStyle::Round;
                state.config.pane.workbar_powerline = true;
                state.do_not_disturb = dnd;
                state.config.workbar.center = vec![item(WorkbarSegment::Text("MID".into()))];
                state.config.workbar.right = right;
            }
            row(&mut backend)
        };

        let bare = prepare(Vec::new(), false);
        let mid = find_run(&bare, "MID").expect("center badge");
        // " MID " is 5 cells wide on a 100 cell bar, flanked by the symmetric pill caps.
        assert_eq!(mid, (100 - 5) / 2 + 1, "centered on the bar");
        assert_eq!(bare[mid - 1].symbol, LEFT_CAP, "pill caps on both sides");
        assert_eq!(bare[mid + 3].symbol, RIGHT_CAP);

        let busy = prepare(
            vec![item(WorkbarSegment::Text(
                "a much longer right side".into(),
            ))],
            true,
        );
        assert_eq!(
            find_run(&busy, "MID"),
            Some(mid),
            "unequal sides and mode chips never move the center"
        );
    });
}

#[test]
fn narrow_workbar_keeps_modes_and_session_before_optional_badges() {
    rozi::test_support::on_deeper_stack(|| {
        let mut b = backend();
        b.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 8,
        });
        let item = |segment| WorkbarItem {
            segment,
            color: None,
            priority: None,
        };
        {
            let state = b.state_mut();
            state.config.pane.workbar_style = CapStyle::Round;
            state.config.pane.workbar_badge_style = CapStyle::Round;
            state.config.workbar.center = vec![item(WorkbarSegment::Text("MID".into()))];
            state.config.workbar.right = vec![
                item(WorkbarSegment::Text("optional detail".into())),
                item(WorkbarSegment::Session),
            ];
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.mode = rozi::state::Mode::Resize;
            state.do_not_disturb = true;
        }
        let cells = row(&mut b);
        let text: String = cells.iter().map(|c| c.symbol.as_str()).collect();
        for label in ["MID", "RESIZE", "DND", "dev"] {
            assert!(find_run(&cells, label).is_some(), "{label} missing: {text}");
        }
        assert!(find_run(&cells, "optional").is_none(), "{text}");
        assert_eq!(
            cells.last().unwrap().fg,
            b.state().theme.border_active,
            "outer cap follows the retained session badge"
        );
        assert_eq!(find_run(&cells, "MID"), Some((60 - 5) / 2 + 1));
    });
}

#[test]
fn uncentered_workbar_charges_only_visible_section_gaps() {
    rozi::test_support::on_deeper_stack(|| {
        // Powerline chips abut; separate chips and left entries have one-cell gaps.
        // The left and right sections never require a gap between them.
        for (powerline, left_labels, width) in [
            (true, vec![], 13),
            (false, vec![], 14),
            (true, vec!["L"], 16),
            (false, vec!["L"], 17),
            (true, vec!["L", "M"], 20),
        ] {
            let mut b = backend();
            b.set_viewport(Rect {
                x: 0,
                y: 0,
                w: width,
                h: 8,
            });
            {
                let state = b.state_mut();
                state.config.pane.workbar_powerline = powerline;
                state.config.workbar.left = left_labels
                    .iter()
                    .map(|label| WorkbarItem {
                        segment: WorkbarSegment::Text((*label).into()),
                        color: None,
                        priority: None,
                    })
                    .collect();
                state.config.workbar.center.clear();
                state.mode = rozi::state::Mode::Resize;
                state.do_not_disturb = true;
            }
            let cells = row(&mut b);
            let text: String = cells.iter().map(|cell| cell.symbol.as_str()).collect();
            for label in left_labels.into_iter().chain(["RESIZE", "DND"]) {
                assert!(
                    find_run(&cells, label).is_some(),
                    "{label} missing at {width} cells: {text}"
                );
            }
            b.set_viewport(Rect {
                x: 0,
                y: 0,
                w: width - 1,
                h: 8,
            });
            let cells = row(&mut b);
            let omitted = if width <= 14 {
                "DND"
            } else if width == 20 {
                "M"
            } else {
                "L"
            };
            assert!(
                find_run(&cells, omitted).is_none(),
                "{omitted} must yield below exact fit"
            );
        }
    });
}

#[test]
fn explicit_priority_keeps_a_later_badge_whole() {
    rozi::test_support::on_deeper_stack(|| {
        for (width, centered) in [(30, true), (15, false)] {
            let mut b = backend();
            b.set_viewport(Rect {
                x: 0,
                y: 0,
                w: width,
                h: 8,
            });
            let item = |label: &str, priority| WorkbarItem {
                segment: WorkbarSegment::Text(label.into()),
                color: None,
                priority,
            };
            if centered {
                b.state_mut().config.workbar.center = vec![item("MID", None)];
            }
            b.state_mut().config.workbar.right =
                vec![item("FIRST", Some(1)), item("SECOND", Some(220))];
            let cells = row(&mut b);
            assert!(find_run(&cells, "FIRST").is_none(), "width {width}");
            assert!(find_run(&cells, "SECOND").is_some(), "width {width}");
        }
    });
}

#[test]
fn narrow_workspace_tabs_scroll_with_the_shared_arrow_controls() {
    rozi::test_support::on_deeper_stack(|| {
        let mut b = backend();
        b.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 40,
            h: 8,
        });
        {
            let state = b.state_mut();
            state.config.pane.workbar_style = CapStyle::Round;
            state.config.workbar.center = vec![WorkbarItem {
                segment: WorkbarSegment::Text("MID".into()),
                color: None,
                priority: None,
            }];
            state.config.workbar.left = vec![WorkbarItem {
                segment: WorkbarSegment::Workspaces,
                color: None,
                priority: None,
            }];
        }
        let initial = row(&mut b);
        let right = find_run(&initial, "❯").expect("shared right overflow arrow") as u16;
        click(&mut b, right);
        let after = row(&mut b);
        let left = find_run(&after, "❮").expect("left arrow survives the workbar cap") as u16;
        assert!(
            find_run(&after, "5").is_some(),
            "scroll reaches the last workspace"
        );
        assert_eq!(
            b.state().current().active_workspace,
            0,
            "scrolling does not select a workspace"
        );
        click(&mut b, left);
        assert!(find_run(&row(&mut b), "1 ·1").is_some());
        b.send_mouse(MouseEvent {
            x: right,
            y: 0,
            kind: MouseKind::ScrollDown,
            mods: KeyMods::NONE,
        })
        .expect("scroll workspace tabs");
        assert!(
            find_run(&row(&mut b), "5").is_some(),
            "wheel reaches hidden tabs"
        );
        b.dispatch(rozi::Msg::RunAction(rozi::input::Action::SwitchWorkspace(
            4,
        )))
        .expect("switch workspace");
        assert!(
            find_run(&row(&mut b), "5").is_some(),
            "active workspace is revealed"
        );
    });
}

#[test]
fn center_pills_chain_under_powerline_and_keep_their_cap_colors_across_the_bar() {
    rozi::test_support::on_deeper_stack(|| {
        // " AAA " and " BBB " fill the bar exactly: abutting under powerline, one gap apart without.
        for (powerline, width) in [(true, 10), (false, 11)] {
            let mut b = backend();
            b.set_viewport(Rect {
                x: 0,
                y: 0,
                w: width,
                h: 8,
            });
            {
                let state = b.state_mut();
                state.config.pane.workbar_style = CapStyle::Round;
                state.config.pane.workbar_badge_style = CapStyle::Round;
                state.config.pane.workbar_powerline = powerline;
                state.config.workbar.left.clear();
                state.config.workbar.center = vec![
                    WorkbarItem {
                        segment: WorkbarSegment::Text("AAA".into()),
                        color: Some(rozi::config::BadgeColor::Info),
                        priority: None,
                    },
                    WorkbarItem {
                        segment: WorkbarSegment::Text("BBB".into()),
                        color: Some(rozi::config::BadgeColor::Success),
                        priority: None,
                    },
                ];
            }
            let cells = row(&mut b);
            let status = &b.state().theme.status;
            let last = usize::from(width) - 1;
            assert_eq!(cells[0].symbol, LEFT_CAP);
            assert_eq!(cells[0].fg, status.info);
            assert_eq!(cells[last].symbol, RIGHT_CAP);
            assert_eq!(cells[last].fg, status.success);
            assert_eq!(find_run(&cells, "AAA"), Some(1));
            let seam = &cells[5];
            if powerline {
                assert_eq!(find_run(&cells, "BBB"), Some(6));
                assert_eq!(seam.symbol, LEFT_CAP, "powerline joins the center pills");
                assert_eq!((seam.fg, seam.bg), (status.success, status.info));
            } else {
                assert_eq!(find_run(&cells, "BBB"), Some(7));
                assert_eq!(seam.bg, b.state().theme.surface.panel, "pills stand apart");
            }
        }
    });
}
