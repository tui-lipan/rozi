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
            // Every test in this binary shares one config file, and other tests save preferences
            // into it. This one writes the file itself and asserts what it contains, so hold it for
            // the whole test and start from no file, which is the state the assertions expect.
            let _config = rozi::test_support::lock_config_file();
            match std::fs::remove_file(rozi::config::config_path()) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => panic!("clear the shared config file: {err}"),
            }
            let extensions = extensions_root(root);
            // Keep filesystem order different from display-group order so selection must be
            // restored by entry identity rather than by the raw scan index.
            copy_fixture("invalid/incompatible-api", &extensions.join("a-future-api"));
            copy_fixture("valid/direct-command", &extensions.join("z-direct"));
            let manifest = extensions.join("z-direct/extension.toml");
            let mut text = std::fs::read_to_string(&manifest)
                .expect("read copied manifest")
                .replacen("[extension]\n", "[extension]\nversion = \"0.2.1\"\n", 1);
            text.push_str(
                "\n[settings]\nrunner = \"auto\"\n\
                 [[suggested_keybindings]]\n\
                 action = \"smart-focus-left\"\n\
                 key = \"ctrl-h\"\n",
            );
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
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::ReloadExtensions))
                .expect("load extension contributions");
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .expect("open extensions");

            // Opening the manager starts a real update check for the git-managed fixture, on its
            // own thread and against an unreachable remote. Its answer ("nothing available") is
            // addressed to the epoch this test would otherwise borrow, so whichever of the two
            // lands second wins - and under a loaded `cargo test` that is usually the real check,
            // wiping the injected update again. Claiming an epoch the in-flight check cannot hold
            // uses the staleness guard the message already carries: the real answer is discarded
            // as stale whenever it arrives, and this stops depending on the order.
            let epoch = u64::MAX;
            {
                let state = backend
                    .state_mut()
                    .extensions
                    .as_mut()
                    .expect("extensions state");
                state.update_check_epoch = epoch;
                state.update_checks.insert(
                    "fixture-direct".to_string(),
                    rozi::state::ExtensionUpdateCheck::Checking,
                );
            }
            let checking = frame(&mut backend);
            // The running check spins on its row beside the version; the tab only counts results.
            assert!(!checking.contains("Installed ·"), "{checking}");
            let row = checking
                .lines()
                .find(|line| line.contains("fixture-direct"))
                .expect("fixture row");
            assert!(
                ['◐', '◓', '◑', '◒']
                    .iter()
                    .any(|glyph| row.contains(&format!("{glyph} 0.2.1 · git"))),
                "{row}"
            );
            backend
                .dispatch(rozi::Msg::ExtensionUpdateChecked {
                    epoch,
                    id: "fixture-direct".to_string(),
                    check: rozi::state::ExtensionUpdateCheck::Available {
                        revision: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
                        version: Some("0.2.2".to_string()),
                    },
                })
                .expect("mark fixture update available");
            let list = frame(&mut backend);
            assert!(list.contains("Active"), "{list}");
            assert!(list.contains("fixture-direct"), "{list}");
            assert!(list.contains("Installed · 1 update"), "{list}");
            assert!(list.contains("0.2.1 → 0.2.2 · git"), "{list}");
            let (original, fixture) = {
                let state = backend.state().extensions.as_ref().unwrap();
                let fixture = state
                    .entries
                    .iter()
                    .position(|entry| entry.id.as_deref() == Some("fixture-direct"))
                    .unwrap();
                (state.selected, fixture)
            };
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Installed(fixture),
                ))
                .expect("select the Git fixture");
            let selected = frame(&mut backend);
            assert!(selected.contains("update Ctrl+U"), "{selected}");
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Installed(original),
                ))
                .expect("restore the selection");
            assert!(list.contains("manual"), "{list}");
            assert!(list.contains("1 key active"), "{list}");
            assert!(list.contains("install"), "{list}");
            assert!(!list.contains("copy report"), "{list}");
            assert!(list.contains("Problems"), "{list}");
            assert!(list.contains("future-a"), "{list}");
            assert!(list.contains("requires extension API 2"), "{list}");
            assert!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .unwrap()
                    .entries
                    .iter()
                    .find(|entry| entry.id.as_deref() == Some("fixture-direct"))
                    .unwrap()
                    .suggested_keybindings
                    .iter()
                    .any(|binding| binding.status
                        == rozi::config::ExtensionSuggestedKeybindingStatus::Active)
            );

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
            let frames: Vec<_> = backend
                .capture_ui_snapshot()
                .widgets
                .into_iter()
                .filter(|widget| widget.kind == tui_lipan::UiWidgetKind::Frame)
                .collect();
            let extensions_frame = frames
                .iter()
                .find(|widget| widget.title.as_deref() == Some("Extensions"))
                .expect("extensions frame");
            let install_frame = frames
                .iter()
                .find(|widget| widget.title.as_deref() == Some("Install extension"))
                .expect("install frame");
            assert_eq!(
                install_frame.rect.y,
                extensions_frame.rect.y + 1,
                "install prompt sits one row below Extensions, like Change keybinding on Keybindings"
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
            let install_error = (1..=10)
                .map(|line| format!("Installation failure detail {line:02}"))
                .collect::<Vec<_>>()
                .join("\n");
            backend.state_mut().extension_install = Some(rozi::state::ExtensionInstall {
                repository: None,
                label: "./missing".to_string(),
                detail: None,
                hidden: false,
            });
            assert!(
                frame(&mut backend).contains("Installing extension"),
                "the prompt's installation shows progress in its place"
            );
            backend
                .dispatch(rozi::Msg::ExtensionsInstallFinished(Err(
                    install_error.clone()
                )))
                .expect("show extension installation failure");
            assert_eq!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .and_then(|state| state.install_prompt.as_ref())
                    .and_then(|prompt| prompt.error.as_deref()),
                Some(install_error.as_str()),
                "installation errors must remain lossless for copying"
            );
            let first_error_page = frame(&mut backend);
            assert!(
                first_error_page.contains("Installation failure detail 01"),
                "{first_error_page}"
            );
            assert!(
                !first_error_page.contains("Installation failure detail 10"),
                "the error document should be capped before scrolling:\n{first_error_page}"
            );
            for _ in 0..10 {
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Down,
                        mods: KeyMods::NONE,
                    })
                    .expect("scroll install error document down");
            }
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref() == "rozi-extension-install-source"),
                "the source input keeps focus while its arrow keys scroll the error"
            );
            let last_error_page = frame(&mut backend);
            assert!(
                last_error_page.contains("Installation failure detail 10"),
                "{last_error_page}"
            );
            let scrolled_offset = backend
                .state()
                .extensions
                .as_ref()
                .and_then(|state| state.install_prompt.as_ref())
                .map(|prompt| prompt.error_scroll_offset)
                .unwrap();
            assert!(scrolled_offset > 0);
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Up,
                    mods: KeyMods::NONE,
                })
                .expect("scroll install error document up");
            assert_eq!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .and_then(|state| state.install_prompt.as_ref())
                    .map(|prompt| prompt.error_scroll_offset),
                Some(scrolled_offset - 1)
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
            assert!(empty.contains("No matches"), "{empty}");
            assert_eq!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .map(|state| state.query.text()),
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
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Installed(loaded),
                ))
                .expect("select loaded fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("disable selected fixture");
            let toggled = frame(&mut backend);
            // The group header, the `enable` hint, and the row's own description carry the new
            // state, so the toggle stays silent and the description does not repeat the group.
            assert!(toggled.contains("Disabled"), "{toggled}");
            assert!(toggled.contains("enable"), "{toggled}");
            assert!(!toggled.contains("Disabled fixture-direct"), "{toggled}");
            assert!(!toggled.contains("· disabled"), "{toggled}");
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
            assert_eq!(
                state.entries[state.selected].suggested_keybindings[0].status,
                rozi::config::ExtensionSuggestedKeybindingStatus::Suppressed
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

            // The config reloads above start fresh background update checks. Fence their replies
            // before comparing report frames so only the scroll position can change the capture.
            {
                let state = backend
                    .state_mut()
                    .extensions
                    .as_mut()
                    .expect("extensions state");
                state.update_check_epoch = u64::MAX;
                state.update_checks.clear();
            }
            backend
                .dispatch(rozi::Msg::ExtensionsOpenDetail)
                .expect("open extension detail");
            let detail = frame(&mut backend);
            for group in ["Overview", "Commands", "Suggested keybindings", "Settings"] {
                assert!(detail.contains(group), "missing {group}:\n{detail}");
            }
            assert!(detail.contains("suppressed"), "{detail}");
            assert!(detail.contains("copy report"), "{detail}");
            assert!(
                !detail.contains("open homepage"),
                "no homepage declared, so no link to open:\n{detail}"
            );
            assert!(!detail.contains("Ctrl+U"), "{detail}");
            assert!(!detail.contains("Search report"), "{detail}");
            // The launch line carries the extension's absolute directory, and a deep enough one
            // wraps inside the file name itself - on Windows the fold lands between `command.` and
            // `py`. What this asserts is that the command is shown, not where the panel folded it,
            // so match with the wrapping and the panel's chrome taken back out.
            let unfolded: String = detail
                .chars()
                .filter(|c| !c.is_whitespace() && !"│─╭╮╰╯".contains(*c))
                .collect();
            assert!(unfolded.contains("command.py"), "{detail}");
            // The report replaces the picker rather than stacking on it.
            assert!(!detail.contains("Search extensions…"), "{detail}");
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref() == "rozi-extension-detail")
            );
            // A viewport too short for the report puts it on the capped branch, where the arrows
            // and Page keys have somewhere to scroll to.
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 24,
            });
            let capped = frame(&mut backend);
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Down,
                    mods: KeyMods::NONE,
                })
                .expect("scroll the extension report down a row");
            let stepped = frame(&mut backend);
            assert_ne!(
                capped, stepped,
                "Down does not scroll the report:\n{capped}"
            );
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Up,
                    mods: KeyMods::NONE,
                })
                .expect("scroll the extension report back up");
            assert_eq!(
                capped,
                frame(&mut backend),
                "Up does not scroll the report back"
            );
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 52,
            });
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
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Installed(problem),
                ))
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
            // By directory name rather than by whole-path string equality. Two spellings of one
            // directory compare unequal - Windows hands `std::env::temp_dir` the 8.3 form
            // (`RUNNER~1`), and a path that has been through the filesystem may come back in the
            // long one - and the row this wants is "the duplicate fixture", which its own directory
            // name says exactly. The paths are printed when it misses so the next failure names the
            // mismatch instead of restating the question.
            let entries = &backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .entries;
            let duplicate_name = duplicate.file_name().expect("duplicate fixture is named");
            let duplicate_row = entries
                .iter()
                .position(|entry| {
                    std::path::Path::new(&entry.path).file_name() == Some(duplicate_name)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no row for the duplicate fixture at {}\nrows: {:?}",
                        duplicate.display(),
                        entries.iter().map(|entry| &entry.path).collect::<Vec<_>>()
                    )
                });
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Installed(duplicate_row),
                ))
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

