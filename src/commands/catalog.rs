//! Static built-in command catalog: ids, default keys, labels, and palette eligibility.

use crate::input::Action;
use crate::state::Direction::{Down, Left, Right, Up};

pub(crate) struct BuiltinCommand {
    pub(crate) action: Action,
    pub(super) label: &'static str,
    pub(super) category: &'static str,
    pub(super) default_keys: &'static [&'static str],
    pub(super) palette: bool,
}

pub(crate) const FORWARD_PREFIX_COMMAND_ID: &str = "rozi.forward-prefix";
pub(super) const PASTE_DIRECT_SHORTCUT: &str = "ctrl-v";

/// Core actions an extension may target with `[[suggested_keybindings]]`. Keeping this as a small,
/// explicit allowlist prevents the action registry from becoming an accidental extension API.
const EXTENSION_BINDABLE_ACTION_IDS: &[&str] = &[
    "smart-focus-left",
    "smart-focus-down",
    "smart-focus-up",
    "smart-focus-right",
];

pub(crate) fn extension_bindable_action_ids() -> &'static [&'static str] {
    EXTENSION_BINDABLE_ACTION_IDS
}

pub(crate) fn is_extension_bindable_action(id: &str) -> bool {
    EXTENSION_BINDABLE_ACTION_IDS.contains(&id)
}

