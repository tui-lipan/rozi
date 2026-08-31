use rozi::AppRoot;
use rozi::config::{SidebarLauncherEntry, SidebarTab, SidebarTabId, UserCommandAction};
use rozi::state::{SidebarCommandOutput, SidebarCommandRow};
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
                // Revealing the sidebar after the first frame is a real toggle, so it runs the
                // real slide; these assertions are about the settled column, not a frame
                // part-way through it.
                state.config.animations.sidebar = false;
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
        env: Vec::new(),
        entries: vec![
            SidebarLauncherEntry {
                label: "Build".into(),
                group: None,
                action: UserCommandAction::run("cargo build --release"),
            },
            SidebarLauncherEntry {
                label: "Date".into(),
                group: None,
                action: UserCommandAction::Send("date\n".into()),
            },
            SidebarLauncherEntry {
                label: "Logs".into(),
                group: None,
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

/// Grouped entries read as sections: a header above each group, a blank row between them, and the
/// ungrouped entries leading without a header of their own.
#[test]
fn grouped_launcher_entries_render_under_section_headers() {
    let entry = |label: &str, group: Option<&str>, command: &str| SidebarLauncherEntry {
        label: label.into(),
        group: group.map(Into::into),
        action: UserCommandAction::run(command),
    };
    let lines = sidebar_lines(SidebarTab::Launcher {
        name: SidebarTabId::new("agents"),
        label: "Agents".into(),
        env: Vec::new(),
        entries: vec![
            entry("Shell", None, "bash"),
            entry("rozi", Some("claude"), "claude"),
            entry("docs", Some("claude"), "claude"),
            entry("rozi", Some("codex"), "codex"),
        ],
    });
    let index = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("sidebar shows {needle:?}"))
    };

    // Ungrouped rows lead with no header over them; sections follow in first-appearance order.
    assert!(index("Shell") < index("claude"));
    assert!(index("claude") < index("codex"));
    // A header sits directly above the first row of its section, separated from what came before
    // it by a blank row.
    assert_eq!(index("rozi"), index("claude") + 1);
    // Only the row's own text matters here; the sidebar's border and scrollbar occupy the tail of
    // every captured line.
    let blank = |line: &str| line.chars().take(24).all(char::is_whitespace);
    assert!(blank(&lines[index("claude") - 1]));
    assert!(blank(&lines[index("codex") - 1]));
    // Both entries of a section sit under the one header rather than repeating it.
    assert_eq!(index("docs"), index("rozi") + 2);
    // A header is a label, not a row: no action glyph.
    assert!(!lines[index("claude")].contains('▸'));
}

#[test]
fn launcher_tab_without_entries_shows_a_muted_placeholder() {
    let lines = sidebar_lines(SidebarTab::Launcher {
        name: SidebarTabId::new("deploy"),
        label: "Deploy".into(),
        entries: Vec::new(),
        env: Vec::new(),
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
                state.config.animations.sidebar = false;
                state.config.sidebar.tabs = vec![SidebarTab::Command {
                    name: tab_id.clone(),
                    label: "Branches".into(),
                    command: "git branch --format='%(refname:short)'".into(),
                    interval_secs: 30,
                    on_click: None,
                    group_prefix: None,
                    env: Vec::new(),
                }];
                state.sidebar.panels[0].tabs = vec![tab_id.clone()];
                state.sidebar.panels[0].active_tab = Some(tab_id.clone());
                state.sidebar.command_output.insert(
                    tab_id,
                    SidebarCommandOutput {
                        epoch: 1,
                        cwd: None,
                        rows: vec![SidebarCommandRow {
                            raw: "master".into(),
                            display: "master".into(),
                            error: false,
                            header: false,
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