#[test]
fn extensions_manager_browses_catalog_entries_without_hiding_installed_management() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            rozi::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 45,
            });
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .expect("open extensions");
            let epoch = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .catalog_epoch;
            let entries = serde_json::from_value(serde_json::json!([{
                "repository": "tui-lipan/vim-rozi-navigator",
                "source": "https://github.com/tui-lipan/vim-rozi-navigator.git",
                "commit": "5b5c8b9323e260a7c10a63d792274ca155d51e26",
                "manifest_path": "extension.toml",
                "id": "vim-rozi-navigator",
                "title": "Vim and Neovim navigator",
                "description": "Split-aware navigation policy for the Vim and Neovim editor plugin",
                "version": "0.2.1",
                "api": 1,
                "min_rozi": "0.0.16",
                "platforms": [],
                "homepage": "https://github.com/tui-lipan/vim-rozi-navigator",
                "stars": 0,
                "updated_at": "2026-09-21T00:00:00Z",
                "commands": 0,
                "services": 0,
                "agents": 0,
                "sidebar_tabs": 0,
                "navigation_targets": 1,
                "suggested_keybindings": 4
            }]))
            .expect("catalog fixture");
            backend
                .dispatch(rozi::Msg::ExtensionsCatalogLoaded {
                    epoch,
                    result: Ok(entries),
                })
                .expect("load catalog");
            assert_eq!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .expect("extensions state")
                    .catalog_entries
                    .len(),
                1
            );

            let catalog = frame(&mut backend);
            if backend
                .state()
                .extensions
                .as_ref()
                .is_some_and(|state| !state.entries.is_empty())
            {
                assert!(
                    catalog.contains("Active") || catalog.contains("Problems"),
                    "{catalog}"
                );
            }

            backend
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Catalog(0),
                ))
                .expect("select catalog extension");
            let selected_catalog = frame(&mut backend);
            assert!(selected_catalog.contains("Discover"), "{selected_catalog}");
            assert!(
                selected_catalog.contains("Vim and Neovim navigator"),
                "{selected_catalog}"
            );
            assert!(
                selected_catalog.contains("tui-lipan/vim-rozi-navigator"),
                "{selected_catalog}"
            );
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("open catalog detail");
            let detail = frame(&mut backend);
            assert!(
                detail.contains("Install extension · Vim and Neovim navigator"),
                "{detail}"
            );
            assert!(detail.contains("Not audited"), "{detail}");
            assert!(detail.contains("External tools may require"), "{detail}");
            assert!(detail.contains("install Enter"), "{detail}");

            backend
                .dispatch(rozi::Msg::CloseExtensionDetail)
                .expect("close catalog detail");
            let epoch = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .catalog_epoch;
            backend
                .dispatch(rozi::Msg::ExtensionsCatalogLoaded {
                    epoch,
                    result: Err("offline".to_string()),
                })
                .expect("show catalog failure");
            assert_eq!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .and_then(|state| state.catalog_error.as_deref()),
                Some("offline")
            );
        })
        .expect("spawn catalog smoke thread")
        .join()
        .expect("catalog smoke completes");
}

