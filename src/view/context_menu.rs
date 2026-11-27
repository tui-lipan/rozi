//! The right-click menu, drawn over everything at the point it opened. What it lists depends on
//! its target; [`crate::ops::context_menu`] carries out the chosen entry.

mod sidebar;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::input::Action;
use crate::layout::model::{LayoutKind, SplitAxis};
use crate::msg::Msg;
use crate::ops::context_menu::{
    ContextMenuMsg, agent_entries, pane_entries, session_entries, workspace_entries,
};
use crate::pane::id::PaneId;
use crate::state::{ContextMenuState, ContextMenuTarget};

pub(crate) fn context_menu_overlay(ctx: &Context<AppRoot>, menu: &ContextMenuState) -> Element {
    let entries = match &menu.target {
        ContextMenuTarget::Pane { id, sidebar, .. } => {
            pane_menu(ctx, *id, *sidebar, menu.pane_selection.is_some())
        }
        ContextMenuTarget::Workspace(id) => {
            crate::ops::context_menu::workspace_slot(&ctx.state, *id)
                .map(|slot| workspace_menu(ctx, slot))
                .unwrap_or_default()
        }
        ContextMenuTarget::Session { identity, sidebar } => {
            crate::ops::context_menu::session_row(&ctx.state, identity, *sidebar)
                .map(|entry| session_menu(ctx, &entry))
                .unwrap_or_default()
        }
        ContextMenuTarget::Agent { location, .. } => agent_menu(ctx, location),
        ContextMenuTarget::EmptyWorkspace(_) => empty_workspace_menu(ctx),
        ContextMenuTarget::SidebarTab(id) => sidebar::tab_menu(&ctx.state, id),
        ContextMenuTarget::SidebarPanel(_, _) => sidebar::panel_menu(&ctx.state),
    };
    let theme = &ctx.state.theme;
    let fg = |style: &Style| crate::view::fg_only(style);
    // The same roles a picker wears: the element surface, its border, and the accent-filled
    // highlight with backdrop text.
    let selection = Style::new()
        .fg(theme.surface.backdrop)
        .bg(theme.border_active)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite);
    ContextMenu::new(Text::new("").width(Length::Px(0)).height(Length::Px(0)))
        .entries(entries)
        .open(true)
        .anchor(Some(menu.anchor))
        // Below the pointer, so the tab or row it opened on stays in view.
        .offset((0, 1))
        .animation(crate::view::animation::context_menu_animation(
            ctx.state.config.animations,
        ))
        .border_style(crate::view::overlay_border_style(ctx))
        // The base foreground paints the frame; every entry carries its own text colour.
        .style(fg(&theme.border).bg(theme.surface.element))
        .selection_style(selection)
        .unfocused_selection_style(selection)
        .item_hover_style(Style::new().bg(theme.surface.element.elevate_by(0.08)))
        .separator_style(fg(&theme.border))
        .header_style(crate::view::rozi_fg(theme).bold())
        .detail_style(fg(&theme.muted))
        .active_detail_style(
            Style::new()
                .fg(theme.surface.backdrop)
                .contrast_policy(ContrastPolicy::BlackOrWhite),
        )
        .disabled_style(fg(&theme.muted))
        .destructive_style(Style::new().fg(theme.status.error))
        .submenu_indicator_style(fg(&theme.muted))
        .item_horizontal_padding((0, 1))
        .header_horizontal_padding((0, 1))
        .on_action(
            ctx.link()
                .callback(|id| Msg::ContextMenu(ContextMenuMsg::Activate(id))),
        )
        .on_close(
            ctx.link()
                .callback(|()| Msg::ContextMenu(ContextMenuMsg::Close)),
        )
        .into()
}

/// The first live shortcut of `action`, right-aligned beside its entry.
fn with_shortcut(
    ctx: &Context<AppRoot>,
    entry: ContextMenuEntry,
    action: Action,
) -> ContextMenuEntry {
    let shortcut = action.id().and_then(|id| {
        crate::commands::command_shortcut_labels(ctx, id)
            .into_iter()
            .next()
    });
    match shortcut {
        Some(shortcut) => entry.detail(shortcut),
        None => entry,
    }
}

