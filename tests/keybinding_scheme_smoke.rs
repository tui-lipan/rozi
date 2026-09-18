//! The Keybindings overlay edits the input scheme itself (Prefix and Mod) and resolves conflicts
//! against source expressions, so nothing is frozen as a physical chord that stops following the
//! scheme. Scenarios share one config file, so they run in sequence inside one test.

use rozi::AppRoot;
use rozi::input::Action;
use rozi::state::{KeybindingEditorStage, ModifierChoice};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

fn backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 110,
        h: 50,
    });
    backend
}

/// Replace the config document, reload it, and open Keybindings on it.
fn load(backend: &mut TestBackend<AppRoot>, text: &str) {
    std::fs::create_dir_all(rozi::config::config_path().parent().unwrap()).unwrap();
    std::fs::write(rozi::config::config_path(), text).unwrap();
    backend
        .dispatch(rozi::Msg::RunAction(Action::ReloadExtensions))
        .expect("reload config");
    if backend.state().keybindings.is_none() {
        backend
            .dispatch(rozi::Msg::RunAction(Action::ToggleHelp))
            .expect("open keybindings");
    }
    backend.render();
}

fn press(backend: &mut TestBackend<AppRoot>, code: KeyCode, mods: KeyMods) {
    backend.send_key(KeyEvent { code, mods }).expect("send key");
    backend.render();
}

fn frame(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

fn config_text() -> String {
    rozi::config::read_config_text().expect("config readable")
}

fn resolved(backend: &TestBackend<AppRoot>, id: &str) -> Vec<String> {
    backend.state().config.key_overrides[id]
        .iter()
        .map(|binding| binding.canonical_lowercase())
        .collect()
}

fn stage(backend: &TestBackend<AppRoot>) -> KeybindingEditorStage {
    backend
        .state()
        .keybindings
        .as_ref()
        .expect("keybindings open")
        .stage
        .clone()
}

#[test]
fn prefix_and_mod_edits_move_scheme_bindings_and_keep_literals() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn scheme smoke thread")
        .join()
        .expect("scheme smoke completes");
}

fn body() {
    let mut backend = backend();
    taking_half_of_a_scheme_binding_rewrites_the_rest_semantically(&mut backend);
    a_prefix_change_moves_scheme_bindings_and_keeps_literals(&mut backend);
    converting_literals_is_explicit(&mut backend);
    a_colliding_prefix_is_refused(&mut backend);
    a_mod_change_moves_scheme_bindings_and_off_keeps_the_modifier(&mut backend);
    a_colliding_mod_is_refused(&mut backend);
    prefix_and_mod_reset_restore_defaults(&mut backend);
    edited_entries_use_rozis_spelling_and_others_stay_as_written(&mut backend);
}

fn edited_entries_use_rozis_spelling_and_others_stay_as_written(
    backend: &mut TestBackend<AppRoot>,
) {
    load(backend, "[keys]\ncopy-mode = \"cmd+f6\"\n");
    backend
        .dispatch(rozi::Msg::KeybindingCapture("close".to_string()))
        .expect("record close");
    let super_shift = KeyMods {
        super_key: true,
        shift: true,
        ..KeyMods::NONE
    };
    press(backend, KeyCode::Char('w'), super_shift);
    press(backend, KeyCode::Enter, KeyMods::NONE);
    let text = config_text();
    assert!(text.contains("close = \"super-shift-w\""), "{text}");
    assert!(
        text.contains("copy-mode = \"cmd+f6\""),
        "untouched entry rewritten:\n{text}"
    );
}

fn taking_half_of_a_scheme_binding_rewrites_the_rest_semantically(
    backend: &mut TestBackend<AppRoot>,
) {
    load(backend, "");
    backend
        .dispatch(rozi::Msg::KeybindingCapture("close".to_string()))
        .expect("record close");
    // New pane's `enter` is Prefix+Enter and Alt+Enter; this takes only the Alt half.
    press(backend, KeyCode::Enter, KeyMods::ALT);
    let conflict = frame(backend);
    assert!(conflict.contains("Already bound to"), "{conflict}");
    assert!(conflict.contains("New pane"), "{conflict}");
    press(backend, KeyCode::Enter, KeyMods::NONE);

    let text = config_text();
    assert!(text.contains("spawn = \"prefix:enter\""), "{text}");
    assert!(text.contains("close = \"alt-enter\""), "{text}");
    assert!(
        !text.contains("ctrl-a enter"),
        "a physical chord was frozen:\n{text}"
    );
    assert_eq!(resolved(backend, "spawn"), ["ctrl+a enter"]);
    assert_eq!(resolved(backend, "close"), ["alt+enter"]);
}

