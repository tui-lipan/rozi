//! Builds and registers hyprmux's `CommandEntry` set with tui-lipan's native command
//! registry/chord dispatch.
//!
//! Three families of commands are registered:
//! - [`BUILTIN_COMMANDS`]: the ~45 stable, individually rebindable actions (`Action::id()`).
//!   Each gets a leader-prefix chord (`<prefix> <key>`) and a WM-modifier chord
//!   (`<modifier>-<key>`) by default; a `[keys]` override replaces both with the user's exact
//!   bindings, except that a bare key step (e.g. `"b"`) re-enters the same prefix/modifier
//!   expansion with that key (resolved at config parse time in `build_key_overrides`).
//! - Workspace digit switch/move/relocate (27 commands, `workspace.<kind>.<1-9>`): not
//!   individually rebindable, generated straight from the configured prefix/modifier.
//! - User `[keys]` `{ run = .. }` / `{ send = .. }` commands (`user.<index>`), one literal
//!   binding each, exactly as configured.
//!
//! [`sync`] (re)registers everything from the current `State`, including a global
//! `enabled` gate ([`commands_active`]) that disables every command while a modal overlay or
//! `Resize`/`Copy` mode has focus, so leader/modifier chords never steal keys from a focused
//! text widget (e.g. `Ctrl+A` for select-all in a rename prompt) or fire mid-resize.

use std::str::FromStr;
use std::sync::Arc;

use tui_lipan::prelude::*;

use crate::config::HyprmuxConfig;
use crate::input::Action;
use crate::state::{
    Direction::{Down, Left, Right, Up},
    Mode, Pane, SCRATCH_PANE_ID, State, cap_style_label,
};
use crate::{HyprmuxApp, Msg};

/// One built-in, individually rebindable command: default key steps (mirrored into a leader
/// chord and a WM-modifier chord), display label/category, and whether it appears in the
/// interactive command palette (vs. help-overlay-only, for frequent single-key actions).
pub(crate) struct BuiltinCommand {
    pub(crate) action: Action,
    label: &'static str,
    category: &'static str,
    default_keys: &'static [&'static str],
    palette: bool,
}

pub(crate) const FORWARD_PREFIX_COMMAND_ID: &str = "hyprmux.forward-prefix";