fn pane_menu(
    ctx: &Context<AppRoot>,
    id: PaneId,
    sidebar: bool,
    has_selection: bool,
) -> Vec<ContextMenuEntry> {
    use pane_entries::*;
    let mut entries = Vec::new();
    if sidebar {
        entries.push(ContextMenuEntry::action(GO, "Go to pane"));
    }
    let Some(pane) = crate::ops::context_menu::visible_pane(&ctx.state, id) else {
        return entries;
    };
    if sidebar {
        entries.push(ContextMenuEntry::separator());
    }
    let action = |entry_id: &'static str, label: &'static str, action: Action| {
        with_shortcut(ctx, ContextMenuEntry::action(entry_id, label), action)
    };
    let scratch = ctx.state.scratch_visible;
    entries.extend([
        ContextMenuEntry::action(COPY, "Copy").disabled(!has_selection),
        action(PASTE, "Paste", Action::Paste),
        action(SEARCH, "Search", Action::OpenSearch),
        ContextMenuEntry::separator(),
    ]);
    // Only Dwindle splits a pane; the other layouts place a new pane by their own order.
    if crate::ops::context_menu::pane_can_split(&ctx.state, id) {
        entries.push(action(
            SPLIT_RIGHT,
            "Split right",
            Action::Split(SplitAxis::Horizontal),
        ));
        entries.push(action(
            SPLIT_DOWN,
            "Split down",
            Action::Split(SplitAxis::Vertical),
        ));
    } else {
        entries.push(action(NEW_PANE, "New pane", Action::Spawn));
    }
    entries.push(ContextMenuEntry::separator());
    entries.push(action(RENAME, "Rename", Action::RenamePane));
    entries.push(action(
        ZOOM,
        if pane.fullscreen { "Unzoom" } else { "Zoom" },
        Action::ToggleFullscreen,
    ));
    entries.push(action(
        FLOAT,
        if pane.floating { "Tile" } else { "Float" },
        Action::ToggleFloat,
    ));
    if !scratch {
        entries.push(ContextMenuEntry::submenu(
            MOVE_TO,
            "Move to workspace",
            move_to_entries(ctx),
        ));
    }
    entries.push(ContextMenuEntry::separator());
    entries.push(
        ContextMenuEntry::action(OPEN_DIRECTORY, "Open directory")
            .disabled(crate::ops::context_menu::pane_directory(&ctx.state, id).is_none()),
    );
    entries.push(ContextMenuEntry::action(COPY_ID, "Copy pane ID"));
    entries.push(ContextMenuEntry::separator());
    entries.push(action(CLOSE, "Close", Action::Close).destructive(true));
    entries
}

fn move_to_entries(ctx: &Context<AppRoot>) -> Vec<ContextMenuEntry> {
    let attachment = ctx.state.current();
    attachment
        .workspaces
        .iter()
        .enumerate()
        .filter(|(slot, _)| *slot != attachment.active_workspace)
        .map(|(slot, workspace)| {
            let label = match workspace.name.as_deref().filter(|name| !name.is_empty()) {
                Some(name) => format!("{}:{name}", slot + 1),
                None => (slot + 1).to_string(),
            };
            with_shortcut(
                ctx,
                ContextMenuEntry::action(format!("{}{slot}", pane_entries::MOVE_TO_PREFIX), label),
                Action::MoveToWorkspace(slot),
            )
        })
        .collect()
}

fn workspace_menu(ctx: &Context<AppRoot>, slot: usize) -> Vec<ContextMenuEntry> {
    use workspace_entries::*;
    let attachment = ctx.state.current();
    let last = attachment.workspaces.len().saturating_sub(1);
    let empty = !attachment.workspaces[slot]
        .panes
        .iter()
        .any(|pane| !pane.closing && !pane.pinned);
    vec![
        with_shortcut(
            ctx,
            ContextMenuEntry::action(SWITCH, "Switch to"),
            Action::SwitchWorkspace(slot),
        )
        .disabled(slot == attachment.active_workspace),
        ContextMenuEntry::action(RENAME, "Rename"),
        ContextMenuEntry::separator(),
        ContextMenuEntry::action(NEW, "New pane"),
        layout_menu(attachment.workspaces[slot].layout_kind),
        ContextMenuEntry::action(SYNC, "Synchronize panes")
            .checked(attachment.workspaces[slot].synchronized),
        ContextMenuEntry::separator(),
        ContextMenuEntry::action(MOVE_LEFT, "Move left").disabled(slot == 0),
        ContextMenuEntry::action(MOVE_RIGHT, "Move right").disabled(slot >= last),
        ContextMenuEntry::separator(),
        ContextMenuEntry::action(KILL, "Kill panes")
            .destructive(true)
            .disabled(empty),
    ]
}

