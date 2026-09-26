use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::session::origin::{SessionOrigin, WorktreeOrigin};
use rozi::session::protocol::WorktreeResult;
use rozi::session::remote::RemoteTarget;
use rozi::state::WorktreePickerState;
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::{KeyMods, Rect};

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

fn picker() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    let target = RemoteTarget::Alias("workbox".into());
    let path = "C:\\code\\repo-worktrees\\feature";
    let mut picker = WorktreePickerState::new("C:\\code\\repo".into(), Some(target.clone()));
    picker.entries = vec![rozi::git::worktrees::WorktreeInfo {
        path: path.into(),
        branch: Some("feat/worktrees".into()),
        detached: false,
        bare: false,
        prunable: false,
        linked: true,
        locked: false,
    }];
    picker.sessions.push(DiscoveredSession {
        name: "review".into(),
        origin: SessionOrigin {
            worktree: Some(WorktreeOrigin { path: path.into() }),
            ..Default::default()
        },
        ephemeral: false,
        host: Some(target.display_label()),
        remote_target: Some(target),
        status: DiscoveredSessionStatus::Restorable,
    });
    backend.state_mut().worktree_picker = Some(picker);
    backend
}

#[test]
fn picker_keeps_remote_paths_opaque_and_shows_restorable_association() {
    on_large_stack(|| {
        let mut backend = picker();
        backend.render();
        let frame = backend.capture_frame().plain_text();
        assert!(
            frame.contains("Worktrees · workbox"),
            "the host titles the picker: {frame}"
        );
        assert!(frame.contains("feat/worktrees"), "{frame}");
        assert!(
            frame.contains("C:\\code\\repo-worktrees\\feature"),
            "{frame}"
        );
        assert!(
            frame.contains("review"),
            "the associated session is named: {frame}"
        );
        backend.dispatch(Msg::WorktreeNew).unwrap();
        backend.render();
        let form = backend.capture_frame().plain_text();
        assert!(form.contains("New worktree"), "{form}");
        assert!(
            form.contains("Branch") && form.contains("Base") && form.contains("Path"),
            "{form}"
        );
    });
}

/// A checkout nested a few directories deep, next to its session, still fits in one row; and the
/// wider picker is clamped rather than overflowing a narrow terminal.
#[test]
fn picker_widens_to_fit_long_checkout_rows() {
    on_large_stack(|| {
        let mut backend = picker();
        let path = "/home/me/src/rozi/.claude/worktrees/session-fade-duration";
        {
            let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
            picker.target = None;
            picker.cwd = "/home/me/src/rozi".into();
            picker.sessions.clear();
            picker.entries.insert(
                0,
                rozi::git::worktrees::WorktreeInfo {
                    path: "/home/me/src/rozi".into(),
                    branch: Some("master".into()),
                    detached: false,
                    bare: false,
                    prunable: false,
                    linked: false,
                    locked: false,
                },
            );
            picker.entries[1].path = path.into();
            picker.entries[1].branch = Some("fix/config-test-race".into());
        }
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 140,
            h: 30,
        });
        backend.render();
        let frame = backend.capture_frame().plain_text();
        assert!(
            frame.contains("fix/config-test-race  rozi/.claude/worktrees/session-fade-duration"),
            "{frame}"
        );
        assert!(frame.contains("primary"), "{frame}");

        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 20,
        });
        backend.render();
        let narrow = backend.capture_frame().plain_text();
        assert!(narrow.contains("Worktrees"), "{narrow}");
        assert!(
            narrow.lines().all(|line| line.chars().count() <= 60),
            "{narrow}"
        );
    });
}

#[test]
fn stale_list_reply_does_not_replace_a_new_picker_request() {
    on_large_stack(|| {
        let mut backend = picker();
        let epoch = backend.state().runtime_epoch;
        backend
            .state_mut()
            .worktree_picker
            .as_mut()
            .unwrap()
            .pending_list = Some(20);
        backend
            .dispatch(Msg::SessionWorktreeResult {
                epoch,
                request_id: 19,
                result: WorktreeResult::Listed {
                    worktrees: vec![],
                    sessions: Default::default(),
                },
            })
            .unwrap();
        assert_eq!(
            backend
                .state()
                .worktree_picker
                .as_ref()
                .unwrap()
                .entries
                .len(),
            1
        );
        backend
            .dispatch(Msg::SessionWorktreeResult {
                epoch,
                request_id: 20,
                result: WorktreeResult::Listed {
                    worktrees: vec![],
                    sessions: Default::default(),
                },
            })
            .unwrap();
        assert!(
            backend
                .state()
                .worktree_picker
                .as_ref()
                .unwrap()
                .entries
                .is_empty()
        );
    });
}

/// A list reply is remembered per repository and host, so the next opening starts populated, and
/// the refresh keeps the selection on the same checkout even when rows move.
#[test]
fn listed_worktrees_are_cached_and_keep_the_selection() {
    on_large_stack(|| {
        let mut backend = picker();
        let epoch = backend.state().runtime_epoch;
        let tree = |path: &str, branch: &str, linked: bool| rozi::git::worktrees::WorktreeInfo {
            path: path.into(),
            branch: Some(branch.into()),
            detached: false,
            bare: false,
            prunable: false,
            linked,
            locked: false,
        };
        backend
            .state_mut()
            .worktree_picker
            .as_mut()
            .unwrap()
            .pending_list = Some(7);
        let refreshed = vec![
            tree("C:\\code\\repo", "main", false),
            tree("C:\\code\\repo-worktrees\\feature", "feat/worktrees", true),
        ];
        backend
            .dispatch(Msg::SessionWorktreeResult {
                epoch,
                request_id: 7,
                result: WorktreeResult::Listed {
                    worktrees: refreshed.clone(),
                    sessions: Default::default(),
                },
            })
            .unwrap();
        let state = backend.state();
        assert_eq!(state.worktree_picker.as_ref().unwrap().selected, 1);
        let target = RemoteTarget::Alias("workbox".into());
        assert_eq!(
            state.worktree_lists.get(Some(&target), "C:\\code\\repo"),
            Some(refreshed.as_slice())
        );
        assert_eq!(state.worktree_lists.get(None, "C:\\code\\repo"), None);
    });
}

