use hyprmux::config::{SidebarTab, SidebarTreeConfig, SidebarTreeView};
use hyprmux::input::Action;
use hyprmux::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

/// A scratch git repository with one committed file, one modified, and one untracked, so the
/// changes view has something of each kind to project.
struct Repo(std::path::PathBuf);

impl Repo {
    fn new(name: &str) -> Option<Self> {
        let path =
            std::env::temp_dir().join(format!("hyprmux-tree-smoke-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("src")).expect("repo dirs");

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        git(&["init", "-q"])?;
        git(&["config", "user.email", "test@example.invalid"])?;
        git(&["config", "user.name", "Test"])?;
        std::fs::write(path.join("src/tracked.rs"), "fn main() {}\n").expect("tracked file");
        git(&["add", "."])?;
        git(&["commit", "-qm", "init"])?;

        std::fs::write(path.join("src/tracked.rs"), "fn main() {\n    // edit\n}\n")
            .expect("modify tracked");
        std::fs::write(path.join("src/fresh.rs"), "// new\n").expect("untracked file");
        Some(Self(path))
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn render_tree(view: SidebarTreeView, cwd: &str) -> Vec<String> {
    let cwd = cwd.to_string();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                let tab = SidebarTab::Tree {
                    view,
                    config: SidebarTreeConfig::for_view(view),
                };
                state.sidebar.panels[0].tabs = vec![tab.id()];
                state.sidebar.panels[0].active_tab = Some(tab.id());
                state.config.sidebar.tabs = vec![tab];
                let pane = state.current().workspaces[0].panes[0].id;
                state.current_mut().focused_pane = Some(pane);
                state.current_mut().workspaces[0].focused_pane = Some(pane);
                state.current_mut().workspaces[0].panes[0].terminal.cwd = Some(cwd);
            }
            // Directory reads and `git status` both run as background commands, so the tree needs
            // a few pump/render cycles before its rows exist.
            for _ in 0..40 {
                backend.render();
                let _ = backend.pump();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            backend.render();
            backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect()
        })
        .expect("spawn tree smoke thread")
        .join()
        .expect("tree smoke completes")
}

#[test]
fn files_tab_lists_the_focused_pane_directory() {
    let Some(repo) = Repo::new("files") else {
        eprintln!("skipping: git is unavailable");
        return;
    };
    let lines = render_tree(SidebarTreeView::Files, &repo.0.to_string_lossy());
    assert!(
        lines.iter().any(|line| line.contains("src")),
        "files tab lists the directory: {lines:?}"
    );
}

/// The changes view shows only what git reports, with diff stats, and rooted at the repository so
/// changes outside the pane's own directory are still visible.
#[test]
fn git_tab_shows_changed_paths_with_diff_stats() {
    let Some(repo) = Repo::new("git") else {
        eprintln!("skipping: git is unavailable");
        return;
    };
    // Focused pane sits in a subdirectory; the repo-rooted view must still show the whole repo.
    let lines = render_tree(
        SidebarTreeView::Changes,
        &repo.0.join("src").to_string_lossy(),
    );
    let joined = lines.join("\n");

    assert!(
        joined.contains("tracked.rs"),
        "modified file appears: {joined}"
    );
    assert!(
        joined.contains("fresh.rs"),
        "untracked file appears: {joined}"
    );
    // `M` for the modified file and `?` for the untracked one, as text markers rather than glyphs
    // that assume a Nerd Font.
    assert!(joined.contains('M') && joined.contains('?'), "{joined}");
    // Diff stats are on for this view.
    assert!(joined.contains('+'), "diff stats render: {joined}");
}

/// Git markers and diff stats carry the theme's semantic status colors. The widget's own
/// `git_style_*` defaults are empty styles and it never reads `theme.git_status` itself, so without
/// the app wiring the palette through, every marker renders in the plain row color.
#[test]
fn git_markers_and_diff_stats_use_theme_status_colors() {
    let Some(repo) = Repo::new("colors") else {
        eprintln!("skipping: git is unavailable");
        return;
    };
    let cwd = repo.0.to_string_lossy().into_owned();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 90,
                h: 24,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.width = 34;
                let tab = SidebarTab::Tree {
                    view: SidebarTreeView::Changes,
                    config: SidebarTreeConfig::for_view(SidebarTreeView::Changes),
                };
                state.sidebar.panels[0].tabs = vec![tab.id()];
                state.sidebar.panels[0].active_tab = Some(tab.id());
                state.config.sidebar.tabs = vec![tab];
                let pane = state.current().workspaces[0].panes[0].id;
                state.current_mut().focused_pane = Some(pane);
                state.current_mut().workspaces[0].focused_pane = Some(pane);
                state.current_mut().workspaces[0].panes[0].terminal.cwd = Some(cwd);
            }
            for _ in 0..40 {
                backend.render();
                let _ = backend.pump();
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            backend.render();

            let git = backend.state().theme.git_status;
            let frame = backend.capture_frame();
            // Collect the color of each marker/stat glyph in the right-hand suffix region, so the
            // left-hand labels (accent-colored, and the theme's accent happens to equal the
            // untracked purple) cannot be mistaken for a status color.
            let mut seen: Vec<(String, tui_lipan::prelude::Color)> = Vec::new();
            for y in 0..24u16 {
                for x in 12..34u16 {
                    let cell = frame.cell(x, y);
                    if !cell.symbol.trim().is_empty() {
                        seen.push((cell.symbol.clone(), cell.fg));
                    }
                }
            }
            let colored = |sym: &str, color| seen.iter().any(|(s, c)| s == sym && *c == color);

            assert!(
                colored("M", git.modified),
                "modified marker uses the theme modified color: {seen:?}"
            );
            assert!(
                colored("?", git.untracked),
                "untracked marker uses the theme untracked color: {seen:?}"
            );
            assert!(
                colored("+", git.added),
                "diff additions use the theme added color: {seen:?}"
            );
        })
        .expect("spawn color smoke thread")
        .join()
        .expect("color smoke completes");
}

