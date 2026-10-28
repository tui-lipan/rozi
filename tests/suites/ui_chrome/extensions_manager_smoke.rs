use std::path::{Path, PathBuf};

use rozi::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

/// Long on purpose: the report echoes the extension's directory into several lines, and a deep
/// directory (a Windows temp path is one) makes them wrap, which is the case worth covering on
/// every platform.
fn direct_dir_name(extensions: &Path) -> String {
    const PATH_LEN: usize = 105;
    let pad = PATH_LEN.saturating_sub(extensions.as_os_str().len()).max(8);
    format!("z{}", "d".repeat(pad - 1))
}

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

/// A real checkout plus its Git record, so `open` scans it and starts an update check.
fn install_git_extension(extensions: &Path, id: &str) {
    let dir = extensions.join(id);
    std::fs::create_dir_all(&dir).expect("create extension directory");
    std::fs::write(
        dir.join("extension.toml"),
        format!("[extension]\nid = \"{id}\"\napi = 1\ntitle = \"{id}\"\nversion = \"0.1.0\"\n"),
    )
    .expect("write extension manifest");
    mark_git_managed(extensions, id);
}

/// Removes the extensions this test installed into the process-wide scratch directory.
struct RemoveInstalledExtensions {
    extensions: PathBuf,
    ids: &'static [&'static str],
}

impl Drop for RemoveInstalledExtensions {
    fn drop(&mut self) {
        let records = self.extensions.join(".rozi/installations");
        for id in self.ids {
            let _ = std::fs::remove_dir_all(self.extensions.join(id));
            let _ = std::fs::remove_file(records.join(format!("{id}.toml")));
        }
    }
}

fn frame(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.advance(std::time::Duration::from_millis(200));
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

#[test]
fn extensions_manager_lists_toggles_and_opens_shared_diagnostics() {
    rozi::test_support::on_large_stack(|| {
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
        let direct_dir = direct_dir_name(&extensions);
        // Keep filesystem order different from display-group order so selection must be
        // restored by entry identity rather than by the raw scan index.
        copy_fixture("invalid/incompatible-api", &extensions.join("a-future-api"));
        copy_fixture("valid/direct-command", &extensions.join(&direct_dir));
        let manifest = extensions.join(&direct_dir).join("extension.toml");
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
            install_prompt.contains("runs as you, with no sandbox"),
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
        });
        let progress = frame(&mut backend);
        assert!(
            progress.contains("Installing ./missing") && progress.contains("Install extension"),
            "the prompt shows its installation's progress in its footer:\n{progress}"
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
        // A toggle reloads config, but it cannot move the fixture's remote, so the update
        // found earlier stays known instead of being checked for again.
        assert!(
            matches!(
                backend
                    .state()
                    .extensions
                    .as_ref()
                    .expect("extensions state")
                    .update_checks
                    .get("fixture-direct"),
                Some(rozi::state::ExtensionUpdateCheck::Available { .. })
            ),
            "a toggle must not restart the update check"
        );
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
        let config =
            std::fs::read_to_string(rozi::config::config_path()).expect("disabled list persisted");
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

        // Forget the injected update and fence any late reply before comparing report frames,
        // so only the scroll position can change the capture.
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
        // Tall enough that the whole report fits whatever length the directory has; the
        // scrolling behaviour is exercised below in deliberately short viewports.
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 110,
            h: 200,
        });
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
        assert!(
            detail.contains("check Ctrl+U"),
            "a Git installation offers its update check from the report:\n{detail}"
        );
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
        // The report's length follows the length of the extension directory (it is echoed
        // into the env lines and wraps), so page down until the frame stops moving instead
        // of counting pages.
        let mut bottom = frame(&mut backend);
        for _ in 0..16 {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::PageDown,
                    mods: KeyMods::NONE,
                })
                .expect("scroll extension report");
            let next = frame(&mut backend);
            if next == bottom {
                break;
            }
            bottom = next;
        }
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
        let neighbor = extensions.join("x-direct-neighbor");
        copy_fixture("valid/direct-command", &neighbor);
        let manifest = neighbor.join("extension.toml");
        let contents = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("fixture-direct", "fixture-direct-neighbor");
        std::fs::write(manifest, contents).unwrap();
        backend
            .dispatch(rozi::Msg::ExtensionsReload)
            .expect("rescan duplicate fixture");
        backend
            .state_mut()
            .extensions
            .as_mut()
            .unwrap()
            .query
            .set_text("fixture-direct");
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
            .position(|entry| std::path::Path::new(&entry.path).file_name() == Some(duplicate_name))
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
        assert!(extensions.join(&direct_dir).exists());
        let state = backend.state().extensions.as_ref().unwrap();
        assert_eq!(
            std::path::Path::new(&state.entries[state.selected].path).file_name(),
            Some(std::ffi::OsStr::new(&direct_dir)),
            "removal keeps the adjacent disabled installation selected"
        );
        let config = std::fs::read_to_string(rozi::config::config_path()).unwrap();
        assert!(
            config.contains("disabled = [\"fixture-direct\"]"),
            "the surviving installation keeps the disabled preference:\n{config}"
        );
    });
}