pub(crate) const BUILTIN_COMMANDS: &[BuiltinCommand] = &[
    BuiltinCommand {
        action: Action::Spawn,
        label: "New pane",
        category: "Panes",
        default_keys: &["enter"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::SpawnFloat,
        label: "New floating pane",
        category: "Panes",
        default_keys: &["shift-enter"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Close,
        label: "Close pane",
        category: "Panes",
        default_keys: &["w"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleFloat,
        label: "Floating",
        category: "Panes",
        default_keys: &["t"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleFullscreen,
        label: "Fullscreen",
        category: "Panes",
        default_keys: &["f"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::RenamePane,
        label: "Rename pane",
        category: "Panes",
        // Shifted sibling of the workspace rename: a pane already derives a title from its process
        // and OSC updates, so overriding it is the rarer half of the naming pair.
        default_keys: &["shift-n"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::TogglePaneSynchronization,
        label: "Pane synchronization",
        category: "Panes",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::TogglePaneLogging,
        label: "Pane logging",
        category: "Panes",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::RespawnPane,
        label: "Respawn exited pane",
        category: "Panes",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Paste,
        label: "Paste from clipboard",
        category: "Panes",
        default_keys: &["v"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::PromoteToMaster,
        label: "Promote to master",
        category: "Panes",
        default_keys: &["."],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Swap(Left),
        label: "Swap pane left",
        category: "Panes",
        default_keys: &["shift-h", "shift-left"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Swap(Down),
        label: "Swap pane down",
        category: "Panes",
        default_keys: &["shift-j", "shift-down"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Swap(Up),
        label: "Swap pane up",
        category: "Panes",
        default_keys: &["shift-k", "shift-up"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Swap(Right),
        label: "Swap pane right",
        category: "Panes",
        default_keys: &["shift-l", "shift-right"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Left),
        label: "Move pane left",
        category: "Panes",
        default_keys: &["ctrl-h", "ctrl-left"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Down),
        label: "Move pane down",
        category: "Panes",
        default_keys: &["ctrl-j", "ctrl-down"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Up),
        label: "Move pane up",
        category: "Panes",
        default_keys: &["ctrl-k", "ctrl-up"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Right),
        label: "Move pane right",
        category: "Panes",
        default_keys: &["ctrl-l", "ctrl-right"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FlipSplit,
        label: "Flip split axis",
        category: "Workspace",
        default_keys: &["space"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::AdjustRatio(true),
        label: "Grow split",
        category: "Workspace",
        default_keys: &["="],
        palette: false,
    },
    BuiltinCommand {
        action: Action::AdjustRatio(false),
        label: "Shrink split",
        category: "Workspace",
        default_keys: &["-"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::RenameWorkspace,
        label: "Rename workspace",
        category: "Workspace",
        // A workspace tab has no derived label at all - only its index - so naming it is the only
        // way to make the always-visible tab strip carry meaning.
        default_keys: &["n"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::EnterResizeMode,
        label: "Resize mode",
        category: "Workspace",
        default_keys: &["r"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleLayout,
        label: "Switch layout",
        category: "Workspace",
        default_keys: &["m"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::OpenLayoutPicker,
        label: "Layouts…",
        category: "Workspace",
        // The shifted sibling of the `m` cycle: same key, one step up from blind cycling to
        // picking a layout outright.
        default_keys: &["shift-m"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Focus(Left),
        label: "Focus left",
        category: "Focus",
        default_keys: &["h", "left"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Focus(Down),
        label: "Focus down",
        category: "Focus",
        default_keys: &["j", "down"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Focus(Up),
        label: "Focus up",
        category: "Focus",
        default_keys: &["k", "up"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Focus(Right),
        label: "Focus right",
        category: "Focus",
        default_keys: &["l", "right"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FocusNoWrap(Left),
        label: "Focus left (no wrap)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FocusNoWrap(Down),
        label: "Focus down (no wrap)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FocusNoWrap(Up),
        label: "Focus up (no wrap)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FocusNoWrap(Right),
        label: "Focus right (no wrap)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Left),
        label: "Smart focus left (split-aware)",
        category: "Focus",
        // Unbound by default: opt in with e.g. `[keys] smart-focus-left = "ctrl-h"` to wire
        // seamless editor split navigation. See docs/keybindings.md.
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Down),
        label: "Smart focus down (split-aware)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Up),
        label: "Smart focus up (split-aware)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Right),
        label: "Smart focus right (split-aware)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleFocus(true),
        label: "Cycle focus next",
        category: "Focus",
        default_keys: &["tab"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleFocus(false),
        label: "Cycle focus previous",
        category: "Focus",
        default_keys: &["shift-tab"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FocusNextBlockedPane,
        label: "Focus next blocked pane",
        category: "Panes",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenSettings,
        label: "Settings…",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenExtensions,
        label: "Extensions…",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenConfigFile,
        label: "Open config file",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::UpdateRozi,
        label: "Update rozi",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ReloadExtensions,
        label: "Reload extensions",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleHelp,
        label: "Keybindings…",
        category: "App",
        default_keys: &["?", "shift-/"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::EnterCopyMode,
        label: "Copy mode",
        category: "App",
        default_keys: &["["],
        palette: true,
    },
    BuiltinCommand {
        action: Action::EnterHintMode,
        label: "Hint mode",
        category: "App",
        default_keys: &["u"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleScratchpad,
        label: "Scratchpad",
        category: "App",
        default_keys: &["`"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenSearch,
        label: "Search scrollback",
        category: "App",
        default_keys: &["/"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ScreenshotPane,
        label: "Screenshot pane",
        category: "Capture",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ScreenshotUi,
        label: "Screenshot UI",
        category: "Capture",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenProfilePicker,
        label: "Profiles…",
        category: "Profile",
        default_keys: &["o"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenWorktrees,
        label: "Worktrees…",
        category: "Git",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::SaveProfile,
        label: "Save session as profile…",
        category: "Profile",
        default_keys: &["shift-o"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ApplyProfile,
        label: "Replace session with profile…",
        category: "Profile",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenSessionPicker,
        label: "Sessions…",
        category: "Session",
        default_keys: &["s"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenAgentPicker,
        label: "Agents…",
        category: "Session",
        default_keys: &["a"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenCollaborators,
        label: "Collaborators…",
        category: "Collaboration",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::RenameSession,
        label: "Rename session",
        category: "Session",
        // Shifted sibling of the session picker: same key, "act on the one I am in".
        default_keys: &["shift-s"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::NewTemporarySession,
        label: "New temporary session",
        category: "Session",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::RequestControl,
        label: "Request layout control",
        category: "Collaboration",
        default_keys: &["g"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::GrantControl,
        label: "Grant layout control to requester",
        category: "Collaboration",
        default_keys: &["e"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleInputLock,
        label: "Toggle input lock",
        category: "Collaboration",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleControlTakeover,
        label: "Toggle immediate control takeover",
        category: "Collaboration",
        default_keys: &[],
        palette: true,
    },
    // `detach` runs the same thing `quit` does — one way out of the client. It keeps its own entry
    // so `prefix d` (the key every tmux user reaches for), `[keys] detach`, and `run-action detach`
    // all keep working, but stays out of the palette: two rows that do the same thing only invite
    // the question of how they differ. The help overlay folds its live binding into the canonical
    // Quit client row.
    BuiltinCommand {
        action: Action::Detach,
        label: "Detach",
        category: "Session",
        default_keys: &["d"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Quit,
        label: "Quit client",
        category: "Session",
        // `q` yields the leader chord `<prefix> q` and the WM-modifier chord `<mod>-q` (Alt+q).
        default_keys: &["q"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::KillWorkspace,
        label: "Kill workspace",
        category: "Workspace",
        // No default key: rarely used and destructive, so it ships unbound and is reached via the
        // command palette or a user `[keys]` binding.
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::KillSession,
        label: "Kill session",
        category: "Session",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::RestartSession,
        label: "Restart session",
        category: "Session",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenThemePicker,
        label: "Change theme",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::OpenAppearance,
        label: "Change appearance…",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::OpenAlerts,
        label: "Alerts…",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleDoNotDisturb,
        label: "Do not disturb",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleTitles,
        label: "Titlebar",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleTitlebar,
        label: "Titlebar layout",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbar,
        label: "Workbar",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarGap,
        label: "Workbar gap",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarBackground,
        label: "Workbar background",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarPosition,
        label: "Workbar position",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarPowerline,
        label: "Workbar powerline",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleSidebar,
        label: "Sidebar",
        category: "Sidebar",
        // `b` for the panel itself, matching the near-universal editor chord for a side panel.
        default_keys: &["b"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleSidebarSplit,
        label: "Sidebar split",
        category: "Sidebar",
        // A backslash depicts the sidebar's vertical split without competing with the session
        // command's `s` mnemonic. The focused-sidebar `s` binding remains a quick local alias.
        default_keys: &["\\"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleSidebarPosition,
        label: "Sidebar position",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleSidebarGap,
        label: "Sidebar gap",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleSidebarBackground,
        label: "Sidebar background",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleSidebarBackgroundFollowsCanvas,
        label: "Sidebar background follows canvas",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleSidebarTabStyle,
        label: "Sidebar tab style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::FocusSidebar,
        label: "Focus sidebar",
        category: "Sidebar",
        // Shifted sibling of the toggle: same key, "and put me in it".
        default_keys: &["shift-b"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::SidebarNextTab,
        label: "Next sidebar tab",
        category: "Sidebar",
        default_keys: &["page-down"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::SidebarPrevTab,
        label: "Previous sidebar tab",
        category: "Sidebar",
        default_keys: &["page-up"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleAnimations,
        label: "Animations",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleNerdIcons,
        label: "Nerd icons",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleFocusOnHover,
        label: "Focus on hover",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedBackground,
        label: "Focused pane background",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedBorder,
        label: "Focused pane border",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedTitlebar,
        label: "Focused pane titlebar",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleBorderMode,
        label: "Border mode",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleAlertBorder,
        label: "Cycle alert border",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarAlert,
        label: "Cycle workspace tab alert",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarAlertPaint,
        label: "Cycle workspace tab alert paint",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleBackgroundFollowsTerminal,
        label: "Background follows terminal",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleBorderStyle,
        label: "Border style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleFloatBorderStyle,
        label: "Floating border style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleScratchBorderStyle,
        label: "Scratchpad border style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleFullscreenBorderStyle,
        label: "Fullscreen border style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CyclePickerBorderStyle,
        label: "Picker border style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::TogglePickerTabBackground,
        label: "Picker tab strip",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CyclePickerTabStyle,
        label: "Picker tab style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CyclePickerSelectionStyle,
        label: "Picker selection style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleTitleStyle,
        label: "Titlebar style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarBadgeStyle,
        label: "Workbar badge style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarTabStyle,
        label: "Workbar tab style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarStyle,
        label: "Workbar style",
        category: "Settings",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::TogglePalette,
        label: "Command palette",
        category: "App",
        default_keys: &["p"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::EditScrollback,
        label: "Edit scrollback",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::CopyLastOutput,
        label: "Copy last command output",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleDevtools,
        label: "Toggle DevTools",
        category: "App",
        default_keys: &["f12"],
        palette: true,
    },
];

/// Workspace digit shifted symbols on a US layout (`shift-1` ==`!` etc.), used for the
/// move-to-workspace/relocate-workspace second chord step. Kept as literal characters (not
/// `shift-<digit>` binding syntax) because terminals report the shifted symbol itself, not a
/// separate shift modifier bit on the base digit - the same convention the rest of rozi's
/// default bindings already rely on (e.g. `shift-7` display as `&`).
pub(super) const WORKSPACE_DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
pub(super) const WORKSPACE_SHIFT_SYMBOLS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];

/// A command id that is registered purely for chord dispatch and never shown in the
/// interactive command palette (workspace digits get a generic "1-9" row in the help overlay
/// instead of 27 individual entries; see `view::overlays::help_overlay`).
pub(crate) fn is_palette_eligible(id: &str) -> bool {
    if id == FORWARD_PREFIX_COMMAND_ID {
        return false;
    }
    if id.starts_with("workspace.") {
        return false;
    }
    // tui-lipan's runtime auto-registers framework commands under the `app.` id prefix
    // (`app.quit`, `app.focus-next`, `app.focus-prev`, `app.dismiss-overlay`,
    // `app.toggle-devtools`). None of them belong in rozi's palette: quit/detach have
    // dedicated commands, panes are terminal shells rather than app-focusable widgets,
    // dismissing an overlay is just `Esc`, and DevTools is exposed as rozi's own
    // `toggle-devtools` (prefix/mod+F12) instead of the framework's bare F12 binding.
    if id.starts_with("app.") {
        return false;
    }
    BUILTIN_COMMANDS
        .iter()
        .find(|command| command.action.id() == Some(id))
        .map(|command| command.palette)
        .unwrap_or(true)
}

/// The static label for a built-in command, before the registry folds in live state.
///
/// The registry deliberately carries the resolved form ("Enable floating", "Switch layout
/// (current: dwindle)") because a palette row is read one at a time and benefits from saying what
/// pressing it will do. A dense grid is scanned rather than read, so the which-key strip wants the
/// terse noun this returns instead. `None` for anything outside [`BUILTIN_COMMANDS`], including
/// user `[keys]` commands, which have no second spelling.
pub(crate) fn builtin_label(id: &str) -> Option<&'static str> {
    BUILTIN_COMMANDS
        .iter()
        .find(|command| command.action.id() == Some(id))
        .map(|command| command.label)
}

/// The leader key reserved for extension chords.
///
/// Extensions suggest steps *inside* this space (`key = "b"` becomes `<prefix> x b`), so a
/// suggestion can never collide with a built-in and a built-in can never quietly take a suggestion
/// away in a later release. Rozi does not assign this key to anything; a test enforces that.
pub(crate) const EXTENSION_KEY_LEADER: &str = "x";
