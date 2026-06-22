use tui_lipan::prelude::*;

use crate::keymap::Keymap;
use crate::state::{Direction, InputConfig, RATIO_STEP, ThemePreset};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Spawn,
    Close,
    Focus(Direction),
    Move(Direction),
    SwitchWorkspace(usize),
    MoveToWorkspace(usize),
    ToggleFloat,
    ToggleFullscreen,
    RenamePane,
    Swap(Direction),
    CycleFocus(bool),
    PromoteToMaster,
    FlipSplit,
    AdjustRatio(f32),
    EnterResizeMode,
    ToggleLayout,
    EnterCopyMode,
    ToggleScratchpad,
    OpenSearch,
    SaveProfile,
    OpenThemePicker,
    SelectTheme(ThemePreset),
    TogglePalette,
    ToggleHelp,
    ToggleTitles,
}

impl Action {
    /// Stable kebab-case id for config binding (`[keys]`). Returns `None` for actions that
    /// are not individually rebindable (workspace switch/move are range-generated; theme
    /// selection is opened through the picker).
    pub fn id(self) -> Option<&'static str> {
        use Direction::{Down, Left, Right, Up};
        Some(match self {
            Action::Spawn => "spawn",
            Action::Close => "close",
            Action::Focus(Left) => "focus-left",
            Action::Focus(Down) => "focus-down",
            Action::Focus(Up) => "focus-up",
            Action::Focus(Right) => "focus-right",
            Action::Move(Left) => "move-left",
            Action::Move(Down) => "move-down",
            Action::Move(Up) => "move-up",
            Action::Move(Right) => "move-right",
            Action::Swap(Left) => "swap-left",
            Action::Swap(Down) => "swap-down",
            Action::Swap(Up) => "swap-up",
            Action::Swap(Right) => "swap-right",
            Action::CycleFocus(true) => "cycle-focus-next",
            Action::CycleFocus(false) => "cycle-focus-prev",
            Action::PromoteToMaster => "promote-to-master",
            Action::ToggleFloat => "toggle-float",
            Action::ToggleFullscreen => "toggle-fullscreen",
            Action::RenamePane => "rename-pane",
            Action::FlipSplit => "flip-split",
            Action::AdjustRatio(delta) if delta >= 0.0 => "grow-split",
            Action::AdjustRatio(_) => "shrink-split",
            Action::EnterResizeMode => "resize-mode",
            Action::ToggleLayout => "toggle-layout",
            Action::EnterCopyMode => "copy-mode",
            Action::ToggleScratchpad => "scratchpad",
            Action::OpenSearch => "search",
            Action::SaveProfile => "save-profile",
            Action::OpenThemePicker => "choose-theme",
            Action::TogglePalette => "command-palette",
            Action::ToggleHelp => "help",
            Action::ToggleTitles => "toggle-titles",
            Action::SwitchWorkspace(_) | Action::MoveToWorkspace(_) | Action::SelectTheme(_) => {
                return None;
            }
        })
    }

    /// Resolve a bindable action from its kebab-case id, or `None` for unknown ids.
    pub fn from_id(id: &str) -> Option<Action> {
        use Direction::{Down, Left, Right, Up};
        Some(match id {
            "spawn" => Action::Spawn,
            "close" => Action::Close,
            "focus-left" => Action::Focus(Left),
            "focus-down" => Action::Focus(Down),
            "focus-up" => Action::Focus(Up),
            "focus-right" => Action::Focus(Right),
            "move-left" => Action::Move(Left),
            "move-down" => Action::Move(Down),
            "move-up" => Action::Move(Up),
            "move-right" => Action::Move(Right),
            "swap-left" => Action::Swap(Left),
            "swap-down" => Action::Swap(Down),
            "swap-up" => Action::Swap(Up),
            "swap-right" => Action::Swap(Right),
            "cycle-focus-next" => Action::CycleFocus(true),
            "cycle-focus-prev" => Action::CycleFocus(false),
            "promote-to-master" => Action::PromoteToMaster,
            "toggle-float" => Action::ToggleFloat,
            "toggle-fullscreen" => Action::ToggleFullscreen,
            "rename-pane" => Action::RenamePane,
            "flip-split" => Action::FlipSplit,
            "grow-split" => Action::AdjustRatio(RATIO_STEP),
            "shrink-split" => Action::AdjustRatio(-RATIO_STEP),
            "resize-mode" => Action::EnterResizeMode,
            "toggle-layout" => Action::ToggleLayout,
            "copy-mode" => Action::EnterCopyMode,
            "scratchpad" => Action::ToggleScratchpad,
            "search" => Action::OpenSearch,
            "save-profile" => Action::SaveProfile,
            "choose-theme" => Action::OpenThemePicker,
            "command-palette" => Action::TogglePalette,
            "help" => Action::ToggleHelp,
            "toggle-titles" => Action::ToggleTitles,
            _ => return None,
        })
    }
}

