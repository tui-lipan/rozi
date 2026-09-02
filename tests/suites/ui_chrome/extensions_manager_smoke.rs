use std::path::{Path, PathBuf};

use rozi::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

fn copy_fixture(name: &str, destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions")
        .join(name);
    std::fs::create_dir_all(destination).expect("create extension fixture directory");
    for entry in std::fs::read_dir(source).expect("read extension fixture") {
        let entry = entry.expect("fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            copy_fixture(
                &format!("{name}/{}", entry.file_name().to_string_lossy()),
                &target,
            );
        } else {
            std::fs::copy(entry.path(), target).expect("copy extension fixture file");
        }
    }
}

fn extensions_root(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.join("AppData/Local/rozi/extensions")
    } else {
        root.join("data/rozi/extensions")
    }
}

fn mark_git_managed(extensions: &Path, id: &str) {
    let records = extensions.join(".rozi/installations");
    std::fs::create_dir_all(&records).expect("create installation records");
    std::fs::write(
        records.join(format!("{id}.toml")),
        format!(
            "schema_version = 1\nid = \"{id}\"\n\n[source]\nkind = \"git\"\nremote = \"https://example.invalid/{id}.git\"\nrevision = \"0123456789012345678901234567890123456789\"\n"
        ),
    )
    .expect("write Git installation record");
}