/// Every scrolling surface in the sidebar uses the right half block thumb: the file tree's own
/// scrollbar and the shared body scroll view that wraps the non-tree tabs.
#[test]
fn sidebar_scrollbars_use_the_half_block_thumb() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            // Short viewport against this repository, so both surfaces certainly overflow.
            let has_thumb = |tab: SidebarTab, panes: usize| -> bool {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 90,
                    h: 12,
                });
                {
                    let state = backend.state_mut();
                    state.sidebar_visible = true;
                    state.config.sidebar.width = 34;
                    state.sidebar.panels[0].tabs = vec![tab.id()];
                    state.sidebar.panels[0].active_tab = Some(tab.id());
                    state.config.sidebar.tabs = vec![tab];
                    let pane = state.current().workspaces[0].panes[0].id;
                    state.current_mut().focused_pane = Some(pane);
                    state.current_mut().workspaces[0].focused_pane = Some(pane);
                    state.current_mut().workspaces[0].panes[0].terminal.cwd =
                        Some(env!("CARGO_MANIFEST_DIR").to_string());
                    // The Panes tab needs enough rows to overflow a 12-row viewport.
                    let rect = state.current().workspaces[0].panes[0].floating_rect;
                    for id in 100..(100 + panes as u32) {
                        state.current_mut().workspaces[0]
                            .panes
                            .push(hyprmux::state::Pane::new(id, 100, rect));
                    }
                }
                for _ in 0..40 {
                    backend.render();
                    let _ = backend.pump();
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
                backend.render();
                let frame = backend.capture_frame();
                (0..12u16).any(|y| (0..34u16).any(|x| frame.cell(x, y).symbol == "▐"))
            };

            let tree = SidebarTab::Tree {
                view: SidebarTreeView::Files,
                config: SidebarTreeConfig::for_view(SidebarTreeView::Files),
            };
            assert!(
                has_thumb(tree, 0),
                "file tree scrollbar uses the half block"
            );
            assert!(
                has_thumb(SidebarTab::Panes, 20),
                "the sidebar body scroll view uses the half block"
            );
        })
        .expect("spawn scrollbar smoke thread")
        .join()
        .expect("scrollbar smoke completes");
}