#[test]
fn extensions_manager_browses_catalog_entries_without_hiding_installed_management() {
    rozi::test_support::on_large_stack(|| {
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
        assert!(selected_catalog.contains("trusted"), "{selected_catalog}");
        backend
            .dispatch(rozi::Msg::ExtensionsToggleSelected)
            .expect("open catalog detail");
        let detail = frame(&mut backend);
        assert!(
            detail.contains("Install extension · Vim and Neovim navigator"),
            "{detail}"
        );
        assert!(
            detail.contains("Trusted · Maintained by the rozi author"),
            "{detail}"
        );
        assert!(!detail.contains("Not audited"), "{detail}");
        assert!(detail.contains("Unsandboxed · runs as you"), "{detail}");
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
    });
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
        detail: Some("5b5c8b9323e2".to_string()),
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
    rozi::test_support::on_large_stack(|| {
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

        backend.state_mut().extension_install = Some(installing("someone/first", "First fixture"));
        let progress = frame(&mut backend);
        assert!(
            progress.contains("Install extension · First fixture")
                && progress.contains("Installing First fixture")
                && progress.contains("5b5c8b9323e2")
                && !progress.contains("install Enter"),
            "the report stays and its footer shows the progress:\n{progress}"
        );
        press(&mut backend, KeyCode::Esc);
        assert!(
            backend.state().extension_install.is_some(),
            "closing the report does not cancel the installation"
        );
        assert!(
            backend
                .state()
                .extensions
                .as_ref()
                .is_some_and(|state| state.catalog_detail.is_none()),
            "it only closes the report"
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
                result: Ok("catalog-first".to_string().into()),
            })
            .expect("finish installation");
        assert_eq!(backend.state().extension_install, None);
        let unblocked = frame(&mut backend);
        assert!(
            unblocked.contains("Install extension · Second fixture")
                && unblocked.contains("install Enter"),
            "another entry's report stays open and becomes installable:\n{unblocked}"
        );

        backend.state_mut().extension_install =
            Some(installing("someone/second", "Second fixture"));
        backend
            .dispatch(rozi::Msg::CloseExtensionDetail)
            .expect("close second report");
        backend
            .dispatch(rozi::Msg::ExtensionsToggleSelected)
            .expect("reopen the installing entry's report");
        assert!(
            frame(&mut backend).contains("Installing Second fixture"),
            "reopening the report of the entry being installed shows its progress again"
        );
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
    });
}

/// Discovery shows its progress rather than popping rows in, and a failed refresh keeps the rows it
/// already listed.
#[test]
fn catalog_loading_shows_a_spinner_and_keeps_listed_rows_offline() {
    rozi::test_support::on_large_stack(|| {
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
    });
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
        screenshots: Vec::new(),
        path: format!("/nonexistent/{id}"),
        manifest_path: format!("/nonexistent/{id}/extension.toml"),
        enabled: true,
        status: rozi::config::ExtensionStatus::Loaded,
        commands: Vec::new(),
        link_handlers: Vec::new(),
        services: Vec::new(),
        agents: Vec::new(),
        sidebar_tabs: Vec::new(),
        navigation_targets: Vec::new(),
        suggested_keybindings: Vec::new(),
        settings: Default::default(),
        setting_specs: Default::default(),
        command_details: Vec::new(),
        service_details: Vec::new(),
        command_paths: Default::default(),
        service_paths: Default::default(),
        bundle: None,
        errors: Vec::new(),
    }
}