/// A discrete, parameterless binding surfaced in the help overlay, and — when
/// `palette` is set — in the command palette. The help overlay is the full
/// keybinding reference: it documents *every* binding. The palette is curated to
/// commands that are awkward to reach by keyboard — those with no quick shortcut
/// (save profile, toggle titlebars, choose theme) plus a few discoverable extras
/// (search, resize mode, toggle layout, help). Frequent single-key actions
/// (spawn/close/float/fullscreen/rename/flip/grow/shrink) live in the help
/// reference only, since invoking them from a search box is slower than the key.
/// Workspace digits (1-9) are handled separately as they expand into a range.
pub struct CommandBinding {
    pub action: Action,
    pub label: &'static str,
    pub keys: &'static str,
    pub category: &'static str,
    /// Whether this binding appears as a runnable entry in the command palette.
    pub palette: bool,
}

pub fn command_bindings() -> Vec<CommandBinding> {
    use Direction::{Down, Left, Right, Up};
    vec![
        CommandBinding {
            action: Action::Spawn,
            label: "New pane",
            keys: "Enter / c",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Close,
            label: "Close pane",
            keys: "w / x",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::ToggleFloat,
            label: "Toggle floating",
            keys: "t",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::ToggleFullscreen,
            label: "Toggle fullscreen",
            keys: "f",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::RenamePane,
            label: "Rename pane",
            keys: "n",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Swap(Left),
            label: "Swap pane left",
            keys: "Ctrl+h / Ctrl+←",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Swap(Down),
            label: "Swap pane down",
            keys: "Ctrl+j / Ctrl+↓",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Swap(Up),
            label: "Swap pane up",
            keys: "Ctrl+k / Ctrl+↑",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Swap(Right),
            label: "Swap pane right",
            keys: "Ctrl+l / Ctrl+→",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::PromoteToMaster,
            label: "Promote to master",
            keys: ".",
            category: "Panes",
            palette: true,
        },
        CommandBinding {
            action: Action::Move(Left),
            label: "Move pane left",
            keys: "Shift+h / Shift+←",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Move(Down),
            label: "Move pane down",
            keys: "Shift+j / Shift+↓",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Move(Up),
            label: "Move pane up",
            keys: "Shift+k / Shift+↑",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::Move(Right),
            label: "Move pane right",
            keys: "Shift+l / Shift+→",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::FlipSplit,
            label: "Flip split axis",
            keys: "Space",
            category: "Layout",
            palette: false,
        },
        CommandBinding {
            action: Action::AdjustRatio(RATIO_STEP),
            label: "Grow split",
            keys: "] / +",
            category: "Layout",
            palette: false,
        },
        CommandBinding {
            action: Action::AdjustRatio(-RATIO_STEP),
            label: "Shrink split",
            keys: "-",
            category: "Layout",
            palette: false,
        },
        CommandBinding {
            action: Action::EnterResizeMode,
            label: "Resize mode",
            keys: "r",
            category: "Layout",
            palette: true,
        },
        CommandBinding {
            action: Action::ToggleLayout,
            label: "Toggle layout",
            keys: "m",
            category: "Layout",
            palette: true,
        },
        CommandBinding {
            action: Action::Focus(Left),
            label: "Focus left",
            keys: "h / ←",
            category: "Focus",
            palette: false,
        },
        CommandBinding {
            action: Action::Focus(Down),
            label: "Focus down",
            keys: "j / ↓",
            category: "Focus",
            palette: false,
        },
        CommandBinding {
            action: Action::Focus(Up),
            label: "Focus up",
            keys: "k / ↑",
            category: "Focus",
            palette: false,
        },
        CommandBinding {
            action: Action::Focus(Right),
            label: "Focus right",
            keys: "l / →",
            category: "Focus",
            palette: false,
        },
        CommandBinding {
            action: Action::CycleFocus(true),
            label: "Cycle focus next",
            keys: "Tab",
            category: "Focus",
            palette: false,
        },
        CommandBinding {
            action: Action::CycleFocus(false),
            label: "Cycle focus previous",
            keys: "Shift+Tab",
            category: "Focus",
            palette: false,
        },
        CommandBinding {
            action: Action::ToggleHelp,
            label: "Show keybindings",
            keys: "?",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::EnterCopyMode,
            label: "Copy mode",
            keys: "[",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::ToggleScratchpad,
            label: "Scratchpad",
            keys: "`",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::OpenSearch,
            label: "Search scrollback",
            keys: "/",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::SaveProfile,
            label: "Save project profile",
            keys: "",
            category: "Profile",
            palette: true,
        },
        CommandBinding {
            action: Action::OpenThemePicker,
            label: "Choose theme",
            keys: "",
            category: "Theme",
            palette: true,
        },
        CommandBinding {
            action: Action::ToggleTitles,
            label: "Toggle pane titlebars",
            keys: "",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::TogglePalette,
            label: "Command palette",
            keys: "p",
            category: "App",
            palette: false,
        },
    ]
}

