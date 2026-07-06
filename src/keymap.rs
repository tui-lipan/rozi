use std::str::FromStr;

use tui_lipan::prelude::*;

use crate::input::Action;
use crate::state::{Direction, RATIO_STEP};

/// How a configured key reaches an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A held-modifier chord (e.g. `alt-enter`) matched in Normal mode.
    Held(KeyBinding),
    /// A key pressed after the prefix (e.g. `prefix c` / `ctrl-a c`).
    Prefix(KeyBinding),
}

impl Trigger {
    fn matches_key(&self, key: KeyEvent) -> bool {
        match self {
            Self::Held(binding) | Self::Prefix(binding) => binding.matches_sequence(&[key]),
        }
    }
}

#[derive(Clone, Debug)]
struct UserBinding {
    action: Action,
    trigger: Trigger,
    /// Display text for the help overlay / palette.
    display: String,
}

/// Active keybindings parsed with tui-lipan's keybinding syntax. Defaults and user `[keys]`
/// overrides live here so routing does not fall back to a separate hand-written key matcher.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<UserBinding>,
    configured_actions: Vec<Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut keymap = Self::empty();

        keymap.bind_default(Action::Spawn, "enter", "Enter");
        keymap.bind_default(Action::Spawn, "c", "c");
        keymap.bind_default(Action::Spawn, "shift-c", "c");
        keymap.bind_default(Action::Close, "w", "w");
        keymap.bind_default(Action::Close, "shift-w", "w");
        keymap.bind_default(Action::Close, "x", "x");
        keymap.bind_default(Action::Close, "shift-x", "x");
        keymap.bind_default(Action::ToggleFloat, "t", "t");
        keymap.bind_default(Action::ToggleFloat, "shift-t", "t");
        keymap.bind_default(Action::ToggleFullscreen, "f", "f");
        keymap.bind_default(Action::ToggleFullscreen, "shift-f", "f");
        keymap.bind_default(Action::RenamePane, "n", "n");
        keymap.bind_default(Action::RenamePane, "shift-n", "n");
        keymap.bind_default(Action::Paste, "v", "v");
        keymap.bind_default(Action::Paste, "shift-v", "v");
        keymap.bind_default(Action::Swap(Direction::Left), "ctrl-h", "Ctrl+h");
        keymap.bind_default(Action::Swap(Direction::Left), "ctrl-left", "Ctrl+Left");
        keymap.bind_default(Action::Swap(Direction::Down), "ctrl-j", "Ctrl+j");
        keymap.bind_default(Action::Swap(Direction::Down), "ctrl-down", "Ctrl+Down");
        keymap.bind_default(Action::Swap(Direction::Up), "ctrl-k", "Ctrl+k");
        keymap.bind_default(Action::Swap(Direction::Up), "ctrl-up", "Ctrl+Up");
        keymap.bind_default(Action::Swap(Direction::Right), "ctrl-l", "Ctrl+l");
        keymap.bind_default(Action::Swap(Direction::Right), "ctrl-right", "Ctrl+Right");
        keymap.bind_default(Action::PromoteToMaster, ".", ".");
        keymap.bind_default(Action::Move(Direction::Left), "shift-h", "Shift+h");
        keymap.bind_default(Action::Move(Direction::Left), "shift-left", "Shift+Left");
        keymap.bind_default(Action::Move(Direction::Down), "shift-j", "Shift+j");
        keymap.bind_default(Action::Move(Direction::Down), "shift-down", "Shift+Down");
        keymap.bind_default(Action::Move(Direction::Up), "shift-k", "Shift+k");
        keymap.bind_default(Action::Move(Direction::Up), "shift-up", "Shift+Up");
        keymap.bind_default(Action::Move(Direction::Right), "shift-l", "Shift+l");
        keymap.bind_default(Action::Move(Direction::Right), "shift-right", "Shift+Right");
        keymap.bind_default(Action::FlipSplit, "space", "Space");
        keymap.bind_default(Action::AdjustRatio(RATIO_STEP), "]", "]");
        keymap.bind_default(Action::AdjustRatio(RATIO_STEP), "=", "=");
        keymap.bind_default(Action::AdjustRatio(RATIO_STEP), "shift-=", "+");
        keymap.bind_default(Action::AdjustRatio(-RATIO_STEP), "minus", "-");
        keymap.bind_default(Action::EnterResizeMode, "r", "r");
        keymap.bind_default(Action::EnterResizeMode, "shift-r", "r");
        keymap.bind_default(Action::ToggleLayout, "m", "m");
        keymap.bind_default(Action::ToggleLayout, "shift-m", "m");
        keymap.bind_default(Action::Focus(Direction::Left), "h", "h");
        keymap.bind_default(Action::Focus(Direction::Left), "left", "Left");
        keymap.bind_default(Action::Focus(Direction::Down), "j", "j");
        keymap.bind_default(Action::Focus(Direction::Down), "down", "Down");
        keymap.bind_default(Action::Focus(Direction::Up), "k", "k");
        keymap.bind_default(Action::Focus(Direction::Up), "up", "Up");
        keymap.bind_default(Action::Focus(Direction::Right), "l", "l");
        keymap.bind_default(Action::Focus(Direction::Right), "right", "Right");
        keymap.bind_default(Action::CycleFocus(true), "tab", "Tab");
        keymap.bind_default(Action::CycleFocus(false), "shift-tab", "Shift+Tab");
        keymap.bind_default(Action::ToggleHelp, "?", "?");
        keymap.bind_default(Action::ToggleHelp, "shift-/", "?");
        keymap.bind_default(Action::EnterCopyMode, "[", "[");
        keymap.bind_default(Action::ToggleScratchpad, "`", "`");
        keymap.bind_default(Action::ToggleScratchpad, "shift-`", "~");
        keymap.bind_default(Action::OpenSearch, "/", "/");
        keymap.bind_default(Action::TogglePalette, "p", "p");
        keymap.bind_default(Action::TogglePalette, "shift-p", "p");

        keymap
    }
}

