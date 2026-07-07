use tui_lipan::prelude::*;

use crate::config::InputConfig;
use crate::keymap::Keymap;
use crate::state::{Direction, Pane, RATIO_STEP, SCRATCH_PANE_ID, State};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Spawn,
    Close,
    Focus(Direction),
    Move(Direction),
    SwitchWorkspace(usize),
    MoveToWorkspace(usize),
    /// Move every pane (and the workspace name) from the active workspace into the target slot,
    /// then switch there. Triggered by `Ctrl+Shift+1`–`9` in prefix/modifier mode.
    RelocateWorkspace(usize),
    ToggleFloat,
    ToggleFullscreen,
    RenamePane,
    RenameWorkspace,
    Paste,
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
    OpenProfilePicker,
    OpenSessionPicker,
    DetachSession,
    OpenThemePicker,
    TogglePalette,
    ToggleHelp,
    ToggleTitles,
    ToggleTopBar,
    ToggleFocusOnHover,
    ToggleHighlightFocusedBackground,
    TogglePaneSynchronization,
    ReloadConfig,
    OpenConfigFile,
    /// Runs `config.user_commands[index]`. Defined only by `[keys]` table entries (see
    /// [`crate::config::build_keymap`]), so - like workspace digits - it has no static id and
    /// isn't independently rebindable or listed in [`command_bindings`].
    RunUserCommand(usize),
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
            Action::RenameWorkspace => "rename-workspace",
            Action::Paste => "paste",
            Action::FlipSplit => "flip-split",
            Action::AdjustRatio(delta) if delta >= 0.0 => "grow-split",
            Action::AdjustRatio(_) => "shrink-split",
            Action::EnterResizeMode => "resize-mode",
            Action::ToggleLayout => "toggle-layout",
            Action::EnterCopyMode => "copy-mode",
            Action::ToggleScratchpad => "scratchpad",
            Action::OpenSearch => "search",
            Action::SaveProfile => "save-profile",
            Action::OpenProfilePicker => "open-profile",
            Action::OpenSessionPicker => "sessions",
            Action::DetachSession => "detach-session",
            Action::OpenThemePicker => "choose-theme",
            Action::TogglePalette => "command-palette",
            Action::ToggleHelp => "help",
            Action::ToggleTitles => "toggle-titles",
            Action::ToggleTopBar => "toggle-top-bar",
            Action::ToggleFocusOnHover => "toggle-focus-on-hover",
            Action::ToggleHighlightFocusedBackground => "toggle-highlight-focused-background",
            Action::TogglePaneSynchronization => "toggle-pane-synchronization",
            Action::ReloadConfig => "reload-config",
            Action::OpenConfigFile => "open-config",
            Action::SwitchWorkspace(_)
            | Action::MoveToWorkspace(_)
            | Action::RelocateWorkspace(_)
            | Action::RunUserCommand(_) => {
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
            "rename-workspace" => Action::RenameWorkspace,
            "paste" => Action::Paste,
            "flip-split" => Action::FlipSplit,
            "grow-split" => Action::AdjustRatio(RATIO_STEP),
            "shrink-split" => Action::AdjustRatio(-RATIO_STEP),
            "resize-mode" => Action::EnterResizeMode,
            "toggle-layout" => Action::ToggleLayout,
            "copy-mode" => Action::EnterCopyMode,
            "scratchpad" => Action::ToggleScratchpad,
            "search" => Action::OpenSearch,
            "save-profile" => Action::SaveProfile,
            "open-profile" => Action::OpenProfilePicker,
            "sessions" => Action::OpenSessionPicker,
            "detach-session" => Action::DetachSession,
            "choose-theme" => Action::OpenThemePicker,
            "command-palette" => Action::TogglePalette,
            "help" => Action::ToggleHelp,
            "toggle-titles" => Action::ToggleTitles,
            "toggle-top-bar" => Action::ToggleTopBar,
            "toggle-focus-on-hover" => Action::ToggleFocusOnHover,
            "toggle-highlight-focused-background" => Action::ToggleHighlightFocusedBackground,
            "toggle-pane-synchronization" => Action::TogglePaneSynchronization,
            "reload-config" => Action::ReloadConfig,
            "open-config" => Action::OpenConfigFile,
            _ => return None,
        })
    }
}

