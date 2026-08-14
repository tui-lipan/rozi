use super::*;
use crate::config::{SidebarTab, SidebarTabId, UserCommandAction};
use crate::state::{Pane, SidebarCommandOutput, SidebarCommandRow};
use tui_lipan::TestBackend;

fn row(text: &str) -> SidebarCommandRow {
    SidebarCommandRow {
        raw: text.to_string(),
        display: text.to_string(),
        error: false,
    }
}

#[test]
fn unsplit_reorder_keeps_the_saved_panel_boundary() {
    let id = |name: &str| crate::config::SidebarTabId::new(name);
    let configured = vec![vec![id("agents")], vec![id("panes"), id("sessions")]];
    let displayed = vec![vec![id("panes"), id("agents"), id("sessions")]];

    assert_eq!(
        persisted_panel_ids(displayed, &configured, false),
        vec![vec![id("panes")], vec![id("agents"), id("sessions")]]
    );
}

fn discovered(name: &str) -> crate::session::discovery::DiscoveredSession {
    crate::session::discovery::DiscoveredSession {
        name: name.to_string(),
        ephemeral: false,
        host: None,
        remote_target: None,
        status: crate::session::discovery::DiscoveredSessionStatus::Running {
            panes: 1,
            clients: 0,
            has_layout: true,
            created_from_profile: None,
        },
    }
}

/// Mount a sidebar backend and settle its mount, so `state.command_link` is wired before the
/// test starts asserting.
///
/// The mount hands the link back from a background task, and a plain `dispatch` only drains
/// whatever has already been queued - it does not wait for that task. So a test that mounted
/// and asserted straight away was reading whichever side of the race the executor happened to
/// land on. With the link still missing, everything that sends through it silently no-ops:
/// `ensure_sessions_refresh_armed` (and with it the host-registry seed) and
/// `request_command_poll` both bail early. Under parallel load that lost race was frequent
/// enough to fail roughly one run in five.
///
/// Pumping until the link arrives makes both sides deterministic: tests that need it can rely
/// on it, and tests that need it *gone* have something real to drop.
fn settled_backend() -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while backend.state().command_link.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the mount never delivered the command link"
        );
        backend.pump().expect("settle the mount");
        std::thread::yield_now();
    }
    backend
}

fn show_files_tab(backend: &mut TestBackend<AppRoot>) {
    let tab = SidebarTab::Tree {
        view: crate::config::SidebarTreeView::Files,
        config: crate::config::SidebarTreeConfig::for_view(crate::config::SidebarTreeView::Files),
    };
    let id = tab.id();
    let state = backend.state_mut();
    state.sidebar_visible = true;
    state.sidebar.panels[0].tabs = vec![id.clone()];
    state.sidebar.panels[0].active_tab = Some(id);
    state.config.sidebar.tabs = vec![tab];
}

#[test]
fn visible_tree_refresh_ticks_both_sources_and_reject_stale_chains() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        show_files_tab(&mut backend);
        backend.state_mut().sidebar.tree_refresh_armed_epoch = Some(7);
        backend.state_mut().sidebar.tree_entry_refresh_token = 3;
        backend.state_mut().sidebar.git_refresh_token = 5;

        backend
            .dispatch(crate::Msg::SidebarTreeRefresh { epoch: 6 })
            .expect("stale tick dispatches");
        assert_eq!(backend.state().sidebar.tree_entry_refresh_token, 3);
        assert_eq!(backend.state().sidebar.git_refresh_token, 5);

        backend
            .dispatch(crate::Msg::SidebarTreeRefresh { epoch: 7 })
            .expect("current tick dispatches");
        assert_eq!(backend.state().sidebar.tree_entry_refresh_token, 4);
        assert_eq!(backend.state().sidebar.git_refresh_token, 6);
        assert_eq!(backend.state().sidebar.tree_refresh_armed_epoch, Some(7));
    });
}

#[test]
fn tree_refresh_loop_arms_once_and_disarms_when_hidden() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        show_files_tab(&mut backend);

        backend
            .dispatch(crate::Msg::SidebarTreeFocused)
            .expect("tree focus dispatches");
        let epoch = backend
            .state()
            .sidebar
            .tree_refresh_armed_epoch
            .expect("visible tree arms refresh");

        backend
            .dispatch(crate::Msg::SidebarTreeFocused)
            .expect("second tree focus dispatches");
        assert_eq!(
            backend.state().sidebar.tree_refresh_armed_epoch,
            Some(epoch),
            "repeated updates do not fork the chain"
        );

        backend.state_mut().sidebar_visible = false;
        backend
            .dispatch(crate::Msg::SidebarTreeRefresh { epoch })
            .expect("hidden tick dispatches");
        assert_eq!(backend.state().sidebar.tree_refresh_armed_epoch, None);
    });
}

/// Open the Sessions tab with its auto-refresh loop disarmed, for a test that drives discovery
/// by hand.
///
/// Armed, the loop kicks a *real* discovery sweep onto a background thread under the same
/// epoch the test dispatches, and whichever of the two lands last wins - so an assertion about
/// the resulting rows races the machine's actual sessions. Dropping the command link stops it:
/// `ensure_sessions_refresh_armed` and `request_sessions_refresh` both need one to send
/// through.
///
/// The order matters. [`settled_backend`] is what makes the link there to drop, and the tab
/// must still be closed while it settles - `command_link_ready` kicks an immediate sweep when
/// it finds the tab already open.
///
/// Only for tests that assert on discovered rows. Anything exercising a flow that sends
/// through the link needs it left alone.
fn open_sessions_tab_unswept(backend: &mut TestBackend<AppRoot>, epoch: u64) {
    assert!(
        backend.state().command_link.is_some(),
        "settle the mount with `settled_backend` before disarming the loop"
    );
    let state = backend.state_mut();
    state.sidebar_visible = true;
    state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
    state.sidebar.sessions_epoch = epoch;
    state.command_link = None;
}

fn on_test_thread(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(test)
        .expect("spawn sidebar test")
        .join()
        .expect("sidebar test completes");
}