impl Keymap {
    pub fn empty() -> Self {
        Self {
            bindings: Vec::new(),
            configured_actions: Vec::new(),
        }
    }

    /// Add a user binding for `action` triggered by `trigger`. Multiple keys for one action
    /// accumulate; `display` is appended for the help/palette text.
    pub fn bind(&mut self, action: Action, trigger: Trigger, display: String) {
        self.bindings.push(UserBinding {
            action,
            trigger,
            display,
        });
    }

    /// Remove all active keys for an action before layering user replacements.
    pub fn clear_action(&mut self, action: Action) {
        self.bindings.retain(|binding| binding.action != action);
    }

    pub fn mark_configured(&mut self, action: Action) {
        if !self.configured_actions.contains(&action) {
            self.configured_actions.push(action);
        }
    }

    pub fn held_action(&self, key: KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .find_map(|binding| match binding.trigger {
                Trigger::Held(_) if binding.trigger.matches_key(key) => Some(binding.action),
                _ => None,
            })
    }

    pub fn prefix_action(&self, key: KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .find_map(|binding| match binding.trigger {
                Trigger::Prefix(_) if binding.trigger.matches_key(key) => Some(binding.action),
                _ => None,
            })
    }

    /// Display text for an action's configured keys, or `None` when it uses the defaults.
    pub fn keys_for(&self, action: Action) -> Option<String> {
        let mut keys = Vec::new();
        for binding in self
            .bindings
            .iter()
            .filter(|binding| binding.action == action)
        {
            if !keys.contains(&binding.display.as_str()) {
                keys.push(binding.display.as_str());
            }
        }
        if keys.is_empty() {
            self.configured_actions.contains(&action).then(String::new)
        } else {
            Some(keys.join(" / "))
        }
    }

    fn bind_default(&mut self, action: Action, raw: &str, display: &str) {
        let binding = KeyBinding::from_str(raw).expect("default key binding parses");
        self.bind(action, Trigger::Prefix(binding), display.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    fn binding(raw: &str) -> KeyBinding {
        KeyBinding::from_str(raw).expect("test binding parses")
    }

    #[test]
    fn held_and_prefix_lookups_respect_trigger_kind() {
        let mut keymap = Keymap::empty();
        keymap.bind(
            Action::Spawn,
            Trigger::Held(binding("alt-enter")),
            "Alt+Enter".to_string(),
        );
        keymap.bind(
            Action::Close,
            Trigger::Prefix(binding("q")),
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
            keymap.prefix_action(key(KeyCode::Char('q'), KeyMods::NONE)),
            Some(Action::Close)
        );
        assert_eq!(keymap.keys_for(Action::Spawn).as_deref(), Some("Alt+Enter"));
        assert_eq!(keymap.keys_for(Action::ToggleHelp), None);
    }

    #[test]
    fn default_prefix_bindings_use_tui_lipan_matching() {
        let keymap = Keymap::default();

        assert_eq!(
            keymap.prefix_action(key(KeyCode::Enter, KeyMods::NONE)),
            Some(Action::Spawn)
        );
        assert_eq!(
            keymap.prefix_action(key(KeyCode::Char('w'), KeyMods::NONE)),
            Some(Action::Close)
        );
        assert_eq!(
            keymap.prefix_action(key(KeyCode::Char('h'), KeyMods::CTRL)),
            Some(Action::Swap(Direction::Left))
        );
        assert_eq!(keymap.held_action(key(KeyCode::Enter, KeyMods::ALT)), None);
        assert_eq!(keymap.keys_for(Action::Spawn).as_deref(), Some("Enter / c"));
    }
}
