//! The Worktrees tab: the focused pane's repository and its checkouts.

use rozi::AppRoot;
use rozi::config::{SidebarTab, SidebarTabId};
use rozi::git::worktrees::WorktreeInfo;
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

fn tree(path: &str, branch: &str, linked: bool, locked: bool) -> WorktreeInfo {
    WorktreeInfo {
        path: path.into(),
        branch: Some(branch.into()),
        detached: false,
        bare: false,
        prunable: false,
        linked,
        locked,
    }
}

/// The shape of a real repository: a primary checkout, a session-backed sibling, a nested agent
/// checkout, and a locked one.
fn seeded(width: u16, height: u16) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: width,
        h: height,
    });
    let state = backend.state_mut();
    state.sidebar_visible = true;
    state.config.animations.sidebar = false;
    state.config.sidebar.tabs = vec![SidebarTab::Worktrees];
    state.sidebar.panels[0].tabs = vec![SidebarTabId::new("worktrees")];
    state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("worktrees"));
    let listing = &mut state.sidebar.worktrees;
    listing.source = Some((None, "/home/me/src/rozi".into()));
    listing.loaded = true;
    listing.entries = vec![
        tree("/home/me/src/rozi", "master", false, false),
        tree(
            "/home/me/src/rozi-worktrees/feat-login",
            "feat/login",
            true,
            false,
        ),
        tree(
            "/home/me/src/rozi/.worktrees/agent-extensions-spinner",
            "agent/extensions-spinner",
            true,
            false,
        ),
        tree(
            "/home/me/src/rozi-worktrees/release",
            "release/0.1",
            true,
            true,
        ),
    ];
    listing.sessions.insert(
        "/home/me/src/rozi-worktrees/feat-login".into(),
        vec!["wt-feat-login".into()],
    );
    backend
}

fn sidebar_lines(backend: &mut TestBackend<AppRoot>, width: usize) -> Vec<String> {
    backend.render();
    backend
        .capture_frame()
        .to_fixed_grid_lines()
        .iter()
        .map(|line| {
            line.chars()
                .take(width)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn the_tab_lists_checkouts_with_their_sessions_and_the_current_one_marked() {
    on_large_stack(|| {
        let mut backend = seeded(100, 30);
        let lines = sidebar_lines(&mut backend, 32);
        let text = lines.join("\n");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("rozi") && line.contains("local")),
            "the header names the repository and its host:\n{text}"
        );
        let master = lines
            .iter()
            .position(|line| line.contains("master"))
            .expect("the primary checkout is listed");
        assert!(
            lines[master].contains('▍'),
            "the focused pane's checkout is marked:\n{text}"
        );
        assert!(lines[master].contains("primary"), "{text}");
        let login = lines
            .iter()
            .position(|line| line.contains("feat/login"))
            .expect("a linked checkout is listed");
        assert!(
            lines[login].contains("wt-feat-log"),
            "its session is named:\n{text}"
        );
        assert!(
            lines[login + 1].contains("rozi-worktrees/feat-login"),
            "its path is shown from the repository's parent:\n{text}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("release/0.1") && line.contains("locked")),
            "{text}"
        );
        assert!(
            lines.iter().any(|line| line.contains("+ New worktree")),
            "{text}"
        );
    });
}

#[test]
fn a_pane_outside_any_repository_says_so() {
    on_large_stack(|| {
        let mut backend = seeded(100, 30);
        {
            let listing = &mut backend.state_mut().sidebar.worktrees;
            listing.source = None;
            listing.unavailable = Some("Not in a Git repository".into());
        }
        let lines = sidebar_lines(&mut backend, 32);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Not in a Git repository")),
            "{lines:#?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("New worktree")),
            "there is no repository to create a checkout in:\n{lines:#?}"
        );
    });
}

#[cfg(feature = "ui-snapshot")]
#[test]
fn worktrees_tab_visual_reference() {
    on_large_stack(|| {
        let mut backend = seeded(100, 26);
        {
            let state = backend.state_mut();
            state.sidebar.pending_row_close = Some(rozi::state::SidebarClose::Worktree {
                path: "/home/me/src/rozi/.worktrees/agent-extensions-spinner".into(),
                force: false,
            });
        }
        backend.render();
        let png = backend.capture_ui_snapshot().to_png_default().unwrap();
        let dir = std::path::Path::new("target/ui-sketches");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("sidebar-worktrees.png"), png).unwrap();
    });
}
