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
    Mode, Pane, RATIO_STEP, SCRATCH_PANE_ID, State,
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
        palette: false,
    },
    BuiltinCommand {
        action: Action::Paste,
        label: "Paste from clipboard",
        category: "Panes",
        default_keys: &["v", "shift-v"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Swap(Left),
        label: "Swap pane left",
        category: "Panes",
        default_keys: &["ctrl-h", "ctrl-left"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Swap(Down),
        label: "Swap pane down",
        category: "Panes",
        default_keys: &["ctrl-j", "ctrl-down"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Swap(Up),
        label: "Swap pane up",
        category: "Panes",
        default_keys: &["ctrl-k", "ctrl-up"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Swap(Right),
        label: "Swap pane right",
        category: "Panes",
        default_keys: &["ctrl-l", "ctrl-right"],
        palette: false,
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
        action: Action::Move(Left),
        label: "Move pane left",
        category: "Panes",
        default_keys: &["shift-h", "shift-left"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Down),
        label: "Move pane down",
        category: "Panes",
        default_keys: &["shift-j", "shift-down"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Up),
        label: "Move pane up",
        category: "Panes",
        default_keys: &["shift-k", "shift-up"],
        palette: false,
    },
    BuiltinCommand {
        action: Action::Move(Right),
        label: "Move pane right",
        category: "Panes",
        default_keys: &["shift-l", "shift-right"],
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
        action: Action::AdjustRatio(RATIO_STEP),
        label: "Grow split",
        category: "Layout",
        default_keys: &["]", "=", "shift-="],
        palette: false,
    },
    BuiltinCommand {
        action: Action::AdjustRatio(-RATIO_STEP),
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
        action: Action::ToggleHelp,
        label: "Keybindings",
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
        category: "Copy",
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
        label: "Save profile",
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
        action: Action::OpenSessionPicker,
        label: "Sessions…",
        category: "Session",
        default_keys: &["s"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::OpenClientList,
        label: "Session clients…",
        category: "Session",
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
        category: "Session",
        default_keys: &["g"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::GrantControl,
        label: "Grant layout control to requester",
        category: "Session",
        default_keys: &["e"],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleInputLock,
        label: "Toggle input lock",
        category: "Session",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::Detach,
        label: "Detach",
        category: "Session",
        default_keys: &["d", "shift-d"],
        palette: true,
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
        action: Action::OpenThemePicker,
        label: "Change theme",
        category: "Theme",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::OpenAppearance,
        label: "Change appearance…",
        category: "App",
        default_keys: &[],
        palette: true,
    },
    BuiltinCommand {
        action: Action::ToggleTitles,
        label: "Titlebar",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbar,
        label: "Workbar",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarGap,
        label: "Workbar gap",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarPosition,
        label: "Workbar position",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleWorkbarPowerline,
        label: "Workbar powerline",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleAnimations,
        label: "Animations",
        category: "App",
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
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleHighlightFocusedBorder,
        label: "Focused pane border",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleBorderMerge,
        label: "Merge pane borders",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::ToggleBackgroundFollowsTerminal,
        label: "Background follows terminal",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleBorderStyle,
        label: "Border style",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleTitleStyle,
        label: "Titlebar style",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarBadgeStyle,
        label: "Workbar badge style",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarTabStyle,
        label: "Workbar tab style",
        category: "App",
        default_keys: &[],
        palette: false,
    },
    BuiltinCommand {
        action: Action::CycleWorkbarStyle,
        label: "Workbar style",
        category: "App",
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
];

/// Workspace digit shifted symbols on a US layout (`shift-1` ==`!` etc.), used for the
/// move-to-workspace/relocate-workspace second chord step. Kept as literal characters (not
/// `shift-<digit>` binding syntax) because terminals report the shifted symbol itself, not a
/// separate shift modifier bit on the base digit - the same convention the rest of hyprmux's
/// default bindings already rely on (e.g. `shift-7` display as `&`).
const WORKSPACE_DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
const WORKSPACE_SHIFT_SYMBOLS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];

/// Whether app command chords should currently match at all: only in `Mode::Normal` with no
/// modal overlay focused. Disabling every command's shortcuts here (rather than special-casing
/// each handler) means a leading `Ctrl+A` never even becomes a pending chord while, say, a
/// rename prompt's `Input` wants that key for select-all, and `Resize`/`Copy` mode's own plain
/// keys are never shadowed by a chord.
pub(crate) fn commands_active(state: &State) -> bool {
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
        && state.client_list.is_none()
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
            && if is_exit_command(action) {
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

    register_workspace_commands(ctx, config, active);
    register_user_commands(ctx, config, active);
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
    if id.starts_with("workspace.") {
        return false;
    }
    // tui-lipan's runtime auto-registers framework commands under the `app.` id prefix
    // (`app.quit`, `app.focus-next`, `app.focus-prev`, `app.dismiss-overlay`,
    // `app.toggle-devtools`). None of them are meaningful in hyprmux's palette: quit/detach
    // have dedicated commands, panes are terminal shells rather than app-focusable widgets,
    // dismissing an overlay is just `Esc`, and DevTools is a dev-only tool. Keep them out.
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
    let shared = state.shared.as_ref();
    match action {
        Action::OpenClientList => shared.is_some_and(|shared| shared.clients.len() > 1),
        Action::ToggleInputLock => shared.is_some_and(|shared| {
            shared.clients.len() > 1 && !shared.read_only && shared.is_controller()
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
/// WM-modifier chord mirrored from its default key steps.
fn resolve_shortcuts(config: &HyprmuxConfig, id: &str, defaults: &[&str]) -> KeyBindings {
    if let Some(bindings) = config.key_overrides.get(id) {
        KeyBindings::from_bindings(bindings.iter().cloned())
    } else {
        KeyBindings::from_bindings(default_shortcuts_for(config, defaults))
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
    if let Some(text) = toggle_command_label(action, state) {
        return text;
    }
    if action == Action::ToggleLayout {
        let layout = state.workspaces[state.active_workspace].layout_kind.label();
        return format!("Switch layout (current: {layout})");
    }
    if action == Action::RenameSession {
        // An ephemeral (or not-yet-attached) session carries no user-facing name, so this command
        // *names* it for the first time (turning it durable) rather than renaming an existing name.
        let named = state.session_attached && !state.is_ephemeral_session();
        return if named {
            "Rename session"
        } else {
            "Name session"
        }
        .to_string();
    }
    base_label.to_string()
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
        Action::ToggleAnimations => {
            enable_disable_label("animations", state.config.animations.enabled)
        }
        Action::ToggleInputLock => enable_disable_label(
            "input lock",
            state
                .shared
                .as_ref()
                .is_some_and(|shared| shared.input_locked),
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
        Action::ToggleBorderMerge => {
            enable_disable_label("border merging", state.config.pane.merge_borders)
        }
        Action::ToggleBackgroundFollowsTerminal => enable_disable_label(
            "background follows terminal",
            state.config.pane.background_follows_terminal,
        ),
        Action::CycleBorderStyle => {
            format!("Border style: {}", state.config.pane.border_style.label())
        }
        Action::CycleTitleStyle => {
            format!("Titlebar style: {}", state.config.pane.title_style.label())
        }
        Action::CycleWorkbarBadgeStyle => {
            format!(
                "Workbar badge style: {}",
                state.config.pane.workbar_badge_style.label()
            )
        }
        Action::CycleWorkbarTabStyle => {
            format!(
                "Workbar tab style: {}",
                state.config.pane.workbar_tab_style.label()
            )
        }
        Action::CycleWorkbarStyle => {
            format!("Workbar style: {}", state.config.pane.workbar_style.label())
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
    let id = state.focused_pane?;
    if id == SCRATCH_PANE_ID {
        return state.scratch.as_ref();
    }
    state.workspaces[state.active_workspace]
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
        assert!(is_palette_eligible("save-profile"));
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
        assert!(!is_palette_eligible("toggle-border-merge"));
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
            })
            .collect();
        state.shared = Some(shared);
        state
    }

    #[test]
    fn collaboration_commands_follow_roster_and_permissions() {
        let solo = shared_state(1, 1, false, 1);
        assert!(!command_available(Action::OpenClientList, &solo));
        assert!(!command_available(Action::RequestControl, &solo));
        assert!(!command_available(Action::ToggleInputLock, &solo));

        let controller = shared_state(1, 1, false, 2);
        assert!(command_available(Action::OpenClientList, &controller));
        assert!(!command_available(Action::RequestControl, &controller));
        assert!(command_available(Action::ToggleInputLock, &controller));
        // Grant is offered only once a follower is actually requesting.
        assert!(!command_available(Action::GrantControl, &controller));

        let follower = shared_state(2, 1, false, 2);
        assert!(command_available(Action::OpenClientList, &follower));
        assert!(command_available(Action::RequestControl, &follower));
        assert!(!command_available(Action::ToggleInputLock, &follower));
        assert!(!command_available(Action::GrantControl, &follower));

        // Controller with a pending request from client 2 can grant; the follower still cannot.
        let mut controller_with_request = shared_state(1, 1, false, 2);
        controller_with_request.shared.as_mut().unwrap().clients[1].requesting_control = true;
        assert!(command_available(
            Action::GrantControl,
            &controller_with_request
        ));

        let viewer = shared_state(2, 1, true, 2);
        assert!(command_available(Action::OpenClientList, &viewer));
        assert!(!command_available(Action::RequestControl, &viewer));
        assert!(!command_available(Action::ToggleInputLock, &viewer));
    }

    #[test]
    fn input_lock_label_reflects_server_state() {
        let mut state = shared_state(1, 1, false, 2);
        assert_eq!(
            resolved_label(Action::ToggleInputLock, "Toggle input lock", &state),
            "Enable input lock"
        );
        state.shared.as_mut().unwrap().input_locked = true;
        assert_eq!(
            resolved_label(Action::ToggleInputLock, "Toggle input lock", &state),
            "Disable input lock"
        );
    }
}