fn catalog_entry(repository: &str, id: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "repository": repository,
        "source": format!("https://github.com/{repository}.git"),
        "commit": "5b5c8b9323e260a7c10a63d792274ca155d51e26",
        "manifest_path": "extension.toml",
        "id": id,
        "title": title,
        "description": "Catalog lifecycle fixture",
        "version": "0.1.0",
        "api": 1,
        "min_rozi": null,
        "platforms": [],
        "homepage": null,
        "stars": 0,
        "updated_at": "2026-09-21T00:00:00Z",
        "commands": 0,
        "services": 0,
        "agents": 0,
        "sidebar_tabs": 0,
        "navigation_targets": 0,
        "suggested_keybindings": 0
    })
}

fn installing(repository: &str, label: &str) -> rozi::state::ExtensionInstall {
    rozi::state::ExtensionInstall {
        repository: Some(repository.to_string()),
        label: label.to_string(),
        detail: Some(format!("{repository} · 5b5c8b9323e2")),
        hidden: false,
    }
}

fn load_catalog(backend: &mut TestBackend<AppRoot>, entries: serde_json::Value) {
    let epoch = backend
        .state()
        .extensions
        .as_ref()
        .expect("extensions state")
        .catalog_epoch;
    backend
        .dispatch(rozi::Msg::ExtensionsCatalogLoaded {
            epoch,
            result: Ok(serde_json::from_value(entries).expect("catalog fixture")),
        })
        .expect("load catalog");
}