#[test]
fn installed_and_catalog_detail_modals_retain_their_closing_animation() {
    rozi::test_support::on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        for style in [
            rozi::layout::anim::PickerAnimationStyle::Fade,
            rozi::layout::anim::PickerAnimationStyle::Portal,
            rozi::layout::anim::PickerAnimationStyle::Scan,
        ] {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 110,
                h: 45,
            });
            backend.state_mut().config.animations.enabled = true;
            backend.state_mut().config.animations.picker = style;
            backend
                .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
                .unwrap();
            let state = backend.state_mut().extensions.as_mut().unwrap();
            state.entries = vec![installed_info("retention-fixture")];
            state.catalog_entries = serde_json::from_value(serde_json::json!([catalog_entry(
                "someone/retention",
                "retention-catalog",
                "Retention fixture"
            )]))
            .unwrap();
            state.catalog_loading = false;
            state.catalog_epoch = u64::MAX;

            for (tab, row, modal_key) in [
                (
                    rozi::state::ExtensionsTab::Installed,
                    rozi::state::ExtensionPickerRow::Installed(0),
                    "rozi-extension-detail-modal",
                ),
                (
                    rozi::state::ExtensionsTab::Discover,
                    rozi::state::ExtensionPickerRow::Catalog(0),
                    "rozi-catalog-extension-detail-modal",
                ),
            ] {
                backend.state_mut().extensions.as_mut().unwrap().tab = tab;
                backend.dispatch(rozi::Msg::ExtensionsSelect(row)).unwrap();
                backend.dispatch(rozi::Msg::ExtensionsOpenDetail).unwrap();
                backend.advance(std::time::Duration::from_millis(500));
                let key = tui_lipan::prelude::Key::from(modal_key);
                assert!(
                    backend.rect_of_key(&key).is_some(),
                    "{style:?}: detail is mounted"
                );
                assert_eq!(
                    backend.focused_key().unwrap().as_ref(),
                    "rozi-extension-detail"
                );
                backend.dispatch(rozi::Msg::CloseExtensionDetail).unwrap();
                backend.advance_frame(std::time::Duration::from_millis(30));
                assert!(
                    backend.rect_of_key(&key).is_some(),
                    "{style:?}: detail must retain its exit"
                );
                assert_ne!(
                    backend.focused_key().map(|key| key.as_ref()),
                    Some("rozi-extension-detail")
                );
                backend.advance(std::time::Duration::from_millis(500));
                assert!(
                    backend.rect_of_key(&key).is_none(),
                    "{style:?}: detail must be pruned after exit"
                );
            }
        }
    });
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
    rozi::test_support::on_large_stack(|| {
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
    });
}

