use crate::state::Direction;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Spawn,
    RespawnPane,
    TogglePaneLogging,
    Close,
    Focus(Direction),
    /// Directional focus that stays on the current pane when no spatial candidate exists.
    FocusNoWrap(Direction),
    /// Move focus in `Direction`, unless the focused pane runs a split-aware program (see
    /// `[navigation] editors`), in which case forward the matching `Ctrl-h/j/k/l` to it. Lets a
    /// single `Ctrl-h/j/k/l` binding navigate hyprmux panes and vim/neovim splits seamlessly
    /// (vim-tmux-navigator style).
    SmartFocus(Direction),
    FocusNextBlockedPane,
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
    EnterHintMode,
    ToggleScratchpad,
    OpenSearch,
    SaveProfile,
    OpenProfilePicker,
    ApplyProfile,
    OpenSessionPicker,
    OpenClientList,
    RenameSession,
    NewTemporarySession,
    RequestControl,
    GrantControl,
    ToggleInputLock,
    Detach,
    Quit,
    KillWorkspace,
    KillSession,
    OpenThemePicker,
    OpenAppearance,
    TogglePalette,
    ToggleHelp,
    /// Toggle tui-lipan's built-in DevTools overlay (`ctx.toggle_devtools`).
    ToggleDevtools,
    ToggleTitles,
    ToggleWorkbar,
    ToggleWorkbarGap,
    ToggleWorkbarPosition,
    ToggleWorkbarPowerline,
    ToggleSidebar,
    SidebarNextTab,
    SidebarPrevTab,
    ToggleAnimations,
    ToggleFocusOnHover,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleBorderMerge,
    ToggleBackgroundFollowsTerminal,
    CycleBorderStyle,
    CycleTitleStyle,
    CycleWorkbarBadgeStyle,
    CycleWorkbarTabStyle,
    CycleWorkbarStyle,
    TogglePaneSynchronization,
    OpenConfigFile,
    EditScrollback,
    CopyLastOutput,
    /// Runs `config.user_commands[index]`. Defined only by `[keys]` table entries (see
    /// `crate::config::build_key_overrides`), so - like workspace digits - it has no static id
    /// and isn't independently rebindable or listed in `crate::commands::BUILTIN_COMMANDS`.
    RunUserCommand(usize),
}

const BINDABLE_ACTIONS: &[Action] = &[
    Action::Spawn,
    Action::RespawnPane,
    Action::TogglePaneLogging,
    Action::Close,
    Action::Focus(Direction::Left),
    Action::Focus(Direction::Down),
    Action::Focus(Direction::Up),
    Action::Focus(Direction::Right),
    Action::FocusNoWrap(Direction::Left),
    Action::FocusNoWrap(Direction::Down),
    Action::FocusNoWrap(Direction::Up),
    Action::FocusNoWrap(Direction::Right),
    Action::SmartFocus(Direction::Left),
    Action::SmartFocus(Direction::Down),
    Action::SmartFocus(Direction::Up),
    Action::SmartFocus(Direction::Right),
    Action::FocusNextBlockedPane,
    Action::Move(Direction::Left),
    Action::Move(Direction::Down),
    Action::Move(Direction::Up),
    Action::Move(Direction::Right),
    Action::Swap(Direction::Left),
    Action::Swap(Direction::Down),
    Action::Swap(Direction::Up),
    Action::Swap(Direction::Right),
    Action::CycleFocus(true),
    Action::CycleFocus(false),
    Action::PromoteToMaster,
    Action::ToggleFloat,
    Action::ToggleFullscreen,
    Action::RenamePane,
    Action::RenameWorkspace,
    Action::Paste,
    Action::FlipSplit,
    Action::AdjustRatio(crate::state::RATIO_STEP),
    Action::AdjustRatio(-crate::state::RATIO_STEP),
    Action::EnterResizeMode,
    Action::ToggleLayout,
    Action::EnterCopyMode,
    Action::EnterHintMode,
    Action::ToggleScratchpad,
    Action::OpenSearch,
    Action::SaveProfile,
    Action::OpenProfilePicker,
    Action::ApplyProfile,
    Action::OpenSessionPicker,
    Action::OpenClientList,
    Action::RenameSession,
    Action::NewTemporarySession,
    Action::RequestControl,
    Action::GrantControl,
    Action::ToggleInputLock,
    Action::Detach,
    Action::Quit,
    Action::KillWorkspace,
    Action::KillSession,
    Action::OpenThemePicker,
    Action::OpenAppearance,
    Action::TogglePalette,
    Action::ToggleHelp,
    Action::ToggleDevtools,
    Action::ToggleTitles,
    Action::ToggleWorkbar,
    Action::ToggleWorkbarGap,
    Action::ToggleWorkbarPosition,
    Action::ToggleWorkbarPowerline,
    Action::ToggleSidebar,
    Action::SidebarNextTab,
    Action::SidebarPrevTab,
    Action::ToggleAnimations,
    Action::ToggleFocusOnHover,
    Action::ToggleHighlightFocusedBackground,
    Action::ToggleHighlightFocusedBorder,
    Action::ToggleBorderMerge,
    Action::ToggleBackgroundFollowsTerminal,
    Action::CycleBorderStyle,
    Action::CycleTitleStyle,
    Action::CycleWorkbarBadgeStyle,
    Action::CycleWorkbarTabStyle,
    Action::CycleWorkbarStyle,
    Action::TogglePaneSynchronization,
    Action::OpenConfigFile,
    Action::EditScrollback,
    Action::CopyLastOutput,
];

