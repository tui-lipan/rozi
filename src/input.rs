use tui_lipan::prelude::*;

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
    FlipSplit,
    AdjustRatio(f32),
    EnterResizeMode,
    ToggleLayout,
    OpenSearch,
    SaveProfile,
    OpenThemePicker,
    SelectTheme(ThemePreset),
    TogglePalette,
    ToggleHelp,
    ToggleTitles,
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
            keys: "[ / -",
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
            action: Action::ToggleHelp,
            label: "Show keybindings",
            keys: "?",
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

pub fn action_for_held(key: KeyEvent, config: InputConfig) -> Option<Action> {
    if !config.modifier.matches(key) || key.mods.ctrl {
        return None;
    }
    action_for_command_key(key)
}

pub fn action_for_prefix(key: KeyEvent) -> Option<Action> {
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
        KeyCode::Char('[') | KeyCode::Char('-') => Some(Action::AdjustRatio(-RATIO_STEP)),
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