fn a_prefix_change_moves_scheme_bindings_and_keeps_literals(backend: &mut TestBackend<AppRoot>) {
    load(
        backend,
        "[keys]\ncopy-mode = \"f6\"\nfocus-left = \"ctrl-a f5\"\nclose = \"mod:f7\"\n",
    );
    backend
        .dispatch(rozi::Msg::KeybindingCapturePrefix)
        .expect("record prefix");
    press(backend, KeyCode::Char('b'), KeyMods::CTRL);
    let review = frame(backend);
    assert!(review.contains("Change prefix"), "{review}");
    assert!(
        review.contains("1 fixed binding stay on Ctrl+A"),
        "{review}"
    );
    assert!(review.contains("convert Tab"), "{review}");
    press(backend, KeyCode::Enter, KeyMods::NONE);

    assert!(config_text().contains("prefix = \"ctrl-b\""));
    assert_eq!(resolved(backend, "copy-mode"), ["ctrl+b f6", "alt+f6"]);
    assert_eq!(resolved(backend, "focus-left"), ["ctrl+a f5"]);
    assert_eq!(resolved(backend, "close"), ["alt+f7"]);
    assert_eq!(
        backend.state().config.input.prefix.canonical_lowercase(),
        "ctrl+b"
    );
}

fn converting_literals_is_explicit(backend: &mut TestBackend<AppRoot>) {
    load(backend, "[keys]\nfocus-left = \"ctrl-a f5\"\n");
    backend
        .dispatch(rozi::Msg::KeybindingCapturePrefix)
        .expect("record prefix");
    press(backend, KeyCode::Char('b'), KeyMods::CTRL);
    press(backend, KeyCode::Tab, KeyMods::NONE);
    let review = frame(backend);
    assert!(
        review.contains("1 fixed binding will follow the new prefix"),
        "{review}"
    );
    assert!(review.contains("keep fixed Tab"), "{review}");
    press(backend, KeyCode::Enter, KeyMods::NONE);

    assert!(config_text().contains("focus-left = \"prefix:f5\""));
    assert_eq!(resolved(backend, "focus-left"), ["ctrl+b f5"]);
}

fn a_colliding_prefix_is_refused(backend: &mut TestBackend<AppRoot>) {
    let text = "[keys]\nfocus-left = \"ctrl-b\"\n";
    load(backend, text);
    backend
        .dispatch(rozi::Msg::KeybindingCapturePrefix)
        .expect("record prefix");
    press(backend, KeyCode::Char('b'), KeyMods::CTRL);
    let conflict = frame(backend);
    assert!(conflict.contains("Collides:"), "{conflict}");
    assert!(conflict.contains("Focus left"), "{conflict}");
    assert!(!conflict.contains("replace Enter"), "{conflict}");
    press(backend, KeyCode::Enter, KeyMods::NONE);
    assert!(matches!(
        stage(backend),
        KeybindingEditorStage::Conflict { .. }
    ));
    assert_eq!(config_text(), text, "nothing is written");
    press(backend, KeyCode::Esc, KeyMods::NONE);
    press(backend, KeyCode::Esc, KeyMods::NONE);
}

fn a_mod_change_moves_scheme_bindings_and_off_keeps_the_modifier(
    backend: &mut TestBackend<AppRoot>,
) {
    load(
        backend,
        "[keys]\ncopy-mode = \"f6\"\nfocus-left = \"alt-f5\"\nclose = \"prefix:f7\"\n",
    );
    backend
        .dispatch(rozi::Msg::KeybindingEditModifier)
        .expect("edit modifier");
    let card = frame(backend);
    assert!(card.contains("Change modifier"), "{card}");
    assert!(card.contains("Alt   Super   Off"), "{card}");
    press(backend, KeyCode::Right, KeyMods::NONE);
    assert!(matches!(
        stage(backend),
        KeybindingEditorStage::Modifier {
            choice: ModifierChoice::Super,
            ..
        }
    ));
    assert!(frame(backend).contains("1 fixed binding stay on Alt"));
    press(backend, KeyCode::Enter, KeyMods::NONE);

    assert!(config_text().contains("modifier = \"super\""));
    assert_eq!(resolved(backend, "copy-mode"), ["ctrl+a f6", "super+f6"]);
    assert_eq!(resolved(backend, "focus-left"), ["alt+f5"]);
    assert_eq!(resolved(backend, "close"), ["ctrl+a f7"]);

    backend
        .dispatch(rozi::Msg::KeybindingEditModifier)
        .expect("edit modifier again");
    press(backend, KeyCode::Right, KeyMods::NONE);
    press(backend, KeyCode::Enter, KeyMods::NONE);
    let text = config_text();
    assert!(text.contains("modifier_shortcuts = false"), "{text}");
    assert!(
        text.contains("modifier = \"super\""),
        "Off keeps the modifier:\n{text}"
    );
    assert_eq!(resolved(backend, "copy-mode"), ["ctrl+a f6"]);
    assert_eq!(
        ModifierChoice::from_input(&backend.state().config.input),
        ModifierChoice::Off
    );
}

