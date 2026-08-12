use hyprmux::AppRoot;
use hyprmux::config::{SidebarLauncherEntry, SidebarTab, SidebarTabId, UserCommandAction};
use hyprmux::state::{SidebarCommandOutput, SidebarCommandRow};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

fn sidebar_lines(tab: SidebarTab) -> Vec<String> {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
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
                state.sidebar.panels[0].tabs = vec![tab.id()];
                state.sidebar.panels[0].active_tab = Some(tab.id());
                state.config.sidebar.tabs = vec![tab];
            }
            backend.render();
            backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect()
        })
        .expect("spawn user tabs smoke thread")
        .join()
        .expect("user tabs smoke completes")
}

/// A launcher entry renders in the same two-line row shape as the built-in tabs: an action glyph in
/// the glyph column, the label as the title, and what the entry does on the detail line directly
/// beneath it — not a bare label at column zero.
#[test]
fn launcher_entries_render_as_glyphed_two_line_rows() {
    let lines = sidebar_lines(SidebarTab::Launcher {
        name: SidebarTabId::new("deploy"),
        label: "Deploy".into(),
        entries: vec![
            SidebarLauncherEntry {
                label: "Build".into(),
                action: UserCommandAction::run("cargo build --release"),
            },
            SidebarLauncherEntry {
                label: "Date".into(),
                action: UserCommandAction::Send("date\n".into()),
            },
            SidebarLauncherEntry {
                label: "Logs".into(),
                action: UserCommandAction::popup("journalctl -f"),
            },
        ],
    });
    let index = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("sidebar shows {needle:?}"))
    };

    // Each label carries its action glyph, and its detail line follows immediately — no blank row
    // between entries.
    assert!(lines[index("Build")].contains('▸'));
    assert_eq!(index("cargo build"), index("Build") + 1);
    assert!(lines[index("Date")].contains('⏎'));
    assert_eq!(index("send"), index("Date") + 1);
    assert!(lines[index("Logs")].contains('▫'));
    assert_eq!(index("journalctl"), index("Logs") + 1);
    assert_eq!(index("Date"), index("cargo build") + 1);

    // Rows are inset past the marker gutter rather than starting at column zero.
    assert!(lines[index("Build")].starts_with(' '));
}

#[test]
fn launcher_tab_without_entries_shows_a_muted_placeholder() {
    let lines = sidebar_lines(SidebarTab::Launcher {
        name: SidebarTabId::new("deploy"),
        label: "Deploy".into(),
        entries: Vec::new(),
    });
    assert!(
        lines
            .iter()
            .any(|line| line.contains("No launcher entries"))
    );
}

#[test]
fn read_only_command_output_has_one_cell_of_leading_padding() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let tab_id = SidebarTabId::new("branches");
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.tabs = vec![SidebarTab::Command {
                    name: tab_id.clone(),
                    label: "Branches".into(),
                    command: "git branch --format='%(refname:short)'".into(),
                    interval_secs: 30,
                    on_click: None,
                }];
                state.sidebar.panels[0].tabs = vec![tab_id.clone()];
                state.sidebar.panels[0].active_tab = Some(tab_id.clone());
                state.sidebar.command_output.insert(
                    tab_id,
                    SidebarCommandOutput {
                        epoch: 1,
                        rows: vec![SidebarCommandRow {
                            raw: "master".into(),
                            display: "master".into(),
                            error: false,
                        }],
                    },
                );
            }
            backend.render();

            let line = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .into_iter()
                .find(|line| line.contains("master"))
                .expect("command output row renders");
            assert!(
                line.starts_with(" master"),
                "row has one-cell inset: {line:?}"
            );
            assert!(
                !line.starts_with("  master"),
                "row is not over-indented: {line:?}"
            );
        })
        .expect("spawn command row smoke thread")
        .join()
        .expect("command row smoke completes");
}
