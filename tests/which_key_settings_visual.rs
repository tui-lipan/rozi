//! Visual reference for the Settings row that toggles the which-key strip.
//!
//! ```bash
//! cargo test --features ui-snapshot --test which_key_settings_visual -- --nocapture
//! ```
#![cfg(feature = "ui-snapshot")]

use rozi::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

fn capture(name: &str, which_key: bool, delay: rozi::config::WhichKeyDelay) {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    {
        let state = backend.state_mut();
        state.show_settings = true;
        state.config.input.which_key = which_key;
        state.config.input.which_key_delay = delay;
    }
    backend.render();
    let png = backend
        .capture_ui_snapshot()
        .to_png_default()
        .expect("encode settings png");
    let dir = std::path::Path::new("target/ui-sketches");
    std::fs::create_dir_all(dir).expect("create sketch dir");
    let path = dir.join(format!("{name}.png"));
    std::fs::write(&path, png).expect("write settings png");
    println!("wrote {}", path.display());
}

#[test]
fn which_key_rows_sit_in_the_general_group() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            capture(
                "which-key-settings-row",
                true,
                rozi::config::WhichKeyDelay::Short,
            );
            // With the panel off the delay row must read as inert rather than as a live value.
            capture(
                "which-key-settings-row-disabled",
                false,
                rozi::config::WhichKeyDelay::Long,
            );
        })
        .expect("spawn settings visual thread")
        .join()
        .expect("settings visual thread panicked");
}