/// A refresh may reorder or drop discovery rows under an open report, and the user may leave the
/// report or the whole manager before an installation finishes. Neither may retarget the report or
/// lose a completed installation.
#[test]
fn catalog_install_survives_refreshes_and_closed_dialogs() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            // The isolated directories are shared by every test in this process, so this test
            // leaves the extensions directory alone and uses ids no other test installs.
            rozi::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 45,
            });
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .expect("open extensions");
            let first = catalog_entry("someone/first", "catalog-first", "First fixture");
            let second = catalog_entry("someone/second", "catalog-second", "Second fixture");
            load_catalog(&mut backend, serde_json::json!([first, second]));
            backend
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Catalog(0),
                ))
                .expect("select first catalog entry");
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("open catalog detail");

            let reviewed = |backend: &TestBackend<AppRoot>| {
                let state = backend.state().extensions.as_ref().expect("extensions");
                (
                    state
                        .catalog_detail
                        .as_ref()
                        .map(|detail| detail.entry.repository().to_string()),
                    state.catalog_selected,
                )
            };
            load_catalog(&mut backend, serde_json::json!([second, first]));
            assert_eq!(
                reviewed(&backend),
                (Some("someone/first".to_string()), Some(1)),
                "a reorder keeps the report and moves the selection with its repository"
            );
            load_catalog(&mut backend, serde_json::json!([second]));
            assert_eq!(
                reviewed(&backend),
                (Some("someone/first".to_string()), Some(0)),
                "the open report keeps its entry while the selection falls back to a visible row"
            );
            let detail = frame(&mut backend);
            assert!(
                detail.contains("Install extension · First fixture"),
                "{detail}"
            );

            backend.state_mut().extension_install =
                Some(installing("someone/first", "First fixture"));
            let progress = frame(&mut backend);
            assert!(
                progress.contains("Installing extension")
                    && progress.contains("First fixture")
                    && progress.contains("someone/first · 5b5c8b9323e2")
                    && progress.contains("hide Esc"),
                "{progress}"
            );
            assert!(
                !progress.contains("Install extension · First fixture"),
                "the progress modal takes the report's place:\n{progress}"
            );
            press(&mut backend, KeyCode::Esc);
            let hidden = backend
                .state()
                .extension_install
                .as_ref()
                .expect("still installing");
            assert!(hidden.hidden, "Esc hides the modal without cancelling");
            assert!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .is_some_and(|state| state.catalog_detail.is_none()),
                "and closes the report under it"
            );

            backend
                .dispatch(rozi::Msg::ExtensionsSelect(
                    rozi::state::ExtensionPickerRow::Catalog(0),
                ))
                .expect("select second catalog entry");
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("open second catalog detail");
            let blocked = frame(&mut backend);
            assert!(
                blocked.contains("Install extension · Second fixture")
                    && !blocked.contains("install Enter"),
                "a second installation cannot start while one is running:\n{blocked}"
            );

            backend
                .dispatch(rozi::Msg::ExtensionsCatalogInstallFinished {
                    repository: "someone/first".to_string(),
                    result: Ok("catalog-first".to_string()),
                })
                .expect("finish installation");
            assert_eq!(backend.state().extension_install, None);
            let unblocked = frame(&mut backend);
            assert!(
                unblocked.contains("Install extension · Second fixture")
                    && unblocked.contains("install Enter"),
                "another entry's report stays open and becomes installable:\n{unblocked}"
            );

            let mut hidden = installing("someone/second", "Second fixture");
            hidden.hidden = true;
            backend.state_mut().extension_install = Some(hidden);
            backend
                .dispatch(rozi::Msg::CloseExtensionDetail)
                .expect("close second report");
            backend
                .dispatch(rozi::Msg::ExtensionsToggleSelected)
                .expect("reopen the installing entry's report");
            assert!(
                backend
                    .state()
                    .extension_install
                    .as_ref()
                    .is_some_and(|install| !install.hidden),
                "reopening the report of the entry being installed shows its progress again"
            );
            assert!(frame(&mut backend).contains("Installing extension"));
            backend
                .dispatch(rozi::Msg::CloseExtensions)
                .expect("close the manager while installing");
            backend
                .dispatch(rozi::Msg::ExtensionsCatalogInstallFinished {
                    repository: "someone/second".to_string(),
                    result: Err("clone failed".to_string()),
                })
                .expect("finish installation with the manager closed");
            assert_eq!(backend.state().extension_install, None);
        })
        .expect("spawn catalog lifecycle thread")
        .join()
        .expect("catalog lifecycle completes");
}