/// A discrete, parameterless binding surfaced in the help overlay, and - when
/// `palette` is set - in the command palette. The help overlay is the full
/// keybinding reference: it documents *every* binding. The palette is curated to
/// commands that are awkward to reach by keyboard - those with no quick shortcut
/// (save profile, toggle titlebars, choose theme, focus-on-hover) plus a few discoverable extras
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
            label: "Floating",
            keys: "t",
            category: "Panes",
            palette: false,
        },
        CommandBinding {
            action: Action::ToggleFullscreen,
            label: "Fullscreen",
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
            action: Action::Paste,
            label: "Paste from clipboard",
            keys: "v",
            category: "Panes",
            palette: true,
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
            action: Action::TogglePaneSynchronization,
            label: "Pane synchronization",
            keys: "",
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
            label: "Switch layout",
            keys: "m",
            category: "Layout",
            palette: true,
        },
        CommandBinding {
            action: Action::RenameWorkspace,
            label: "Rename workspace",
            keys: "",
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
            label: "Keybindings",
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
            label: "Save profile",
            keys: "",
            category: "Profile",
            palette: true,
        },
        CommandBinding {
            action: Action::OpenProfilePicker,
            label: "Show profiles",
            keys: "",
            category: "Profile",
            palette: true,
        },
        CommandBinding {
            action: Action::OpenSessionPicker,
            label: "Show sessions",
            keys: "",
            category: "Session",
            palette: true,
        },
        CommandBinding {
            action: Action::DetachSession,
            label: "Detach from session",
            keys: "",
            category: "Session",
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
            label: "Pane titlebars",
            keys: "",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::ToggleTopBar,
            label: "Top bar",
            keys: "",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::ToggleFocusOnHover,
            label: "Focus on hover",
            keys: "",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::ToggleHighlightFocusedBackground,
            label: "Focused pane background",
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
        CommandBinding {
            action: Action::ReloadConfig,
            label: "Reload config",
            keys: "",
            category: "App",
            palette: true,
        },
        CommandBinding {
            action: Action::OpenConfigFile,
            label: "Open config file",
            keys: "",
            category: "App",
            palette: true,
        },
    ]
}

/// Resolve the user-facing label for a command, reflecting current state for toggle actions.
pub fn command_label(action: Action, state: &State) -> String {
    if let Some(label) = toggle_command_label(action, state) {
        return label;
    }

    if action == Action::ToggleLayout {
        let layout = state.workspaces[state.active_workspace].layout_kind.label();
        return format!("Switch layout (current: {layout})");
    }

    command_bindings()
        .into_iter()
        .find(|binding| binding.action == action)
        .map(|binding| binding.label.to_string())
        .unwrap_or_else(|| action.id().unwrap_or("command").to_string())
}

fn toggle_command_label(action: Action, state: &State) -> Option<String> {
    Some(match action {
        Action::ToggleFloat => {
            let enabled = focused_pane(state).is_some_and(|pane| pane.floating);
            enable_disable_label("floating", enabled)
        }
        Action::ToggleFullscreen => {
            let enabled = focused_pane(state).is_some_and(|pane| pane.fullscreen);
            enable_disable_label("fullscreen", enabled)
        }
        Action::TogglePaneSynchronization => {
            let enabled = state.workspaces[state.active_workspace].synchronized;
            enable_disable_label("pane synchronization", enabled)
        }
        Action::ToggleTitles => {
            enable_disable_label("pane titlebars", state.config.pane.show_titles)
        }
        Action::ToggleTopBar => enable_disable_label("top bar", state.config.pane.show_top_bar),
        Action::ToggleFocusOnHover => {
            enable_disable_label("focus on hover", state.config.pane.focus_on_hover)
        }
        Action::ToggleHighlightFocusedBackground => enable_disable_label(
            "focused pane background",
            state.config.pane.highlight_focused_background,
        ),
        Action::ToggleScratchpad => enable_disable_label("scratchpad", state.scratch_visible),
        Action::ToggleHelp => enable_disable_label("keybindings", state.show_help),
        Action::TogglePalette => enable_disable_label("command palette", state.show_palette),
        _ => return None,
    })
}

fn enable_disable_label(feature: &str, enabled: bool) -> String {
    if enabled {
        format!("Disable {feature}")
    } else {
        format!("Enable {feature}")
    }
}

fn focused_pane(state: &State) -> Option<&Pane> {
    let id = state.focused_pane?;
    if id == SCRATCH_PANE_ID {
        return state.scratch.as_ref();
    }
    state.workspaces[state.active_workspace]
        .panes
        .iter()
        .find(|pane| pane.id == id && !pane.closing)
}

pub fn is_prefix_key(key: KeyEvent, config: &InputConfig) -> bool {
    config.prefix.matches_sequence(&[key])
}

/// Resolve a normal-mode held chord to an action. Explicit held bindings win; otherwise the
/// configured WM modifier is stripped and the remaining key is matched against the active
/// tui-lipan-backed command keymap, so unbound modifier chords still fall through to the PTY.
pub fn action_for_held(key: KeyEvent, config: &InputConfig, keymap: &Keymap) -> Option<Action> {
    if let Some(action) = keymap.held_action(key) {
        return Some(action);
    }

    modifier_command_key(key, config).and_then(|command_key| action_for_prefix(command_key, keymap))
}