/// `Ctrl+U` checks every Git installation from a row without a known update, `Ctrl+Y`
/// appears once more than one update is known and runs them all, and the run settles each row as
/// it finishes but reports once at the end.
#[test]
fn extensions_manager_checks_and_updates_every_git_installation() {
    rozi::test_support::on_large_stack(|| {
        // The isolated directories are shared by every test in this process, so this test
        // injects its rows. Their ids have no installation record, so a real update or check
        // fails at once without reaching a network.
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
        let available = || rozi::state::ExtensionUpdateCheck::Available {
            revision: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
            version: Some("0.2.0".to_string()),
        };
        let current = rozi::state::ExtensionUpdateCheck::Current;
        {
            let state = backend
                .state_mut()
                .extensions
                .as_mut()
                .expect("extensions state");
            for id in ["update-alpha", "update-beta", "update-local"] {
                state.entries.push(installed_info(id));
            }
            rozi::test_support::mark_extension_git_managed(state, "update-alpha");
            rozi::test_support::mark_extension_git_managed(state, "update-beta");
            // The check the manager started on opening answers an epoch this one replaces.
            state.update_check_epoch = u64::MAX;
            state.update_checks.clear();
            state
                .update_checks
                .insert("update-alpha".to_string(), current.clone());
            state
                .update_checks
                .insert("update-beta".to_string(), current.clone());
            state.selected = state.entries.len() - 1;
        }
        let check = |backend: &TestBackend<AppRoot>, id: &str| {
            backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .update_checks
                .get(id)
                .cloned()
        };

        let local = frame(&mut backend);
        assert!(local.contains("check Ctrl+U"), "{local}");
        assert!(!local.contains("update all"), "{local}");
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateSelected)
            .expect("check from a local row");
        // Without installation records both checks fail at once; wait for those answers so
        // they cannot land on the rows this test injects next.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while ["update-alpha", "update-beta"].iter().any(|id| {
            !matches!(
                check(&backend, id),
                Some(rozi::state::ExtensionUpdateCheck::Failed(_))
            )
        }) {
            assert!(
                std::time::Instant::now() < deadline,
                "Ctrl+U on any row asks every Git remote again"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
            backend.pump().expect("pump check answers");
        }

        {
            let state = backend.state_mut().extensions.as_mut().unwrap();
            state
                .update_checks
                .insert("update-alpha".to_string(), available());
            state
                .update_checks
                .insert("update-beta".to_string(), current.clone());
            state.selected = state.entries.len() - 3;
        }
        let one = frame(&mut backend);
        assert!(one.contains("update Ctrl+U"), "{one}");
        assert!(
            !one.contains("update all"),
            "a single update is Ctrl+U on its row:\n{one}"
        );
        backend
            .state_mut()
            .extensions
            .as_mut()
            .unwrap()
            .update_checks
            .insert("update-beta".to_string(), available());
        let two = frame(&mut backend);
        assert!(two.contains("update all Ctrl+Y"), "{two}");

        // A run in progress: each row settles when its own update finishes. The run itself
        // stays until the aggregate result, so nothing else can start in between.
        let run = 1;
        backend.state_mut().extension_update = Some(rozi::state::ExtensionUpdateRun {
            id: run,
            pending: ["update-alpha", "update-beta"].map(String::from).into(),
        });
        backend.state_mut().extensions.as_mut().unwrap().updating =
            ["update-alpha", "update-beta"].map(String::from).into();
        let running = frame(&mut backend);
        assert!(running.contains("updating…"), "{running}");
        assert!(!running.contains("update all"), "{running}");
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateFinished {
                run,
                id: "update-beta".to_string(),
                result: Ok(false.into()),
            })
            .expect("beta finished");
        assert_eq!(check(&backend, "update-beta"), Some(current.clone()));
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateFinished {
                run,
                id: "update-alpha".to_string(),
                result: Err("remote unreachable".to_string()),
            })
            .expect("alpha finished");
        assert_eq!(check(&backend, "update-alpha"), Some(available()));
        assert!(
            backend
                .state()
                .extensions
                .as_ref()
                .unwrap()
                .updating
                .is_empty()
        );
        assert!(backend.state().extension_update.is_some());
        backend
            .dispatch(rozi::Msg::ExtensionsUpdatesFinished {
                run,
                results: vec![
                    (
                        "update-alpha".to_string(),
                        Err("remote unreachable".to_string()),
                    ),
                    ("update-beta".to_string(), Ok(false.into())),
                ],
            })
            .expect("run finished");
        let reported = frame(&mut backend);
        assert!(
            reported.contains("update-alpha: remote unreachable"),
            "a run names the extension each failure belongs to:\n{reported}"
        );
        assert!(!reported.contains("up to date"), "{reported}");

        // The key itself: Ctrl+Y runs both, where Ctrl+U would update only the row.
        backend
            .state_mut()
            .extensions
            .as_mut()
            .unwrap()
            .update_checks
            .insert("update-beta".to_string(), available());
        frame(&mut backend);
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char('y'),
                mods: KeyMods::CTRL,
            })
            .expect("update all");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            backend.pump().expect("pump update results");
            let settled = frame(&mut backend);
            if settled.contains("update-beta:") {
                assert!(settled.contains("update-alpha:"), "{settled}");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "both updates report:\n{settled}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
}