/// Pretend an attach is already in flight so PTY-creating sidebar actions queue into
/// `pending_spawns` instead of starting a real ephemeral (see `needs_session_for_pty`).
fn hold_attach_open(backend: &mut TestBackend<AppRoot>) {
    backend.state_mut().current_mut().pending_session_attach =
        Some(crate::state::PendingSessionAttach {
            epoch: backend.state().runtime_epoch,
            name: "test".to_string(),
            client: None,
            autostart: false,
            read_only: false,
            reconnect: false,
            remote_host: None,
            intent: crate::state::AttachIntent::Plain,
            left: None,
            parked_epoch: None,
        });
}

/// A scratch directory scoped to this process and test name, matching how the rest of the
/// suite isolates filesystem fixtures. Removed first so a previous crashed run cannot leak in.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rozi-sidebar-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A repository whose `.git` is a plain file, the shape git uses for worktrees and submodules.
/// Testing existence rather than directory-ness is what makes those work.
#[test]
fn repo_root_walks_ancestors_and_accepts_a_git_file() {
    let dir = ScratchDir::new("gitfile");
    let repo = dir.path().join("repo");
    let nested = repo.join("src/deep");
    std::fs::create_dir_all(&nested).expect("nested dirs");
    std::fs::write(repo.join(".git"), "gitdir: /elsewhere").expect("git file");

    let repo_str = repo.to_string_lossy().into_owned();
    assert_eq!(
        crate::platform::paths::discover_project_root(&nested.to_string_lossy()),
        Some(repo_str.clone())
    );
    assert_eq!(
        crate::platform::paths::discover_project_root(&repo_str),
        Some(repo_str.clone())
    );
    assert_eq!(
        crate::platform::paths::discover_project_root(&dir.path().to_string_lossy()),
        None
    );
    assert_eq!(
        crate::platform::paths::display_cwd(&nested.to_string_lossy()),
        std::path::Path::new("repo")
            .join("src/deep")
            .to_string_lossy()
    );
    assert_eq!(crate::platform::paths::display_cwd(&repo_str), "repo");
}

/// The roots follow the focused pane, and a repeated directory report — which a shell emits at
/// every prompt — must not redo the ancestor walk or churn the git refresh token.
#[test]
fn tree_roots_track_the_focused_pane_and_settle_when_unchanged() {
    let dir = ScratchDir::new("roots");
    let repo = dir.path().join("repo");
    let nested = repo.join("src");
    std::fs::create_dir_all(&nested).expect("nested dirs");
    std::fs::create_dir_all(repo.join(".git")).expect("git dir");
    let nested = nested.to_string_lossy().into_owned();
    let repo = repo.to_string_lossy().into_owned();
    let outside = dir.path().to_string_lossy().into_owned();

    on_test_thread(move || {
        let mut backend = settled_backend();
        let pane = backend.state().current().workspaces[0].panes[0].id;
        {
            let state = backend.state_mut();
            state.current_mut().focused_pane = Some(pane);
            state.current_mut().workspaces[0].focused_pane = Some(pane);
            state.current_mut().workspaces[0].panes[0].terminal.cwd = Some(nested.clone());
        }
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("sync runs after any message");
        assert_eq!(backend.state().sidebar.tree_cwd.as_deref(), Some(&*nested));
        assert_eq!(backend.state().sidebar.tree_repo.as_deref(), Some(&*repo));
        let settled = backend.state().sidebar.git_refresh_token;
        assert!(settled > 0, "resolving a root schedules a git refresh");

        // Same directory reported again: no walk, no refresh.
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("repeat sync");
        assert_eq!(backend.state().sidebar.git_refresh_token, settled);

        // Attachment identity is part of the source even when another session reports the
        // same path. Never retain one remote host's provided rows under another host.
        backend
            .state_mut()
            .sidebar
            .tree_listings
            .push(FileTreeDirectoryListing::error(
                nested.clone(),
                "old attachment",
            ));
        backend.state_mut().runtime_epoch = 99;
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("attachment change sync");
        assert!(backend.state().sidebar.tree_listings.is_empty());
        assert_eq!(backend.state().sidebar.tree_source_epoch, Some(99));
        let settled = backend.state().sidebar.git_refresh_token;

        // Leaving the repository clears the repo root but keeps the working directory.
        backend.state_mut().current_mut().workspaces[0].panes[0]
            .terminal
            .cwd = Some(outside.clone());
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("cwd change sync");
        assert_eq!(backend.state().sidebar.tree_cwd.as_deref(), Some(&*outside));
        assert_eq!(backend.state().sidebar.tree_repo, None);
        assert!(backend.state().sidebar.git_refresh_token > settled);
    });
}

#[test]
fn stale_remote_tree_results_cannot_repopulate_a_replaced_root() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let epoch = backend.state().runtime_epoch;
        backend
            .state_mut()
            .sidebar
            .tree_pending
            .insert("/new".to_string());
        backend.state_mut().sidebar.tree_changes_pending_root = Some("/new".to_string());

        backend
            .dispatch(crate::Msg::SessionDirectoryListing {
                epoch,
                path: "/old".to_string(),
                entries: Vec::new(),
                error: None,
            })
            .expect("stale directory result");
        backend
            .dispatch(crate::Msg::SessionChangeListing {
                epoch,
                root: "/old".to_string(),
                changes: Vec::new(),
                error: None,
            })
            .expect("stale changes result");

        assert!(backend.state().sidebar.tree_listings.is_empty());
        assert!(backend.state().sidebar.tree_changes_root.is_none());
        assert!(backend.state().sidebar.tree_pending.contains("/new"));
        assert_eq!(
            backend.state().sidebar.tree_changes_pending_root.as_deref(),
            Some("/new")
        );
    });
}