/// Resolve a prefix-sequence key to an action from the active tui-lipan-backed keymap.
/// Workspace digits remain generated as a range rather than individual bindings.
pub fn action_for_prefix(key: KeyEvent, keymap: &Keymap) -> Option<Action> {
    if let Some((index, symbol_implies_shift)) = workspace_key(key) {
        let shifted = key.mods.shift || symbol_implies_shift;
        return Some(if key.mods.ctrl && shifted {
            Action::RelocateWorkspace(index)
        } else if shifted {
            Action::MoveToWorkspace(index)
        } else {
            Action::SwitchWorkspace(index)
        });
    }

    keymap.prefix_action(key)
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

fn modifier_command_key(key: KeyEvent, config: &InputConfig) -> Option<KeyEvent> {
    if !config.modifier.matches(key) {
        return None;
    }

    let mut mods = key.mods;
    match config.modifier {
        crate::config::WmModifier::Alt => mods.alt = false,
        crate::config::WmModifier::Super => mods.super_key = false,
    }
    Some(KeyEvent {
        code: key.code,
        mods,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn save_profile_binding_is_palette_command() {
        let binding = command_bindings()
            .into_iter()
            .find(|binding| binding.action == Action::SaveProfile)
            .expect("save profile binding exists");

        assert_eq!(binding.label, "Save profile");
        assert_eq!(binding.keys, "");
        assert_eq!(binding.category, "Profile");
        assert!(binding.palette);
    }

    #[test]
    fn profiles_binding_is_palette_command() {
        let binding = command_bindings()
            .into_iter()
            .find(|binding| binding.action == Action::OpenProfilePicker)
            .expect("profiles binding exists");

        assert_eq!(binding.label, "Show profiles");
        assert_eq!(binding.keys, "");
        assert_eq!(binding.category, "Profile");
        assert!(binding.palette);
    }

    #[test]
    fn focus_on_hover_binding_is_palette_command() {
        let binding = command_bindings()
            .into_iter()
            .find(|binding| binding.action == Action::ToggleFocusOnHover)
            .expect("focus-on-hover binding exists");

        assert_eq!(binding.label, "Focus on hover");
        assert_eq!(binding.keys, "");
        assert_eq!(binding.category, "App");
        assert!(binding.palette);
    }

    #[test]
    fn reload_and_open_config_bindings_are_palette_commands_without_default_keys() {
        let bindings = command_bindings();
        for action in [Action::ReloadConfig, Action::OpenConfigFile] {
            let binding = bindings
                .iter()
                .find(|binding| binding.action == action)
                .expect("binding exists");
            assert_eq!(binding.keys, "");
            assert_eq!(binding.category, "App");
            assert!(binding.palette);
        }
    }

    #[test]
    fn every_command_binding_action_id_round_trips() {
        for binding in command_bindings() {
            let id = binding
                .action
                .id()
                .expect("every command_bindings() entry has a stable id");
            assert_eq!(
                Action::from_id(id),
                Some(binding.action),
                "id `{id}` should round-trip"
            );
        }
    }

    #[test]
    fn pane_synchronization_binding_is_palette_command_without_fake_key() {
        let binding = command_bindings()
            .into_iter()
            .find(|binding| binding.action == Action::TogglePaneSynchronization)
            .expect("pane synchronization binding exists");

        assert_eq!(binding.label, "Pane synchronization");
        assert_eq!(binding.keys, "");
        assert_eq!(binding.category, "Panes");
        assert!(binding.palette);
    }

    #[test]
    fn command_label_reflects_toggle_state() {
        use crate::config::HyprmuxConfig;
        use tui_lipan::Theme;

        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        assert_eq!(
            command_label(Action::ToggleFocusOnHover, &state),
            "Disable focus on hover"
        );
        assert_eq!(
            command_label(Action::ToggleTitles, &state),
            "Disable pane titlebars"
        );
        assert_eq!(
            command_label(Action::TogglePaneSynchronization, &state),
            "Enable pane synchronization"
        );

        state.config.pane.focus_on_hover = false;
        state.config.pane.show_titles = false;
        state.workspaces[0].synchronized = true;
        state.scratch_visible = true;
        state.config.pane.show_top_bar = false;
        state.workspaces[0].panes[0].floating = true;
        state.workspaces[0].panes[0].fullscreen = true;

        assert_eq!(
            command_label(Action::ToggleFocusOnHover, &state),
            "Enable focus on hover"
        );
        assert_eq!(
            command_label(Action::ToggleTitles, &state),
            "Enable pane titlebars"
        );
        assert_eq!(
            command_label(Action::TogglePaneSynchronization, &state),
            "Disable pane synchronization"
        );
        assert_eq!(
            command_label(Action::ToggleScratchpad, &state),
            "Disable scratchpad"
        );
        assert_eq!(
            command_label(Action::ToggleFloat, &state),
            "Disable floating"
        );
        assert_eq!(
            command_label(Action::ToggleFullscreen, &state),
            "Disable fullscreen"
        );
        assert_eq!(
            command_label(Action::ToggleTopBar, &state),
            "Enable top bar"
        );
        assert_eq!(
            command_label(Action::ToggleHighlightFocusedBackground, &state),
            "Enable focused pane background"
        );

        state.config.pane.show_top_bar = true;
        state.config.pane.highlight_focused_background = true;
        assert_eq!(
            command_label(Action::ToggleTopBar, &state),
            "Disable top bar"
        );
        assert_eq!(
            command_label(Action::ToggleHighlightFocusedBackground, &state),
            "Disable focused pane background"
        );
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

    #[test]
    fn unconfigured_held_modifier_chord_falls_through() {
        let key = KeyEvent {
            code: KeyCode::Char('z'),
            mods: KeyMods::ALT,
        };

        assert_eq!(
            action_for_held(key, &InputConfig::default(), &Keymap::default()),
            None
        );
    }

    #[test]
    fn modifier_uses_active_prefix_keymap_directly() {
        let key = KeyEvent {
            code: KeyCode::Enter,
            mods: KeyMods::ALT,
        };

        assert_eq!(
            action_for_held(key, &InputConfig::default(), &Keymap::default()),
            Some(Action::Spawn)
        );
    }

    #[test]
    fn modifier_plus_uses_tui_lipan_shift_equal_binding() {
        let key = KeyEvent {
            code: KeyCode::Char('='),
            mods: KeyMods {
                alt: true,
                shift: true,
                ..KeyMods::NONE
            },
        };

        assert_eq!(
            action_for_held(key, &InputConfig::default(), &Keymap::default()),
            Some(Action::AdjustRatio(RATIO_STEP))
        );
    }

    #[test]
    fn modifier_chord_falls_through_after_action_rebind_removes_key() {
        let key = KeyEvent {
            code: KeyCode::Char('n'),
            mods: KeyMods::ALT,
        };
        let mut keymap = Keymap::default();
        keymap.clear_action(Action::RenamePane);
        keymap.bind(
            Action::RenamePane,
            crate::keymap::Trigger::Prefix(KeyBinding::from_str("r").unwrap()),
            "r".to_string(),
        );

        assert_eq!(action_for_held(key, &InputConfig::default(), &keymap), None);
    }

    #[test]
    fn configured_held_chord_is_consumed() {
        let key = KeyEvent {
            code: KeyCode::Char('n'),
            mods: KeyMods::ALT,
        };
        let mut keymap = Keymap::default();
        keymap.bind(
            Action::RenamePane,
            crate::keymap::Trigger::Held(KeyBinding::from_str("alt-n").unwrap()),
            "Alt+N".to_string(),
        );

        assert_eq!(
            action_for_held(key, &InputConfig::default(), &keymap),
            Some(Action::RenamePane)
        );
    }

    #[test]
    fn prefix_ctrl_shift_workspace_digit_relocates_workspace() {
        let key = KeyEvent {
            code: KeyCode::Char('3'),
            mods: KeyMods {
                ctrl: true,
                shift: true,
                ..KeyMods::NONE
            },
        };

        assert_eq!(
            action_for_prefix(key, &Keymap::default()),
            Some(Action::RelocateWorkspace(2))
        );
    }

    #[test]
    fn prefix_shift_workspace_digit_moves_pane_without_ctrl() {
        let key = KeyEvent {
            code: KeyCode::Char('3'),
            mods: KeyMods {
                shift: true,
                ..KeyMods::NONE
            },
        };

        assert_eq!(
            action_for_prefix(key, &Keymap::default()),
            Some(Action::MoveToWorkspace(2))
        );
    }

    #[test]
    fn modifier_alt_ctrl_shift_workspace_digit_relocates_workspace() {
        let key = KeyEvent {
            code: KeyCode::Char('3'),
            mods: KeyMods {
                alt: true,
                ctrl: true,
                shift: true,
                ..KeyMods::NONE
            },
        };

        assert_eq!(
            action_for_held(key, &InputConfig::default(), &Keymap::default()),
            Some(Action::RelocateWorkspace(2))
        );
    }

    #[test]
    fn prefix_ctrl_direction_swaps_by_default() {
        let key = KeyEvent {
            code: KeyCode::Char('h'),
            mods: KeyMods::CTRL,
        };

        assert_eq!(
            action_for_prefix(key, &Keymap::default()),
            Some(Action::Swap(Direction::Left))
        );
    }
}