/// An update keeps its identity when Extensions is closed. Reopening cannot start another run
/// beside it, and a completion from any other run cannot clear the one that is still busy.
#[test]
fn extensions_manager_keeps_an_update_run_across_reopen() {
    rozi::test_support::on_large_stack(|| {
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
        let alpha = "update-alpha";
        let beta = "update-beta";
        let available = || rozi::state::ExtensionUpdateCheck::Available {
            revision: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
            version: Some("0.2.0".to_string()),
        };
        let alpha_path = format!("/nonexistent/{alpha}");
        {
            let state = backend.state_mut().extensions.as_mut().unwrap();
            state.entries.push(installed_info(alpha));
            state.entries.push(installed_info(beta));
            rozi::test_support::mark_extension_removable(state, &alpha_path);
            state.update_checks.insert(alpha.to_string(), available());
            state.update_checks.insert(beta.to_string(), available());
            state.selected = state
                .entries
                .iter()
                .position(|entry| entry.id.as_deref() == Some(alpha))
                .expect("alpha row");
            state.updating.insert(alpha.to_string());
        }
        backend.state_mut().extension_update = Some(rozi::state::ExtensionUpdateRun {
            id: 40,
            pending: [alpha.to_string()].into(),
        });
        let running = frame(&mut backend);
        assert!(running.contains("updating…"), "{running}");
        assert!(!running.contains("update all"), "{running}");
        assert!(
            !running.contains("remove"),
            "a running update cannot be removed:\n{running}"
        );

        backend
            .dispatch(rozi::Msg::CloseExtensions)
            .expect("close extensions");
        assert!(backend.state().extensions.is_none());
        assert_eq!(
            pending_ids(&backend),
            vec![alpha.to_string()],
            "the update run survives closing the picker"
        );

        backend
            .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
            .expect("reopen extensions");
        {
            let state = backend.state_mut().extensions.as_mut().unwrap();
            assert!(
                state.updating.contains(alpha),
                "reopening shows the update that was already running"
            );
            state.entries.push(installed_info(alpha));
            state.entries.push(installed_info(beta));
            rozi::test_support::mark_extension_removable(state, &alpha_path);
            state.update_checks.insert(alpha.to_string(), available());
            state.update_checks.insert(beta.to_string(), available());
            state.selected = state
                .entries
                .iter()
                .position(|entry| entry.id.as_deref() == Some(alpha))
                .expect("alpha row after reopen");
        }
        let reopened = frame(&mut backend);
        assert!(reopened.contains("updating…"), "{reopened}");
        assert!(
            !reopened.contains("remove"),
            "the reopened update still cannot be removed:\n{reopened}"
        );

        backend
            .dispatch(rozi::Msg::ExtensionsUpdateAll)
            .expect("update all while the first run is active");
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateSelected)
            .expect("update selected while the first run is active");
        backend
            .dispatch(rozi::Msg::ExtensionsRemoveSelected)
            .expect("remove while the first run is active");
        assert_eq!(
            backend.state().extension_update.as_ref().map(|run| run.id),
            Some(40)
        );
        assert_eq!(pending_ids(&backend), vec![alpha.to_string()]);
        assert!(
            backend
                .state()
                .extensions
                .as_ref()
                .unwrap()
                .pending_remove
                .is_none()
        );

        backend
            .dispatch(rozi::Msg::ExtensionsUpdatesFinished {
                run: 7,
                results: vec![(alpha.to_string(), Ok(true.into()))],
            })
            .expect("stale aggregate");
        assert_eq!(pending_ids(&backend), vec![alpha.to_string()]);

        backend
            .dispatch(rozi::Msg::ExtensionsUpdateFinished {
                run: 40,
                id: alpha.to_string(),
                result: Ok(true.into()),
            })
            .expect("alpha's own row finished");
        backend
            .dispatch(rozi::Msg::ExtensionsUpdatesFinished {
                run: 40,
                results: vec![(alpha.to_string(), Ok(true.into()))],
            })
            .expect("first run finished");
        assert!(backend.state().extension_update.is_none());

        {
            // Finishing the run reloads, and that rescan drops rows this test injected.
            let state = backend.state_mut().extensions.as_mut().unwrap();
            if !state
                .entries
                .iter()
                .any(|entry| entry.id.as_deref() == Some(beta))
            {
                state.entries.push(installed_info(beta));
            }
            state.updating.insert(beta.to_string());
            rozi::test_support::mark_extension_removable(state, &format!("/nonexistent/{beta}"));
            state.selected = state
                .entries
                .iter()
                .position(|entry| entry.id.as_deref() == Some(beta))
                .expect("beta row");
        }
        backend.state_mut().extension_update = Some(rozi::state::ExtensionUpdateRun {
            id: 41,
            pending: [beta.to_string()].into(),
        });
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateFinished {
                run: 40,
                id: beta.to_string(),
                result: Ok(true.into()),
            })
            .expect("previous run's row result");
        backend
            .dispatch(rozi::Msg::ExtensionsUpdatesFinished {
                run: 40,
                results: vec![(beta.to_string(), Ok(true.into()))],
            })
            .expect("previous run's aggregate");
        backend
            .dispatch(rozi::Msg::ExtensionsRemoveSelected)
            .expect("remove the newer run");
        assert_eq!(
            backend.state().extension_update.as_ref().map(|run| run.id),
            Some(41)
        );
        assert_eq!(pending_ids(&backend), vec![beta.to_string()]);
        assert!(
            backend
                .state()
                .extensions
                .as_ref()
                .unwrap()
                .pending_remove
                .is_none()
        );
        let newer = frame(&mut backend);
        assert!(newer.contains("updating…"), "{newer}");
        assert!(
            !newer.contains("remove"),
            "the newer run stays busy after the previous run reports:\n{newer}"
        );
    });
}