#[test]
fn transient_remote_refresh_errors_keep_the_last_successful_data() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let epoch = backend.state().runtime_epoch;
        backend
            .state_mut()
            .sidebar
            .tree_listings
            .push(FileTreeDirectoryListing::new(
                "/repo",
                [FileTreeEntry::file("kept.rs")],
            ));
        backend.state_mut().sidebar.tree_changes_root = Some("/repo".to_string());
        backend
            .state_mut()
            .sidebar
            .tree_changes
            .push(FileTreeChange::new(
                "kept.rs",
                FileTreeChangeStatus::Modified,
            ));
        backend
            .state_mut()
            .sidebar
            .tree_pending
            .insert("/repo".to_string());
        backend.state_mut().sidebar.tree_changes_pending_root = Some("/repo".to_string());

        backend
            .dispatch(crate::Msg::SessionDirectoryListing {
                epoch,
                path: "/repo".to_string(),
                entries: Vec::new(),
                error: Some("retry later".to_string()),
            })
            .expect("directory refresh error");
        backend
            .dispatch(crate::Msg::SessionChangeListing {
                epoch,
                root: "/repo".to_string(),
                changes: Vec::new(),
                error: Some("retry later".to_string()),
            })
            .expect("change refresh error");

        assert_eq!(backend.state().sidebar.tree_listings.len(), 1);
        assert_eq!(backend.state().sidebar.tree_changes.len(), 1);

        backend.state_mut().sidebar.tree_changes.clear();
        backend.state_mut().sidebar.tree_changes_root = None;
        backend.state_mut().sidebar.tree_changes_pending_root = Some("/other".to_string());
        backend
            .dispatch(crate::Msg::SessionChangeListing {
                epoch,
                root: "/other".to_string(),
                changes: Vec::new(),
                error: Some("timed out".to_string()),
            })
            .expect("initial change error");
        assert_eq!(
            backend.state().sidebar.tree_changes_error.as_deref(),
            Some("timed out")
        );
        assert!(backend.state().sidebar.tree_changes_root.is_none());
    });
}

/// Git status is refreshed immediately on the edge into `Completed` — the moment a command
/// finished changing the working tree — without waiting for the periodic fallback.
#[test]
fn finishing_a_command_refreshes_git_status_once() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let pane = backend.state().current().workspaces[0].panes[0].id;
        {
            let state = backend.state_mut();
            state.current_mut().focused_pane = Some(pane);
            state.current_mut().workspaces[0].focused_pane = Some(pane);
            state.current_mut().workspaces[0].panes[0]
                .terminal
                .command_phase = crate::session::protocol::PaneCommandPhase::Executing;
        }
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("observe executing");
        let running = backend.state().sidebar.git_refresh_token;

        backend.state_mut().current_mut().workspaces[0].panes[0]
            .terminal
            .command_phase = crate::session::protocol::PaneCommandPhase::Completed {
            exit_status: Some(0),
        };
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("observe completion");
        let finished = backend.state().sidebar.git_refresh_token;
        assert_eq!(finished, running + 1, "one refresh per finished command");

        // Still completed on the next message: no repeat refresh.
        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
            .expect("steady state");
        assert_eq!(backend.state().sidebar.git_refresh_token, finished);
    });
}

/// Activating a file runs the tab's action; activating a directory only expands it in the
/// widget and must not run the action, and a stale config epoch drops the click entirely. A
/// dropped activation returns `Update::none()`, so `dispatch` reports no redraw.
#[test]
fn tree_activation_runs_for_files_and_skips_directories_and_stale_clicks() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Tree {
            view: crate::config::SidebarTreeView::Files,
            config: crate::config::SidebarTreeConfig::for_view(
                crate::config::SidebarTreeView::Files,
            ),
        }];
        backend.state_mut().sidebar.config_epoch = 6;
        let activate = |backend: &mut TestBackend<AppRoot>, is_dir: bool, epoch: u64| {
            backend
                .dispatch(crate::Msg::SidebarTreeActivate {
                    config_epoch: epoch,
                    tab_id: SidebarTabId::new("files"),
                    path: "/repo/src/main.rs".to_string(),
                    is_dir,
                })
                .expect("tree click")
        };

        // A file activation runs the action (a send, which redraws); a directory and a stale
        // click are both dropped without running anything.
        assert!(activate(&mut backend, false, 6), "file runs the action");
        assert!(!activate(&mut backend, true, 6), "directory only expands");
        assert!(!activate(&mut backend, false, 5), "stale epoch is dropped");
    });
}

/// A `run` action opens a pane whose command is untouched, with the activated path handed over
/// as `ROZI_FILE`. This is what lets a diff viewer be scoped to the clicked file without the
/// filename ever entering the command line: a repository can contain a file named
/// `; rm -rf ~`, and the spawned command string must not be able to carry it.
#[test]
fn tree_run_actions_pass_the_path_as_env_never_in_the_command() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let mut config =
            crate::config::SidebarTreeConfig::for_view(crate::config::SidebarTreeView::Changes);
        config.on_click = Some(UserCommandAction::run("git diff -- \"$ROZI_FILE\""));
        backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Tree {
            view: crate::config::SidebarTreeView::Changes,
            config,
        }];
        backend.state_mut().sidebar.config_epoch = 1;
        backend.state_mut().current_mut().pending_spawns.clear();
        hold_attach_open(&mut backend);

        let hostile = "/repo/; rm -rf ~/.rs";
        backend
            .dispatch(crate::Msg::SidebarTreeActivate {
                config_epoch: 1,
                tab_id: SidebarTabId::new("git"),
                path: hostile.to_string(),
                is_dir: false,
            })
            .expect("file click");

        let spawn = backend
            .state()
            .current()
            .pending_spawns
            .last()
            .cloned()
            .expect("run action queues a pane spawn");
        // The command is exactly what the config said — the path is nowhere in it.
        assert_eq!(spawn.command.as_deref(), Some("git diff -- \"$ROZI_FILE\""));
        assert!(
            !spawn.command.as_deref().unwrap().contains("rm -rf"),
            "the filename never reaches the command line"
        );
        // It arrives as environment instead, verbatim.
        assert!(
            spawn
                .env
                .iter()
                .any(|(key, value)| key == "ROZI_FILE" && value == hostile),
            "the activated path is handed over as ROZI_FILE: {:?}",
            spawn.env
        );
    });
}

