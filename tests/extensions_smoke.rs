use std::time::{Duration, Instant};

use rozi::input::Action;
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{
    App, KeyCode, KeyDispatchPolicy, KeyEvent, KeyMods, Rect, TerminalKeyPolicy,
};

#[test]
fn discovered_extension_command_is_registered_and_dispatched() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(discovered_extension_command_is_registered_and_dispatched_inner)
        .unwrap()
        .join()
        .unwrap();
}

fn discovered_extension_command_is_registered_and_dispatched_inner() {
    rozi::test_support::isolate_user_dirs();
    let env = rozi::platform::paths::PlatformEnv::from_process();
    let extension_dir = rozi::platform::paths::extensions_dir(&env).join("smoke");
    let bin_dir = extension_dir.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = bin_dir.join("probe");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$ROZI_EXTENSION\" > \"$ROZI_EXTENSION_DIR/result\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(script, permissions).unwrap();
    }
    #[cfg(windows)]
    std::fs::write(
        bin_dir.join("probe.cmd"),
        "@echo off\r\n<nul set /p =%ROZI_EXTENSION%>\"%ROZI_EXTENSION_DIR%\\result\"\r\n",
    )
    .unwrap();

    let program = if cfg!(windows) {
        "./bin/probe.cmd"
    } else {
        "./bin/probe"
    };
    std::fs::write(
        extension_dir.join("extension.toml"),
        format!(
            "[extension]\ntitle = \"Extension smoke\"\nversion = \"1.0.0\"\n\
             [[commands]]\nid = \"probe\"\nlabel = \"Run extension probe\"\nexec = \"{program}\"\n"
        ),
    )
    .unwrap();
    let config_path = rozi::config::config_path();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "[keys]\n\"smoke.probe\" = \"i\"\n").unwrap();

    let loaded = rozi::config::load_config();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.config.commands[0].id, "smoke.probe");
    assert_eq!(loaded.config.commands[0].category, "Extension smoke");

    let app = App::new()
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal);
    let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    {
        let state = backend.state_mut();
        state.config = loaded.config;
        state.commands_dirty = true;
    }
    backend
        .dispatch(Msg::RunAction(Action::ToggleDoNotDisturb))
        .unwrap();

    backend.state_mut().show_palette = true;
    backend.render();
    for ch in "extension".chars() {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char(ch),
                mods: KeyMods::NONE,
            })
            .unwrap();
    }
    let palette = backend.capture_frame().to_fixed_grid_lines().join("\n");
    assert!(palette.contains("Run extension probe"), "{palette}");
    backend.state_mut().show_palette = false;
    backend.render();

    backend
        .send_key(KeyEvent {
            code: KeyCode::Char('a'),
            mods: KeyMods {
                ctrl: true,
                ..KeyMods::NONE
            },
        })
        .unwrap();
    backend
        .send_key(KeyEvent {
            code: KeyCode::Char('i'),
            mods: KeyMods::NONE,
        })
        .unwrap();

    let result = extension_dir.join("result");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !result.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read_to_string(result).unwrap(), "smoke");

    {
        let state = backend.state_mut();
        state.config.commands.clear();
        state.config.key_overrides.clear();
        state.commands_dirty = true;
    }
    backend
        .dispatch(Msg::RunAction(Action::ToggleDoNotDisturb))
        .unwrap();
    backend.state_mut().show_palette = true;
    backend.render();
    let reloaded = backend.capture_frame().to_fixed_grid_lines().join("\n");
    assert!(!reloaded.contains("Run extension probe"), "{reloaded}");
}