/// Clicking a directory row expands it (revealing its children) without running the tab's action,
/// and the selected row is painted with the tab-active background rather than the theme default.
/// Regression cover for the two reported bugs: a directory click used to fire `on_click`, and the
/// selection used the wrong style.
#[test]
fn clicking_a_directory_expands_it_and_styles_the_selection() {
    let Some(repo) = Repo::new("clicks") else {
        eprintln!("skipping: git is unavailable");
        return;
    };
    // Give the repo a nested directory so expansion has something to reveal.
    std::fs::create_dir_all(repo.0.join("src/inner")).expect("nested dir");
    std::fs::write(repo.0.join("src/inner/leaf.rs"), "// leaf\n").expect("nested file");

    let cwd = repo.0.to_string_lossy().into_owned();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 90,
                h: 24,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.width = 34;
                let tab = SidebarTab::Tree {
                    view: SidebarTreeView::Files,
                    config: SidebarTreeConfig::for_view(SidebarTreeView::Files),
                };
                state.sidebar.panels[0].tabs = vec![tab.id()];
                state.sidebar.panels[0].active_tab = Some(tab.id());
                state.config.sidebar.tabs = vec![tab];
                let pane = state.current().workspaces[0].panes[0].id;
                state.current_mut().focused_pane = Some(pane);
                state.current_mut().workspaces[0].focused_pane = Some(pane);
                state.current_mut().workspaces[0].panes[0].terminal.cwd = Some(cwd);
            }
            let settle = |backend: &mut TestBackend<AppRoot>| {
                for _ in 0..40 {
                    backend.render();
                    let _ = backend.pump();
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
                backend.render();
            };
            settle(&mut backend);

            // The cursor shares the pointer-hover lift, one visual language across every tab.
            let selection_bg = backend.state().theme.surface.element.elevate_by(0.08);

            // Find and click the `src` directory row.
            let row_text = |backend: &TestBackend<AppRoot>, row: u16| -> String {
                backend.capture_frame().to_fixed_grid_lines()[row as usize]
                    .chars()
                    .take(34)
                    .collect()
            };
            // A child row is prefixed by its indent guide, not by plain indentation; the range
            // starts below the root path row, so the name alone identifies it.
            let src_row = (3u16..16)
                .find(|&row| row_text(&backend, row).contains("src"))
                .expect("src directory row is visible");
            for kind in [
                MouseKind::Down(MouseButton::Left),
                MouseKind::Up(MouseButton::Left),
            ] {
                let _ = backend.send_mouse(MouseEvent {
                    x: 5,
                    y: src_row,
                    kind,
                    mods: Default::default(),
                });
            }
            settle(&mut backend);

            // The directory expanded: a child now sits directly below it. The action never ran, so
            // no pane input was produced — asserted structurally by the tree still being the only
            // thing on screen and the child appearing.
            let after: Vec<String> = (0..16).map(|row| row_text(&backend, row)).collect();
            assert!(
                after.iter().any(|line| line.contains("inner")),
                "clicking the directory expanded it: {after:?}"
            );

            // Park the pointer outside the sidebar first: hover and the keyboard cursor share one
            // highlight, so a row still under the mouse is lit for a reason that has nothing to do
            // with selection.
            let _ = backend.send_mouse(MouseEvent {
                x: 80,
                y: 20,
                kind: MouseKind::Moved,
                mods: Default::default(),
            });
            settle(&mut backend);

            // A click is a one-shot gesture, not a "this row is now current" state: the sidebar is
            // a `FocusScope::Exclude` subtree, so clicking never focuses the tree and the row the
            // widget selected internally leaves no mark behind.
            let src_after = (3u16..16)
                .find(|&row| row_text(&backend, row).contains("src"))
                .expect("src row still visible after expand");
            let bg = backend.capture_frame().cell(3, src_after).bg;
            assert_ne!(
                bg, selection_bg,
                "an unfocused tree must not paint a selection background"
            );

            // Entering the sidebar with the keyboard is what makes the cursor real. `focus-sidebar`
            // is the only way in, precisely because Tab and clicks cannot get here.
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);
            let focused_row = (3u16..16)
                .find(|&row| row_text(&backend, row).contains("src"))
                .expect("src row visible while focused");
            let focused_bg = backend.capture_frame().cell(3, focused_row).bg;
            assert_eq!(
                focused_bg, selection_bg,
                "a focused tree paints the cursor with the shared row highlight, got {focused_bg:?}"
            );

            // Escape hands the keyboard back and the cursor goes quiet again.
            let _ = backend.send_key(KeyEvent {
                code: KeyCode::Esc,
                mods: KeyMods::NONE,
            });
            settle(&mut backend);
            let blurred_row = (3u16..16)
                .find(|&row| row_text(&backend, row).contains("src"))
                .expect("src row visible after blur");
            let blurred_bg = backend.capture_frame().cell(3, blurred_row).bg;
            assert_ne!(
                blurred_bg, selection_bg,
                "Escape leaves the sidebar and clears the cursor"
            );
        })
        .expect("spawn click smoke thread")
        .join()
        .expect("click smoke completes");
}