/// `{path}` is substituted only into `send` text; a `run`/`popup` command is left as-is because
/// config validation already rejected the placeholder there.
#[test]
fn path_substitution_is_literal_and_send_only() {
    assert_eq!(
        substitute(
            &UserCommandAction::Send("{path}".into()),
            "{path}",
            "/repo/src/main.rs"
        ),
        UserCommandAction::Send("/repo/src/main.rs".into())
    );
    assert_eq!(
        substitute(
            &UserCommandAction::run("ls {path}"),
            "{path}",
            "/etc/passwd"
        ),
        UserCommandAction::run("ls {path}")
    );
}

#[test]
fn sidebar_focus_switches_workspace_and_clears_activity() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = settled_backend();
            let mut pane = Pane::new(
                2,
                100,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 40.0,
                    h: 20.0,
                },
            );
            pane.activity.has_unseen_output = true;
            backend.state_mut().current_mut().workspaces[1]
                .panes
                .push(pane);
            backend
                .dispatch(crate::Msg::SidebarFocusPane(2))
                .expect("focus sidebar pane");
            assert_eq!(backend.state().current().active_workspace, 1);
            assert_eq!(backend.state().current().focused_pane, Some(2));
            assert!(
                !backend.state().current().workspaces[1].panes[0]
                    .activity
                    .has_unseen_output
            );
        })
        .expect("spawn sidebar focus test")
        .join()
        .expect("sidebar focus test completes");
}

#[test]
fn stale_session_results_are_ignored_after_close_switch_and_reload_epochs() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            state.sidebar.sessions_epoch = 10;
        }
        let stale = vec![discovered("old")];

        backend.state_mut().sidebar_visible = false;
        backend.state_mut().sidebar.invalidate_sessions();
        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 10,
                rows: Ok(stale.clone()),
                host_status: Vec::new(),
            })
            .expect("stale close result");
        assert!(backend.state().sidebar.sessions.is_empty());

        backend.state_mut().sidebar_visible = true;
        backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        backend.state_mut().sidebar.invalidate_sessions();
        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 11,
                rows: Ok(stale.clone()),
                host_status: Vec::new(),
            })
            .expect("stale tab result");
        assert!(backend.state().sidebar.sessions.is_empty());

        backend
            .state_mut()
            .sidebar
            .reconcile(&crate::config::SidebarConfig::default());
        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 12,
                rows: Ok(stale),
                host_status: Vec::new(),
            })
            .expect("stale reload result");
        assert!(backend.state().sidebar.sessions.is_empty());
    });
}

#[test]
fn current_session_results_apply() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        open_sessions_tab_unswept(&mut backend, 7);
        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 7,
                rows: Ok(vec![discovered("dev")]),
                host_status: Vec::new(),
            })
            .expect("current result");
        assert_eq!(backend.state().sidebar.sessions, vec![discovered("dev")]);
    });
}

/// The host title and description form one connect/disconnect row. Connecting marks the host in
/// flight; disconnecting takes two activations of that same row.
#[test]
fn host_connect_and_two_click_disconnect() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        {
            let state = backend.state_mut();
            state.config.remote.hosts.insert(
                "winvm".to_string(),
                crate::config::RemoteHostConfig::default(),
            );
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
        }
        // Seed the host registry (offline) the way opening the tab does.
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("run the post-update chokepoint over the open tab");
        assert!(
            backend.state().hosts.get(&target).is_some(),
            "the configured host is seeded into the registry"
        );

        // `row_activate` rebuilds the rows from the current state before the post-update sweep
        // repopulates local sessions, so clearing them here makes the layout deterministic:
        //   0 LOCAL header · 1 "No local sessions" · 2 "+ New session" · 3 spacer
        //   4 WINVM / "Click to connect" · 5 spacer · 6 "Connect a host…"
        backend.state_mut().sidebar.sessions.clear();
        backend
            .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
            .expect("connect through host row");
        assert_eq!(
            backend.state().hosts.get(&target).unwrap().probe,
            crate::state::HostProbe::InFlight,
            "connecting marks the host in flight"
        );

        // Now online, the same row reads "Click to disconnect". Force the host reached and arm.
        backend.state_mut().hosts.get_mut(&target).unwrap().probe =
            crate::state::HostProbe::Reached;
        //   … 4 WINVM / "Click to disconnect" · 5 "No sessions here yet" · 6 "+ New…"
        backend.state_mut().sidebar.sessions.clear();
        backend
            .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
            .expect("arm disconnect");
        assert_eq!(
            backend.state().sidebar.pending_host_disconnect.as_ref(),
            Some(&target),
            "first click arms the confirmation"
        );
        backend.state_mut().hosts.get_mut(&target).unwrap().probe =
            crate::state::HostProbe::Reached;
        backend.state_mut().sidebar.sessions.clear();
        backend
            .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
            .expect("confirm disconnect");
        assert_eq!(
            backend.state().hosts.get(&target).unwrap().probe,
            crate::state::HostProbe::Idle,
            "confirming disconnect returns the host to offline"
        );
        assert!(backend.state().sidebar.pending_host_disconnect.is_none());
    });
}

/// The ✕ on a pane row takes two clicks: the first arms a confirmation, the second kills the
/// pane. Clicking the row body in between abandons the arming rather than carrying it, so a
/// confirmation can never be committed by a gesture that meant something else.
#[test]
fn the_close_affordance_takes_two_clicks_and_is_disarmed_by_acting_elsewhere() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        }
        let id = backend
            .state()
            .current()
            .focused_pane
            .expect("the default attachment has a focused pane");
        // Row 0 is the workspace header; row 1 is the only pane.
        let pane_row = 1;

        backend
            .dispatch(crate::Msg::SidebarRowClose {
                panel: 0,
                index: pane_row,
            })
            .expect("arm the close");
        assert_eq!(
            backend.state().sidebar.pending_row_close,
            Some(crate::state::SidebarClose::Pane(id)),
            "the first click arms the confirmation"
        );

        // Activating the row instead of confirming abandons the arming.
        backend
            .dispatch(crate::Msg::SidebarRowActivate {
                panel: 0,
                index: pane_row,
            })
            .expect("activate the row");
        assert!(
            backend.state().sidebar.pending_row_close.is_none(),
            "acting on the row disarms the pending close"
        );
        assert!(
            crate::pane_lifecycle::find_pane(backend.state(), id).is_some_and(|pane| !pane.closing),
            "an abandoned confirmation leaves the pane alone"
        );

        backend
            .dispatch(crate::Msg::SidebarRowClose {
                panel: 0,
                index: pane_row,
            })
            .expect("re-arm the close");
        backend
            .dispatch(crate::Msg::SidebarRowClose {
                panel: 0,
                index: pane_row,
            })
            .expect("confirm the close");
        assert!(
            backend.state().sidebar.pending_row_close.is_none(),
            "committing consumes the arming"
        );
        assert!(
            crate::pane_lifecycle::find_pane(backend.state(), id).is_none_or(|pane| pane.closing),
            "the confirming click closes the pane"
        );
    });
}

