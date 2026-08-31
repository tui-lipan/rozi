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
    let manifest = format!(
        "[extension]\nid = \"smoke\"\ntitle = \"Extension smoke\"\nversion = \"1.0.0\"\napi = 1\n\
         [[commands]]\nid = \"probe\"\nlabel = \"Run extension probe\"\nexec = [\"{program}\"]\n"
    );
    std::fs::write(extension_dir.join("extension.toml"), &manifest).unwrap();
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

    // Opened and closed through the real action rather than by poking the flag: closing an overlay
    // is what re-enables the command registry, and a test that skips that path is testing a state
    // the app never reaches.
    backend
        .dispatch(Msg::RunAction(Action::TogglePalette))
        .unwrap();
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
    backend
        .send_key(KeyEvent {
            code: KeyCode::Esc,
            mods: KeyMods::NONE,
        })
        .unwrap();
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

    // Wait for the probe's *content*, not for its file to appear. A shell redirection creates the
    // target before the command writes to it, so `exists()` is true a moment before any bytes are
    // there - which made this read back an empty string and fail with `left: ""` on a Windows
    // runner that happened to poll inside that window.
    //
    // The budget is generous for the same reason the install self-test's is: spawning an
    // interpreter for a freshly written script on a loaded CI runner, past a virus scanner that
    // has never seen the file, is slow in a way that says nothing about whether it works.
    let result = extension_dir.join("result");
    let deadline = Instant::now() + Duration::from_secs(30);
    let probe_output = loop {
        let contents = std::fs::read_to_string(&result).unwrap_or_default();
        if !contents.is_empty() || Instant::now() >= deadline {
            break contents;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        probe_output,
        "smoke",
        "extension probe never wrote {}",
        result.display()
    );

    let changed_manifest = manifest
        .replace("id = \"probe\"", "id = \"probe-two\"")
        .replace("Run extension probe", "Changed extension probe");
    std::fs::write(extension_dir.join("extension.toml"), &changed_manifest).unwrap();
    std::fs::write(&config_path, "[keys]\n\"smoke.probe-two\" = \"i\"\n").unwrap();
    let changed = rozi::config::load_config();
    assert!(changed.warnings.is_empty(), "{:?}", changed.warnings);
    {
        let state = backend.state_mut();
        state.config = changed.config;
        state.commands_dirty = true;
    }
    backend
        .dispatch(Msg::RunAction(Action::ToggleDoNotDisturb))
        .unwrap();
    // Opened and closed through the real action rather than by poking the flag: closing an overlay
    // is what re-enables the command registry, and a test that skips that path is testing a state
    // the app never reaches.
    backend
        .dispatch(Msg::RunAction(Action::TogglePalette))
        .unwrap();
    backend.render();
    for ch in "changed".chars() {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char(ch),
                mods: KeyMods::NONE,
            })
            .unwrap();
    }
    let changed_palette = backend.capture_frame().to_fixed_grid_lines().join("\n");
    assert!(
        changed_palette.contains("Changed extension probe"),
        "{changed_palette}"
    );
    assert!(
        !changed_palette.contains("Run extension probe"),
        "{changed_palette}"
    );
    backend.state_mut().show_palette = false;

    {
        let state = backend.state_mut();
        state.config.commands.clear();
        state.config.key_overrides.clear();
        state.commands_dirty = true;
    }
    backend
        .dispatch(Msg::RunAction(Action::ToggleDoNotDisturb))
        .unwrap();
    // Opened and closed through the real action rather than by poking the flag: closing an overlay
    // is what re-enables the command registry, and a test that skips that path is testing a state
    // the app never reaches.
    backend
        .dispatch(Msg::RunAction(Action::TogglePalette))
        .unwrap();
    backend.render();
    let reloaded = backend.capture_frame().to_fixed_grid_lines().join("\n");
    assert!(!reloaded.contains("Run extension probe"), "{reloaded}");

    std::fs::write(
        &config_path,
        "[extensions]\ndisabled = [\"smoke\"]\n[keys]\n\"smoke.probe\" = \"i\"\n",
    )
    .unwrap();
    let disabled = rozi::config::load_config();
    assert!(disabled.config.commands.is_empty());
    assert!(disabled.config.key_overrides.contains_key("smoke.probe"));
    assert!(
        disabled
            .warnings
            .iter()
            .any(|warning| warning.contains("preserved but inactive"))
    );

    std::fs::write(extension_dir.join("extension.toml"), "not a valid manifest").unwrap();
    std::fs::write(&config_path, "[keys]\n\"smoke.probe\" = \"i\"\n").unwrap();
    let invalid = rozi::config::load_config();
    assert!(invalid.config.commands.is_empty());
    assert!(invalid.config.key_overrides.contains_key("smoke.probe"));
    assert!(
        invalid
            .warnings
            .iter()
            .any(|warning| warning.contains("invalid extension.toml"))
    );

    std::fs::write(extension_dir.join("extension.toml"), manifest).unwrap();
    let restored = rozi::config::load_config();
    assert_eq!(restored.config.commands[0].id, "smoke.probe");
    assert!(restored.config.key_overrides.contains_key("smoke.probe"));
    assert!(
        !restored
            .warnings
            .iter()
            .any(|warning| warning.contains("currently unavailable"))
    );
}
