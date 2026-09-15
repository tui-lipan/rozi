//! Sidebar fill and the blank row under each tab bar.

use rozi::AppRoot;
use rozi::config::SidebarTab;
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

fn sidebar_backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 80,
        h: 24,
    });
    {
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.animations.sidebar = false;
        state.config.sidebar.tabs = vec![SidebarTab::Panes];
        state.config.sidebar.split = false;
        state.sidebar.apply_configured_panels(&state.config.sidebar);
        state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
    }
    backend
}

fn sidebar_columns(backend: &mut TestBackend<AppRoot>) -> Vec<String> {
    backend.render();
    let width = backend.state().effective_sidebar_width(backend.viewport()) as usize;
    backend
        .capture_frame()
        .to_fixed_grid_lines()
        .iter()
        .map(|line| line.chars().take(width.saturating_sub(1)).collect())
        .collect()
}

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn sidebar appearance smoke thread")
        .join()
        .expect("sidebar appearance smoke completes");
}

fn tab_then_body(lines: &[String]) -> (usize, usize) {
    let tab = lines
        .iter()
        .position(|line| line.contains("Panes"))
        .unwrap_or_else(|| panic!("tab bar shows Panes:\n{lines:#?}"));
    let body = lines
        .iter()
        .position(|line| line.contains("Workspace") || line.contains("+ New pane"))
        .unwrap_or_else(|| panic!("body shows a panes row:\n{lines:#?}"));
    (tab, body)
}

#[test]
fn sidebar_gap_inserts_one_row_between_the_tab_bar_and_its_list() {
    on_large_stack(|| {
        let mut backend = sidebar_backend();
        let (tab, body) = tab_then_body(&sidebar_columns(&mut backend));
        assert_eq!(
            body,
            tab + 2,
            "default gap is one blank row under the tab bar; tab={tab} body={body}"
        );

        backend.state_mut().config.sidebar.gap = false;
        let (tab, body) = tab_then_body(&sidebar_columns(&mut backend));
        assert_eq!(
            body,
            tab + 1,
            "disabled gap seats the list on the next row; tab={tab} body={body}"
        );
    });
}
