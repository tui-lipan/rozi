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
    backend.state_mut().config.animations.picker = rozi::layout::anim::PickerAnimationStyle::Off;
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
        lock: None,
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
fn picker_hides_remote_paths_and_keeps_the_association_for_opening() {
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
            !frame.contains("C:\\code\\repo-worktrees\\feature"),
            "{frame}"
        );
        assert!(
            !frame.contains("review"),
            "session names stay out of the status column: {frame}"
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

#[test]
fn removal_confirmation_preserves_branch_padding_and_current_marker() {
    on_large_stack(|| {
        use rozi::state::{PendingWorktreeRemove, PendingWorktreeRemoveKind};

        for current in [false, true] {
            let mut backend = picker();
            if current {
                let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
                picker.cwd = picker.entries[0].path.clone();
            }
            backend.render();
            let before = backend.capture_frame().plain_text();
            let before = before
                .lines()
                .find(|line| line.contains("feat/worktrees"))
                .unwrap();
            let prefix = before.split_once("feat/worktrees").unwrap().0;
            assert_eq!(prefix.contains('●'), current);

            for kind in [
                PendingWorktreeRemoveKind::Clean,
                PendingWorktreeRemoveKind::Dirty,
                PendingWorktreeRemoveKind::StaleLock,
            ] {
                let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
                picker.pending_remove = Some(PendingWorktreeRemove {
                    path: picker.entries[0].path.clone(),
                    kind,
                });
                backend.render();
                let armed = backend.capture_frame().plain_text();
                let row = armed
                    .lines()
                    .find(|line| line.contains("feat/worktrees"))
                    .unwrap();
                assert_eq!(
                    row.split_once("feat/worktrees").unwrap().0,
                    prefix,
                    "{armed}"
                );
                assert!(row.contains("again to"), "{armed}");
            }

            backend
                .state_mut()
                .worktree_picker
                .as_mut()
                .unwrap()
                .pending_remove = None;
            backend.render();
            let after = backend.capture_frame().plain_text();
            assert_eq!(
                after
                    .lines()
                    .find(|line| line.contains("feat/worktrees"))
                    .unwrap(),
                before
            );
        }
    });
}

/// Paths do not consume row width, and the picker is clamped on a narrow terminal.
#[test]
fn picker_hides_long_paths_at_wide_and_narrow_viewports() {
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
                    lock: None,
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
            !frame.contains("rozi/.claude/worktrees/session-fade-duration"),
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

/// Compact PR status and checkout locks remain readable in a narrow picker.
#[test]
fn narrow_picker_keeps_work_status_and_lock() {
    on_large_stack(|| {
        let mut backend = picker();
        {
            let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
            picker.target = None;
            picker.cwd = "/home/me/src/rozi".into();
            picker.sessions.clear();
            picker.entries[0].path =
                "/home/me/src/rozi/.claude/worktrees/session-fade-duration".into();
            picker.entries[0].branch = Some("feat/narrow-layout".into());
            picker.statuses.checkouts.insert(
                picker.entries[0].path.clone(),
                rozi::git::pull_requests::PullRequestStatus {
                    number: 114,
                    status: rozi::git::pull_requests::WorkStatus::Passed,
                },
            );
            picker.entries[0].lock = Some(rozi::git::worktrees::WorktreeLock {
                reason: String::new(),
                stale: false,
            });
        }
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 20,
        });
        backend.render();
        let frame = backend.capture_frame().plain_text();
        let row = frame
            .lines()
            .find(|line| line.contains("feat/narrow-layout"))
            .unwrap_or_else(|| panic!("the branch is kept whole: {frame}"));
        assert!(!row.contains("duration"), "{frame}");
        assert!(row.contains("#114 ✓ · locked"), "{frame}");
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
            lock: None,
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

#[test]
fn status_replies_are_scoped_cached_and_shared_with_the_sidebar() {
    on_large_stack(|| {
        use rozi::git::pull_requests::{PullRequestStatus, WorkStatus, WorktreeStatuses};
        let mut backend = picker();
        let epoch = backend.state().runtime_epoch;
        let target = RemoteTarget::Alias("workbox".into());
        let cwd = "C:\\code\\repo";
        let statuses = WorktreeStatuses {
            checkouts: [(
                "C:\\code\\repo-worktrees\\feature".into(),
                PullRequestStatus {
                    number: 112,
                    status: WorkStatus::Merged,
                },
            )]
            .into(),
            unavailable: false,
        };
        backend.state_mut().sidebar.worktrees.source = Some((Some(target.clone()), cwd.into()));
        backend
            .state_mut()
            .worktree_picker
            .as_mut()
            .unwrap()
            .pending_status = Some(20);
        for (reply_epoch, id) in [(epoch, 19), (epoch.wrapping_add(1), 20)] {
            backend
                .dispatch(Msg::SessionWorktreeResult {
                    epoch: reply_epoch,
                    request_id: id,
                    result: WorktreeResult::Statuses {
                        statuses: statuses.clone(),
                    },
                })
                .unwrap();
            assert!(
                backend
                    .state()
                    .worktree_picker
                    .as_ref()
                    .unwrap()
                    .statuses
                    .checkouts
                    .is_empty()
            );
        }
        backend
            .dispatch(Msg::SessionWorktreeResult {
                epoch,
                request_id: 20,
                result: WorktreeResult::Statuses {
                    statuses: statuses.clone(),
                },
            })
            .unwrap();
        assert_eq!(
            backend.state().worktree_picker.as_ref().unwrap().statuses,
            statuses
        );
        assert_eq!(backend.state().sidebar.worktrees.statuses, statuses);
        assert_eq!(
            backend.state().worktree_statuses.get(Some(&target), cwd),
            statuses
        );
        assert!(
            backend
                .state()
                .worktree_statuses
                .get(None, cwd)
                .checkouts
                .is_empty()
        );
        backend.render();
        assert!(backend.capture_frame().plain_text().contains("#112 merged"));
    });
}

#[test]
fn hidden_remote_paths_remain_searchable_and_copyable_by_key_and_click() {
    on_large_stack(|| {
        use std::cell::RefCell;
        use std::rc::Rc;
        use tui_lipan::prelude::{App, KeyCode, KeyEvent};
        use tui_lipan::{ClipboardError, ClipboardProvider};

        struct Clipboard(Rc<RefCell<String>>);
        impl ClipboardProvider for Clipboard {
            fn read_clipboard_text(&mut self) -> Result<String, ClipboardError> {
                Ok(self.0.borrow().clone())
            }
            fn write_clipboard_text(&mut self, text: &str) -> Result<(), ClipboardError> {
                *self.0.borrow_mut() = text.into();
                Ok(())
            }
        }

        let copied = Rc::new(RefCell::new(String::new()));
        let state = picker().state_mut().worktree_picker.take();
        let mut backend = TestBackend::new_with_app(
            App::new()
                .clipboard_provider(Clipboard(copied.clone()))
                .clipboard_config(tui_lipan::ClipboardConfig {
                    enable_osc52: false,
                    ..Default::default()
                }),
            AppRoot::default(),
            (),
        );
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        backend.state_mut().config.animations.picker =
            rozi::layout::anim::PickerAnimationStyle::Off;
        backend.state_mut().worktree_picker = state;
        backend.render();
        assert!(backend.focus_key(&"rozi-worktree-picker".into()));
        for query in ["repo-worktrees", "repo feature"] {
            backend
                .dispatch(Msg::WorktreeQueryChanged(query.into()))
                .unwrap();
            backend.render();
            let frame = backend.capture_frame().plain_text();
            assert!(
                frame.contains("feat/worktrees"),
                "path aliases find the branch: {frame}"
            );
            assert!(!frame.contains("C:\\code"), "paths stay hidden: {frame}");
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char('c'),
                    mods: KeyMods::CTRL,
                })
                .unwrap();
            assert_eq!(&*copied.borrow(), "C:\\code\\repo-worktrees\\feature");
        }
        copied.borrow_mut().clear();
        click_hint(&mut backend, "copy path");
        assert_eq!(&*copied.borrow(), "C:\\code\\repo-worktrees\\feature");
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
        backend.render();
        assert!(!backend.capture_frame().plain_text().contains("Esc"));
        assert!(backend.focus_key(&"rozi-worktree-form-input".into()));
        backend
            .send_key(tui_lipan::prelude::KeyEvent {
                code: tui_lipan::prelude::KeyCode::Esc,
                mods: KeyMods::NONE,
            })
            .unwrap();
        assert!(!form_open(&backend), "Esc cancels the form");
        assert!(
            backend.state().worktree_picker.is_some(),
            "cancelling the form returns to the picker"
        );
        backend.render();
        assert!(!backend.capture_frame().plain_text().contains("Esc"));
        assert!(backend.focus_key(&"rozi-worktree-picker".into()));
        backend
            .send_key(tui_lipan::prelude::KeyEvent {
                code: tui_lipan::prelude::KeyCode::Esc,
                mods: KeyMods::NONE,
            })
            .unwrap();
        assert!(
            backend.state().worktree_picker.is_none(),
            "Esc closes the picker"
        );
    });
}