pub fn is_prefix_key(key: KeyEvent, config: InputConfig) -> bool {
    key == config.prefix
}

/// Resolve a held-modifier chord to an action, honoring user `[keys]` overrides first and
/// then the compiled-in defaults (with any action the user has remapped suppressed).
pub fn action_for_held(key: KeyEvent, config: InputConfig, keymap: &Keymap) -> Option<Action> {
    if let Some(action) = keymap.held_action(key) {
        return Some(action);
    }
    match default_action_for_held(key, config) {
        Some(action) if keymap.overrides_action(action) => None,
        other => other,
    }
}

fn default_action_for_held(key: KeyEvent, config: InputConfig) -> Option<Action> {
    if !config.modifier.matches(key) {
        return None;
    }
    // modifier + Ctrl + direction swaps the focused pane with its neighbor. This is the one
    // held chord that uses Ctrl; every other held binding rejects it.
    if key.mods.ctrl {
        return swap_action_for_key(key);
    }
    action_for_command_key(key)
}

fn swap_action_for_key(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('h' | 'H') | KeyCode::Left => Some(Action::Swap(Direction::Left)),
        KeyCode::Char('j' | 'J') | KeyCode::Down => Some(Action::Swap(Direction::Down)),
        KeyCode::Char('k' | 'K') | KeyCode::Up => Some(Action::Swap(Direction::Up)),
        KeyCode::Char('l' | 'L') | KeyCode::Right => Some(Action::Swap(Direction::Right)),
        _ => None,
    }
}

/// Resolve a prefix-sequence key to an action, honoring user `[keys]` overrides first and
/// then the compiled-in defaults (with any action the user has remapped suppressed).
pub fn action_for_prefix(key: KeyEvent, keymap: &Keymap) -> Option<Action> {
    if let Some(action) = keymap.prefix_action(key) {
        return Some(action);
    }
    match default_action_for_prefix(key) {
        Some(action) if keymap.overrides_action(action) => None,
        other => other,
    }
}

fn default_action_for_prefix(key: KeyEvent) -> Option<Action> {
    if key.mods.ctrl || key.mods.alt || key.mods.super_key {
        return None;
    }
    action_for_command_key(key)
}

