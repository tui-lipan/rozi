//! Extension commands an extension shows or hides in the command palette at runtime.
//!
//! A manifest can only say whether a command starts hidden; whether it is worth offering often
//! depends on things only the extension can see, such as whether something it installs is already
//! installed. `command-visibility` lets the extension say so. The choice belongs to the extension
//! generation that made it, so a reload that changes the extension falls back to its manifest
//! instead of carrying an old process's opinion forward.
//!
//! Only the palette honors it. A key binding and `run-action` still reach a hidden command, like
//! [`crate::commands::palette_visible`]: the palette lists what is worth offering, while a bound key
//! is something the user chose.

use crate::config::{ExtensionProvenance, NamedCommand};
use crate::control::ControlResponse;
use crate::state::State;

/// One extension's choice for one of its commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub generation: String,
    pub visible: bool,
}

/// Whether the command palette lists `command` now.
pub(crate) fn palette_lists(state: &State, command: &NamedCommand) -> bool {
    let Some(extension) = command.extension() else {
        return true;
    };
    match state.command_visibility.get(&command.id) {
        Some(choice) if state.extension_generations.get(extension) == Some(&choice.generation) => {
            choice.visible
        }
        _ => !command.hidden,
    }
}

/// Record an extension's choice to show or hide one of its own commands.
pub(crate) fn set(
    state: &mut State,
    extension: Option<&ExtensionProvenance>,
    command: &str,
    visible: bool,
) -> ControlResponse {
    let Some(extension) = extension else {
        return ControlResponse::error(
            "command-visibility is for extensions: run it from an extension's command or service",
        );
    };
    let command = command.trim();
    // Command IDs match `[a-z0-9_-]+`, so a dot always separates an extension ID from one.
    let public = match command.split_once('.') {
        None => format!("{}.{command}", extension.id),
        Some((owner, _)) if owner == extension.id => command.to_string(),
        Some(_) => {
            return ControlResponse::error(format!(
                "`{command}` is not a command of extension `{}`; an extension shows or hides only its own commands",
                extension.id
            ));
        }
    };
    let declared = state
        .config
        .commands
        .iter()
        .any(|declared| declared.id == public && declared.extension() == Some(&extension.id));
    if !declared {
        return ControlResponse::error(format!(
            "extension `{}` declares no command `{command}`",
            extension.id
        ));
    }
    // Choices from retired generations can never apply again.
    let generations = &state.extension_generations;
    state.command_visibility.retain(|_, choice| {
        generations
            .values()
            .any(|generation| generation == &choice.generation)
    });
    state.command_visibility.insert(
        public,
        Choice {
            generation: extension.generation.clone(),
            visible,
        },
    );
    ControlResponse::empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, UserCommandAction};

    fn command(id: &str, extension: Option<&str>, hidden: bool) -> NamedCommand {
        NamedCommand {
            id: id.to_string(),
            label: None,
            action: UserCommandAction::Send("x".to_string()),
            category: "Tools".to_string(),
            env: extension
                .map(|extension| vec![("ROZI_EXTENSION".to_string(), extension.to_string())])
                .unwrap_or_default(),
            default_key: None,
            hidden,
        }
    }

    fn state() -> State {
        let config = Config {
            commands: vec![
                command("tools.install", Some("tools"), true),
                command("tools.open", Some("tools"), false),
                command("other.open", Some("other"), false),
                command("mine", None, false),
            ],
            ..Config::default()
        };
        let mut state = State::new(config, tui_lipan::prelude::Theme::default());
        state.extension_generations = [
            ("tools".to_string(), "gen-1".to_string()),
            ("other".to_string(), "gen-9".to_string()),
        ]
        .into();
        state
    }

    fn tools(generation: &str) -> ExtensionProvenance {
        ExtensionProvenance {
            id: "tools".to_string(),
            generation: generation.to_string(),
        }
    }

    fn listed(state: &State, id: &str) -> bool {
        let command = state
            .config
            .commands
            .iter()
            .find(|command| command.id == id)
            .expect("declared");
        palette_lists(state, command)
    }

    #[test]
    fn the_manifest_decides_until_the_extension_does() {
        let mut state = state();
        assert!(!listed(&state, "tools.install"));
        assert!(listed(&state, "tools.open"));
        assert!(set(&mut state, Some(&tools("gen-1")), "install", true).ok);
        assert!(listed(&state, "tools.install"));
        assert!(set(&mut state, Some(&tools("gen-1")), "tools.open", false).ok);
        assert!(!listed(&state, "tools.open"));
    }

    #[test]
    fn the_palette_lists_a_hidden_command_once_its_extension_shows_it() {
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                crate::test_support::isolate_user_dirs();
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(tui_lipan::prelude::Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let mut install = command("tools.install", Some("tools"), true);
                install.label = Some("Install probe".to_string());
                let mut open = command("tools.open", Some("tools"), false);
                open.label = Some("Open probe".to_string());
                {
                    let state = backend.state_mut();
                    state.config.commands = vec![install, open];
                    state.extension_generations =
                        [("tools".to_string(), "gen-1".to_string())].into();
                    state.commands_dirty = true;
                }
                let palette = |backend: &mut tui_lipan::TestBackend<crate::AppRoot>| {
                    backend
                        .dispatch(crate::Msg::RunAction(crate::input::Action::TogglePalette))
                        .expect("open the palette");
                    backend.render();
                    for ch in "probe".chars() {
                        backend
                            .send_key(tui_lipan::prelude::KeyEvent {
                                code: tui_lipan::prelude::KeyCode::Char(ch),
                                mods: tui_lipan::prelude::KeyMods::NONE,
                            })
                            .expect("type");
                    }
                    backend.advance(std::time::Duration::from_millis(200));
                    let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
                    backend
                        .send_key(tui_lipan::prelude::KeyEvent {
                            code: tui_lipan::prelude::KeyCode::Esc,
                            mods: tui_lipan::prelude::KeyMods::NONE,
                        })
                        .expect("close the palette");
                    backend.render();
                    frame
                };

                let before = palette(&mut backend);
                assert!(before.contains("Open probe"), "{before}");
                assert!(!before.contains("Install probe"), "{before}");

                assert!(set(backend.state_mut(), Some(&tools("gen-1")), "install", true).ok);
                let after = palette(&mut backend);
                assert!(after.contains("Install probe"), "{after}");
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn a_reloaded_extension_starts_again_from_its_manifest() {
        let mut state = state();
        assert!(set(&mut state, Some(&tools("gen-1")), "install", true).ok);
        state
            .extension_generations
            .insert("tools".to_string(), "gen-2".to_string());
        assert!(!listed(&state, "tools.install"));
    }

    #[test]
    fn only_an_extension_and_only_for_its_own_declared_commands() {
        let mut state = state();
        let refused = |response: ControlResponse| response.error.unwrap_or_default();
        assert!(refused(set(&mut state, None, "tools.install", true)).contains("for extensions"));
        assert!(
            refused(set(&mut state, Some(&tools("gen-1")), "other.open", false))
                .contains("only its own")
        );
        assert!(
            refused(set(&mut state, Some(&tools("gen-1")), "missing", false))
                .contains("declares no command")
        );
        assert!(listed(&state, "other.open"));
        // A config.toml command has no extension and is always listed.
        assert!(listed(&state, "mine"));
        assert!(state.command_visibility.is_empty());
    }
}