/// Reopening Extensions while an update runs must not check the installations that update owns.
/// The check would be ignored, and a failed update would leave the row on `checking…`, which
/// `Ctrl+U` then skips.
#[test]
fn extensions_manager_reopen_does_not_stick_an_owned_row_on_checking() {
    rozi::test_support::on_large_stack(|| {
        let root = rozi::test_support::isolate_user_dirs();
        let extensions = extensions_root(root);
        let owned = "update-owned";
        let bystander = "update-bystander";
        let _cleanup = RemoveInstalledExtensions {
            extensions: extensions.clone(),
            ids: &["update-owned", "update-bystander"],
        };
        install_git_extension(&extensions, owned);
        install_git_extension(&extensions, bystander);

        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 110,
            h: 45,
        });
        // Mount before opening. The command link arrives with the first pump, and the check
        // started by `open` needs it. A dispatch that is also the first mount can open the
        // picker before that link exists, so no check starts at all.
        backend.render();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the mount never delivered the command link"
            );
            backend.pump().expect("pump the mount");
            std::thread::yield_now();
        }
        let run = 8;
        backend.state_mut().extension_update = Some(rozi::state::ExtensionUpdateRun {
            id: run,
            pending: [owned.to_string()].into(),
        });
        backend
            .dispatch(rozi::Msg::RunAction(rozi::input::Action::OpenExtensions))
            .expect("reopen extensions during the update");

        let check = |backend: &TestBackend<AppRoot>, id: &str| {
            backend
                .state()
                .extensions
                .as_ref()
                .expect("extensions state")
                .update_checks
                .get(id)
                .cloned()
        };
        {
            let state = backend.state().extensions.as_ref().unwrap();
            assert!(
                state
                    .entries
                    .iter()
                    .any(|entry| entry.id.as_deref() == Some(owned)),
                "open scanned the installation the update owns"
            );
            assert!(
                state.updating.contains(owned),
                "reopening shows the update that is still running"
            );
            assert!(
                check(&backend, owned).is_none(),
                "open does not check an installation the update owns, got {:?}",
                check(&backend, owned)
            );
            assert!(
                check(&backend, bystander).is_some(),
                "open still checks a Git installation the update does not own, got {:?}",
                check(&backend, bystander)
            );
        }

        let epoch = backend
            .state()
            .extensions
            .as_ref()
            .unwrap()
            .update_check_epoch;
        backend
            .dispatch(rozi::Msg::ExtensionUpdateChecked {
                epoch,
                id: owned.to_string(),
                check: rozi::state::ExtensionUpdateCheck::Available {
                    revision: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
                    version: Some("0.2.0".to_string()),
                },
            })
            .expect("check answer for the owned installation");
        assert!(
            check(&backend, owned).is_none(),
            "a check answer is ignored while the update owns the installation"
        );
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateSelected)
            .expect("check while the update runs");
        assert!(
            check(&backend, owned).is_none(),
            "Ctrl+U does not start a check for an installation the update owns"
        );

        backend
            .dispatch(rozi::Msg::ExtensionsUpdateFinished {
                run,
                id: owned.to_string(),
                result: Err("remote unreachable".to_string()),
            })
            .expect("owned update failed");
        assert!(
            !matches!(
                check(&backend, owned),
                Some(rozi::state::ExtensionUpdateCheck::Checking)
            ),
            "a failed update does not leave the row checking"
        );
        backend
            .dispatch(rozi::Msg::ExtensionsUpdatesFinished {
                run,
                results: vec![(owned.to_string(), Err("remote unreachable".to_string()))],
            })
            .expect("update run finished");
        assert!(backend.state().extension_update.is_none());

        // The run released the row, so the check it skipped can finish. A stuck
        // `checking…` has no worker left, and this wait would time out.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match check(&backend, owned) {
                Some(rozi::state::ExtensionUpdateCheck::Failed(_)) => break,
                Some(rozi::state::ExtensionUpdateCheck::Checking) | None => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the row stayed {:?} after the update failed",
                        check(&backend, owned)
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    backend.pump().expect("pump the check");
                }
                other => panic!("unexpected check for the failed update: {other:?}"),
            }
        }
        let settled = frame(&mut backend);
        let row = settled
            .lines()
            .find(|line| line.contains(owned))
            .expect("owned row");
        assert!(
            !['◐', '◓', '◑', '◒']
                .iter()
                .any(|glyph| row.contains(*glyph)),
            "the failed row is not stuck on the checking spinner:\n{row}"
        );
        assert!(row.contains("git"), "{row}");

        let owned_index = backend
            .state()
            .extensions
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|entry| entry.id.as_deref() == Some(owned))
            .expect("owned row");
        backend
            .dispatch(rozi::Msg::ExtensionsSelect(
                rozi::state::ExtensionPickerRow::Installed(owned_index),
            ))
            .expect("select the failed row");
        // A known answer, not `checking…`. Ctrl+U has to replace it. A skipped check would
        // leave this in place, including when the new check answers before this assertion.
        backend
            .state_mut()
            .extensions
            .as_mut()
            .unwrap()
            .update_checks
            .insert(
                owned.to_string(),
                rozi::state::ExtensionUpdateCheck::Current,
            );
        backend
            .dispatch(rozi::Msg::ExtensionsUpdateSelected)
            .expect("check the failed row");
        assert_ne!(
            check(&backend, owned),
            Some(rozi::state::ExtensionUpdateCheck::Current),
            "Ctrl+U checks the row after the failed update"
        );
    });
}