#[cfg(feature = "ui-snapshot")]
#[test]
fn worktree_picker_visual_reference() {
    on_large_stack(|| {
        let mut backend = picker();
        {
            // The shape of a real repository with agent worktrees nested inside it.
            let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
            let tree = |path: &str, branch: &str, linked: bool, locked: bool| {
                rozi::git::worktrees::WorktreeInfo {
                    path: path.into(),
                    branch: Some(branch.into()),
                    detached: false,
                    bare: false,
                    prunable: false,
                    linked,
                    locked,
                }
            };
            picker.target = None;
            picker.cwd = "/home/me/src/rozi".into();
            picker.entries = vec![
                tree("/home/me/src/rozi", "master", false, false),
                tree(
                    "/home/me/src/rozi/.claude/worktrees/extensions-checking-spinner",
                    "worktree-extensions-checking-spinner",
                    true,
                    true,
                ),
                tree(
                    "/home/me/src/rozi/.claude/worktrees/session-fade-duration",
                    "fix/config-test-race",
                    true,
                    false,
                ),
                tree(
                    "/home/me/src/rozi-worktrees/feat-login",
                    "feat/login",
                    true,
                    false,
                ),
            ];
            picker.sessions[0].remote_target = None;
            picker.sessions[0].origin.worktree = Some(WorktreeOrigin {
                path: "/home/me/src/rozi-worktrees/feat-login".into(),
            });
            picker.selected = 2;
        }
        for (width, height) in [(72, 22), (100, 30), (140, 40)] {
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            });
            backend.render();
            let png = backend.capture_ui_snapshot().to_png_default().unwrap();
            let dir = std::path::Path::new("target/ui-sketches");
            std::fs::create_dir_all(dir).unwrap();
            let path = dir.join(format!("worktree-picker-{width}x{height}.png"));
            std::fs::write(&path, png).unwrap();
            println!("wrote {}", path.display());
        }
        backend.dispatch(Msg::WorktreeNew).unwrap();
        {
            let form = backend
                .state_mut()
                .worktree_picker
                .as_mut()
                .unwrap()
                .form
                .as_mut()
                .unwrap();
            form.branch.set_text("feat/login".to_string());
            form.path
                .set_text("/home/me/src/rozi/.worktrees/feat-login".to_string());
            form.unignored = Some(".worktrees".into());
        }
        for (width, height) in [(72, 22), (100, 30)] {
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            });
            backend.render();
            let png = backend.capture_ui_snapshot().to_png_default().unwrap();
            let path = std::path::Path::new("target/ui-sketches")
                .join(format!("worktree-form-{width}x{height}.png"));
            std::fs::write(&path, png).unwrap();
            println!("wrote {}", path.display());
        }
    });
}

/// Where the footer hint `label` starts, on the last row that carries it: hints sit below the
/// rows, so a row that happens to share the word never wins.
fn hint_at(backend: &mut TestBackend<AppRoot>, label: &str) -> (u16, u16) {
    backend.render();
    let lines = backend.capture_frame().to_fixed_grid_lines();
    let (y, x) = lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(y, line)| {
            let byte = line.find(&format!("{label} "))?;
            Some((y, line[..byte].chars().count()))
        })
        .unwrap_or_else(|| panic!("no `{label}` hint:\n{}", lines.join("\n")));
    (x as u16, y as u16)
}

fn mouse(backend: &mut TestBackend<AppRoot>, (x, y): (u16, u16), kinds: &[MouseKind]) {
    for &kind in kinds {
        backend
            .send_mouse(MouseEvent {
                x,
                y,
                kind,
                mods: KeyMods::NONE,
            })
            .expect("mouse event");
    }
    backend.render();
}

fn click_hint(backend: &mut TestBackend<AppRoot>, label: &str) {
    let at = hint_at(backend, label);
    mouse(
        backend,
        at,
        &[
            MouseKind::Down(MouseButton::Left),
            MouseKind::Up(MouseButton::Left),
        ],
    );
}

#[test]
fn footer_hints_lift_under_the_pointer() {
    on_large_stack(|| {
        let mut backend = picker();
        let at = hint_at(&mut backend, "refresh");
        let resting = backend.capture_frame().cell(at.0, at.1).bg;
        mouse(&mut backend, at, &[MouseKind::Moved]);
        assert_ne!(
            backend.capture_frame().cell(at.0, at.1).bg,
            resting,
            "a hovered hint lifts so it reads as clickable"
        );
    });
}

#[test]
fn footer_hints_click_through_to_their_key_action() {
    on_large_stack(|| {
        let mut backend = picker();
        click_hint(&mut backend, "new");
        let form_open = |backend: &TestBackend<AppRoot>| {
            backend
                .state()
                .worktree_picker
                .as_ref()
                .is_some_and(|picker| picker.form.is_some())
        };
        assert!(
            form_open(&backend),
            "the picker's `new` hint opens the form"
        );
        click_hint(&mut backend, "cancel");
        assert!(!form_open(&backend), "the form's `cancel` hint closes it");
        assert!(
            backend.state().worktree_picker.is_some(),
            "cancelling the form returns to the picker"
        );
        click_hint(&mut backend, "close");
        assert!(
            backend.state().worktree_picker.is_none(),
            "the picker's `close` hint closes it"
        );
    });
}