/// An arming lapses on its own after [`crate::ops::confirm::CONFIRM_WINDOW`]. The expiry is
/// matched by token rather than by wall time here: an expiry belonging to an arming that has
/// already been replaced must leave the replacement alone, which is the case a bare timer would
/// get wrong.
#[test]
fn a_lapsed_confirmation_clears_itself_and_never_disarms_a_later_one() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        }
        let pane_row = 1;
        backend
            .dispatch(crate::Msg::SidebarRowClose {
                panel: 0,
                index: pane_row,
            })
            .expect("arm the close");
        let armed_epoch = backend.state().confirm_epoch;
        assert!(backend.state().sidebar.pending_row_close.is_some());

        // Re-arming (here, the same row after a disarm) advances the token, so the first
        // arming's expiry is now stale and must not clear what replaced it.
        backend
            .dispatch(crate::Msg::SidebarRowActivate {
                panel: 0,
                index: pane_row,
            })
            .expect("disarm");
        backend
            .dispatch(crate::Msg::SidebarRowClose {
                panel: 0,
                index: pane_row,
            })
            .expect("re-arm");
        assert_ne!(backend.state().confirm_epoch, armed_epoch);
        backend
            .dispatch(crate::Msg::ConfirmationExpired(armed_epoch))
            .expect("stale expiry");
        assert!(
            backend.state().sidebar.pending_row_close.is_some(),
            "a stale expiry leaves the current arming alone"
        );

        let current = backend.state().confirm_epoch;
        backend
            .dispatch(crate::Msg::ConfirmationExpired(current))
            .expect("the window lapses");
        assert!(
            backend.state().sidebar.pending_row_close.is_none(),
            "the arming lapses on its own"
        );
    });
}

/// Hover drives the ✕, and the row plus the ✕ nested inside it both report against the same
/// index. Moving between them fires leave-then-enter for that one index, which has to settle on
/// "hovered" rather than cancelling itself out.
#[test]
fn hover_survives_the_pointer_crossing_into_the_close_affordance() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        backend
            .dispatch(crate::Msg::SidebarRowHover {
                panel: 0,
                index: 1,
                hovered: true,
            })
            .expect("enter the row");
        assert_eq!(backend.state().sidebar.panels[0].hovered_row, Some(1));

        // Crossing onto the ✕: the row leaves, then the ✕ enters, both naming row 1.
        backend
            .dispatch(crate::Msg::SidebarRowHover {
                panel: 0,
                index: 1,
                hovered: false,
            })
            .expect("leave the row");
        backend
            .dispatch(crate::Msg::SidebarRowHover {
                panel: 0,
                index: 1,
                hovered: true,
            })
            .expect("enter the ✕");
        assert_eq!(
            backend.state().sidebar.panels[0].hovered_row,
            Some(1),
            "the ✕ stays revealed under the pointer"
        );

        // A leave naming a row that is no longer the hovered one is stale and must not clear it.
        backend
            .dispatch(crate::Msg::SidebarRowHover {
                panel: 0,
                index: 4,
                hovered: false,
            })
            .expect("stale leave");
        assert_eq!(backend.state().sidebar.panels[0].hovered_row, Some(1));

        backend
            .dispatch(crate::Msg::SidebarRowHover {
                panel: 0,
                index: 1,
                hovered: false,
            })
            .expect("leave the sidebar");
        assert_eq!(backend.state().sidebar.panels[0].hovered_row, None);
    });
}

/// After a session switch bumps the sessions epoch — which kills the old refresh loop — the
/// post-update chokepoint re-arms it while the tab is open, so the Sessions tab keeps updating
/// instead of freezing on "No local sessions" until it is reopened.
#[test]
fn bumping_the_sessions_epoch_rearms_the_refresh_loop() {
    on_test_thread(|| {
        // `settled_backend` wires the command link, which the re-arm sends through.
        let mut backend = settled_backend();
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            // Simulate a session switch: epoch advanced, old loop's armed epoch left behind.
            state.sidebar.sessions_epoch = 99;
            state.sidebar.sessions_refresh_armed_epoch = Some(98);
        }
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("post-update runs");
        assert_eq!(
            backend.state().sidebar.sessions_refresh_armed_epoch,
            Some(99),
            "the refresh loop must re-arm for the new epoch"
        );

        // Leaving the sessions tab clears the arm so it re-arms cleanly on return.
        backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("tab left");
        assert_eq!(backend.state().sidebar.sessions_refresh_armed_epoch, None);
    });
}