fn a_colliding_mod_is_refused(backend: &mut TestBackend<AppRoot>) {
    // Toggle floating's default `t` would become Super+T, which this literal already owns.
    let text = "[keys]\ncopy-mode = \"super-t\"\n";
    load(backend, text);
    backend
        .dispatch(rozi::Msg::KeybindingEditModifier)
        .expect("edit modifier");
    press(backend, KeyCode::Right, KeyMods::NONE);
    press(backend, KeyCode::Enter, KeyMods::NONE);
    let KeybindingEditorStage::Modifier { conflicts, .. } = stage(backend) else {
        panic!("the modifier card stays open");
    };
    assert!(!conflicts.is_empty());
    assert!(frame(backend).contains("Collides:"));
    assert_eq!(config_text(), text, "nothing is written");
    press(backend, KeyCode::Esc, KeyMods::NONE);
}

fn prefix_and_mod_reset_restore_defaults(backend: &mut TestBackend<AppRoot>) {
    if backend.state().keybindings.is_some() {
        backend
            .dispatch(rozi::Msg::CloseHelp)
            .expect("close leftover editor");
    }
    load(
        backend,
        "[input]\nprefix = \"ctrl-b\"\nmodifier = \"super\"\nmodifier_shortcuts = false\n",
    );
    let list = frame(backend);
    assert!(list.contains("Ctrl+B ← Ctrl+A"), "{list}");
    assert!(list.contains("Off ← Alt"), "{list}");
    assert!(list.contains("reset Ctrl+D"), "{list}");
    assert!(!list.contains("unbind Ctrl+U"), "{list}");

    backend
        .dispatch(rozi::Msg::KeybindingResetPrefix)
        .expect("reset prefix");
    let after_prefix = config_text();
    assert!(
        !after_prefix.contains("prefix ="),
        "prefix key remained:\n{after_prefix}"
    );
    assert_eq!(
        backend.state().config.input.prefix.canonical_lowercase(),
        "ctrl+a"
    );
    let prefix_restored = frame(backend);
    assert!(
        !prefix_restored.contains("Ctrl+B ← Ctrl+A"),
        "{prefix_restored}"
    );
    assert!(prefix_restored.contains("Ctrl+A"), "{prefix_restored}");
    assert!(prefix_restored.contains("Off ← Alt"), "{prefix_restored}");
    assert!(
        !prefix_restored.contains("reset Ctrl+D"),
        "prefix is already default:\n{prefix_restored}"
    );

    press(backend, KeyCode::Down, KeyMods::NONE);
    let mod_row = frame(backend);
    assert!(mod_row.contains("reset Ctrl+D"), "{mod_row}");
    backend
        .dispatch(rozi::Msg::KeybindingResetModifier)
        .expect("reset modifier");
    let after_mod = config_text();
    assert!(
        !after_mod.contains("modifier ="),
        "modifier key remained:\n{after_mod}"
    );
    assert!(
        !after_mod.contains("modifier_shortcuts"),
        "modifier_shortcuts remained:\n{after_mod}"
    );
    assert_eq!(
        ModifierChoice::from_input(&backend.state().config.input),
        ModifierChoice::Alt
    );
    let restored = frame(backend);
    assert!(!restored.contains("Off ← Alt"), "{restored}");
    assert!(restored.contains("Alt"), "{restored}");
    assert!(
        !restored.contains("reset Ctrl+D"),
        "mod is already default:\n{restored}"
    );
}
