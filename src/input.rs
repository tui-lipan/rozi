use crate::state::Direction;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Spawn,
    Close,
    Focus(Direction),
    /// Move focus in `Direction`, unless the focused pane runs a split-aware program (see
    /// `[navigation] editors`), in which case forward the matching `Ctrl-h/j/k/l` to it. Lets a
    /// single `Ctrl-h/j/k/l` binding navigate hyprmux panes and vim/neovim splits seamlessly
    /// (vim-tmux-navigator style).
    SmartFocus(Direction),
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
    RenameSession,
    TakeControl,
    Detach,
    Quit,
    KillWorkspace,
    KillSession,
    OpenThemePicker,
    OpenAppearance,
    TogglePalette,
    ToggleHelp,
    ToggleTitles,
    ToggleWorkbar,
    ToggleWorkbarGap,
    ToggleWorkbarPosition,
    ToggleWorkbarPowerline,
    ToggleAnimations,
    ToggleFocusOnHover,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleBorderMerge,
    CycleBorderStyle,
    CycleTitleStyle,
    CycleWorkbarBadgeStyle,
    CycleWorkbarTabStyle,
    CycleWorkbarStyle,
    TogglePaneSynchronization,
    OpenConfigFile,
    /// Runs `config.user_commands[index]`. Defined only by `[keys]` table entries (see
    /// `crate::config::build_key_overrides`), so - like workspace digits - it has no static id
    /// and isn't independently rebindable or listed in `crate::commands::BUILTIN_COMMANDS`.
    RunUserCommand(usize),
}

impl Action {
    /// Stable kebab-case id for config binding (`[keys]`) and command registration. Returns
    /// `None` for actions that are not individually rebindable (workspace switch/move are
    /// range-generated; theme selection is opened through the picker).
    pub fn id(self) -> Option<&'static str> {
        use Direction::{Down, Left, Right, Up};
        Some(match self {
            Action::Spawn => "spawn",
            Action::Close => "close",
            Action::Focus(Left) => "focus-left",
            Action::Focus(Down) => "focus-down",
            Action::Focus(Up) => "focus-up",
            Action::Focus(Right) => "focus-right",
            Action::SmartFocus(Left) => "smart-focus-left",
            Action::SmartFocus(Down) => "smart-focus-down",
            Action::SmartFocus(Up) => "smart-focus-up",
            Action::SmartFocus(Right) => "smart-focus-right",
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
            Action::RenameSession => "rename-session",
            Action::TakeControl => "take-control",
            Action::Detach => "detach",
            Action::Quit => "quit",
            Action::KillWorkspace => "kill-workspace",
            Action::KillSession => "kill-session",
            Action::OpenThemePicker => "choose-theme",
            Action::OpenAppearance => "change-appearance",
            Action::TogglePalette => "command-palette",
            Action::ToggleHelp => "help",
            Action::ToggleTitles => "toggle-titles",
            Action::ToggleWorkbar => "toggle-workbar",
            Action::ToggleWorkbarGap => "toggle-workbar-gap",
            Action::ToggleWorkbarPosition => "toggle-workbar-position",
            Action::ToggleWorkbarPowerline => "toggle-workbar-powerline",
            Action::ToggleAnimations => "toggle-animations",
            Action::ToggleFocusOnHover => "toggle-focus-on-hover",
            Action::ToggleHighlightFocusedBackground => "toggle-highlight-focused-background",
            Action::ToggleHighlightFocusedBorder => "toggle-highlight-focused-border",
            Action::ToggleBorderMerge => "toggle-border-merge",
            Action::CycleBorderStyle => "cycle-border-style",
            Action::CycleTitleStyle => "cycle-title-style",
            Action::CycleWorkbarBadgeStyle => "cycle-workbar-badge-style",
            Action::CycleWorkbarTabStyle => "cycle-workbar-tab-style",
            Action::CycleWorkbarStyle => "cycle-workbar-style",
            Action::TogglePaneSynchronization => "toggle-pane-synchronization",
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
        use crate::state::RATIO_STEP;
        use Direction::{Down, Left, Right, Up};
        Some(match id {
            "spawn" => Action::Spawn,
            "close" => Action::Close,
            "focus-left" => Action::Focus(Left),
            "focus-down" => Action::Focus(Down),
            "focus-up" => Action::Focus(Up),
            "focus-right" => Action::Focus(Right),
            "smart-focus-left" => Action::SmartFocus(Left),
            "smart-focus-down" => Action::SmartFocus(Down),
            "smart-focus-up" => Action::SmartFocus(Up),
            "smart-focus-right" => Action::SmartFocus(Right),
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
            "rename-session" => Action::RenameSession,
            "take-control" => Action::TakeControl,
            "detach" => Action::Detach,
            "quit" => Action::Quit,
            "kill-workspace" => Action::KillWorkspace,
            "kill-session" => Action::KillSession,
            "choose-theme" => Action::OpenThemePicker,
            "change-appearance" => Action::OpenAppearance,
            "command-palette" => Action::TogglePalette,
            "help" => Action::ToggleHelp,
            "toggle-titles" => Action::ToggleTitles,
            "toggle-workbar" => Action::ToggleWorkbar,
            "toggle-workbar-gap" => Action::ToggleWorkbarGap,
            "toggle-workbar-position" => Action::ToggleWorkbarPosition,
            "toggle-workbar-powerline" => Action::ToggleWorkbarPowerline,
            "toggle-animations" => Action::ToggleAnimations,
            "toggle-focus-on-hover" => Action::ToggleFocusOnHover,
            "toggle-highlight-focused-background" => Action::ToggleHighlightFocusedBackground,
            "toggle-highlight-focused-border" => Action::ToggleHighlightFocusedBorder,
            "toggle-border-merge" => Action::ToggleBorderMerge,
            "cycle-border-style" => Action::CycleBorderStyle,
            "cycle-title-style" => Action::CycleTitleStyle,
            "cycle-workbar-badge-style" => Action::CycleWorkbarBadgeStyle,
            "cycle-workbar-tab-style" => Action::CycleWorkbarTabStyle,
            "cycle-workbar-style" => Action::CycleWorkbarStyle,
            "toggle-pane-synchronization" => Action::TogglePaneSynchronization,
            "open-config" => Action::OpenConfigFile,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_command_action_id_round_trips() {
        for command in crate::commands::BUILTIN_COMMANDS {
            let id = command
                .action
                .id()
                .expect("every builtin command has a stable id");
            assert_eq!(
                Action::from_id(id),
                Some(command.action),
                "id `{id}` should round-trip"
            );
        }
    }
}
