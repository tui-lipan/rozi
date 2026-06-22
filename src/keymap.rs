use tui_lipan::prelude::*;

use crate::input::Action;

/// How a configured key reaches an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A held-modifier chord (e.g. `alt-enter`) matched in Normal mode.
    Held(KeyEvent),
    /// A key pressed after the prefix (e.g. `prefix c` / `ctrl-a c`).
    Prefix(KeyEvent),
}

#[derive(Clone, Debug)]
struct UserBinding {
    action: Action,
    trigger: Trigger,
    /// Display text for the help overlay / palette.
    display: String,
}

/// User keybinding overrides parsed from `[keys]`, layered over the compiled-in defaults.
/// Overrides use replace-per-action semantics: configuring an action removes its default keys
/// and uses only the configured ones. The defaults live in `input.rs`; this type only stores
/// the user's additions and reports which actions they replace.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<UserBinding>,
}

impl Keymap {
    /// Add a user binding for `action` triggered by `trigger`. Multiple keys for one action
    /// accumulate; `display` is appended for the help/palette text.
    pub fn bind(&mut self, action: Action, trigger: Trigger, display: String) {
        self.bindings.push(UserBinding {
            action,
            trigger,
            display,
        });
    }

    /// True when the user has configured at least one key for this action, so its compiled-in
    /// default keys should be suppressed.
    pub fn overrides_action(&self, action: Action) -> bool {
        self.bindings.iter().any(|binding| binding.action == action)
    }

    pub fn held_action(&self, key: KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .find_map(|binding| match binding.trigger {
                Trigger::Held(trigger) if keys_match(trigger, key) => Some(binding.action),
                _ => None,
            })
    }

    pub fn prefix_action(&self, key: KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .find_map(|binding| match binding.trigger {
                Trigger::Prefix(trigger) if keys_match(trigger, key) => Some(binding.action),
                _ => None,
            })
    }

    /// Display text for an action's configured keys, or `None` when it uses the defaults.
    pub fn keys_for(&self, action: Action) -> Option<String> {
        let keys: Vec<&str> = self
            .bindings
            .iter()
            .filter(|binding| binding.action == action)
            .map(|binding| binding.display.as_str())
            .collect();
        if keys.is_empty() {
            None
        } else {
            Some(keys.join(" / "))
        }
    }
}

/// Compare two keys ignoring char case (the modifier bits must match exactly). The pressed
/// key and the configured trigger are normalized the same way.
fn keys_match(a: KeyEvent, b: KeyEvent) -> bool {
    normalize_code(a.code) == normalize_code(b.code) && a.mods == b.mods
}

fn normalize_code(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Direction;

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    #[test]
    fn held_and_prefix_lookups_respect_trigger_kind() {
        let mut keymap = Keymap::default();
        keymap.bind(
            Action::Spawn,
            Trigger::Held(key(KeyCode::Enter, KeyMods::ALT)),
            "Alt+Enter".to_string(),
        );
        keymap.bind(
            Action::Close,
            Trigger::Prefix(key(KeyCode::Char('q'), KeyMods::NONE)),
            "q".to_string(),
        );

        assert_eq!(
            keymap.held_action(key(KeyCode::Enter, KeyMods::ALT)),
            Some(Action::Spawn)
        );
        // A prefix binding does not fire on the held path and vice-versa.
        assert_eq!(
            keymap.held_action(key(KeyCode::Char('q'), KeyMods::NONE)),
            None
        );
        assert_eq!(
            keymap.prefix_action(key(KeyCode::Char('Q'), KeyMods::NONE)),
            Some(Action::Close)
        );
        assert!(keymap.overrides_action(Action::Spawn));
        assert!(!keymap.overrides_action(Action::Focus(Direction::Left)));
        assert_eq!(keymap.keys_for(Action::Spawn).as_deref(), Some("Alt+Enter"));
        assert_eq!(keymap.keys_for(Action::ToggleHelp), None);
    }
}