/// Disconnecting the host the *current* session lives on must not quit and must not auto-attach.
/// A parked local session remains as a choice, so the client lands sessionless with the picker
/// open. The regression underneath: the sidebar dropped the update `disconnect_host` returns,
/// so a hop command never ran and left a pending attach nothing would complete.
#[test]
fn disconnecting_the_current_host_opens_the_picker_instead_of_auto_attaching() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        {
            let state = backend.state_mut();
            state.config.remote.hosts.insert(
                "winvm".to_string(),
                crate::config::RemoteHostConfig::default(),
            );
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
        }
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("run the post-update chokepoint over the open tab");

        {
            let state = backend.state_mut();
            // The local session used before hopping onto the remote one: parked, still live.
            let mut incoming = crate::state::fresh_default_attachment(&state.config);
            incoming.session_name = Some("build".to_string());
            incoming.session_attached = true;
            incoming.connection = crate::state::ConnectionState::Connected;
            incoming.remote_target = Some(target.clone());
            incoming.remote_host = Some("winvm".to_string());
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().session_attached = true;
            // Settled, not mid-connect: the launch attach this test never completes would
            // otherwise leave it pending.
            state.current_mut().pending_session_attach = None;
            state.current_mut().connection = crate::state::ConnectionState::Connected;
            let parked_epoch = state.runtime_epoch;
            state.park_current(parked_epoch, incoming);
            state.runtime_epoch = state.mint_attachment_id();

            // Online, and armed, so the next activation of the host row commits the disconnect.
            state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::Reached;
            state.sidebar.pending_host_disconnect = Some(target.clone());
            state.sidebar.sessions.clear();
        }

        // Row 4 is the WINVM host row — see `host_connect_and_two_click_disconnect`.
        backend
            .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
            .expect("confirm disconnect");

        let state = backend.state();
        assert!(
            state.is_launcher(),
            "disconnecting the active host leaves the foreground sessionless"
        );
        assert!(
            state.show_session_picker,
            "the parked local session remains as a choice, so the picker opens"
        );
        assert!(
            state.background.values().any(|attachment| {
                attachment.session_name.as_deref() == Some("dev")
                    && attachment.remote_target.is_none()
            }),
            "the local parked session stays retained for an explicit picker choice"
        );
        assert!(
            state.current().pending_session_attach.is_none(),
            "nothing auto-attaches after a disconnect"
        );
    });
}

/// Killing the session on screen must not auto-attach a parked session and must not mint a
/// fresh ephemeral. The killed one is gone; other choices stay available via the picker.
#[test]
fn killing_the_current_session_opens_the_picker_instead_of_auto_attaching() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
        }
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("run the post-update chokepoint over the open tab");
        {
            let state = backend.state_mut();
            // `dev` is the session used before `build`: parked, settled, still live.
            let mut incoming = crate::state::fresh_default_attachment(&state.config);
            incoming.session_name = Some("build".to_string());
            incoming.session_attached = true;
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().session_attached = true;
            state.current_mut().pending_session_attach = None;
            state.current_mut().connection = crate::state::ConnectionState::Connected;
            let parked_epoch = state.runtime_epoch;
            state.park_current(parked_epoch, incoming);
            state.runtime_epoch = state.mint_attachment_id();
            //   0 LOCAL header · 1 `build` · 2 "+ New session"
            state.sidebar.sessions = vec![discovered("build")];
            // Hide the sidebar so the recurring sweep stops replacing this fixed row list with
            // whatever sessions happen to be running on the machine the test runs on. Row
            // activation reads the active tab, not visibility, so the rows still resolve.
            state.sidebar_visible = false;
        }

        // The ✕ on the current session's row: arm, then confirm.
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
            .expect("arm the kill");
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
            .expect("confirm the kill");

        let state = backend.state();
        assert!(
            state.is_launcher(),
            "killing the active session leaves the foreground sessionless"
        );
        assert!(
            state.show_session_picker,
            "a parked session remains as a choice, so the picker opens"
        );
        assert!(
            state
                .background
                .values()
                .any(|attachment| { attachment.session_name.as_deref() == Some("dev") }),
            "the parked session stays retained for an explicit picker choice"
        );
        assert!(
            state.current().pending_session_attach.is_none(),
            "nothing auto-attaches after a kill"
        );
    });
}

/// A host the user connected keeps being swept even after a probe fails. Connecting is an
/// intent; a failure is one sweep's outcome. Dropping failed hosts from the sweep meant a single
/// blip demoted a connected host to Offline for good, taking its sessions with it —
/// only `Idle`, the disconnected state, is left alone.
#[test]
fn a_connected_host_is_still_swept_after_a_probe_fails() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        {
            let state = backend.state_mut();
            state.config.remote.hosts.insert(
                "winvm".to_string(),
                crate::config::RemoteHostConfig::default(),
            );
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            state.sidebar.sessions_epoch = 7;
        }
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("run the post-update chokepoint over the open tab");
        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 7,
                rows: Ok(Vec::new()),
                host_status: vec![(target.clone(), Some("connection reset".to_string()))],
            })
            .expect("failed probe");
        assert!(matches!(
            backend.state().hosts.get(&target).unwrap().probe,
            crate::state::HostProbe::Failed(_)
        ));

        // The next sweep must still contact it, so the failure can clear on its own.
        assert!(
            probe_targets_for_test(backend.state()).contains(&target),
            "a failed-but-connected host stays in the sweep"
        );

        // Disconnecting is what takes it out.
        backend.state_mut().hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::Idle;
        assert!(!probe_targets_for_test(backend.state()).contains(&target));
    });
}

/// Mirrors the `probe_targets` filter in [`refresh_sessions`].
fn probe_targets_for_test(
    state: &crate::state::State,
) -> Vec<crate::session::remote::RemoteTarget> {
    state
        .hosts
        .iter()
        .filter(|host| {
            !matches!(host.probe, crate::state::HostProbe::Idle)
                || state
                    .background
                    .values()
                    .chain(std::iter::once(state.current()))
                    .any(|attachment| attachment.remote_target.as_ref() == Some(&host.target))
        })
        .map(|host| host.target.clone())
        .collect()
}

/// A probe failure records the reason on the host's registry entry (surfaced inline on its
/// group header) as a failed probe; a subsequent success flips it to reached. The host is
/// seeded from config by the handler, so the registry entry exists to receive the outcome.
#[test]
fn host_probe_errors_are_recorded_then_cleared() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let target = crate::session::remote::RemoteTarget::Alias("prod".to_string());
        {
            let state = backend.state_mut();
            state.config.remote.hosts.insert(
                "prod".to_string(),
                crate::config::RemoteHostConfig::default(),
            );
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            state.sidebar.sessions_epoch = 3;
        }
        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 3,
                rows: Ok(Vec::new()),
                host_status: vec![(target.clone(), Some("no route to host".to_string()))],
            })
            .expect("failed probe");
        assert_eq!(
            backend.state().hosts.get(&target).unwrap().probe.error(),
            Some("no route to host")
        );

        backend
            .dispatch(crate::Msg::SidebarSessionsDiscovered {
                epoch: 3,
                rows: Ok(Vec::new()),
                host_status: vec![(target.clone(), None)],
            })
            .expect("recovered probe");
        assert_eq!(
            backend.state().hosts.get(&target).unwrap().probe,
            crate::state::HostProbe::Reached
        );
    });
}