fn frame(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

#[test]
fn extensions_manager_lists_toggles_and_opens_shared_diagnostics() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let root = rozi::test_support::isolate_user_dirs();
            let extensions = extensions_root(root);
            // Keep filesystem order different from display-group order so selection must be
            // restored by entry identity rather than by the raw scan index.
            copy_fixture("invalid/incompatible-api", &extensions.join("a-future-api"));
            copy_fixture("valid/direct-command", &extensions.join("z-direct"));
            let manifest = extensions.join("z-direct/extension.toml");
            let mut text = std::fs::read_to_string(&manifest).expect("read copied manifest");
            text.push_str("\n[settings]\nrunner = \"auto\"\n");
            std::fs::write(&manifest, text).expect("add fixture setting");
            mark_git_managed(&extensions, "fixture-direct");

            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 52,
            });
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .expect("open extensions");

            let epoch = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .update_check_epoch;
            backend
                .dispatch(rozi::Msg::ExtensionsUpdatesChecked {
                    epoch,
                    available: vec!["fixture-direct".to_string()],
                })
                .expect("mark fixture update available");
            let list = frame(&mut backend);
            assert!(list.contains("Active"), "{list}");
            assert!(list.contains("fixture-direct"), "{list}");
            assert!(list.contains("update available"), "{list}");
            assert!(list.contains("install"), "{list}");
            assert!(list.contains("Problems"), "{list}");
            assert!(list.contains("future-a"), "{list}");
            assert!(list.contains("requires extension API 2"), "{list}");

            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char('i'),
                    mods: KeyMods::CTRL,
                })
                .expect("open extension install prompt");
            let install_prompt = frame(&mut backend);
            assert!(
                install_prompt.contains("Install extension"),
                "{install_prompt}"
            );
            assert!(
                install_prompt.contains("Local path or Git HTTPS/SSH URL"),
                "{install_prompt}"
            );
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref() == "rozi-extension-install-source")
            );
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Esc,
                    mods: KeyMods::NONE,
                })
                .expect("close extension install prompt");

            for character in "no-match".chars() {
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char(character),
                        mods: KeyMods::NONE,
                    })
                    .expect("type unmatched extension query");
            }
            let empty = frame(&mut backend);
            assert!(empty.contains("No extensions installed"), "{empty}");
            assert_eq!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .map(|state| state.query.as_str()),
                Some("no-match")
            );
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char('k'),
                    mods: KeyMods::CTRL,
                })
                .expect("hidden removal shortcut is ignored");
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Enter,
                    mods: KeyMods::NONE,
                })
                .expect("hidden activation is ignored");
            assert!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .is_some_and(|state| state.pending_remove.is_none())
            );
            assert!(!rozi::config::config_path().exists());
            for _ in 0.."no-match".len() {
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Backspace,
                        mods: KeyMods::NONE,
                    })
                    .expect("clear extension query");
            }
            frame(&mut backend);

            let loaded = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .entries
                .iter()
                .position(|entry| entry.id.as_deref() == Some("fixture-direct"))
                .expect("loaded fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(loaded))
                .expect("select loaded fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("disable selected fixture");
            let toggled = frame(&mut backend);
            assert!(toggled.contains("Disabled"), "{toggled}");
            let state = backend
                .state()
                .extensions
                .as_ref()
                .expect("overlay remains open");
            assert_eq!(
                state.entries[state.selected].id.as_deref(),
                Some("fixture-direct")
            );
            assert_eq!(
                state.entries[state.selected].status,
                rozi::config::ExtensionStatus::Disabled
            );
            let config = std::fs::read_to_string(rozi::config::config_path())
                .expect("disabled list persisted");
            assert!(
                config.contains("disabled = [\"fixture-direct\"]"),
                "{config}"
            );
            std::fs::write(
                rozi::config::config_path(),
                config.replace("disabled = [\"fixture-direct\"]", "disabled = []"),
            )
            .expect("edit disabled list outside manager");
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::ReloadExtensions))
                .expect("reload externally edited extension config");
            let state = backend
                .state()
                .extensions
                .as_ref()
                .expect("manager remains open after reload");
            assert_eq!(
                state.entries[state.selected].status,
                rozi::config::ExtensionStatus::Loaded
            );
            std::fs::write(
                rozi::config::config_path(),
                "[extensions]\ndisabled = [\" fixture-direct \"]\n",
            )
            .expect("write whitespace-padded disabled id");
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::ReloadExtensions))
                .expect("reload whitespace-padded disabled id");
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("enable whitespace-padded disabled id");
            let config = std::fs::read_to_string(rozi::config::config_path()).unwrap();
            assert!(
                !config.contains("disabled"),
                "enabling removes normalized disabled ids:\n{config}"
            );
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("restore disabled fixture for the remaining checks");

            backend
                .dispatch(rozi::Msg::ExtensionsOpenDetail)
                .expect("open extension detail");
            let detail = frame(&mut backend);
            for group in ["Overview", "Commands", "Settings"] {
                assert!(detail.contains(group), "missing {group}:\n{detail}");
            }
            assert!(detail.contains("update"), "{detail}");
            assert!(!detail.contains("Search report"), "{detail}");
            assert!(detail.contains("command.py"), "{detail}");
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref() == "rozi-extension-detail")
            );
            backend
                .send_key(KeyEvent {
                    code: KeyCode::PageDown,
                    mods: KeyMods::NONE,
                })
                .expect("scroll extension report");
            let bottom = frame(&mut backend);
            assert!(
                bottom
                    .lines()
                    .any(|line| line.contains("runner") && line.contains("\"auto\"")),
                "{bottom}"
            );
            assert!(
                !detail.contains("Services"),
                "empty group rendered:\n{detail}"
            );
            backend
                .dispatch(rozi::Msg::CloseExtensionDetail)
                .expect("close detail");

            let problem = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .entries
                .iter()
                .position(|entry| entry.id.as_deref() == Some("future-api"))
                .expect("problem fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(problem))
                .expect("select problem fixture");
            let before = std::fs::read_to_string(rozi::config::config_path()).unwrap();
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("problem enter is ignored");
            let after = std::fs::read_to_string(rozi::config::config_path()).unwrap();
            assert_eq!(after, before);
            assert!(backend.state().extensions.is_some());

            let duplicate = extensions.join("y-direct-duplicate");
            copy_fixture("valid/direct-command", &duplicate);
            backend
                .dispatch(rozi::Msg::ExtensionsReload)
                .expect("rescan duplicate fixture");
            let duplicate_row = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .entries
                .iter()
                .position(|entry| entry.path == duplicate.to_string_lossy())
                .expect("duplicate fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(duplicate_row))
                .expect("select duplicate fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsRemoveSelected)
                .expect("arm duplicate removal");
            backend
                .dispatch(rozi::Msg::ExtensionsRemoveSelected)
                .expect("remove duplicate fixture");
            assert!(!duplicate.exists());
            assert!(extensions.join("z-direct").exists());
            let config = std::fs::read_to_string(rozi::config::config_path()).unwrap();
            assert!(
                config.contains("disabled = [\"fixture-direct\"]"),
                "the surviving installation keeps the disabled preference:\n{config}"
            );
        })
        .expect("spawn extensions smoke thread")
        .join()
        .expect("extensions smoke completes");
}
