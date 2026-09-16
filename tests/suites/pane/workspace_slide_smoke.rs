use std::time::Duration;

use rozi::input::Action;
use rozi::state::{Pane, PaneBorderMode};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseKind};
use tui_lipan::prelude::{FloatRect, MouseEvent, Rect};

fn backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 80,
        h: 24,
    });
    let state = backend.state_mut();
    state.config.animations.enabled = true;
    state.config.animations.workspace = true;
    state.config.animations.workspace_duration = Duration::from_secs(2);
    state.config.pane.border_mode = PaneBorderMode::Separate;
    state.config.pane.show_titles = true;
    for (index, title) in [(0, "FIRST"), (1, "SECOND"), (4, "FIFTH")] {
        let mut pane = Pane::new(
            index as u32 + 1,
            100,
            FloatRect {
                x: 4.0,
                y: 3.0,
                w: 30.0,
                h: 12.0,
            },
        );
        pane.opening = false;
        pane.terminal_active = true;
        pane.set_custom_title(title);
        pane.terminal.process_server_output(b"terminal body");
        let workspace = &mut state.current_mut().workspaces[index];
        workspace.panes = vec![pane];
        workspace.focused_pane = Some(index as u32 + 1);
    }
    state.current_mut().focused_pane = Some(1);
    backend.render();
    backend
}

fn run(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
}

fn switch(backend: &mut TestBackend<AppRoot>, index: usize) {
    backend
        .dispatch(Msg::RunAction(Action::SwitchWorkspace(index)))
        .unwrap();
    backend.render();
}

fn grid(backend: &TestBackend<AppRoot>) -> String {
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

fn terminal_rect(backend: &TestBackend<AppRoot>, id: u32) -> Rect {
    let key = format!("rozi-terminal-{id}");
    backend
        .capture_ui_snapshot_with_options(&tui_lipan::UiSnapshotOptions::diagnostic())
        .widgets
        .into_iter()
        .find(|widget| widget.key.as_ref().is_some_and(|k| k.as_ref() == key))
        .expect("terminal remains mounted")
        .rect
}

#[test]
fn workspace_slide_carries_both_pages_and_keeps_terminal_width() {
    run(|| {
        let mut backend = backend();
        let width = terminal_rect(&backend, 1).w;
        switch(&mut backend, 1);
        backend.advance(Duration::from_millis(500));
        let frame = grid(&backend);
        assert!(frame.contains("SECOND"), "incoming title: {frame}");
        // The outgoing right border and incoming left border are both inside the viewport.
        let middle = frame.lines().nth(12).unwrap();
        assert!(
            middle.chars().filter(|ch| matches!(ch, '│' | '║')).count() >= 2,
            "{}",
            backend
                .capture_ui_snapshot_with_options(&tui_lipan::UiSnapshotOptions::diagnostic())
                .to_markdown()
        );
        assert_eq!(terminal_rect(&backend, 1).w, width);
        assert_eq!(terminal_rect(&backend, 2).w, width);
        backend
            .send_mouse(MouseEvent {
                x: 10,
                y: 12,
                kind: MouseKind::Down(MouseButton::Left),
                mods: Default::default(),
            })
            .unwrap();
        assert_eq!(backend.state().current().focused_pane, Some(2));
        backend.advance(Duration::from_secs(3));
        assert!(grid(&backend).contains("SECOND"));
        assert!(!grid(&backend).contains("FIRST"));
    });
}

#[test]
fn workspace_slide_reverses_and_skips_intermediate_workspaces() {
    run(|| {
        let mut backend = backend();
        switch(&mut backend, 4);
        backend.advance(Duration::from_millis(300));
        assert!(!grid(&backend).contains("SECOND"));
        switch(&mut backend, 0);
        backend.advance(Duration::from_secs(3));
        assert!(grid(&backend).contains("FIRST"));
        assert!(!grid(&backend).contains("FIFTH"));
        assert_eq!(backend.state().current().focused_pane, Some(1));
    });
}

#[test]
fn workspace_slide_snaps_when_disabled_or_resized() {
    run(|| {
        for master in [false, true] {
            let mut backend = backend();
            switch(&mut backend, 1);
            backend.advance(Duration::from_millis(300));
            if master {
                backend.state_mut().config.animations.enabled = false;
            } else {
                backend.state_mut().config.animations.workspace = false;
            }
            backend.render();
            assert!(grid(&backend).contains("SECOND"));
            assert!(!grid(&backend).contains("FIRST"));
            switch(&mut backend, 0);
            assert!(grid(&backend).contains("FIRST"));
        }
        let mut backend = backend();
        switch(&mut backend, 1);
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        backend.render();
        assert!(grid(&backend).contains("SECOND"));
        assert!(!grid(&backend).contains("FIRST"));
    });
}

#[test]
fn workspace_slide_supports_floating_fullscreen_and_each_layout() {
    run(|| {
        for layout in rozi::state::LayoutKind::all() {
            for (floating, fullscreen) in [(false, false), (true, false), (false, true)] {
                let mut backend = backend();
                for index in [0, 1] {
                    let workspace = &mut backend.state_mut().current_mut().workspaces[index];
                    workspace.layout_kind = *layout;
                    workspace.panes[0].floating = floating;
                    workspace.panes[0].fullscreen = fullscreen;
                }
                backend.render();
                let settled = terminal_rect(&backend, 1);
                switch(&mut backend, 1);
                backend.advance(Duration::from_millis(500));
                let moving = terminal_rect(&backend, 2);
                assert_eq!((moving.w, moving.h), (settled.w, settled.h), "{layout:?}");
                backend.advance(Duration::from_secs(3));
                assert!(grid(&backend).contains("SECOND"), "{layout:?}");
            }
        }
    });
}

#[test]
fn workspace_slide_handles_empty_pages_zero_duration_and_attachment_changes() {
    run(|| {
        let mut backend = backend();
        switch(&mut backend, 2);
        backend.advance(Duration::from_secs(3));
        assert!(!grid(&backend).contains("FIRST"));
        switch(&mut backend, 0);
        backend.advance(Duration::from_secs(3));
        assert!(grid(&backend).contains("FIRST"));
        switch(&mut backend, 1);
        backend.state_mut().config.animations.workspace_duration = Duration::ZERO;
        backend.render();
        assert!(grid(&backend).contains("SECOND"));
        assert!(!grid(&backend).contains("FIRST"));
        backend.state_mut().config.animations.workspace_duration = Duration::from_secs(2);
        switch(&mut backend, 0);
        backend.state_mut().runtime_epoch += 1;
        backend.render();
        assert!(grid(&backend).contains("FIRST"));
        assert!(!grid(&backend).contains("SECOND"));
    });
}