fn pending_ids(backend: &TestBackend<AppRoot>) -> Vec<String> {
    backend
        .state()
        .extension_update
        .as_ref()
        .map(|run| run.pending.iter().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn failed_revision_bump_during_removal_completes_local_mutation_and_surfaces_warning() {
    rozi::test_support::on_large_stack(|| {
        let root = rozi::test_support::isolate_user_dirs();
        let _config = rozi::test_support::lock_config_file();
        let extensions = extensions_root(root);
        let ext_dir = extensions.join("to-remove-fail-bump");
        copy_fixture("valid/direct-command", &ext_dir);
        let manifest = ext_dir.join("extension.toml");
        let text = std::fs::read_to_string(&manifest)
            .expect("read manifest")
            .replace("id = \"fixture-direct\"", "id = \"fixture-fail-bump\"");
        std::fs::write(&manifest, text).expect("update manifest id");
        mark_git_managed(&extensions, "fixture-fail-bump");

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

        assert!(ext_dir.exists());
        assert!(
            extensions
                .join(".rozi/installations/fixture-fail-bump.toml")
                .exists()
        );

        let row = backend
            .state()
            .extensions
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.id.as_deref() == Some("fixture-fail-bump"))
            .expect("find fixture-fail-bump row");
        backend
            .dispatch(rozi::Msg::ExtensionsSelect(
                rozi::state::ExtensionPickerRow::Installed(row),
            ))
            .expect("select fixture-fail-bump");

        // Arm remove
        backend
            .dispatch(rozi::Msg::ExtensionsRemoveSelected)
            .expect("arm remove");

        // Force revision persistence to fail by replacing .rozi/revision with a directory
        let revision_path = extensions.join(".rozi/revision");
        let _ = std::fs::remove_file(&revision_path);
        std::fs::create_dir_all(&revision_path).expect("create revision directory");

        // Confirm remove while revision bump cannot succeed
        backend
            .dispatch(rozi::Msg::ExtensionsRemoveSelected)
            .expect("remove selected");

        // Clean up the revision directory immediately so subsequent ops/tests are not disrupted
        let _ = std::fs::remove_dir_all(&revision_path);

        // 1. The operation remains committed
        assert!(!ext_dir.exists(), "extension directory must be removed");

        // 2. Local bookkeeping is completed
        assert!(
            !extensions
                .join(".rozi/installations/fixture-fail-bump.toml")
                .exists(),
            "metadata record must be removed"
        );

        // 3. The current client reconciles
        let frame_text = frame(&mut backend);
        assert!(
            !backend
                .state()
                .extensions
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .any(|e| e.id.as_deref() == Some("fixture-fail-bump")),
            "current client must have reloaded without the removed extension"
        );

        // 4. Notification failure is surfaced separately
        assert!(
            frame_text.contains("Extension removed")
                && frame_text.contains("were not told to reload"),
            "warning toast must be surfaced:\n{frame_text}"
        );
    });
}
