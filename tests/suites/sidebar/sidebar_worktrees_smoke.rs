//! The Worktrees tab: the focused pane's repository and its checkouts.

use rozi::AppRoot;
use rozi::config::{SidebarTab, SidebarTabId};
use rozi::git::worktrees::WorktreeInfo;
use rozi::session::protocol::WorktreeSession;
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

/// The shape of a real repository: a primary checkout, a sibling with a running session, a nested
/// agent checkout named after its branch, one whose folder differs from its branch, and a locked
/// one whose session can only be restored.
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
            "/home/me/src/rozi/.claude/worktrees/extensions-spinner",
            "worktree-extensions-spinner",
            true,
            false,
        ),
        tree(
            "/home/me/src/rozi/.claude/worktrees/session-fade-duration",
            "fix/config-test-race",
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
    let session = |name: &str, running: bool| WorktreeSession {
        name: name.into(),
        running,
    };
    listing.sessions.insert(
        "/home/me/src/rozi-worktrees/feat-login".into(),
        vec![session("wt-feat-login", true)],
    );
    listing.sessions.insert(
        "/home/me/src/rozi-worktrees/release".into(),
        vec![session("release", false)],
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
fn the_tab_lists_checkouts_compactly_with_their_session_state() {
    on_large_stack(|| {
        let mut backend = seeded(100, 30);
        let lines = sidebar_lines(&mut backend, 32);
        let text = lines.join("\n");
        let row = |needle: &str| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("no `{needle}` row:\n{text}"))
        };
        assert!(
            lines
                .iter()
                .any(|line| line.contains("rozi") && line.contains("local")),
            "the header names the repository and its host:\n{text}"
        );
        let master = row("master");
        assert!(
            lines[master].contains('▍'),
            "the focused pane's checkout is marked:\n{text}"
        );
        assert!(lines[master].contains("primary"), "{text}");

        // A running session is a filled marker, a restorable one a ring; names stay off the row.
        let login = row("feat/login");
        assert!(lines[login].contains('●'), "{text}");
        assert!(!text.contains("wt-feat-login"), "{text}");
        assert!(lines[row("release/0.1")].contains('○'), "{text}");

        // A checkout named after its branch is one line; a folder the branch does not imply is
        // noted under it.
        assert_eq!(row("worktree-extensions-spinner"), login + 1, "{text}");
        let race = row("fix/config-test-race");
        assert!(lines[race + 1].contains("session-fade-duration"), "{text}");
        assert!(
            !text.contains(".claude/worktrees"),
            "no repeated path prefix:\n{text}"
        );
        assert!(text.contains("+ New worktree"), "{text}");
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
            // The pointer on `feat/login` (header, master, then it) shows what Enter would do.
            state.sidebar.panels[0].hovered_row = Some(2);
            state.sidebar.pending_row_close = Some(rozi::state::SidebarClose::Worktree {
                path: "/home/me/src/rozi/.claude/worktrees/extensions-spinner".into(),
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
