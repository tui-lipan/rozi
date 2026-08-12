use rozi::AppRoot;
use rozi::config::{SidebarPosition, SidebarTab};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;
use tui_lipan::style::Color;

/// A cell inside the sidebar body, far left of the centered modal so nothing is painted over it,
/// and a workbar cell on the workspace side. Both carry a painted background, so a dim shows up
/// as a changed color rather than blending invisibly into the backdrop.
const SIDEBAR_CELL: (u16, u16) = (2, 6);
const WORKSPACE_CELL: (u16, u16) = (35, 0);

fn backend() -> TestBackend<AppRoot> {
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
        state.config.sidebar.position = SidebarPosition::Left;
        state.config.sidebar.tabs = vec![SidebarTab::Panes];
        state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
        // The dim eases in with the dialog; disabling animations settles it in one frame.
        state.config.animations.enabled = false;
    }
    backend.render();
    backend
}

fn cells(backend: &TestBackend<AppRoot>) -> (Color, Color) {
    let frame = backend.capture_frame();
    (
        frame.cell(SIDEBAR_CELL.0, SIDEBAR_CELL.1).bg,
        frame.cell(WORKSPACE_CELL.0, WORKSPACE_CELL.1).bg,
    )
}

#[test]
fn modal_dim_covers_the_sidebar_as_well_as_the_workspace() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend();
            let (sidebar_lit, workspace_lit) = cells(&backend);

            backend.state_mut().show_help = true;
            backend.render();
            let (sidebar_dimmed, workspace_dimmed) = cells(&backend);

            assert_ne!(
                sidebar_lit, sidebar_dimmed,
                "the sidebar should dim behind a modal dialog"
            );
            assert_ne!(
                workspace_lit, workspace_dimmed,
                "the workspace should dim behind a modal dialog"
            );
            let backdrop = backend.state().theme.surface.backdrop;
            assert!(
                (sidebar_dimmed.luminance() - backdrop.luminance()).abs()
                    < (sidebar_lit.luminance() - backdrop.luminance()).abs(),
                "the dimmed sidebar should sit closer to the backdrop: \
                 lit={sidebar_lit:?} dimmed={sidebar_dimmed:?} backdrop={backdrop:?}"
            );

            backend.state_mut().show_help = false;
            backend.render();
            assert_eq!(
                cells(&backend),
                (sidebar_lit, workspace_lit),
                "closing the dialog should restore both layers"
            );
        })
        .expect("spawn sidebar dim smoke thread")
        .join()
        .expect("sidebar dim smoke completes");
}