#[test]
fn command_rows_are_sanitized_bounded_and_keep_raw_separate_from_display() {
    let long = "x".repeat(COMMAND_RAW_ROW_CHARS + 20);
    let stdout = format!("\x1b[31mred\x1b[0m\n{long}\n{}", "row\n".repeat(600));
    let rows = command_rows(Ok(crate::platform::command::CommandOutput {
        stdout: stdout.into_bytes(),
        stderr: Vec::new(),
        status: Some(0),
        timed_out: false,
    }));
    assert_eq!(rows.len(), COMMAND_MAX_ROWS);
    assert_eq!(rows[0].raw, "red");
    assert_eq!(rows[1].raw.chars().count(), COMMAND_RAW_ROW_CHARS);
    assert_eq!(
        rows[1].display.chars().count(),
        COMMAND_DISPLAY_ROW_CHARS + 1
    );
}

#[test]
fn command_errors_cover_timeout_nonzero_stderr_and_spawn_failure() {
    let timeout = command_rows(Ok(crate::platform::command::CommandOutput {
        stdout: b"ignored".to_vec(),
        stderr: Vec::new(),
        status: None,
        timed_out: true,
    }));
    assert!(timeout[0].error && timeout[0].raw.contains("timed out"));

    let nonzero = command_rows(Ok(crate::platform::command::CommandOutput {
        stdout: Vec::new(),
        stderr: b"\x1b[31mbad\x1b[0m".to_vec(),
        status: Some(7),
        timed_out: false,
    }));
    assert_eq!(nonzero[0].raw, "Error: bad");

    let spawn = command_rows(Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "missing",
    )));
    assert!(spawn[0].error && spawn[0].raw.contains("missing"));
}

#[test]
fn line_substitution_is_literal_and_send_only() {
    assert_eq!(
        resolve_row_action(
            &UserCommandAction::Send("open [{line}] {line}\n".to_string()),
            "$(touch /tmp/nope); 'quoted'"
        ),
        UserCommandAction::Send(
            "open [$(touch /tmp/nope); 'quoted'] $(touch /tmp/nope); 'quoted'\n".to_string()
        )
    );
    assert_eq!(
        resolve_row_action(&UserCommandAction::run("show fixed".to_string()), "ignored"),
        UserCommandAction::run("show fixed".to_string())
    );
}

/// Build a one-entry launcher tab and activate it, returning the spawn request it queued. An
/// in-flight attach (`hold_attach_open`) keeps the request in `pending_spawns` so the test can
/// assert the payload without starting a real ephemeral session.
fn activate_launcher(
    action: UserCommandAction,
    cwd: Option<&str>,
) -> crate::state::PendingPaneSpawn {
    let mut backend = settled_backend();
    let id = SidebarTabId::new("launch");
    backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Launcher {
        name: id.clone(),
        label: "Launch".to_string(),
        entries: vec![crate::config::SidebarLauncherEntry {
            label: "Entry".to_string(),
            action,
        }],
    }];
    backend.state_mut().sidebar.config_epoch = 1;
    if let Some(cwd) = cwd {
        let focused = backend.state().current().workspaces[0].panes[0].id;
        backend.state_mut().current_mut().workspaces[0].focused_pane = Some(focused);
        backend.state_mut().current_mut().focused_pane = Some(focused);
        backend.state_mut().current_mut().workspaces[0].panes[0]
            .terminal
            .cwd = Some(cwd.to_string());
    }
    backend.state_mut().current_mut().pending_spawns.clear();
    hold_attach_open(&mut backend);
    backend
        .dispatch(crate::Msg::SidebarLauncherActivate {
            config_epoch: 1,
            tab_id: id,
            entry_index: 0,
        })
        .expect("launcher click");
    backend
        .state()
        .current()
        .pending_spawns
        .last()
        .cloned()
        .expect("launcher click queues a spawn")
}

/// A launcher `run` opens where the focused pane is, not where the session server was started:
/// `cargo build` means "build the project I am looking at". It also holds the pane after the
/// command exits, so a build that fails in milliseconds leaves its errors on screen.
#[test]
fn launcher_run_inherits_the_focused_pane_cwd_and_holds_the_pane_open() {
    on_test_thread(|| {
        let spawn = activate_launcher(
            UserCommandAction::run("cargo build"),
            Some("/home/x/work/rozi"),
        );
        assert_eq!(spawn.command.as_deref(), Some("cargo build"));
        assert_eq!(spawn.cwd.as_deref(), Some("/home/x/work/rozi"));
        assert!(spawn.keep_open);
    });
}

/// The popup carries the same two properties. Its `keep_open` used to be dropped between the
/// identity and the wire request, so a popup running a fast command flashed and vanished.
#[test]
fn launcher_popup_inherits_cwd_and_keeps_its_identity_keep_open() {
    on_test_thread(|| {
        let spawn = activate_launcher(UserCommandAction::popup("date"), Some("/home/x/notes"));
        assert_eq!(spawn.pane_id, crate::state::POPUP_PANE_ID);
        assert_eq!(spawn.cwd.as_deref(), Some("/home/x/notes"));
        assert!(
            spawn.keep_open,
            "the wire request must agree with the pane identity"
        );

        let opt_out = activate_launcher(
            UserCommandAction::Popup {
                command: "fzf".to_string(),
                keep_open: false,
            },
            None,
        );
        assert!(!opt_out.keep_open);
    });
}