/// Discovery shows its progress rather than popping rows in, and a failed refresh keeps the rows it
/// already listed.
#[test]
fn catalog_loading_shows_a_spinner_and_keeps_listed_rows_offline() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            rozi::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 45,
            });
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .expect("open extensions");
            backend
                .dispatch(rozi::Msg::ExtensionsTabSelected(
                    rozi::state::ExtensionsTab::Discover.index(),
                ))
                .expect("switch to Discover");
            let loading = |backend: &mut TestBackend<AppRoot>, value: bool| {
                backend
                    .state_mut()
                    .extensions
                    .as_mut()
                    .expect("extensions")
                    .catalog_loading = value;
            };

            loading(&mut backend, true);
            let first = frame(&mut backend);
            assert!(first.contains("loading index"), "{first}");

            load_catalog(
                &mut backend,
                serde_json::json!([catalog_entry(
                    "someone/listed",
                    "catalog-listed",
                    "Listed fixture"
                )]),
            );
            let loaded = frame(&mut backend);
            assert!(loaded.contains("Listed fixture"), "{loaded}");
            assert!(!loaded.contains("loading index"), "{loaded}");

            loading(&mut backend, true);
            let refreshing = frame(&mut backend);
            assert!(
                refreshing.contains("refreshing index") && refreshing.contains("Listed fixture"),
                "listed rows stay while a refresh runs:\n{refreshing}"
            );

            let epoch = backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions")
                .catalog_epoch;
            backend
                .dispatch(rozi::Msg::ExtensionsCatalogLoaded {
                    epoch,
                    result: Err("offline".to_string()),
                })
                .expect("fail the refresh");
            let offline = frame(&mut backend);
            assert!(
                offline.contains("index not refreshed · offline"),
                "{offline}"
            );
            assert!(offline.contains("Listed fixture"), "{offline}");
            assert!(!offline.contains("refreshing index"), "{offline}");
        })
        .expect("spawn catalog loading thread")
        .join()
        .expect("catalog loading completes");
}

