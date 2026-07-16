use hyprmux::HyprmuxApp;
use hyprmux::config::{SidebarPosition, SidebarTab};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

fn rendered_sidebar(position: SidebarPosition) -> Vec<String> {
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    {
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.sidebar.position = position;
        state.config.sidebar.tabs = vec![SidebarTab::Panes];
        state.sidebar.active_tab = Some(SidebarTab::Panes.id());
    }
    backend.render();
    assert_eq!(backend.state().content_viewport(backend.viewport()).w, 68);
    assert_eq!(backend.state().last_viewport.get().unwrap().w, 100);
    assert_eq!(backend.state().last_content_viewport.get().unwrap().w, 68);
    backend.capture_frame().to_fixed_grid_lines()
}

#[test]
fn sidebar_shell_reserves_the_same_content_width_on_both_docks() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let left = rendered_sidebar(SidebarPosition::Left);
            let right = rendered_sidebar(SidebarPosition::Right);
            assert!(left.iter().any(|line| line.contains("Panes")));
            assert!(right.iter().any(|line| line.contains("Panes")));
            assert!(
                left.iter()
                    .any(|line| line.chars().take(32).collect::<String>().contains("Panes"))
            );
            assert!(
                right.iter().any(|line| line
                    .chars()
                    .skip(68)
                    .collect::<String>()
                    .contains("Panes"))
            );
        })
        .expect("spawn sidebar smoke thread")
        .join()
        .expect("sidebar smoke completes");
}

#[test]
fn narrow_sidebar_yields_to_a_one_column_canvas() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 5,
            });
            backend.state_mut().sidebar_visible = true;
            assert_eq!(
                backend.state().effective_sidebar_width(backend.viewport()),
                9
            );
            assert_eq!(backend.state().content_viewport(backend.viewport()).w, 1);
        })
        .expect("spawn narrow sidebar test")
        .join()
        .expect("narrow sidebar test completes");
}
