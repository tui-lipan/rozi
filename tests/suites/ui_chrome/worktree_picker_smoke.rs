use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::session::origin::{SessionOrigin, WorktreeOrigin};
use rozi::session::protocol::WorktreeResult;
use rozi::session::remote::RemoteTarget;
use rozi::state::WorktreePickerState;
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

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
                result: WorktreeResult::Listed { worktrees: vec![] },
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
                result: WorktreeResult::Listed { worktrees: vec![] },
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

#[cfg(feature = "ui-snapshot")]
#[test]
fn worktree_picker_visual_reference() {
    on_large_stack(|| {
        let mut backend = picker();
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