fn action_for_command_key(key: KeyEvent) -> Option<Action> {
    if let Some((index, symbol_implies_shift)) = workspace_key(key) {
        return Some(if key.mods.shift || symbol_implies_shift {
            Action::MoveToWorkspace(index)
        } else {
            Action::SwitchWorkspace(index)
        });
    }

    if key.mods.shift || matches!(key.code, KeyCode::Char('H' | 'J' | 'K' | 'L')) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => {
                return Some(Action::Move(Direction::Left));
            }
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => {
                return Some(Action::Move(Direction::Down));
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => {
                return Some(Action::Move(Direction::Up));
            }
            KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => {
                return Some(Action::Move(Direction::Right));
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Tab => Some(Action::CycleFocus(!key.mods.shift)),
        KeyCode::BackTab => Some(Action::CycleFocus(false)),
        KeyCode::Char('.') => Some(Action::PromoteToMaster),
        KeyCode::Char('`' | '~') => Some(Action::ToggleScratchpad),
        KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::Spawn),
        KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char('x') | KeyCode::Char('X') => {
            Some(Action::Close)
        }
        KeyCode::Char('t') | KeyCode::Char('T') => Some(Action::ToggleFloat),
        KeyCode::Char('f') | KeyCode::Char('F') => Some(Action::ToggleFullscreen),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::RenamePane),
        KeyCode::Char(' ') => Some(Action::FlipSplit),
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => {
            Some(Action::Focus(Direction::Left))
        }
        KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => {
            Some(Action::Focus(Direction::Down))
        }
        KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => Some(Action::Focus(Direction::Up)),
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => {
            Some(Action::Focus(Direction::Right))
        }
        KeyCode::Char('[') => Some(Action::EnterCopyMode),
        KeyCode::Char('-') => Some(Action::AdjustRatio(-RATIO_STEP)),
        KeyCode::Char(']') | KeyCode::Char('=') | KeyCode::Char('+') => {
            Some(Action::AdjustRatio(RATIO_STEP))
        }
        KeyCode::Char('r') | KeyCode::Char('R') => Some(Action::EnterResizeMode),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::ToggleLayout),
        KeyCode::Char('/') => Some(Action::OpenSearch),
        KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::TogglePalette),
        KeyCode::Char('?') => Some(Action::ToggleHelp),
        _ => None,
    }
}

fn workspace_key(key: KeyEvent) -> Option<(usize, bool)> {
    let (digit, symbol_implies_shift) = match key.code {
        KeyCode::Char('1') => (1, false),
        KeyCode::Char('2') => (2, false),
        KeyCode::Char('3') => (3, false),
        KeyCode::Char('4') => (4, false),
        KeyCode::Char('5') => (5, false),
        KeyCode::Char('6') => (6, false),
        KeyCode::Char('7') => (7, false),
        KeyCode::Char('8') => (8, false),
        KeyCode::Char('9') => (9, false),
        KeyCode::Char('!') => (1, true),
        KeyCode::Char('@') => (2, true),
        KeyCode::Char('#') => (3, true),
        KeyCode::Char('$') => (4, true),
        KeyCode::Char('%') => (5, true),
        KeyCode::Char('^') => (6, true),
        KeyCode::Char('&') => (7, true),
        KeyCode::Char('*') => (8, true),
        KeyCode::Char('(') => (9, true),
        _ => return None,
    };

    Some((digit - 1, symbol_implies_shift))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_profile_binding_is_palette_command() {
        let binding = command_bindings()
            .into_iter()
            .find(|binding| binding.action == Action::SaveProfile)
            .expect("save profile binding exists");

        assert_eq!(binding.label, "Save project profile");
        assert_eq!(binding.keys, "");
        assert_eq!(binding.category, "Profile");
        assert!(binding.palette);
    }

    #[test]
    fn frequent_single_key_actions_stay_out_of_palette() {
        let bindings = command_bindings();
        for action in [
            Action::Spawn,
            Action::Close,
            Action::ToggleFloat,
            Action::ToggleFullscreen,
            Action::RenamePane,
            Action::FlipSplit,
        ] {
            let binding = bindings
                .iter()
                .find(|binding| binding.action == action)
                .expect("binding exists");
            assert!(
                !binding.palette,
                "{:?} should not be a palette command",
                action
            );
        }
    }
}