fn installed_info(id: &str) -> rozi::config::ExtensionInfo {
    rozi::config::ExtensionInfo {
        id: Some(id.to_string()),
        title: None,
        description: None,
        version: Some("0.1.0".to_string()),
        api: Some(1),
        min_rozi: None,
        platforms: Vec::new(),
        homepage: None,
        path: format!("/nonexistent/{id}"),
        manifest_path: format!("/nonexistent/{id}/extension.toml"),
        enabled: true,
        status: rozi::config::ExtensionStatus::Loaded,
        commands: Vec::new(),
        services: Vec::new(),
        agents: Vec::new(),
        sidebar_tabs: Vec::new(),
        navigation_targets: Vec::new(),
        suggested_keybindings: Vec::new(),
        settings: Default::default(),
        command_details: Vec::new(),
        service_details: Vec::new(),
        command_paths: Default::default(),
        service_paths: Default::default(),
        errors: Vec::new(),
    }
}

fn press(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
    backend
        .send_key(KeyEvent {
            code,
            mods: KeyMods::NONE,
        })
        .expect("send key");
}

/// Installed extensions and the public index each get a tab. Discover keeps installed entries
/// listed with a badge, and a search filters the active tab.
#[test]
fn extensions_manager_splits_installed_and_discover_tabs() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            // The isolated directories are shared by every test in this process, so this test
            // leaves the extensions directory alone and injects its installed row instead.
            rozi::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 45,
            });
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .expect("open extensions");
            let tab = |backend: &TestBackend<AppRoot>| {
                backend.state().extensions.as_ref().expect("extensions").tab
            };
            assert_eq!(tab(&backend), rozi::state::ExtensionsTab::Installed);
            backend
                .state_mut()
                .extensions
                .as_mut()
                .expect("extensions")
                .entries
                .push(installed_info("catalog-owned"));
            let installed = frame(&mut backend);
            assert!(
                installed.contains("Installed") && installed.contains("Discover"),
                "{installed}"
            );
            assert!(installed.contains("catalog-owned"), "{installed}");
            assert!(installed.contains("reload"), "{installed}");

            press(&mut backend, KeyCode::Tab);
            assert_eq!(tab(&backend), rozi::state::ExtensionsTab::Discover);
            load_catalog(
                &mut backend,
                serde_json::json!([
                    catalog_entry("someone/owned", "catalog-owned", "Owned fixture"),
                    catalog_entry("someone/fresh", "catalog-fresh", "Fresh fixture"),
                ]),
            );
            let discover = frame(&mut backend);
            assert!(
                discover.contains("installed · 0.1.0 · someone/owned"),
                "an installed entry stays listed, badged:\n{discover}"
            );
            assert!(discover.contains("Fresh fixture"), "{discover}");
            assert!(discover.contains("refresh"), "{discover}");
            assert!(!discover.contains("reload"), "{discover}");

            for character in "fresh".chars() {
                press(&mut backend, KeyCode::Char(character));
            }
            let searching = frame(&mut backend);
            assert!(
                searching.contains("1/2") && searching.contains("Installed   Discover"),
                "the search field counts the active tab's matches; tab labels stay plain:\n{searching}"
            );
            assert!(!searching.contains("Owned fixture"), "{searching}");
            press(&mut backend, KeyCode::Enter);
            let fresh = frame(&mut backend);
            assert!(
                fresh.contains("Install extension · Fresh fixture")
                    && fresh.contains("install Enter")
                    && fresh.contains("open source Ctrl+L"),
                "{fresh}"
            );
            backend
                .dispatch(rozi::Msg::CloseExtensionDetail)
                .expect("close fresh report");
            for _ in 0.."fresh".len() {
                press(&mut backend, KeyCode::Backspace);
            }

            press(&mut backend, KeyCode::Up);
            press(&mut backend, KeyCode::Enter);
            let owned = frame(&mut backend);
            assert!(owned.contains("Extensions · Owned fixture"), "{owned}");
            assert!(
                !owned.contains("install Enter"),
                "an installed entry cannot be installed again:\n{owned}"
            );
            backend
                .dispatch(rozi::Msg::CloseExtensionDetail)
                .expect("close owned report");

            press(&mut backend, KeyCode::Left);
            assert_eq!(tab(&backend), rozi::state::ExtensionsTab::Installed);
        })
        .expect("spawn tabs thread")
        .join()
        .expect("tabs smoke completes");
}