#[test]
fn launcher_click_revalidates_config_epoch_tab_and_index() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let id = SidebarTabId::new("launch");
        backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Launcher {
            name: id.clone(),
            label: "Launch".to_string(),
            entries: vec![crate::config::SidebarLauncherEntry {
                label: "Run".to_string(),
                action: UserCommandAction::run("printf safe".to_string()),
            }],
        }];
        backend.state_mut().sidebar.config_epoch = 4;
        let initial = backend.state().current().workspaces[0].panes.len();
        backend
            .dispatch(crate::Msg::SidebarLauncherActivate {
                config_epoch: 3,
                tab_id: id.clone(),
                entry_index: 0,
            })
            .expect("stale launcher click");
        backend
            .dispatch(crate::Msg::SidebarLauncherActivate {
                config_epoch: 4,
                tab_id: SidebarTabId::new("other"),
                entry_index: 0,
            })
            .expect("wrong tab click");
        assert_eq!(backend.state().current().workspaces[0].panes.len(), initial);
        backend
            .dispatch(crate::Msg::SidebarLauncherActivate {
                config_epoch: 4,
                tab_id: id,
                entry_index: 0,
            })
            .expect("current launcher click");
        assert_eq!(
            backend.state().current().workspaces[0].panes.len(),
            initial + 1
        );
    });
}

#[test]
fn command_click_rejects_stale_output_epoch_and_changed_raw_line() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let id = SidebarTabId::new("rows");
        backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Command {
            name: id.clone(),
            label: "Rows".to_string(),
            command: "printf row".to_string(),
            interval_secs: 30,
            on_click: Some(UserCommandAction::run("printf fixed".to_string())),
        }];
        backend.state_mut().sidebar.config_epoch = 2;
        backend.state_mut().sidebar.command_output.insert(
            id.clone(),
            SidebarCommandOutput {
                epoch: 9,
                rows: vec![row("safe")],
            },
        );
        let initial = backend.state().current().workspaces[0].panes.len();
        for (output_epoch, line) in [(8, "safe"), (9, "changed")] {
            backend
                .dispatch(crate::Msg::SidebarCommandRowActivate {
                    config_epoch: 2,
                    tab_id: id.clone(),
                    output_epoch,
                    line: line.to_string(),
                })
                .expect("stale row click");
        }
        assert_eq!(backend.state().current().workspaces[0].panes.len(), initial);
        backend
            .dispatch(crate::Msg::SidebarCommandRowActivate {
                config_epoch: 2,
                tab_id: id,
                output_epoch: 9,
                line: "safe".to_string(),
            })
            .expect("current row click");
        assert_eq!(
            backend.state().current().workspaces[0].panes.len(),
            initial + 1
        );
    });
}

#[test]
fn stale_command_result_clears_only_its_run_and_cannot_replace_output() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let id = SidebarTabId::new("rows");
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(id.clone());
            state.sidebar.command_epoch = 8;
            state.sidebar.command_in_flight.insert(id.clone(), 7);
            state.sidebar.command_output.insert(
                id.clone(),
                SidebarCommandOutput {
                    epoch: 3,
                    rows: vec![row("current")],
                },
            );
        }
        backend
            .dispatch(crate::Msg::SidebarCommandOutput {
                epoch: 7,
                tab_id: id.clone(),
                rows: vec![row("stale")],
            })
            .expect("stale command result");
        assert!(!backend.state().sidebar.command_in_flight.contains_key(&id));
        assert_eq!(
            backend.state().sidebar.command_output[&id].rows,
            vec![row("current")]
        );
    });
}

#[test]
fn polling_rejects_hidden_inactive_stale_and_overlapping_runs() {
    on_test_thread(|| {
        let mut backend = settled_backend();
        let id = SidebarTabId::new("rows");
        {
            let state = backend.state_mut();
            state.config.sidebar.tabs = vec![SidebarTab::Command {
                name: id.clone(),
                label: "Rows".to_string(),
                command: "sleep 1".to_string(),
                interval_secs: 5,
                on_click: None,
            }];
            state.sidebar.panels[0].active_tab = Some(id.clone());
            state.sidebar.command_epoch = 6;
        }
        for (visible, epoch, active) in [(false, 6, "rows"), (true, 5, "rows"), (true, 6, "other")]
        {
            backend.state_mut().sidebar_visible = visible;
            backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new(active));
            backend
                .dispatch(crate::Msg::SidebarCommandPoll {
                    epoch,
                    tab_id: id.clone(),
                })
                .expect("guarded poll");
            assert!(!backend.state().sidebar.command_in_flight.contains_key(&id));
        }

        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.sidebar.panels[0].active_tab = Some(id.clone());
        state.sidebar.command_in_flight.insert(id.clone(), 5);
        backend
            .dispatch(crate::Msg::SidebarCommandPoll {
                epoch: 6,
                tab_id: id.clone(),
            })
            .expect("overlap guard");
        assert_eq!(backend.state().sidebar.command_in_flight.get(&id), Some(&5));
        backend.state_mut().sidebar_visible = false;
    });
}

#[test]
fn sessions_and_command_panels_refresh_together() {
    on_test_thread(|| {
        // The command panel starts through the command link, so it has to be wired first.
        let mut backend = settled_backend();
        let command_id = SidebarTabId::new("rows");
        {
            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.config.sidebar.tabs = vec![
                SidebarTab::Sessions,
                SidebarTab::Panes,
                SidebarTab::Command {
                    name: command_id.clone(),
                    label: "Rows".to_string(),
                    command: "echo command-panel".to_string(),
                    interval_secs: 5,
                    on_click: None,
                },
            ];
            state.sidebar.panels = vec![
                crate::state::SidebarPanelState {
                    tabs: vec![SidebarTabId::new("sessions")],
                    active_tab: Some(SidebarTabId::new("sessions")),
                    ..Default::default()
                },
                crate::state::SidebarPanelState {
                    tabs: vec![SidebarTabId::new("panes"), command_id.clone()],
                    active_tab: Some(SidebarTabId::new("panes")),
                    ..Default::default()
                },
            ];
        }

        backend
            .dispatch(crate::Msg::SidebarTabSelected { panel: 1, index: 1 })
            .expect("select command beside sessions");
        assert!(
            backend
                .state()
                .sidebar
                .command_in_flight
                .contains_key(&command_id)
                || backend
                    .state()
                    .sidebar
                    .command_output
                    .contains_key(&command_id),
            "the command panel must start even while Sessions is visible"
        );
        backend.state_mut().sidebar_visible = false;
    });
}