pub(crate) const BUILTIN_COMMANDS: &[BuiltinCommand] = &[
    BuiltinCommand {
        action: Action::Spawn,
        label: "New pane",
        category: "Panes",
        default_keys: &["enter", "c"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::RespawnPane,
        label: "Respawn exited pane",
        category: "Panes",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::TogglePaneLogging,
        label: "Toggle pane logging",
        category: "Panes",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Close,
        label: "Close pane",
        category: "Panes",
        default_keys: &["w", "shift-w", "x", "shift-x"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleFloat,
        label: "Floating",
        category: "Panes",
        default_keys: &["t", "shift-t"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleFullscreen,
        label: "Fullscreen",
        category: "Panes",
        default_keys: &["f", "shift-f"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::RenamePane,
        label: "Rename pane",
        category: "Panes",
        default_keys: &["n", "shift-n"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Paste,
        label: "Paste from clipboard",
        category: "Panes",
        default_keys: &["v", "shift-v"],
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
        action: Action::TogglePaneSynchronization,
        label: "Pane synchronization",
        category: "Panes",
        default_keys: &[],
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
        category: "Layout",
        default_keys: &["space"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::AdjustRatio(true),
        label: "Grow split",
        category: "Layout",
        default_keys: &["]", "=", "shift-="],
        palette: false,
    },
    BuiltinCommand {
        action: Action::AdjustRatio(false),
        label: "Shrink split",
        category: "Layout",
        default_keys: &["minus", "shift-minus"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::EnterResizeMode,
        label: "Resize mode",
        category: "Layout",
        default_keys: &["r", "shift-r"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleLayout,
        label: "Switch layout",
        category: "Layout",
        default_keys: &["m", "shift-m"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::RenameWorkspace,
        label: "Rename workspace",
        category: "Layout",
        default_keys: &[],
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
        action: Action::SmartFocus(Left),
        label: "Smart focus left (vim-aware)",
        category: "Focus",
        // Unbound by default: opt in with e.g. `[keys] smart-focus-left = "ctrl-h"` to wire
        // seamless vim/neovim split navigation. See docs/keybindings.md.
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Down),
        label: "Smart focus down (vim-aware)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Up),
        label: "Smart focus up (vim-aware)",
        category: "Focus",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::SmartFocus(Right),
        label: "Smart focus right (vim-aware)",
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
        category: "Focus",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleHelp,
        label: "Keybindings",
        category: "App",
        default_keys: &["?", "shift-/"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleDevtools,
        label: "Toggle DevTools",
        category: "App",
        default_keys: &["f12"],
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
        default_keys: &["`", "shift-`"],
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
        action: Action::SaveProfile,
        label: "Capture session as profile…",
        category: "Profile",
        default_keys: &["shift-o"],
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
        action: Action::OpenCollaborators,
        label: "Manage collaborators…",
        category: "Collaboration",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::RenameSession,
        label: "Rename session",
        category: "Session",
        default_keys: &[],
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
    // the question of how they differ. The help overlay still lists the key, which is where a
    // reader is looking for keys rather than for things to do.
    BuiltinCommand {
        action: Action::Detach,
        label: "Detach",
        category: "Session",
        default_keys: &["d", "shift-d"],
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
        category: "Session",
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
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::OpenAppearance,
        label: "Change appearance…",
        category: "Appearance",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleTitles,
        label: "Titlebar",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleTitlebar,
        label: "Titlebar layout",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbar,
        label: "Workbar",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarGap,
        label: "Workbar gap",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarPosition,
        label: "Workbar position",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarPowerline,
        label: "Workbar powerline",
        category: "Appearance",
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
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::SidebarPrevTab,
        label: "Previous sidebar tab",
        category: "Sidebar",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleAnimations,
        label: "Animations",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleFocusOnHover,
        label: "Focus on hover",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedBackground,
        label: "Focused pane background",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedBorder,
        label: "Focused pane border",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedTitlebar,
        label: "Focused pane titlebar",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleBorderMode,
        label: "Border mode",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleBackgroundFollowsTerminal,
        label: "Background follows terminal",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleBorderStyle,
        label: "Border style",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleTitleStyle,
        label: "Titlebar cap style",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarBadgeStyle,
        label: "Workbar badge style",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarTabStyle,
        label: "Workbar tab style",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarStyle,
        label: "Workbar style",
        category: "Appearance",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::TogglePalette,
        label: "Command palette",
        category: "App",
        default_keys: &["p", "shift-p"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::OpenConfigFile,
        label: "Open config file",
        category: "App",
        default_keys: &[],
        palette: true,
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
];

/// Workspace digit shifted symbols on a US layout (`shift-1` ==`!` etc.), used for the
/// move-to-workspace/relocate-workspace second chord step. Kept as literal characters (not
/// `shift-<digit>` binding syntax) because terminals report the shifted symbol itself, not a
/// separate shift modifier bit on the base digit - the same convention the rest of hyprmux's
/// default bindings already rely on (e.g. `shift-7` display as `&`).
const WORKSPACE_DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
const WORKSPACE_SHIFT_SYMBOLS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];

/// Whether app command chords should currently match at all: only in `Mode::Normal` with no
/// modal overlay or scratchpad focused. Disabling every command's shortcuts here (rather than
/// special-casing each handler) means a leading `Ctrl+A` never even becomes a pending chord while,
/// say, a rename prompt's `Input` wants that key for select-all, and `Resize`/`Copy` mode's own
/// plain keys are never shadowed by a chord.
pub(crate) fn commands_active(state: &State) -> bool {
    commands_active_without_scratchpad(state) && !state.scratch_visible
}

fn commands_active_without_scratchpad(state: &State) -> bool {
    state.mode == Mode::Normal
        && !state.show_help
        && !state.show_palette
        && !state.show_appearance
        && !state.show_theme_picker
        && state.search.is_none()
        && state.rename.is_none()
        && state.rename_session.is_none()
        && state.save_profile_prompt.is_none()
        && !state.show_profile_picker
        && !state.show_session_picker
        && state.collaboration.is_none()
        && state.follow_prompt.is_none()
}

/// Same as [`commands_active`], except the help overlay doesn't block: it is a static,
/// read-only list with no focused text input, so it carries none of the `Ctrl+A` collision
/// risk that motivates disabling every other command there. Used for the small set of
/// "get me out of here" exit actions ([`is_exit_command`]) so a user browsing help always has a
/// keyboard escape hatch, matching the pre-migration framework `Ctrl-Q` guarantee (every other
/// overlay legitimately captures raw keys - text input select-all, or its own ctrl-chords like
/// the profile picker's `ctrl+d` - so those still fully gate exit commands too).
fn exit_commands_active(state: &State) -> bool {
    commands_active(state) || (state.mode == Mode::Normal && state.show_help)
}

/// Whether `action` is one of the "get me out of here" exit actions exempted from the help
/// overlay via [`exit_commands_active`].
fn is_exit_command(action: Action) -> bool {
    matches!(
        action,
        Action::Quit | Action::Detach | Action::KillWorkspace | Action::KillSession
    )
}

/// Register (or re-register, replacing by id) every command from the current `State`: builtin
/// actions, workspace digit switches, and user `[keys]` commands. Idempotent - call again after
/// anything that changes shortcuts (config reload), labels (toggle actions, layout cycling), or
/// the `commands_active` gate (mode/overlay transitions).
pub(crate) fn sync(ctx: &Context<HyprmuxApp>) {
    let state = &ctx.state;
    let config = &state.config;
    let active = commands_active(state);
    let exit_active = exit_commands_active(state);
    let scratchpad_toggle_active =
        state.scratch_visible && commands_active_without_scratchpad(state);

    for command in BUILTIN_COMMANDS {
        let id = command
            .action
            .id()
            .expect("BUILTIN_COMMANDS entries always have a stable id");
        let shortcuts = resolve_shortcuts(config, id, command.default_keys);
        let hint = builtin_keybinding_hint(config, id, command.default_keys);
        let label = resolved_label(command.action, command.label, state);
        let action = command.action;
        let enabled = command_available(action, state)
            && if matches!(action, Action::ToggleScratchpad | Action::ToggleSidebar)
                && state.scratch_visible
            {
                scratchpad_toggle_active
            } else if is_exit_command(action) {
                exit_active
            } else {
                active
            };
        let link = ctx.link().clone();
        ctx.register_command(
            CommandEntry::builder(id)
                .label(label)
                .category(command.category)
                .keybinding_hint_opt(hint)
                .shortcuts(shortcuts)
                .enabled(enabled)
                .handler(Callback::new(move |_| link.send(Msg::RunAction(action))))
                .build(),
        );
    }

    register_forward_prefix_command(ctx, config, active);
    register_workspace_commands(ctx, config, active);
    register_user_commands(ctx, config, active);
}

fn register_forward_prefix_command(
    ctx: &Context<HyprmuxApp>,
    config: &HyprmuxConfig,
    active: bool,
) {
    let (shortcuts, prefix_key) = prefix_forward_binding(config)
        .map(|(binding, key)| (KeyBindings::from_bindings([binding]), Some(key)))
        .unwrap_or_else(|| (KeyBindings::from_bindings([]), None));
    let link = ctx.link().clone();
    ctx.register_command(
        CommandEntry::builder(FORWARD_PREFIX_COMMAND_ID)
            .shortcuts(shortcuts)
            .enabled(active)
            .handler(Callback::new(move |_| {
                if let Some(key) = prefix_key {
                    link.send(Msg::ForwardPrefix(key));
                }
            }))
            .build(),
    );
}

fn prefix_forward_binding(config: &HyprmuxConfig) -> Option<(KeyBinding, KeyEvent)> {
    let prefix = config.input.prefix.canonical_lowercase();
    let binding = KeyBinding::from_str(&format!("{prefix} {prefix}")).ok()?;
    let mut events = config.input.prefix.key_events().ok()?;
    (events.len() == 1).then(|| (binding, events.remove(0)))
}

fn register_workspace_commands(ctx: &Context<HyprmuxApp>, config: &HyprmuxConfig, active: bool) {
    for index in 0..9 {
        let digit = WORKSPACE_DIGITS[index];
        let symbol = WORKSPACE_SHIFT_SYMBOLS[index];

        register_workspace_command(
            ctx,
            &format!("workspace.switch.{}", index + 1),
            default_shortcuts_for(config, &[digit]),
            active,
            Action::SwitchWorkspace(index),
        );
        register_workspace_command(
            ctx,
            &format!("workspace.move.{}", index + 1),
            default_shortcuts_for(config, &[symbol.to_string(), format!("shift-{digit}")]),
            active,
            Action::MoveToWorkspace(index),
        );
        register_workspace_command(
            ctx,
            &format!("workspace.relocate.{}", index + 1),
            default_shortcuts_for(
                config,
                &[format!("ctrl-{symbol}"), format!("ctrl-shift-{digit}")],
            ),
            active,
            Action::RelocateWorkspace(index),
        );
    }
}

fn register_workspace_command(
    ctx: &Context<HyprmuxApp>,
    id: &str,
    shortcuts: Vec<KeyBinding>,
    active: bool,
    action: Action,
) {
    let link = ctx.link().clone();
    ctx.register_command(
        CommandEntry::builder(id.to_string())
            .category("Workspaces")
            .shortcuts(KeyBindings::from_bindings(shortcuts))
            .enabled(active)
            .handler(Callback::new(move |_| link.send(Msg::RunAction(action))))
            .build(),
    );
}

fn register_user_commands(ctx: &Context<HyprmuxApp>, config: &HyprmuxConfig, active: bool) {
    for (index, command) in config.user_commands.iter().enumerate() {
        let id = format!("user.{index}");
        let hint = command.binding.canonical_lowercase();
        let link = ctx.link().clone();
        ctx.register_command(
            CommandEntry::builder(id)
                .label(command.label())
                .category("Custom")
                .keybinding_hint(hint)
                .shortcut(command.binding.clone())
                .enabled(active)
                .handler(Callback::new(move |_| {
                    link.send(Msg::RunAction(Action::RunUserCommand(index)))
                }))
                .build(),
        );
    }

    // A config reload can shrink `user_commands`; drop any `user.N` entries left registered
    // from a longer previous list; `register_command` only ever replaces an id, it never
    // removes ids that stop being passed to it.
    let registry = ctx.command_registry();
    let current_len = config.user_commands.len();
    let stale_ids: Vec<_> = registry
        .entries()
        .into_iter()
        .filter_map(|entry| {
            let index: usize = entry.id.as_str().strip_prefix("user.")?.parse().ok()?;
            (index >= current_len).then_some(entry.id)
        })
        .collect();
    for id in stale_ids {
        registry.unregister(id);
    }
}

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
    // `app.toggle-devtools`). None of them belong in hyprmux's palette: quit/detach have
    // dedicated commands, panes are terminal shells rather than app-focusable widgets,
    // dismissing an overlay is just `Esc`, and DevTools is exposed as hyprmux's own
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

pub(crate) fn command_available(action: Action, state: &State) -> bool {
    let shared = state.current().shared.as_ref();
    match action {
        Action::RespawnPane => state.current().focused_pane.is_some_and(|focused| {
            state
                .current()
                .workspaces
                .iter()
                .flat_map(|workspace| &workspace.panes)
                .find(|pane| pane.id == focused)
                .is_some_and(|pane| {
                    matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_))
                })
        }),
        // The roster lists other clients and acts on them, so it is worth opening only when there is
        // somebody in it. Parked clients count: they are still attached and still removable.
        Action::OpenCollaborators => shared.is_some_and(|shared| {
            shared
                .clients
                .iter()
                .any(|client| client.id != shared.client_id)
        }),
        Action::ToggleInputLock => shared.is_some_and(|shared| {
            shared.clients.len() > 1 && !shared.read_only && shared.is_controller()
        }),
        Action::ToggleControlTakeover => shared.is_some_and(|shared| {
            !shared.read_only
                && shared.is_controller()
                && state
                    .current()
                    .session_client
                    .as_ref()
                    .is_some_and(|client| {
                        client.effective_protocol()
                            >= crate::session::protocol::CONTROL_TAKEOVER_PROTOCOL
                    })
        }),
        Action::RequestControl => shared.is_some_and(|shared| {
            shared.clients.len() > 1 && !shared.read_only && !shared.is_controller()
        }),
        Action::GrantControl => shared
            .is_some_and(|shared| shared.is_controller() && shared.has_pending_control_requests()),
        _ => true,
    }
}

/// The display chord for a built-in command's current binding — the configured leader prefix plus
/// the command's first key step (e.g. `ctrl+a e`) — read live from the registry so `[keys]`
/// overrides are honored. `None` when the command is unbound. Prefer this over hardcoding keys in
/// toasts/hints, since every binding is user-configurable.
pub(crate) fn command_prefix_chord(ctx: &Context<HyprmuxApp>, id: &str) -> Option<String> {
    let hint = ctx
        .command_registry()
        .entries()
        .into_iter()
        .find(|entry| entry.id.as_str() == id)?
        .keybinding_hint
        .as_deref()
        .map(str::to_string)
        .filter(|hint| !hint.is_empty())?;
    let prefix = ctx.state.config.input.prefix.to_string();
    Some(format!("{prefix} {hint}"))
}

/// Resolve a builtin command's shortcuts: an explicit `[keys]` override (verbatim, including an
/// explicit empty override that unbinds it) if configured, otherwise the leader-prefix chord and
/// WM-modifier chord mirrored from its default key steps. Paste also accepts plain `Ctrl+V`, which
/// tui-lipan handles for text inputs but intentionally forwards from terminal widgets by default.
fn resolve_shortcuts(config: &HyprmuxConfig, id: &str, defaults: &[&str]) -> KeyBindings {
    if let Some(bindings) = config.key_overrides.get(id) {
        KeyBindings::from_bindings(bindings.iter().cloned())
    } else {
        let mut bindings = default_shortcuts_for(config, defaults);
        if id == "paste" {
            bindings.push(KeyBinding::from_str("ctrl-v").expect("paste shortcut parses"));
        }
        KeyBindings::from_bindings(bindings)
    }
}

fn builtin_keybinding_hint(
    config: &HyprmuxConfig,
    id: &str,
    defaults: &[&str],
) -> Option<Arc<str>> {
    let hint = if config.key_overrides.contains_key(id) {
        resolve_shortcuts(config, id, defaults).canonical_lowercase()
    } else {
        defaults
            .iter()
            .filter_map(|key| KeyBinding::from_str(key).ok())
            .map(|binding| binding.canonical_lowercase())
            .next()
            .unwrap_or_default()
    };
    (!hint.is_empty()).then(|| Arc::<str>::from(hint))
}

/// For each key step, build the leader-prefix chord (`<prefix> <key>`) and, when
/// `[input] modifier_shortcuts` is enabled (the default), the WM-modifier held chord
/// (`<modifier>-<key>`), skipping either that fails to parse (e.g. a key step that already carries
/// `ctrl-`/`shift-` composes fine with a modifier prefix, but a malformed default would simply be
/// dropped rather than panic).
///
/// The modifier mirror is an all-or-nothing layer controlled by `modifier_shortcuts`: with it off,
/// only leader chords are emitted so held `Alt`/`Super` chords reach the focused pane instead. A
/// user who wants to drop the mirror for one specific command uses a `[keys]` override instead.
fn default_shortcuts_for<S: AsRef<str>>(config: &HyprmuxConfig, keys: &[S]) -> Vec<KeyBinding> {
    keys.iter()
        .flat_map(|key| crate::config::scheme_shortcuts(&config.input, key.as_ref()))
        .collect()
}

pub(crate) fn default_shortcuts_for_action(
    input: &crate::config::InputConfig,
    id: &str,
) -> Option<Vec<KeyBinding>> {
    BUILTIN_COMMANDS
        .iter()
        .find(|command| command.action.id() == Some(id))
        .map(|command| {
            command
                .default_keys
                .iter()
                .flat_map(|key| crate::config::scheme_shortcuts(input, key))
                .collect()
        })
}

/// Resolve a command's live display label, reflecting current state for toggle actions (e.g.
/// "Disable floating" vs "Enable floating") and the active layout for `ToggleLayout`.
fn resolved_label(action: Action, base_label: &str, state: &State) -> String {
    if action == Action::EditScrollback {
        return edit_scrollback_label(&crate::ops::config::config_editor());
    }
    if let Some(text) = toggle_command_label(action, state) {
        return text;
    }
    if action == Action::ToggleLayout {
        let layout = state.current().workspaces[state.current().active_workspace]
            .layout_kind
            .label();
        return format!("Switch layout (current: {layout})");
    }
    if action == Action::RenameSession {
        // An ephemeral (or not-yet-attached) session carries no user-facing name, so this command
        // *names* it for the first time (turning it durable) rather than renaming an existing name.
        let named = state.current().session_attached && !state.is_ephemeral_session();
        return if named {
            "Rename session"
        } else {
            "Name session"
        }
        .to_string();
    }
    base_label.to_string()
}

fn edit_scrollback_label(editor: &str) -> String {
    format!("Edit scrollback in {editor}")
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
            let enabled = state.current().workspaces[state.current().active_workspace].synchronized;
            enable_disable_label("pane synchronization", enabled)
        }
        Action::ToggleTitles => enable_disable_label("titlebar", state.config.pane.show_titles),
        Action::ToggleWorkbar => enable_disable_label("workbar", state.config.pane.show_workbar),
        Action::ToggleWorkbarGap => {
            enable_disable_label("workbar gap", state.config.pane.workbar_gap)
        }
        Action::ToggleWorkbarPosition => {
            let edge = if state.config.pane.workbar_at_bottom {
                "top"
            } else {
                "bottom"
            };
            format!("Move workbar to {edge}")
        }
        Action::ToggleWorkbarPowerline => {
            enable_disable_label("workbar powerline", state.config.pane.workbar_powerline)
        }
        Action::ToggleSidebar => enable_disable_label("sidebar", state.sidebar_visible),
        Action::ToggleAnimations => {
            enable_disable_label("animations", state.config.animations.enabled)
        }
        Action::ToggleInputLock => enable_disable_label(
            "input lock",
            state
                .current()
                .shared
                .as_ref()
                .is_some_and(|shared| shared.input_locked),
        ),
        Action::RequestControl => {
            let takeover = state
                .current()
                .shared
                .as_ref()
                .is_some_and(|shared| shared.allow_takeover);
            if takeover {
                "Take layout control".to_string()
            } else {
                "Request layout control".to_string()
            }
        }
        Action::ToggleControlTakeover => enable_disable_label(
            "immediate control takeover",
            state
                .current()
                .shared
                .as_ref()
                .is_some_and(|shared| shared.allow_takeover),
        ),
        Action::ToggleFocusOnHover => {
            enable_disable_label("focus on hover", state.config.pane.focus_on_hover)
        }
        Action::ToggleHighlightFocusedBackground => enable_disable_label(
            "focused pane background",
            state.config.pane.highlight_focused_background,
        ),
        Action::ToggleHighlightFocusedBorder => enable_disable_label(
            "focused pane border",
            state.config.pane.highlight_focused_border,
        ),
        Action::ToggleHighlightFocusedTitlebar => enable_disable_label(
            "focused pane titlebar",
            state.config.pane.highlight_focused_titlebar,
        ),
        Action::CycleBorderMode => {
            format!("Border mode: {}", state.config.pane.border_mode.label())
        }
        Action::ToggleBackgroundFollowsTerminal => enable_disable_label(
            "background follows terminal",
            state.config.pane.background_follows_terminal,
        ),
        Action::CycleBorderStyle => {
            format!("Border style: {}", state.config.pane.border_style.label())
        }
        Action::CycleTitlebar => {
            format!("Titlebar layout: {}", state.config.pane.titlebar.label())
        }
        Action::CycleTitleStyle => {
            format!(
                "Titlebar cap style: {}",
                cap_style_label(state.config.pane.title_style)
            )
        }
        Action::CycleWorkbarBadgeStyle => {
            format!(
                "Workbar badge style: {}",
                cap_style_label(state.config.pane.workbar_badge_style)
            )
        }
        Action::CycleWorkbarTabStyle => {
            format!(
                "Workbar tab style: {}",
                cap_style_label(state.config.pane.workbar_tab_style)
            )
        }
        Action::CycleWorkbarStyle => {
            format!(
                "Workbar style: {}",
                cap_style_label(state.config.pane.workbar_style)
            )
        }
        Action::ToggleScratchpad => enable_disable_label("scratchpad", state.scratch_visible),
        Action::ToggleHelp => return None,
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
    let id = state.current().focused_pane?;
    if id == SCRATCH_PANE_ID {
        return state.scratch.as_ref();
    }
    let workspace = &state.current().workspaces[state.current().active_workspace];
    workspace
        .panes
        .iter()
        .find(|pane| pane.id == id && !pane.closing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::core::event::{KeyCode, KeyEvent, KeyMods};

    fn plain(ch: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(ch),
            mods: KeyMods::NONE,
        }
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(ch),
            mods: KeyMods {
                ctrl: true,
                ..KeyMods::NONE
            },
        }
    }

    fn alt(ch: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(ch),
            mods: KeyMods {
                alt: true,
                ..KeyMods::NONE
            },
        }
    }

    #[test]
    fn default_shortcuts_mirror_prefix_chord_and_modifier() {
        let config = HyprmuxConfig::default();
        let shortcuts = default_shortcuts_for(&config, &["c"]);

        assert!(
            shortcuts
                .iter()
                .any(|binding| binding.matches_sequence(&[ctrl('a'), plain('c')])),
            "expected a ctrl-a c leader chord in {shortcuts:?}"
        );
        assert!(
            shortcuts
                .iter()
                .any(|binding| binding.matches_sequence(&[alt('c')])),
            "expected an alt-c modifier mirror in {shortcuts:?}"
        );
    }

    #[test]
    fn modifier_shortcuts_toggle_controls_the_alt_mirror() {
        let has_alt = |shortcuts: &[KeyBinding], code: KeyCode| {
            shortcuts.iter().any(|binding| {
                binding.matches_sequence(&[KeyEvent {
                    code,
                    mods: KeyMods::ALT,
                }])
            })
        };
        let code_for = |key: &str| match key {
            "tab" => KeyCode::Tab,
            "enter" => KeyCode::Enter,
            other => KeyCode::Char(other.chars().next().unwrap()),
        };

        // Enabled by default: every default key mirrors onto its Alt chord, with no per-key
        // carve-outs - keys that once had exceptions (Tab, detach `d`, paste `v`, spawn `enter`)
        // all mirror now.
        let config = HyprmuxConfig::default();
        assert!(config.input.modifier_shortcuts);
        for key in ["d", "v", "tab", "enter", "c"] {
            let shortcuts = default_shortcuts_for(&config, &[key]);
            assert!(
                has_alt(&shortcuts, code_for(key)),
                "expected an Alt mirror for `{key}` when modifier_shortcuts is on: {shortcuts:?}"
            );
        }

        // Disabled: only leader chords remain, no Alt mirror anywhere.
        let mut prefix_only = HyprmuxConfig::default();
        prefix_only.input.modifier_shortcuts = false;
        let shortcuts = default_shortcuts_for(&prefix_only, &["c"]);
        assert!(
            shortcuts
                .iter()
                .any(|binding| binding.matches_sequence(&[ctrl('a'), plain('c')])),
            "leader chord must remain when modifier_shortcuts is off: {shortcuts:?}"
        );
        assert!(
            !has_alt(&shortcuts, KeyCode::Char('c')),
            "no Alt mirror when modifier_shortcuts is off: {shortcuts:?}"
        );
    }

    #[test]
    fn no_two_builtin_commands_share_a_default_key() {
        use std::collections::HashMap;
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for command in BUILTIN_COMMANDS {
            let id = command
                .action
                .id()
                .expect("every builtin command has a stable id");
            for key in command.default_keys {
                if let Some(previous) = seen.insert(key, id) {
                    panic!("default key `{key}` is bound to both `{previous}` and `{id}`");
                }
            }
        }
    }

    #[test]
    fn key_override_replaces_defaults_verbatim() {
        let mut config = HyprmuxConfig::default();
        config.key_overrides.insert(
            "spawn".to_string(),
            vec![KeyBinding::from_str("ctrl-b c").unwrap()],
        );

        let shortcuts = resolve_shortcuts(&config, "spawn", &["enter", "c"]);
        assert_eq!(shortcuts.len(), 1);
        assert!(
            shortcuts
                .primary()
                .unwrap()
                .matches_sequence(&[ctrl('b'), plain('c')])
        );
    }

    #[test]
    fn paste_accepts_ctrl_v_unless_explicitly_overridden() {
        let mut config = HyprmuxConfig::default();
        let shortcuts = resolve_shortcuts(&config, "paste", &["v", "shift-v"]);
        assert!(
            shortcuts
                .iter()
                .any(|binding| binding.matches_sequence(&[ctrl('v')]))
        );

        config.key_overrides.insert(
            "paste".to_string(),
            vec![KeyBinding::from_str("ctrl-shift-v").unwrap()],
        );
        let shortcuts = resolve_shortcuts(&config, "paste", &["v", "shift-v"]);
        assert!(
            !shortcuts
                .iter()
                .any(|binding| binding.matches_sequence(&[ctrl('v')]))
        );
    }

    #[test]
    fn empty_override_unbinds_a_default() {
        let mut config = HyprmuxConfig::default();
        config
            .key_overrides
            .insert("scratchpad".to_string(), Vec::new());

        let shortcuts = resolve_shortcuts(&config, "scratchpad", &["`", "shift-`"]);
        assert!(shortcuts.is_empty());
    }

    #[test]
    fn workspace_relocate_covers_both_terminal_encodings_of_ctrl_shift_digit() {
        let config = HyprmuxConfig::default();
        let shortcuts = default_shortcuts_for(&config, &["ctrl-#", "ctrl-shift-3"]);

        // Some terminals report Ctrl+Shift+3 as the shifted symbol with only ctrl set...
        assert!(shortcuts.iter().any(|binding| binding.matches_sequence(&[
            ctrl('a'),
            KeyEvent {
                code: KeyCode::Char('#'),
                mods: KeyMods {
                    ctrl: true,
                    ..KeyMods::NONE
                },
            }
        ])));
        // ...and others as the base digit with both ctrl and shift set.
        assert!(shortcuts.iter().any(|binding| binding.matches_sequence(&[
            ctrl('a'),
            KeyEvent {
                code: KeyCode::Char('3'),
                mods: KeyMods {
                    ctrl: true,
                    shift: true,
                    ..KeyMods::NONE
                },
            }
        ])));
    }

    #[test]
    fn commands_active_is_false_during_resize_mode_and_overlays() {
        let config = HyprmuxConfig::default();
        let mut state = State::new(config, tui_lipan::Theme::default());
        assert!(commands_active(&state));

        state.mode = Mode::Resize;
        assert!(!commands_active(&state));
        state.mode = Mode::Normal;

        state.show_palette = true;
        assert!(!commands_active(&state));
        state.show_palette = false;

        state.show_appearance = true;
        assert!(!commands_active(&state));
    }

    #[test]
    fn is_palette_eligible_excludes_workspace_and_frequent_single_key_actions() {
        assert!(!is_palette_eligible("workspace.switch.1"));
        assert!(!is_palette_eligible("spawn"));
        assert!(is_palette_eligible("rename-pane"));
        assert!(is_palette_eligible("save-profile"));
    }

    /// `detach` and `quit` run the same thing. Both keys stay bound and both ids stay valid for
    /// `[keys]` and the control socket, but the palette offers one of them: listing two rows that
    /// do the same thing only asks the reader to work out how they differ.
    #[test]
    fn leaving_the_client_is_one_palette_entry_with_both_keys_bound() {
        assert!(is_palette_eligible("quit"));
        assert!(!is_palette_eligible("detach"));
        let detach = BUILTIN_COMMANDS
            .iter()
            .find(|command| command.action == Action::Detach)
            .expect("detach stays registered so its key and id keep working");
        assert!(detach.default_keys.contains(&"d"));
        assert_eq!(Action::from_id("detach"), Some(Action::Detach));
    }

    #[test]
    fn appearance_settings_are_grouped_behind_change_appearance() {
        assert!(is_palette_eligible("change-appearance"));
        assert!(!is_palette_eligible("choose-theme"));
        assert!(!is_palette_eligible("toggle-titles"));
        assert!(!is_palette_eligible("toggle-workbar"));
        assert!(!is_palette_eligible("toggle-animations"));
        assert!(!is_palette_eligible("toggle-highlight-focused-background"));
        assert!(!is_palette_eligible("toggle-highlight-focused-border"));
        assert!(!is_palette_eligible("cycle-border-mode"));
        assert!(!is_palette_eligible("cycle-border-style"));
    }

    #[test]
    fn utility_commands_are_not_palette_eligible() {
        assert!(!is_palette_eligible("cycle-focus-next"));
        assert!(!is_palette_eligible("cycle-focus-prev"));
        assert!(!is_palette_eligible("command-palette"));
    }

    #[test]
    fn framework_app_commands_are_not_palette_eligible() {
        assert!(!is_palette_eligible("app.quit"));
        assert!(!is_palette_eligible("app.focus-next"));
        assert!(!is_palette_eligible("app.focus-prev"));
        assert!(!is_palette_eligible("app.dismiss-overlay"));
        assert!(!is_palette_eligible("app.toggle-devtools"));
    }

    #[test]
    fn default_command_hints_show_command_keys_only() {
        let config = HyprmuxConfig::default();

        assert_eq!(
            builtin_keybinding_hint(&config, "close", &["w", "shift-w", "x", "shift-x"]),
            Some(Arc::<str>::from("w"))
        );
        assert_eq!(
            builtin_keybinding_hint(&config, "cycle-focus-next", &["tab"]),
            Some(Arc::<str>::from("tab"))
        );
    }

    #[test]
    fn override_command_hints_stay_verbatim() {
        let mut config = HyprmuxConfig::default();
        config.key_overrides.insert(
            "close".to_string(),
            vec![KeyBinding::from_str("ctrl-b k").unwrap()],
        );

        assert_eq!(
            builtin_keybinding_hint(&config, "close", &["w", "shift-w"]),
            Some(Arc::<str>::from("ctrl+b k"))
        );
    }

    fn shared_state(client_id: u64, controller: u64, read_only: bool, count: u64) -> State {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        let mut shared = crate::state::SharedSessionState::new(client_id);
        shared.controller = Some(controller);
        shared.read_only = read_only;
        shared.clients = (1..=count)
            .map(|id| crate::session::protocol::ClientInfo {
                id,
                label: format!("client-{id}"),
                read_only: read_only && id == client_id,
                requesting_control: false,
                parked: false,
            })
            .collect();
        state.current_mut().shared = Some(shared);
        state
    }

    /// Every collaboration command is an ordinary palette row under one category. Only the roster
    /// is a dialog, and it is a dialog because it acts on a live list of people — not a menu of the
    /// commands beside it.
    #[test]
    fn collaboration_commands_are_flat_palette_rows() {
        for id in [
            "collaborators",
            "request-control",
            "grant-control",
            "toggle-input-lock",
            "toggle-control-takeover",
        ] {
            assert!(is_palette_eligible(id), "{id} belongs in the palette");
            assert!(
                Action::from_id(id).is_some(),
                "{id} must stay valid for `[keys]` and run-action"
            );
        }
    }

    #[test]
    fn collaboration_commands_follow_roster_and_permissions() {
        let solo = shared_state(1, 1, false, 1);
        assert!(!command_available(Action::OpenCollaborators, &solo));
        assert!(!command_available(Action::RequestControl, &solo));
        assert!(!command_available(Action::ToggleInputLock, &solo));

        let controller = shared_state(1, 1, false, 2);
        assert!(command_available(Action::OpenCollaborators, &controller));
        assert!(!command_available(Action::RequestControl, &controller));
        assert!(command_available(Action::ToggleInputLock, &controller));
        // Grant is offered only once a follower is actually requesting.
        assert!(!command_available(Action::GrantControl, &controller));

        let follower = shared_state(2, 1, false, 2);
        assert!(command_available(Action::OpenCollaborators, &follower));
        assert!(command_available(Action::RequestControl, &follower));
        assert!(!command_available(Action::ToggleInputLock, &follower));
        assert!(!command_available(Action::GrantControl, &follower));

        // Controller with a pending request from client 2 can grant; the follower still cannot.
        let mut controller_with_request = shared_state(1, 1, false, 2);
        controller_with_request
            .current_mut()
            .shared
            .as_mut()
            .unwrap()
            .clients[1]
            .requesting_control = true;
        assert!(command_available(
            Action::GrantControl,
            &controller_with_request
        ));

        let viewer = shared_state(2, 1, true, 2);
        assert!(command_available(Action::OpenCollaborators, &viewer));
        assert!(!command_available(Action::RequestControl, &viewer));
        assert!(!command_available(Action::ToggleInputLock, &viewer));
    }

    #[test]
    fn respawn_is_available_only_for_the_focused_exited_pane() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        assert!(!command_available(Action::RespawnPane, &state));

        let focused = state.current().focused_pane.unwrap();
        state.current_mut().workspaces[0]
            .panes
            .iter_mut()
            .find(|pane| pane.id == focused)
            .unwrap()
            .terminal
            .status = ManagedTerminalStatus::Exited(1);
        assert!(command_available(Action::RespawnPane, &state));

        state.current_mut().focused_pane = None;
        assert!(!command_available(Action::RespawnPane, &state));
    }

    #[test]
    fn input_lock_label_reflects_server_state() {
        let mut state = shared_state(1, 1, false, 2);
        assert_eq!(
            resolved_label(Action::ToggleInputLock, "Toggle input lock", &state),
            "Enable input lock"
        );
        state.current_mut().shared.as_mut().unwrap().input_locked = true;
        assert_eq!(
            resolved_label(Action::ToggleInputLock, "Toggle input lock", &state),
            "Disable input lock"
        );
    }
}