impl Action {
    /// Stable kebab-case id for config binding (`[keys]`) and command registration. Returns
    /// `None` for actions that are not individually rebindable (workspace switch/move are
    /// range-generated; theme selection is opened through the picker).
    pub fn id(self) -> Option<&'static str> {
        use Direction::{Down, Left, Right, Up};
        Some(match self {
            Action::Spawn => "spawn",
            Action::RespawnPane => "respawn-pane",
            Action::TogglePaneLogging => "toggle-pane-logging",
            Action::Close => "close",
            Action::Focus(Left) => "focus-left",
            Action::Focus(Down) => "focus-down",
            Action::Focus(Up) => "focus-up",
            Action::Focus(Right) => "focus-right",
            Action::FocusNoWrap(Left) => "focus-left-no-wrap",
            Action::FocusNoWrap(Down) => "focus-down-no-wrap",
            Action::FocusNoWrap(Up) => "focus-up-no-wrap",
            Action::FocusNoWrap(Right) => "focus-right-no-wrap",
            Action::SmartFocus(Left) => "smart-focus-left",
            Action::SmartFocus(Down) => "smart-focus-down",
            Action::SmartFocus(Up) => "smart-focus-up",
            Action::SmartFocus(Right) => "smart-focus-right",
            Action::FocusNextBlockedPane => "focus-next-blocked-pane",
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
            Action::EnterHintMode => "hint-mode",
            Action::ToggleScratchpad => "scratchpad",
            Action::OpenSearch => "search",
            Action::SaveProfile => "save-profile",
            Action::OpenProfilePicker => "open-profile",
            Action::ApplyProfile => "apply-profile",
            Action::OpenSessionPicker => "sessions",
            Action::OpenClientList => "session-clients",
            Action::RenameSession => "rename-session",
            Action::NewTemporarySession => "new-temporary-session",
            Action::RequestControl => "request-control",
            Action::GrantControl => "grant-control",
            Action::ToggleInputLock => "toggle-input-lock",
            Action::Detach => "detach",
            Action::Quit => "quit",
            Action::KillWorkspace => "kill-workspace",
            Action::KillSession => "kill-session",
            Action::OpenThemePicker => "choose-theme",
            Action::OpenAppearance => "change-appearance",
            Action::TogglePalette => "command-palette",
            Action::ToggleHelp => "help",
            Action::ToggleDevtools => "toggle-devtools",
            Action::ToggleTitles => "toggle-titles",
            Action::ToggleWorkbar => "toggle-workbar",
            Action::ToggleWorkbarGap => "toggle-workbar-gap",
            Action::ToggleWorkbarPosition => "toggle-workbar-position",
            Action::ToggleWorkbarPowerline => "toggle-workbar-powerline",
            Action::ToggleSidebar => "toggle-sidebar",
            Action::SidebarNextTab => "sidebar-next-tab",
            Action::SidebarPrevTab => "sidebar-prev-tab",
            Action::ToggleAnimations => "toggle-animations",
            Action::ToggleFocusOnHover => "toggle-focus-on-hover",
            Action::ToggleHighlightFocusedBackground => "toggle-highlight-focused-background",
            Action::ToggleHighlightFocusedBorder => "toggle-highlight-focused-border",
            Action::ToggleBorderMerge => "toggle-border-merge",
            Action::ToggleBackgroundFollowsTerminal => "toggle-background-follows-terminal",
            Action::CycleBorderStyle => "cycle-border-style",
            Action::CycleTitleStyle => "cycle-title-style",
            Action::CycleWorkbarBadgeStyle => "cycle-workbar-badge-style",
            Action::CycleWorkbarTabStyle => "cycle-workbar-tab-style",
            Action::CycleWorkbarStyle => "cycle-workbar-style",
            Action::TogglePaneSynchronization => "toggle-pane-synchronization",
            Action::OpenConfigFile => "open-config",
            Action::EditScrollback => "edit-scrollback",
            Action::CopyLastOutput => "copy-last-output",
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
        BINDABLE_ACTIONS
            .iter()
            .copied()
            .find(|action| action.id() == Some(id))
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

    #[test]
    fn non_wrapping_focus_action_ids_round_trip() {
        for action in [
            Action::FocusNoWrap(Direction::Left),
            Action::FocusNoWrap(Direction::Down),
            Action::FocusNoWrap(Direction::Up),
            Action::FocusNoWrap(Direction::Right),
        ] {
            let id = action.id().expect("non-wrapping focus has a stable id");
            assert_eq!(Action::from_id(id), Some(action));
        }
    }
}