fn layout_menu(current: LayoutKind) -> ContextMenuEntry {
    ContextMenuEntry::submenu(
        "layout",
        "Layout",
        LayoutKind::all().iter().map(|kind| {
            ContextMenuEntry::action(
                format!(
                    "{}{kind}",
                    workspace_entries::LAYOUT_PREFIX,
                    kind = kind.label()
                ),
                kind.title(),
            )
            .checked(*kind == current)
            .disabled(*kind == current)
        }),
    )
}

fn empty_workspace_menu(ctx: &Context<AppRoot>) -> Vec<ContextMenuEntry> {
    vec![
        ContextMenuEntry::action(workspace_entries::NEW, "New pane"),
        ContextMenuEntry::action(workspace_entries::NEW_FLOAT, "New floating pane"),
        layout_menu(
            ctx.state.current().workspaces[ctx.state.current().active_workspace].layout_kind,
        ),
    ]
}

fn session_menu(
    ctx: &Context<AppRoot>,
    entry: &crate::session::discovery::DiscoveredSession,
) -> Vec<ContextMenuEntry> {
    use session_entries::*;
    let killing = ctx.state.remote.session_is_killing(entry);
    let mut entries = Vec::new();
    let restorable = crate::ops::session::session_row_is_restorable(entry);
    let last_seen = crate::ops::session::session_row_is_last_seen(entry);
    if restorable {
        entries.push(ContextMenuEntry::action(OPEN, "Restore").disabled(killing));
    } else if !crate::ops::session::session_row_is_current(&ctx.state, entry) {
        let verb = crate::view::vocabulary::session_presence_for(
            &ctx.state,
            &entry.name,
            entry.remote_target.as_ref(),
        )
        .discovered(&entry.status)
        .verb()
        .unwrap_or("attach");
        entries.push(ContextMenuEntry::action(OPEN, capitalized(verb)).disabled(killing));
    }
    if crate::ops::session::session_row_can_disconnect(&ctx.state, entry) {
        entries.push(ContextMenuEntry::action(DISCONNECT, "Disconnect").disabled(killing));
    }
    if restorable || last_seen {
        entries.push(ContextMenuEntry::action(KILL, "Forget").destructive(true));
        return entries;
    }
    entries.push(ContextMenuEntry::action(RESTART, "Restart").disabled(
        !crate::ops::session::can_restart_discovered_session(&ctx.state, entry),
    ));
    entries.push(
        ContextMenuEntry::action(KILL, "Kill")
            .destructive(true)
            .disabled(killing || !crate::ops::session::session_row_can_kill(entry)),
    );
    entries
}

fn agent_menu(
    ctx: &Context<AppRoot>,
    location: &crate::state::AgentLocation,
) -> Vec<ContextMenuEntry> {
    let open = ctx.state.config.sidebar.agents_config().open;
    let background = crate::ops::agents::background_conversation(&ctx.state, location).is_some();
    if !background {
        // The agent already on screen has nowhere to go.
        let on_screen = crate::view::agents::global_agent_rows(&ctx.state)
            .iter()
            .any(|row| &row.location == location && row.is_on_screen(&ctx.state));
        let label = crate::ops::agents::open_label(location, crate::config::AgentRowOpen::Switch);
        return vec![
            ContextMenuEntry::action(agent_entries::OPEN, capitalized(label)).disabled(on_screen),
        ];
    }
    vec![
        ContextMenuEntry::action(
            agent_entries::OPEN,
            capitalized(crate::ops::agents::open_label(location, open)),
        ),
        ContextMenuEntry::action(
            agent_entries::OPEN_OTHER,
            capitalized(crate::ops::agents::open_label(location, open.other())),
        ),
    ]
}

/// A footer verb (`go to`) as a menu entry (`Go to`).
fn capitalized(verb: &str) -> String {
    let mut chars = verb.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
