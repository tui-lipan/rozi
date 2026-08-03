use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::{SidebarTab, SidebarTabId, UserCommandAction};
use crate::state::{SidebarCommandOutput, SidebarCommandRow, ToastChannel};
use crate::view::sidebar::RowTarget;

const SESSION_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const COMMAND_CAPTURE_BYTES: usize = 64 * 1024;
const COMMAND_MAX_ROWS: usize = 500;
const COMMAND_RAW_ROW_CHARS: usize = 4096;
const COMMAND_DISPLAY_ROW_CHARS: usize = 160;
const COMMAND_BUSY_RETRY: std::time::Duration = std::time::Duration::from_millis(50);

fn sessions_active(ctx: &Context<HyprmuxApp>) -> bool {
    ctx.state.sidebar_visible
        && ctx
            .state
            .sidebar
            .active_tabs()
            .any(|id| id.as_str() == "sessions")
}

fn command_active(ctx: &Context<HyprmuxApp>, id: &SidebarTabId) -> bool {
    ctx.state.sidebar_visible && ctx.state.sidebar.active_tabs().any(|active| active == id)
}

fn command_tab(ctx: &Context<HyprmuxApp>, id: &SidebarTabId) -> Option<(String, u64)> {
    ctx.state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Command {
                name,
                command,
                interval_secs,
                ..
            } if name == id => Some((command.clone(), *interval_secs)),
            _ => None,
        })
}

pub(crate) fn invalidate_sessions(ctx: &mut Context<HyprmuxApp>) {
    ctx.state.sidebar.invalidate_sessions();
}

/// Nudge the open Sessions tab to re-sweep now (e.g. after a config reload or a session change),
/// without disturbing the epoch. The steady-state loop is kept alive by [`ensure_sessions_refresh_armed`];
/// this just kicks an extra immediate sweep for the current epoch.
pub(crate) fn request_sessions_refresh(ctx: &Context<HyprmuxApp>) {
    if sessions_active(ctx)
        && let Some(link) = ctx.state.command_link.as_ref()
    {
        link.send(crate::Msg::SidebarSessionsRefresh {
            epoch: ctx.state.sidebar.sessions_epoch,
        });
    }
}

/// Keep the Sessions tab's auto-refresh loop alive. Called from the post-update chokepoint after
/// every message: if the tab is active but the loop's epoch has fallen behind (a session switch,
/// create, or reopen bumped `sessions_epoch` and killed the old loop), re-arm it. The armed-epoch
/// guard makes this fire exactly once per epoch, so it never stacks parallel loops.
pub(crate) fn ensure_sessions_refresh_armed(ctx: &mut Context<HyprmuxApp>) {
    if !sessions_active(ctx) {
        ctx.state.sidebar.sessions_refresh_armed_epoch = None;
        return;
    }
    let epoch = ctx.state.sidebar.sessions_epoch;
    if ctx.state.sidebar.sessions_refresh_armed_epoch == Some(epoch) {
        return;
    }
    // Only mark the epoch armed once we can actually kick the loop, so a missing link (very early
    // startup) retries on the next message instead of latching a loop that never started.
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    ctx.state.sidebar.sessions_refresh_armed_epoch = Some(epoch);
    // Fill the tab instantly with local rows + known hosts when it would otherwise be blank (the
    // epoch bump on a switch clears the list), so it never flashes empty while the async sweep runs.
    if ctx.state.sidebar.sessions.is_empty() {
        ctx.state.sidebar.sessions = crate::ops::session::local_picker_rows(ctx);
    }
    crate::ops::session::seed_host_registry(ctx);
    link.send(crate::Msg::SidebarSessionsRefresh { epoch });
}

pub(crate) fn request_command_poll(ctx: &Context<HyprmuxApp>) {
    if !ctx.state.sidebar_visible {
        return;
    }
    let Some(link) = ctx.state.command_link.as_ref() else {
        return;
    };
    for tab_id in ctx.state.sidebar.active_tabs().cloned() {
        if command_tab(ctx, &tab_id).is_some() {
            link.send(crate::Msg::SidebarCommandPoll {
                epoch: ctx.state.sidebar.command_epoch,
                tab_id,
            });
        }
    }
}

/// Start the Agents tab's elapsed-time tick unless one is already running or there is nothing to
/// advance. Sent rather than returned as a command so the call sites — which already return
/// commands of their own — do not have to compose two.
pub(crate) fn arm_agent_tick(ctx: &mut Context<HyprmuxApp>) {
    if ctx.state.sidebar.agent_tick_armed
        || crate::view::sidebar::agent_durations(&ctx.state).is_none()
    {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    ctx.state.sidebar.agent_tick_armed = true;
    link.send(crate::Msg::AgentTick);
}

/// One step of the Agents tab's elapsed-time refresh: reschedule while the column is on screen,
/// repaint only when the text it would show actually differs. A row sitting at `12m` therefore
/// costs one string comparison a second rather than sixty repaints, and the chain stops outright
/// once nothing is showing a duration.
pub(super) fn agent_tick(ctx: &mut Context<HyprmuxApp>) -> Update {
    let current = crate::view::sidebar::agent_durations(&ctx.state);
    if current.is_none() {
        ctx.state.sidebar.agent_tick_armed = false;
        ctx.state.sidebar.last_agent_durations = None;
        return Update::none();
    }
    let command = crate::schedule_agent_tick();
    if ctx.state.sidebar.last_agent_durations == current {
        return Update::command_only(command);
    }
    ctx.state.sidebar.last_agent_durations = current;
    Update::with_command(command)
}

pub(super) fn tab_selected(ctx: &mut Context<HyprmuxApp>, panel: usize, index: usize) -> Update {
    let Some(id) = ctx
        .state
        .sidebar
        .panels
        .get(panel)
        .and_then(|panel| panel.tabs.get(index))
        .cloned()
    else {
        return Update::none();
    };
    if ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .any(|tab| tab.id() == id)
    {
        if ctx.state.sidebar.active_tab_in(panel) == Some(&id) {
            let changed_panel = ctx.state.sidebar.active_panel != panel;
            ctx.state.sidebar.active_panel = panel;
            if changed_panel {
                refocus_body(ctx);
                return Update::full();
            }
            return Update::none();
        }
        let Some(panel_state) = ctx.state.sidebar.panels.get_mut(panel) else {
            return Update::none();
        };
        if !panel_state.tabs.contains(&id) {
            return Update::none();
        }
        ctx.state.sidebar.invalidate_sessions();
        ctx.state.sidebar.invalidate_commands();
        ctx.state.sidebar.active_panel = panel;
        let panel_state = &mut ctx.state.sidebar.panels[panel];
        panel_state.active_tab = Some(id);
        // A different tab is a different row list; carrying the old index over would drop the
        // cursor somewhere arbitrary.
        panel_state.cursor = 0;
        panel_state.suppress_row_hover = true;
        panel_state.hovered_row = None;
        // Clicking the tab strip does not move focus — the strip is not focusable and the sidebar
        // is outside click-to-focus — but the body it was on unmounts, and focus goes with it. The
        // file tree feels this worst: each tree keys on its root, so even Files -> Git is a
        // remount, and without this the keyboard would be left pointing at nothing.
        refocus_body(ctx);
        arm_agent_tick(ctx);
        refresh_active_tabs(ctx)
    } else {
        Update::none()
    }
}

pub(super) fn tab_reordered(
    ctx: &mut Context<HyprmuxApp>,
    panel: usize,
    event: DraggableTabReorderEvent,
) -> Update {
    if !ctx.state.sidebar.reorder_tab(panel, event.from, event.to) {
        return Update::none();
    }
    sync_and_persist_panels(ctx);
    Update::layout()
}

pub(super) fn tab_transferred(
    ctx: &mut Context<HyprmuxApp>,
    event: DraggableTabTransferEvent,
) -> Update {
    let Some(from_panel) = crate::view::sidebar::panel_from_bar_id(&event.from_bar) else {
        return Update::none();
    };
    let Some(to_panel) = crate::view::sidebar::panel_from_bar_id(&event.to_bar) else {
        return Update::none();
    };
    if !ctx
        .state
        .sidebar
        .transfer_tab(from_panel, to_panel, event.from, event.to)
    {
        return Update::none();
    }
    ctx.state.sidebar.active_panel = to_panel;
    sync_and_persist_panels(ctx);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(super) fn panels_resized(ctx: &mut Context<HyprmuxApp>, event: SplitterResizeEvent) -> Update {
    let Some(ratio) = event.weights.first().copied() else {
        return Update::none();
    };
    set_split_ratio(ctx, ratio)
}

fn width_from_resize_event(ctx: &Context<HyprmuxApp>, event: &SplitterResizeEvent) -> Option<u16> {
    let viewport = ctx.viewport();
    let sidebar_index =
        usize::from(ctx.state.config.sidebar.position == crate::config::SidebarPosition::Right);
    let weight = event.weights.get(sidebar_index).copied()?;
    let available = viewport.w.saturating_sub(1);
    let pane_width = (weight * f32::from(available)).round() as u16;
    Some(pane_width.saturating_add(1).clamp(
        crate::config::SIDEBAR_MIN_WIDTH,
        crate::config::SIDEBAR_MAX_WIDTH,
    ))
}

pub(super) fn width_resizing(ctx: &mut Context<HyprmuxApp>, event: SplitterResizeEvent) -> Update {
    let Some(width) = width_from_resize_event(ctx, &event) else {
        return Update::none();
    };
    if ctx.state.sidebar.width_preview == Some(width) {
        return Update::none();
    }
    ctx.state.sidebar.width_preview = Some(width);
    Update::full()
}

pub(super) fn width_resized(ctx: &mut Context<HyprmuxApp>, event: SplitterResizeEvent) -> Update {
    let Some(width) = width_from_resize_event(ctx, &event) else {
        ctx.state.sidebar.width_preview = None;
        return Update::full();
    };
    ctx.state.sidebar.width_preview = None;
    set_width(ctx, width)
}

fn sync_and_persist_panels(ctx: &mut Context<HyprmuxApp>) {
    let panels = persisted_panel_ids(
        ctx.state.sidebar.panel_ids(),
        &ctx.state.config.sidebar.panels,
        ctx.state.config.sidebar.split,
    );
    ctx.state.config.sidebar.panels = panels.clone();
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_panels(&panels));
}

fn persisted_panel_ids(
    mut displayed: Vec<Vec<crate::config::SidebarTabId>>,
    configured: &[Vec<crate::config::SidebarTabId>],
    split: bool,
) -> Vec<Vec<crate::config::SidebarTabId>> {
    if split || configured.len() < 2 || displayed.len() != 1 {
        return displayed;
    }

    let flat = displayed.pop().unwrap_or_default();
    let mut offset: usize = 0;
    configured
        .iter()
        .enumerate()
        .map(|(index, panel)| {
            let end = if index + 1 == configured.len() {
                flat.len()
            } else {
                offset.saturating_add(panel.len()).min(flat.len())
            };
            let tabs = flat[offset..end].to_vec();
            offset = end;
            tabs
        })
        .collect()
}

fn set_split_enabled(ctx: &mut Context<HyprmuxApp>, split: bool) {
    if ctx.state.config.sidebar.split == split {
        return;
    }
    ctx.state.config.sidebar.split = split;
    if split && ctx.state.config.sidebar.panels.len() == 1 {
        ctx.state.config.sidebar.panels.push(Vec::new());
    }
    ctx.state
        .sidebar
        .apply_configured_panels(&ctx.state.config.sidebar);
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_split(split));
}

fn persist_sidebar_preference(
    ctx: &mut Context<HyprmuxApp>,
    result: std::result::Result<std::path::PathBuf, String>,
) {
    if let Err(error) = result {
        crate::pty_events::notify_on(
            ctx,
            ToastChannel::PreferenceSave,
            Some("Sidebar preference not saved".to_string()),
            error,
        );
    }
}

/// Re-aim keyboard focus at the active tab's body after the previous one unmounted. A no-op unless
/// the sidebar already had the keyboard — switching tabs with the mouse must not steal it.
pub(crate) fn refocus_body(ctx: &mut Context<HyprmuxApp>) {
    if !ctx.state.sidebar.focused {
        return;
    }
    let key = crate::view::sidebar_focus_key(ctx);
    ctx.request_focus(key);
}

pub(crate) fn visibility_changed(ctx: &mut Context<HyprmuxApp>) -> Update {
    // Hiding the sidebar unmounts the body, so hand the keyboard back before it disappears rather
    // than leaving focus on a widget that is about to stop existing.
    if !ctx.state.sidebar_visible && ctx.state.sidebar.focused {
        ctx.state.sidebar.focused = false;
        release_focus(ctx);
    }
    if !ctx.state.sidebar_visible {
        ctx.state.sidebar.width_preview = None;
    }
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.sidebar.invalidate_commands();
    arm_agent_tick(ctx);
    refresh_active_tabs(ctx)
}

/// `focus-sidebar`: reveal the sidebar if it is hidden, then move keyboard focus into its row list.
/// The body sits in a `FocusScope::Exclude` subtree, so an explicit keyed request is the only way
/// in — Tab and clicks deliberately cannot do this.
pub(crate) fn focus_body(ctx: &mut Context<HyprmuxApp>) -> Update {
    let command = if ctx.state.sidebar_visible {
        None
    } else {
        ctx.state.sidebar_visible = true;
        visibility_changed(ctx).command
    };
    // Resolves after reconciliation, so requesting it in the same pass that reveals the sidebar is
    // fine even though the body has not mounted yet.
    let key = crate::view::sidebar_focus_key(ctx);
    ctx.request_focus(key);
    // The request resolves after reconciliation, so record the intent now. Nothing can read this
    // back off the framework — the body sits in a `FocusScope::Exclude` subtree, which is invisible
    // to `has_focus_within_key` — so `ops::focus` retracts it whenever focus goes elsewhere.
    ctx.state.sidebar.focused = true;
    if let Some(panel) = ctx.state.sidebar.active_panel_mut() {
        panel.suppress_row_hover = true;
    }
    ctx.state.commands_dirty = true;
    Update::with_command(command)
}

/// Escape from the sidebar or a pointer-focused explorer: give the keyboard back to the focused
/// pane.
pub(crate) fn blur_body(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.sidebar.focused = false;
    ctx.state.sidebar.explorer_entered_from_tree = false;
    ctx.state.commands_dirty = true;
    release_focus(ctx);
    Update::full()
}

pub(crate) fn explorer_focus(
    ctx: &mut Context<HyprmuxApp>,
    origin: Option<FileTreeExplorerFocusOrigin>,
) -> Update {
    ctx.state.sidebar.explorer_entered_from_tree =
        origin == Some(FileTreeExplorerFocusOrigin::Tree);
    Update::none()
}

/// The explorer committed its query with Enter and returned focus to the tree. This is a real
/// sidebar-mode entry, unlike a pointer click into the explorer, so restore the sidebar cursor and
/// its keyboard ownership before the next key arrives.
pub(crate) fn tree_focused(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.sidebar.focused = true;
    ctx.state.sidebar.explorer_entered_from_tree = false;
    if let Some(panel) = ctx.state.sidebar.active_panel_mut() {
        panel.suppress_row_hover = true;
    }
    ctx.state.commands_dirty = true;
    Update::full()
}

/// Drop focus from the sidebar body and hand it to the focused pane when there is one to hand it
/// to. The unconditional `blur` matters: a pane whose terminal has not come up yet refuses focus,
/// and without this the sidebar would keep the keyboard with no way out.
fn release_focus(ctx: &mut Context<HyprmuxApp>) {
    ctx.blur();
    crate::ops::focus::request_current_pane_focus(ctx);
}

/// Tab / Shift-Tab while the body has focus. Cycling remounts the body under a new key, so focus
/// has to be re-requested for the tab the user just landed on.
pub(crate) fn cycle_tab(ctx: &mut Context<HyprmuxApp>, forward: bool) -> Update {
    if !ctx.state.sidebar_visible {
        return Update::none();
    }
    let panel = ctx.state.sidebar.active_panel;
    ctx.state.sidebar.cycle(panel, forward);
    if let Some(panel) = ctx.state.sidebar.panels.get_mut(panel) {
        panel.cursor = 0;
        panel.suppress_row_hover = true;
        panel.hovered_row = None;
    }
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

/// Store the number of selectable rows currently visible in a composed row list. The event is
/// keyed by tab because switching tabs can queue the old list's final viewport event alongside the
/// new one.
pub(crate) fn viewport_changed(
    ctx: &mut Context<HyprmuxApp>,
    panel: usize,
    tab_id: crate::config::SidebarTabId,
    event: ScrollViewportEvent,
) -> Update {
    if ctx.state.sidebar.active_tab_in(panel) != Some(&tab_id) {
        return Update::none();
    }
    let Some(first) = event.first_visible_index else {
        return Update::none();
    };
    let last = event.last_visible_index.unwrap_or(first).max(first);
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let rows = crate::view::sidebar::body_rows(ctx, &tab);
    let page_rows = rows
        .iter()
        .enumerate()
        .filter(|(index, row)| *index >= first && *index <= last && row.selectable())
        .count()
        .max(1);
    let Some(panel_state) = ctx.state.sidebar.panels.get_mut(panel) else {
        return Update::none();
    };
    panel_state.page_rows = page_rows;
    Update::none()
}

/// Move by the number of selectable rows that fit in the active row list's viewport.
pub(crate) fn move_cursor_page(ctx: &mut Context<HyprmuxApp>, down: bool) -> Update {
    let page_rows = ctx
        .state
        .sidebar
        .active_panel()
        .map(|panel| panel.page_rows.max(1) as isize)
        .unwrap_or(1);
    move_cursor(ctx, if down { page_rows } else { -page_rows })
}

pub(crate) fn focus_panel(ctx: &mut Context<HyprmuxApp>, down: bool) -> Update {
    if ctx.state.sidebar.panels.len() < 2 {
        return Update::none();
    }
    let next = if down { 1 } else { 0 };
    if ctx.state.sidebar.active_panel == next {
        return Update::none();
    }
    ctx.state.sidebar.active_panel = next;
    if let Some(panel) = ctx.state.sidebar.active_panel_mut() {
        panel.suppress_row_hover = true;
    }
    refocus_body(ctx);
    Update::full()
}

pub(crate) fn reorder_active_tab(ctx: &mut Context<HyprmuxApp>, right: bool) -> Update {
    let panel = ctx.state.sidebar.active_panel;
    let Some(panel_state) = ctx.state.sidebar.panels.get(panel) else {
        return Update::none();
    };
    let Some(active) = panel_state.active_tab.as_ref() else {
        return Update::none();
    };
    let Some(from) = panel_state.tabs.iter().position(|id| id == active) else {
        return Update::none();
    };
    let to = if right {
        (from + 1).min(panel_state.tabs.len().saturating_sub(1))
    } else {
        from.saturating_sub(1)
    };
    if !ctx.state.sidebar.reorder_tab(panel, from, to) {
        return Update::none();
    }
    sync_and_persist_panels(ctx);
    Update::layout()
}

pub(crate) fn move_active_tab_to_panel(ctx: &mut Context<HyprmuxApp>, down: bool) -> Update {
    if ctx.state.sidebar.panels.len() == 1 {
        if !down {
            return Update::none();
        }
        set_split_enabled(ctx, true);
    }
    let from_panel = ctx.state.sidebar.active_panel;
    let to_panel = if down { 1 } else { 0 };
    if from_panel == to_panel {
        return Update::none();
    }
    let Some(active) = ctx.state.sidebar.active_tab().cloned() else {
        return Update::none();
    };
    let Some(from) = ctx.state.sidebar.panels[from_panel]
        .tabs
        .iter()
        .position(|id| id == &active)
    else {
        return Update::none();
    };
    let to = ctx.state.sidebar.panels[to_panel].tabs.len();
    if !ctx
        .state
        .sidebar
        .transfer_tab(from_panel, to_panel, from, to)
    {
        return Update::none();
    }
    ctx.state.sidebar.active_panel = to_panel;
    sync_and_persist_panels(ctx);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(crate) fn toggle_visible(ctx: &mut Context<HyprmuxApp>) -> Update {
    set_visible(ctx, !ctx.state.sidebar_visible);
    visibility_changed(ctx)
}

fn set_visible(ctx: &mut Context<HyprmuxApp>, visible: bool) {
    ctx.state.sidebar_visible = visible;
    if ctx.state.config.sidebar.visible == visible {
        return;
    }
    ctx.state.config.sidebar.visible = visible;
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_visible(visible));
}

pub(crate) fn toggle_split(ctx: &mut Context<HyprmuxApp>) -> Update {
    let split = !ctx.state.config.sidebar.split;
    set_split_enabled(ctx, split);
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

pub(crate) fn resize_width(ctx: &mut Context<HyprmuxApp>, handle_right: bool) -> Update {
    let wider = match ctx.state.config.sidebar.position {
        crate::config::SidebarPosition::Left => handle_right,
        crate::config::SidebarPosition::Right => !handle_right,
    };
    let delta = if wider { 2 } else { -2 };
    let width = ctx.state.config.sidebar.width.saturating_add_signed(delta);
    set_width(ctx, width)
}

fn set_width(ctx: &mut Context<HyprmuxApp>, width: u16) -> Update {
    let width = width.clamp(
        crate::config::SIDEBAR_MIN_WIDTH,
        crate::config::SIDEBAR_MAX_WIDTH,
    );
    ctx.state.sidebar.invalidate_outer_splitter();
    if ctx.state.config.sidebar.width == width {
        return Update::layout();
    }
    ctx.state.config.sidebar.width = width;
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_width(width));
    Update::full()
}

pub(crate) fn resize_panel_split(ctx: &mut Context<HyprmuxApp>, down: bool) -> Update {
    if ctx.state.sidebar.panels.len() < 2 {
        return Update::none();
    }
    let delta = if down { 0.05 } else { -0.05 };
    set_split_ratio(ctx, ctx.state.config.sidebar.split_ratio + delta)
}

fn set_split_ratio(ctx: &mut Context<HyprmuxApp>, ratio: f32) -> Update {
    let ratio = ratio.clamp(
        crate::config::SIDEBAR_MIN_SPLIT_RATIO,
        crate::config::SIDEBAR_MAX_SPLIT_RATIO,
    );
    ctx.state.sidebar.invalidate_panel_splitter();
    if (ctx.state.config.sidebar.split_ratio - ratio).abs() < 0.001 {
        return Update::layout();
    }
    ctx.state.config.sidebar.split_ratio = ratio;
    persist_sidebar_preference(ctx, crate::config::persist_sidebar_split_ratio(ratio));
    Update::full()
}

fn open_sessions(ctx: &mut Context<HyprmuxApp>) {
    // Populate the tab instantly with local rows, then run the full sweep (configured remote hosts
    // included) off the UI thread. Querying remote hosts over ssh here used to block the tab switch
    // on a round-trip — or the whole connect timeout when a host was down — every time it opened.
    ctx.state.sidebar.sessions = crate::ops::session::local_picker_rows(ctx);
    crate::ops::session::seed_host_registry(ctx);
}

fn refresh_active_tabs(ctx: &mut Context<HyprmuxApp>) -> Update {
    if sessions_active(ctx) {
        open_sessions(ctx);
    }
    request_command_poll(ctx);
    Update::full()
}

/// Move the keyboard cursor by `delta` selectable rows, stopping at the ends rather than wrapping —
/// the row list is a panel, not a carousel, and wrapping past the last agent back to the first reads
/// as a glitch. Headers and spacers are stepped over rather than landed on.
pub(crate) fn move_cursor(ctx: &mut Context<HyprmuxApp>, delta: isize) -> Update {
    let panel = ctx.state.sidebar.active_panel;
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let rows = crate::view::sidebar::body_rows(ctx, &tab);
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.selectable())
        .map(|(index, _)| index)
        .collect();
    if selectable.is_empty() {
        return Update::none();
    }
    let current =
        crate::view::sidebar::resolve_cursor(ctx.state.sidebar.panels[panel].cursor, &rows);
    let position = current
        .and_then(|current| selectable.iter().position(|index| *index == current))
        .unwrap_or(0);
    let next = position
        .saturating_add_signed(delta)
        .min(selectable.len() - 1);
    let cursor = selectable[next];
    if ctx.state.sidebar.panels[panel].cursor == cursor {
        return Update::none();
    }
    // Moving off a row disarms any pending confirmation.
    ctx.state.sidebar.pending_host_disconnect = None;
    ctx.state.sidebar.pending_row_close = None;
    ctx.state.sidebar.panels[panel].cursor = cursor;
    ctx.state.sidebar.panels[panel].suppress_row_hover = true;
    Update::full()
}

/// A real pointer move ends keyboard modality and lets row hover follow the pointer again.
pub(crate) fn pointer_moved(ctx: &mut Context<HyprmuxApp>, panel: usize) -> Update {
    let Some(panel) = ctx.state.sidebar.panels.get_mut(panel) else {
        return Update::none();
    };
    if !panel.suppress_row_hover {
        return Update::none();
    }
    panel.suppress_row_hover = false;
    // Re-enabling the rows' hover effects changes how the row list describes itself, so the view has
    // to run - but the description is the same shape, so reconciling it is enough.
    Update::layout()
}

/// The pointer entered or left a row, which is what reveals that row's ✕.
///
/// A leave only clears when it is still this row's: moving from a row onto the ✕ nested inside it
/// fires leave for the row and then enter for the ✕, both naming the same index, and the queue is
/// drained before the next paint — so ordering them this way keeps the glyph steady under the
/// pointer instead of flickering out from under it.
pub(crate) fn row_hover(
    ctx: &mut Context<HyprmuxApp>,
    panel: usize,
    index: usize,
    hovered: bool,
) -> Update {
    let Some(panel) = ctx.state.sidebar.panels.get_mut(panel) else {
        return Update::none();
    };
    let next = if hovered {
        Some(index)
    } else if panel.hovered_row == Some(index) {
        None
    } else {
        return Update::none();
    };
    if panel.hovered_row == next {
        return Update::none();
    }
    panel.hovered_row = next;
    // A click-only region does not repaint on a hover transition by itself, and the ✕ appearing is
    // exactly what the transition has to show — so the view has to run. `layout` rather than `full`:
    // the row list is described the same way either side of the transition, one glyph aside, so
    // re-running the view and reconciling is enough without rebuilding the whole element tree.
    Update::layout()
}

/// A row's ✕ was clicked. The first click arms a confirmation (the row strikes through and its
/// detail line asks for the second click), the second commits it — within
/// [`crate::ops::confirm::CONFIRM_WINDOW`], after which the arming lapses on its own.
///
/// Deliberately confirms regardless of `[confirm]`, which gates the *keyboard* close/kill actions.
/// This is a one-cell pointer target sitting on a row whose ordinary click merely focuses a pane or
/// attaches a session, so a slip is both easy and expensive — the two are not the same gesture and
/// do not share a switch.
pub(crate) fn row_close(ctx: &mut Context<HyprmuxApp>, panel: usize, index: usize) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let mut rows = crate::view::sidebar::body_rows(ctx, &tab);
    if index >= rows.len() {
        return Update::none();
    }
    let row = rows.swap_remove(index);
    let Some(close) = row.close else {
        return Update::none();
    };
    // Any other pending confirmation is abandoned by acting here, as it is on an activation.
    ctx.state.sidebar.pending_host_disconnect = None;
    if ctx.state.sidebar.pending_row_close.take() != Some(close.clone()) {
        ctx.state.sidebar.pending_row_close = Some(close);
        return crate::ops::confirm::arm(ctx);
    }
    match close {
        crate::state::SidebarClose::Pane(id) => {
            crate::ops::exit::clear_pending(ctx);
            crate::pane_lifecycle::close_pane(ctx, id)
        }
        // The row carries the live discovered entry the identity was built from, so the kill acts
        // on what is actually on screen rather than re-looking it up and risking a stale match.
        crate::state::SidebarClose::Session { .. } => match row.target {
            RowTarget::Session(entry) => crate::ops::session::kill_discovered_session(ctx, *entry),
            _ => Update::none(),
        },
    }
}

/// Enter: run whatever the row under the cursor does — the same path a click on it takes.
pub(crate) fn activate_cursor(ctx: &mut Context<HyprmuxApp>) -> Update {
    let panel = ctx.state.sidebar.active_panel;
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let rows = crate::view::sidebar::body_rows(ctx, &tab);
    match crate::view::sidebar::resolve_cursor(ctx.state.sidebar.panels[panel].cursor, &rows) {
        Some(index) => row_activate(ctx, panel, index),
        None => Update::none(),
    }
}

/// A row was activated by Enter or by a click. The index is resolved against a freshly rebuilt row
/// list — the same pure function of `State` the view rendered from — so both gestures land on the
/// same handler and a row list that changed underneath simply resolves to nothing.
pub(super) fn row_activate(ctx: &mut Context<HyprmuxApp>, panel: usize, index: usize) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab_in(ctx, panel).cloned() else {
        return Update::none();
    };
    let mut rows = crate::view::sidebar::body_rows(ctx, &tab);
    if index >= rows.len() {
        return Update::none();
    }
    // Acting on anything disarms a pending confirmation; capture the host one first so the matching
    // disconnect row can still see its own armed state below.
    let armed_disconnect = ctx.state.sidebar.pending_host_disconnect.take();
    ctx.state.sidebar.pending_row_close = None;
    match rows.swap_remove(index).target {
        RowTarget::Inert => Update::none(),
        RowTarget::Pane(id) => focus_pane(ctx, id),
        RowTarget::Session(entry) => activate_session(ctx, *entry),
        RowTarget::HostConnect(target) => connect_host(ctx, target),
        RowTarget::HostDisconnect(target) => disconnect_host(ctx, target, armed_disconnect),
        RowTarget::NewSession(None) => crate::ops::session::open_create_session(ctx),
        RowTarget::NewSession(Some(target)) => {
            crate::ops::session::open_create_session_on_host(ctx, target)
        }
        RowTarget::ConnectHost => crate::ops::session::open_connect_remote_host(ctx),
        RowTarget::Launcher {
            config_epoch,
            tab_id,
            entry_index,
        } => launcher_activate(ctx, config_epoch, tab_id, entry_index),
        RowTarget::CommandRow {
            config_epoch,
            tab_id,
            output_epoch,
            line,
        } => command_row_activate(ctx, config_epoch, tab_id, output_epoch, line),
    }
}

pub(super) fn launcher_activate(
    ctx: &mut Context<HyprmuxApp>,
    config_epoch: u64,
    tab_id: SidebarTabId,
    entry_index: usize,
) -> Update {
    if config_epoch != ctx.state.sidebar.config_epoch {
        return Update::none();
    }
    let action = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Launcher { name, entries, .. } if name == &tab_id => {
                entries.get(entry_index).map(|entry| entry.action.clone())
            }
            _ => None,
        });
    action.map_or_else(Update::none, |action| {
        crate::actions::execute_user_command_action(ctx, &action)
    })
}

pub(super) fn poll_command(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    tab_id: SidebarTabId,
) -> Update {
    if epoch != ctx.state.sidebar.command_epoch || !command_active(ctx, &tab_id) {
        return Update::none();
    }
    let Some((command_line, _)) = command_tab(ctx, &tab_id) else {
        return Update::none();
    };
    if ctx.state.sidebar.command_in_flight.contains_key(&tab_id) {
        return Update::command_only(Command::after(
            COMMAND_BUSY_RETRY,
            move |link: CommandLink<crate::Msg>| {
                link.send(crate::Msg::SidebarCommandPoll { epoch, tab_id });
            },
        ));
    }
    ctx.state
        .sidebar
        .command_in_flight
        .insert(tab_id.clone(), epoch);
    let shell = crate::platform::command::resolve_command_shell(
        ctx.state.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    Update::command_only(Command::spawn(move |link: CommandLink<crate::Msg>| {
        let rows = command_rows(crate::platform::command::run_bounded_shell_command(
            &shell,
            &command_line,
            COMMAND_TIMEOUT,
            COMMAND_CAPTURE_BYTES,
        ));
        link.send(crate::Msg::SidebarCommandOutput {
            epoch,
            tab_id,
            rows,
        });
    }))
}

pub(super) fn command_output(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    tab_id: SidebarTabId,
    rows: Vec<SidebarCommandRow>,
) -> Update {
    if ctx.state.sidebar.command_in_flight.get(&tab_id) == Some(&epoch) {
        ctx.state.sidebar.command_in_flight.remove(&tab_id);
    }
    if epoch != ctx.state.sidebar.command_epoch || !command_active(ctx, &tab_id) {
        return Update::none();
    }
    let Some((_, interval_secs)) = command_tab(ctx, &tab_id) else {
        return Update::none();
    };
    let changed = ctx
        .state
        .sidebar
        .command_output
        .get(&tab_id)
        .is_none_or(|output| output.rows != rows);
    if changed {
        ctx.state.sidebar.next_output_epoch = ctx.state.sidebar.next_output_epoch.wrapping_add(1);
        ctx.state.sidebar.command_output.insert(
            tab_id.clone(),
            SidebarCommandOutput {
                epoch: ctx.state.sidebar.next_output_epoch,
                rows,
            },
        );
    }
    let command = Command::after(
        std::time::Duration::from_secs(interval_secs),
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::SidebarCommandPoll { epoch, tab_id });
        },
    );
    if changed {
        Update::with_command(command)
    } else {
        Update::command_only(command)
    }
}

pub(super) fn command_row_activate(
    ctx: &mut Context<HyprmuxApp>,
    config_epoch: u64,
    tab_id: SidebarTabId,
    output_epoch: u64,
    line: String,
) -> Update {
    if config_epoch != ctx.state.sidebar.config_epoch {
        return Update::none();
    }
    let current = ctx.state.sidebar.command_output.get(&tab_id);
    if current.is_none_or(|output| {
        output.epoch != output_epoch || !output.rows.iter().any(|row| !row.error && row.raw == line)
    }) {
        return Update::none();
    }
    let action = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Command {
                name,
                on_click: Some(action),
                ..
            } if name == &tab_id => Some(action.clone()),
            _ => None,
        });
    action
        .map(|action| resolve_row_action(&action, &line))
        .map_or_else(Update::none, |action| {
            crate::actions::execute_user_command_action(ctx, &action)
        })
}

fn resolve_row_action(action: &UserCommandAction, line: &str) -> UserCommandAction {
    substitute(action, "{line}", line)
}

fn substitute(action: &UserCommandAction, placeholder: &str, value: &str) -> UserCommandAction {
    match action {
        UserCommandAction::Send(text) => UserCommandAction::Send(text.replace(placeholder, value)),
        // Config validation rejects placeholders here; run/popup commands are always fixed.
        action => action.clone(),
    }
}

/// Activate a file-tree row: run the tab's `on_click` with `{path}` replaced by the activated path.
///
/// A directory activation only expands the tree (handled in the widget); running the action for it
/// would type the directory's path at the prompt just because it was opened, so directories are
/// dropped here.
pub(super) fn tree_activate(
    ctx: &mut Context<HyprmuxApp>,
    config_epoch: u64,
    tab_id: SidebarTabId,
    path: String,
    is_dir: bool,
) -> Update {
    if is_dir || config_epoch != ctx.state.sidebar.config_epoch {
        return Update::none();
    }
    let action = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Tree { config, .. } if tab.id() == tab_id => config.on_click.clone(),
            _ => None,
        });
    action.map_or_else(Update::none, |action| {
        // `send` gets the path substituted as literal keystrokes. `run`/`popup` never do — a path
        // comes from the filesystem and must not compose a command line — so they receive it as
        // `$HYPRMUX_FILE` instead, which a shell expands as one word inside quotes.
        let with_path = substitute(&action, "{path}", &path);
        let env = vec![("HYPRMUX_FILE".to_string(), path)];
        crate::actions::execute_user_command_action_with_env(ctx, &with_path, env)
    })
}

/// The git repository containing `cwd`, found by walking ancestors for a `.git` entry. `.git` is a
/// file rather than a directory inside worktrees and submodules, so this tests existence, not kind.
/// The file tree needs a directory it has no listing for. Ask the session server to read it.
///
/// Deduplicated against in-flight and already-delivered paths: the widget re-emits a request on
/// every rebuild while a directory is still absent from the provided source, so without this an
/// expanded-but-slow directory would issue one `ListDirectory` per frame.
pub(super) fn tree_entry_request(ctx: &mut Context<HyprmuxApp>, path: String) -> Update {
    if ctx.state.current().remote_host.is_none() {
        return Update::none();
    }
    if ctx.state.sidebar.tree_pending.contains(&path)
        || ctx
            .state
            .sidebar
            .tree_listings
            .iter()
            .any(|listing| &*listing.path == path.as_str())
    {
        return Update::none();
    }
    let Some(client) = ctx.state.current().session_client.as_ref() else {
        return Update::none();
    };
    if !client.supports_file_tree() {
        return Update::none();
    }
    // Always fetch dotfiles: `show_hidden` is per-tab, and the widget filters provided entries by
    // it anyway, so one listing serves every tab and toggling the option needs no refetch.
    client.list_directory(path.clone(), true);
    ctx.state.sidebar.tree_pending.insert(path);
    Update::none()
}

/// A server-served directory listing arrived. Replaces any previous listing for that path so a
/// refresh overwrites rather than duplicating.
pub(super) fn tree_directory_listed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    path: String,
    entries: Vec<crate::session::protocol::WireDirEntry>,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    ctx.state.sidebar.tree_pending.remove(&path);
    ctx.state
        .sidebar
        .tree_listings
        .retain(|listing| &*listing.path != path.as_str());
    let listing = match error {
        Some(error) => FileTreeDirectoryListing::error(path, error),
        None => FileTreeDirectoryListing::new(path, entries.into_iter().map(wire_entry_to_widget)),
    };
    ctx.state.sidebar.tree_listings.push(listing);
    Update::full()
}

/// A server-served change scan arrived, backing the `Changes` tab under `--remote`.
pub(super) fn tree_changes_listed(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    root: String,
    changes: Vec<crate::session::protocol::WireChange>,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    // An error here means "no repository" or "no git on the server" — an empty change set is the
    // honest projection, and the tab already renders that as "No changes".
    ctx.state.sidebar.tree_changes = if error.is_some() {
        Vec::new()
    } else {
        changes.into_iter().map(wire_change_to_widget).collect()
    };
    ctx.state.sidebar.tree_changes_root = Some(root);
    Update::full()
}

fn wire_entry_to_widget(entry: crate::session::protocol::WireDirEntry) -> FileTreeEntry {
    let mut out = FileTreeEntry::new(entry.name, entry.is_dir)
        .symlink(entry.is_symlink)
        .ignored(entry.ignored);
    if entry.git_staged.is_some() || entry.git_unstaged.is_some() {
        out = out.git_status(GitFileStatus::new(
            entry.git_staged.map(wire_state_to_change),
            entry.git_unstaged.map(wire_state_to_change),
        ));
    }
    out
}

fn wire_change_to_widget(change: crate::session::protocol::WireChange) -> FileTreeChange {
    FileTreeChange::new(change.path, wire_state_to_status(change.state)).staged(change.staged)
}

fn wire_state_to_change(state: crate::session::protocol::WireChangeState) -> GitChangeState {
    use crate::session::protocol::WireChangeState as Wire;
    match state {
        Wire::Added => GitChangeState::Added,
        Wire::Modified => GitChangeState::Modified,
        Wire::Deleted => GitChangeState::Deleted,
        Wire::Renamed => GitChangeState::Renamed,
        Wire::Untracked => GitChangeState::Untracked,
        Wire::Conflicted => GitChangeState::Conflicted,
    }
}

fn wire_state_to_status(state: crate::session::protocol::WireChangeState) -> FileTreeChangeStatus {
    use crate::session::protocol::WireChangeState as Wire;
    match state {
        Wire::Added => FileTreeChangeStatus::Added,
        Wire::Modified => FileTreeChangeStatus::Modified,
        Wire::Deleted => FileTreeChangeStatus::Deleted,
        Wire::Renamed => FileTreeChangeStatus::Renamed,
        Wire::Untracked => FileTreeChangeStatus::Untracked,
        Wire::Conflicted => FileTreeChangeStatus::Conflicted,
    }
}

fn bump_git_refresh(sidebar: &mut crate::state::SidebarState) {
    // The widget ignores a token that does not increase, so this only counts up.
    sidebar.git_refresh_token = sidebar.git_refresh_token.saturating_add(1);
}

/// File-tree chokepoint: keep the resolved roots in step with the focused pane, and refresh git
/// status when a command finishes.
///
/// Runs after every message like the focus chokepoint, so the common case must be cheap: it
/// compares the pane's reported directory against the cached one and does nothing when unchanged.
/// The ancestor walk only runs when the directory actually changed, which is user-paced — a shell
/// re-reporting the same directory on every prompt costs one string comparison.
pub(crate) fn sync_tree_roots(ctx: &mut Context<HyprmuxApp>) {
    // Compared as a borrow: this runs per message, including output from off-screen panes that the
    // session handler deliberately makes free, so the steady state must not allocate.
    // Under `--remote` the tree roots at the server's path, not a local one, so this follows
    // `server_cwd_ref`. The repository walk stays local-only: `.git` cannot be probed across the
    // link, and `root_for` already falls back to the cwd when there is no repo root.
    if crate::pane_lifecycle::focused_server_cwd_ref(&ctx.state)
        != ctx.state.sidebar.tree_cwd.as_deref()
    {
        let cwd = crate::pane_lifecycle::focused_server_cwd_ref(&ctx.state).map(str::to_string);
        ctx.state.sidebar.tree_repo = if ctx.state.current().remote_host.is_some() {
            None
        } else {
            cwd.as_deref()
                .and_then(crate::platform::paths::discover_project_root)
        };
        ctx.state.sidebar.tree_cwd = cwd;
        // A new root invalidates every server-served listing: paths under the old root will never
        // be asked for again, and keeping them would leak one host's tree into another's.
        ctx.state.sidebar.tree_listings.clear();
        ctx.state.sidebar.tree_pending.clear();
        ctx.state.sidebar.tree_changes.clear();
        ctx.state.sidebar.tree_changes_root = None;
        bump_git_refresh(&mut ctx.state.sidebar);
    }

    // A command finishing is the moment the working tree most likely changed, and it is a far
    // better refresh trigger than a timer: no polling while the user reads, immediate feedback
    // after a build, commit, or checkout.
    let phase = ctx.state.current().focused_pane.and_then(|id| {
        crate::pane_lifecycle::find_pane(&ctx.state, id)
            .map(|pane| (id, pane.terminal.command_phase))
    });
    if phase != ctx.state.sidebar.last_command_phase {
        ctx.state.sidebar.last_command_phase = phase;
        if matches!(
            phase,
            Some((
                _,
                crate::session::protocol::PaneCommandPhase::Completed { .. }
            ))
        ) {
            bump_git_refresh(&mut ctx.state.sidebar);
        }
    }

    refresh_remote_tree(ctx);
}

/// Re-ask the session server for tree data whose git state may have gone stale.
///
/// Keyed on `git_refresh_token`, the same signal the local tree refreshes on, so this fires once
/// per root change or completed command rather than per message. Already-known directories are
/// re-requested in place rather than cleared, so the tree does not flash back to loading rows.
fn refresh_remote_tree(ctx: &mut Context<HyprmuxApp>) {
    if ctx.state.current().remote_host.is_none() {
        return;
    }
    let token = ctx.state.sidebar.git_refresh_token;
    if token == ctx.state.sidebar.tree_server_token {
        return;
    }
    let Some(root) = ctx.state.sidebar.tree_cwd.clone() else {
        return;
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    if !client.supports_file_tree() {
        return;
    }
    ctx.state.sidebar.tree_server_token = token;
    client.list_changes(root);
    let known: Vec<String> = ctx
        .state
        .sidebar
        .tree_listings
        .iter()
        .map(|listing| listing.path.to_string())
        .collect();
    for path in known {
        if ctx.state.sidebar.tree_pending.insert(path.clone()) {
            client.list_directory(path, true);
        }
    }
}

fn command_rows(
    result: std::io::Result<crate::platform::command::CommandOutput>,
) -> Vec<SidebarCommandRow> {
    let output = match result {
        Ok(output) => output,
        Err(error) => return vec![error_row(&format!("command failed: {error}"))],
    };
    if output.timed_out {
        return vec![error_row("command timed out after 5 seconds")];
    }
    let mut rows = text_rows(&output.stderr, true);
    if output.status != Some(0) && !rows.iter().any(|row| row.error) {
        rows.push(error_row(&format!(
            "command exited with status {}",
            output
                .status
                .map_or_else(|| "unknown".to_string(), |status| status.to_string())
        )));
    }
    rows.extend(text_rows(&output.stdout, false));
    rows.truncate(COMMAND_MAX_ROWS);
    rows
}

fn text_rows(bytes: &[u8], error: bool) -> Vec<SidebarCommandRow> {
    bytes
        .split(|byte| *byte == b'\n')
        .take(COMMAND_MAX_ROWS)
        .filter_map(|line| {
            let bounded = &line[..line.len().min(COMMAND_RAW_ROW_CHARS * 4)];
            let sanitized =
                tui_lipan::utils::sanitize_display_text(&String::from_utf8_lossy(bounded))
                    .trim()
                    .to_string();
            if sanitized.is_empty() {
                None
            } else if error {
                Some(error_row(&sanitized))
            } else {
                Some(row(&sanitized, false))
            }
        })
        .collect()
}

fn error_row(text: &str) -> SidebarCommandRow {
    row(&format!("Error: {text}"), true)
}

fn row(text: &str, error: bool) -> SidebarCommandRow {
    let raw: String = text.chars().take(COMMAND_RAW_ROW_CHARS).collect();
    let mut display: String = raw.chars().take(COMMAND_DISPLAY_ROW_CHARS).collect();
    if raw.chars().count() > COMMAND_DISPLAY_ROW_CHARS {
        display.push('…');
    }
    SidebarCommandRow {
        raw,
        display,
        error,
    }
}

pub(super) fn refresh_sessions(ctx: &mut Context<HyprmuxApp>, epoch: u64) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    // The loop is now live for this epoch, so the post-update chokepoint won't kick a duplicate.
    ctx.state.sidebar.sessions_refresh_armed_epoch = Some(epoch);
    // Only a *local* current session is excluded from the local scan; see
    // [`crate::state::State::local_current_session_name`].
    let current_name = ctx.state.local_current_session_name().map(str::to_string);
    let attached = crate::ops::session::attached_session_rows(&ctx.state);
    let remote_config = ctx.state.config.remote.clone();
    // On-demand: only *connected* hosts are contacted over ssh — those the user connected, or that
    // already hold an attachment. `Idle` is the disconnected state and is never probed, so the sweep
    // touches nothing the user has not asked for.
    //
    // A failed probe keeps being retried, because connecting is an intent the user expressed and a
    // failure is just this sweep's outcome. Dropping a failed host from the sweep meant one blip —
    // a laptop lid, a VPN reconnect — demoted a connected host to Offline permanently, with its
    // sessions gone until it was connected by hand again.
    let probe_targets: Vec<crate::session::remote::RemoteTarget> = ctx
        .state
        .hosts
        .iter()
        .filter(|host| {
            !matches!(host.probe, crate::state::HostProbe::Idle)
                || ctx
                    .state
                    .background
                    .values()
                    .chain(std::iter::once(ctx.state.current()))
                    .any(|attachment| attachment.remote_target.as_ref() == Some(&host.target))
        })
        .map(|host| host.target.clone())
        .collect();
    Update::with_command(Command::spawn(move |link: CommandLink<crate::Msg>| {
        let (rows, host_status) = crate::ops::session::discover_sidebar_sessions(
            current_name.as_deref(),
            &remote_config,
            probe_targets,
            attached,
        );
        link.send(crate::Msg::SidebarSessionsDiscovered {
            epoch,
            rows: rows.map_err(|error| error.to_string()),
            host_status,
        });
    }))
}

pub(super) fn sessions_discovered(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    rows: std::result::Result<Vec<crate::session::discovery::DiscoveredSession>, String>,
    host_status: Vec<(crate::session::remote::RemoteTarget, Option<String>)>,
) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    if let Ok(rows) = rows {
        ctx.state.sidebar.sessions = rows;
    }
    crate::ops::session::seed_host_registry(ctx);
    // Apply fresh probe outcomes after the reseed so they win over the carried-over state: a host
    // that answered reads as reached (online), a host that failed shows why on its group header.
    // A host that answered also refreshes its persisted session cache (written only when it
    // changed, so a steady 1.5s sweep does not churn the disk).
    for (target, status) in host_status {
        if status.is_none() {
            let label = target.display_label();
            let sessions: Vec<crate::session::CachedHostSession> = ctx
                .state
                .sidebar
                .sessions
                .iter()
                .filter(|entry| entry.remote_target.as_ref() == Some(&target))
                .map(|entry| crate::session::CachedHostSession {
                    name: entry.name.clone(),
                    ephemeral: entry.ephemeral,
                    panes: match &entry.status {
                        crate::session::discovery::DiscoveredSessionStatus::Running {
                            panes,
                            ..
                        } => *panes,
                        _ => 0,
                    },
                })
                .collect();
            // Only persist a real change, and never write an empty list for a host that never had
            // one cached — there is nothing to remember, and it keeps the sweep from creating a
            // file on the first probe of a session-less host.
            let known = ctx.state.host_session_cache.contains_key(&label);
            if (!sessions.is_empty() || known)
                && ctx.state.host_session_cache.get(&label) != Some(&sessions)
            {
                crate::session::record_host_sessions(&label, sessions.clone());
                ctx.state.host_session_cache.insert(label, sessions);
            }
        }
        if let Some(entry) = ctx.state.hosts.get_mut(&target) {
            entry.probe = match status {
                Some(error) => crate::state::HostProbe::Failed(error),
                None => crate::state::HostProbe::Reached,
            };
        }
    }
    Update::with_command(Command::after(
        SESSION_REFRESH_INTERVAL,
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::SidebarSessionsRefresh { epoch });
        },
    ))
}

pub(super) fn activate_session(
    ctx: &mut Context<HyprmuxApp>,
    entry: crate::session::discovery::DiscoveredSession,
) -> Update {
    crate::ops::session::activate_discovered_session(ctx, entry)
}

/// "Click to connect": bring a host online. Mark its probe in flight (so the header reads
/// "Connecting…" at once) and bump the sessions epoch so the post-update chokepoint re-sweeps with
/// this host now included, probing it immediately rather than at the next periodic tick.
pub(super) fn connect_host(
    ctx: &mut Context<HyprmuxApp>,
    target: crate::session::remote::RemoteTarget,
) -> Update {
    let Some(entry) = ctx.state.hosts.get_mut(&target) else {
        return Update::none();
    };
    if matches!(entry.probe, crate::state::HostProbe::InFlight) {
        return Update::none();
    }
    entry.probe = crate::state::HostProbe::InFlight;
    ctx.state.sidebar.sessions_epoch = ctx.state.sidebar.sessions_epoch.wrapping_add(1);
    Update::full()
}

/// "Click to disconnect": the first activation arms a confirmation (`armed` is what the row was
/// showing); the second commits it. Disconnecting closes any live attachments to the host — their
/// servers keep running — and returns it to offline.
pub(super) fn disconnect_host(
    ctx: &mut Context<HyprmuxApp>,
    target: crate::session::remote::RemoteTarget,
    armed: Option<crate::session::remote::RemoteTarget>,
) -> Update {
    if armed.as_ref() != Some(&target) {
        // Arm: the render turns the row red and reads "Click again to confirm".
        ctx.state.sidebar.pending_host_disconnect = Some(target);
        return crate::ops::confirm::arm(ctx);
    }
    // The update is the disconnect's *result*, not a repaint hint: when the current session lived on
    // this host it carries the command that lands the user somewhere else — an attach round-trip for
    // a fresh ephemeral, or a reconnect for the session being switched to. Dropping it left the UI
    // holding an attachment marked `Connecting` with a pending attach that nothing would ever
    // complete: an empty workspace, a phantom pane, and every later session activation refused as
    // "attach already in progress".
    let landed = crate::ops::session::disconnect_host(ctx, &target);
    ctx.state.sidebar.sessions_epoch = ctx.state.sidebar.sessions_epoch.wrapping_add(1);
    landed
}

pub(super) fn focus_pane(ctx: &mut Context<HyprmuxApp>, id: crate::state::PaneId) -> Update {
    if crate::ops::focus::focus_pane_anywhere(ctx, id) {
        Update::full()
    } else {
        Update::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Pane;
    use tui_lipan::TestBackend;

    fn row(text: &str) -> SidebarCommandRow {
        SidebarCommandRow {
            raw: text.to_string(),
            display: text.to_string(),
            error: false,
        }
    }

    #[test]
    fn unsplit_reorder_keeps_the_saved_panel_boundary() {
        let id = |name: &str| crate::config::SidebarTabId::new(name);
        let configured = vec![vec![id("agents")], vec![id("panes"), id("sessions")]];
        let displayed = vec![vec![id("panes"), id("agents"), id("sessions")]];

        assert_eq!(
            persisted_panel_ids(displayed, &configured, false),
            vec![vec![id("panes")], vec![id("agents"), id("sessions")]]
        );
    }

    fn discovered(name: &str) -> crate::session::discovery::DiscoveredSession {
        crate::session::discovery::DiscoveredSession {
            name: name.to_string(),
            ephemeral: false,
            host: None,
            remote_target: None,
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: 1,
                clients: 0,
                has_layout: true,
                created_from_profile: None,
            },
        }
    }

    /// Open the Sessions tab with its auto-refresh loop disarmed, for a test that drives discovery
    /// by hand.
    ///
    /// Armed, the loop kicks a *real* discovery sweep onto a background thread under the same
    /// epoch the test dispatches, and whichever of the two lands last wins - so an assertion about
    /// the resulting rows races the machine's actual sessions. Dropping the command link stops it:
    /// `ensure_sessions_refresh_armed` and `request_sessions_refresh` both need one to send
    /// through.
    ///
    /// The order matters. The mount delivers the link as a message, so it arrives during the first
    /// dispatch and reinstalls itself - and `command_link_ready` kicks an immediate sweep when it
    /// finds the tab already open. Settling the mount with the tab still closed is what makes the
    /// link there to drop.
    ///
    /// Only for tests that assert on discovered rows. Anything exercising a flow that sends
    /// through the link needs it left alone.
    fn open_sessions_tab_unswept(backend: &mut TestBackend<HyprmuxApp>, epoch: u64) {
        backend
            .dispatch(crate::Msg::SidebarPointerMoved(0))
            .expect("settle the mount");
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
        state.sidebar.sessions_epoch = epoch;
        state.command_link = None;
    }

    fn on_test_thread(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn sidebar test")
            .join()
            .expect("sidebar test completes");
    }

    /// Pretend an attach is already in flight so PTY-creating sidebar actions queue into
    /// `pending_spawns` instead of starting a real ephemeral (see `needs_session_for_pty`).
    fn hold_attach_open(backend: &mut TestBackend<HyprmuxApp>) {
        backend.state_mut().current_mut().pending_session_attach =
            Some(crate::state::PendingSessionAttach {
                epoch: backend.state().runtime_epoch,
                name: "test".to_string(),
                client: None,
                autostart: false,
                read_only: false,
                reconnect: false,
                remote_host: None,
                intent: crate::state::AttachIntent::Plain,
                left: None,
                parked_epoch: None,
            });
    }

    /// A scratch directory scoped to this process and test name, matching how the rest of the
    /// suite isolates filesystem fixtures. Removed first so a previous crashed run cannot leak in.
    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "hyprmux-sidebar-tree-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch dir");
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A repository whose `.git` is a plain file, the shape git uses for worktrees and submodules.
    /// Testing existence rather than directory-ness is what makes those work.
    #[test]
    fn repo_root_walks_ancestors_and_accepts_a_git_file() {
        let dir = ScratchDir::new("gitfile");
        let repo = dir.path().join("repo");
        let nested = repo.join("src/deep");
        std::fs::create_dir_all(&nested).expect("nested dirs");
        std::fs::write(repo.join(".git"), "gitdir: /elsewhere").expect("git file");

        let repo_str = repo.to_string_lossy().into_owned();
        assert_eq!(
            crate::platform::paths::discover_project_root(&nested.to_string_lossy()),
            Some(repo_str.clone())
        );
        assert_eq!(
            crate::platform::paths::discover_project_root(&repo_str),
            Some(repo_str.clone())
        );
        assert_eq!(
            crate::platform::paths::discover_project_root(&dir.path().to_string_lossy()),
            None
        );
        assert_eq!(
            crate::platform::paths::display_cwd(&nested.to_string_lossy()),
            std::path::Path::new("repo")
                .join("src/deep")
                .to_string_lossy()
        );
        assert_eq!(crate::platform::paths::display_cwd(&repo_str), "repo");
    }

    /// The roots follow the focused pane, and a repeated directory report — which a shell emits at
    /// every prompt — must not redo the ancestor walk or churn the git refresh token.
    #[test]
    fn tree_roots_track_the_focused_pane_and_settle_when_unchanged() {
        let dir = ScratchDir::new("roots");
        let repo = dir.path().join("repo");
        let nested = repo.join("src");
        std::fs::create_dir_all(&nested).expect("nested dirs");
        std::fs::create_dir_all(repo.join(".git")).expect("git dir");
        let nested = nested.to_string_lossy().into_owned();
        let repo = repo.to_string_lossy().into_owned();
        let outside = dir.path().to_string_lossy().into_owned();

        on_test_thread(move || {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let pane = backend.state().current().workspaces[0].panes[0].id;
            {
                let state = backend.state_mut();
                state.current_mut().focused_pane = Some(pane);
                state.current_mut().workspaces[0].focused_pane = Some(pane);
                state.current_mut().workspaces[0].panes[0].terminal.cwd = Some(nested.clone());
            }
            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("sync runs after any message");
            assert_eq!(backend.state().sidebar.tree_cwd.as_deref(), Some(&*nested));
            assert_eq!(backend.state().sidebar.tree_repo.as_deref(), Some(&*repo));
            let settled = backend.state().sidebar.git_refresh_token;
            assert!(settled > 0, "resolving a root schedules a git refresh");

            // Same directory reported again: no walk, no refresh.
            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("repeat sync");
            assert_eq!(backend.state().sidebar.git_refresh_token, settled);

            // Leaving the repository clears the repo root but keeps the working directory.
            backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .cwd = Some(outside.clone());
            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("cwd change sync");
            assert_eq!(backend.state().sidebar.tree_cwd.as_deref(), Some(&*outside));
            assert_eq!(backend.state().sidebar.tree_repo, None);
            assert!(backend.state().sidebar.git_refresh_token > settled);
        });
    }

    /// Git status is refreshed on the edge into `Completed` — the moment a command finished
    /// changing the working tree — rather than on a timer.
    #[test]
    fn finishing_a_command_refreshes_git_status_once() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let pane = backend.state().current().workspaces[0].panes[0].id;
            {
                let state = backend.state_mut();
                state.current_mut().focused_pane = Some(pane);
                state.current_mut().workspaces[0].focused_pane = Some(pane);
                state.current_mut().workspaces[0].panes[0]
                    .terminal
                    .command_phase = crate::session::protocol::PaneCommandPhase::Executing;
            }
            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("observe executing");
            let running = backend.state().sidebar.git_refresh_token;

            backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .command_phase = crate::session::protocol::PaneCommandPhase::Completed {
                exit_status: Some(0),
            };
            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("observe completion");
            let finished = backend.state().sidebar.git_refresh_token;
            assert_eq!(finished, running + 1, "one refresh per finished command");

            // Still completed on the next message: no repeat refresh.
            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 0, index: 0 })
                .expect("steady state");
            assert_eq!(backend.state().sidebar.git_refresh_token, finished);
        });
    }

    /// Activating a file runs the tab's action; activating a directory only expands it in the
    /// widget and must not run the action, and a stale config epoch drops the click entirely. A
    /// dropped activation returns `Update::none()`, so `dispatch` reports no redraw.
    #[test]
    fn tree_activation_runs_for_files_and_skips_directories_and_stale_clicks() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Tree {
                view: crate::config::SidebarTreeView::Files,
                config: crate::config::SidebarTreeConfig::for_view(
                    crate::config::SidebarTreeView::Files,
                ),
            }];
            backend.state_mut().sidebar.config_epoch = 6;
            let activate = |backend: &mut TestBackend<HyprmuxApp>, is_dir: bool, epoch: u64| {
                backend
                    .dispatch(crate::Msg::SidebarTreeActivate {
                        config_epoch: epoch,
                        tab_id: SidebarTabId::new("files"),
                        path: "/repo/src/main.rs".to_string(),
                        is_dir,
                    })
                    .expect("tree click")
            };

            // A file activation runs the action (a send, which redraws); a directory and a stale
            // click are both dropped without running anything.
            assert!(activate(&mut backend, false, 6), "file runs the action");
            assert!(!activate(&mut backend, true, 6), "directory only expands");
            assert!(!activate(&mut backend, false, 5), "stale epoch is dropped");
        });
    }

    /// A `run` action opens a pane whose command is untouched, with the activated path handed over
    /// as `HYPRMUX_FILE`. This is what lets a diff viewer be scoped to the clicked file without the
    /// filename ever entering the command line: a repository can contain a file named
    /// `; rm -rf ~`, and the spawned command string must not be able to carry it.
    #[test]
    fn tree_run_actions_pass_the_path_as_env_never_in_the_command() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let mut config =
                crate::config::SidebarTreeConfig::for_view(crate::config::SidebarTreeView::Changes);
            config.on_click = Some(UserCommandAction::run("git diff -- \"$HYPRMUX_FILE\""));
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Tree {
                view: crate::config::SidebarTreeView::Changes,
                config,
            }];
            backend.state_mut().sidebar.config_epoch = 1;
            backend.state_mut().current_mut().pending_spawns.clear();
            hold_attach_open(&mut backend);

            let hostile = "/repo/; rm -rf ~/.rs";
            backend
                .dispatch(crate::Msg::SidebarTreeActivate {
                    config_epoch: 1,
                    tab_id: SidebarTabId::new("git"),
                    path: hostile.to_string(),
                    is_dir: false,
                })
                .expect("file click");

            let spawn = backend
                .state()
                .current()
                .pending_spawns
                .last()
                .cloned()
                .expect("run action queues a pane spawn");
            // The command is exactly what the config said — the path is nowhere in it.
            assert_eq!(
                spawn.command.as_deref(),
                Some("git diff -- \"$HYPRMUX_FILE\"")
            );
            assert!(
                !spawn.command.as_deref().unwrap().contains("rm -rf"),
                "the filename never reaches the command line"
            );
            // It arrives as environment instead, verbatim.
            assert!(
                spawn
                    .env
                    .iter()
                    .any(|(key, value)| key == "HYPRMUX_FILE" && value == hostile),
                "the activated path is handed over as HYPRMUX_FILE: {:?}",
                spawn.env
            );
        });
    }

    /// `{path}` is substituted only into `send` text; a `run`/`popup` command is left as-is because
    /// config validation already rejected the placeholder there.
    #[test]
    fn path_substitution_is_literal_and_send_only() {
        assert_eq!(
            substitute(
                &UserCommandAction::Send("{path}".into()),
                "{path}",
                "/repo/src/main.rs"
            ),
            UserCommandAction::Send("/repo/src/main.rs".into())
        );
        assert_eq!(
            substitute(
                &UserCommandAction::run("ls {path}"),
                "{path}",
                "/etc/passwd"
            ),
            UserCommandAction::run("ls {path}")
        );
    }

    #[test]
    fn sidebar_focus_switches_workspace_and_clears_activity() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                let mut pane = Pane::new(
                    2,
                    100,
                    FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 40.0,
                        h: 20.0,
                    },
                );
                pane.activity.has_unseen_output = true;
                backend.state_mut().current_mut().workspaces[1]
                    .panes
                    .push(pane);
                backend
                    .dispatch(crate::Msg::SidebarFocusPane(2))
                    .expect("focus sidebar pane");
                assert_eq!(backend.state().current().active_workspace, 1);
                assert_eq!(backend.state().current().focused_pane, Some(2));
                assert!(
                    !backend.state().current().workspaces[1].panes[0]
                        .activity
                        .has_unseen_output
                );
            })
            .expect("spawn sidebar focus test")
            .join()
            .expect("sidebar focus test completes");
    }

    #[test]
    fn stale_session_results_are_ignored_after_close_switch_and_reload_epochs() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 10;
            }
            let stale = vec![discovered("old")];

            backend.state_mut().sidebar_visible = false;
            backend.state_mut().sidebar.invalidate_sessions();
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 10,
                    rows: Ok(stale.clone()),
                    host_status: Vec::new(),
                })
                .expect("stale close result");
            assert!(backend.state().sidebar.sessions.is_empty());

            backend.state_mut().sidebar_visible = true;
            backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
            backend.state_mut().sidebar.invalidate_sessions();
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 11,
                    rows: Ok(stale.clone()),
                    host_status: Vec::new(),
                })
                .expect("stale tab result");
            assert!(backend.state().sidebar.sessions.is_empty());

            backend
                .state_mut()
                .sidebar
                .reconcile(&crate::config::SidebarConfig::default());
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 12,
                    rows: Ok(stale),
                    host_status: Vec::new(),
                })
                .expect("stale reload result");
            assert!(backend.state().sidebar.sessions.is_empty());
        });
    }

    #[test]
    fn current_session_results_apply() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            open_sessions_tab_unswept(&mut backend, 7);
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 7,
                    rows: Ok(vec![discovered("dev")]),
                    host_status: Vec::new(),
                })
                .expect("current result");
            assert_eq!(backend.state().sidebar.sessions, vec![discovered("dev")]);
        });
    }

    /// The host title and description form one connect/disconnect row. Connecting marks the host in
    /// flight; disconnecting takes two activations of that same row.
    #[test]
    fn host_connect_and_two_click_disconnect() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
            {
                let state = backend.state_mut();
                state.config.remote.hosts.insert(
                    "winvm".to_string(),
                    crate::config::RemoteHostConfig::default(),
                );
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            }
            // Seed the host registry (offline) the way opening the tab does.
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("settle");
            assert!(
                backend.state().hosts.get(&target).is_some(),
                "the configured host is seeded into the registry"
            );

            // `row_activate` rebuilds the rows from the current state before the post-update sweep
            // repopulates local sessions, so clearing them here makes the layout deterministic:
            //   0 LOCAL header · 1 "No local sessions" · 2 "+ New session" · 3 spacer
            //   4 WINVM / "Click to connect" · 5 spacer · 6 "Connect a host…"
            backend.state_mut().sidebar.sessions.clear();
            backend
                .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
                .expect("connect through host row");
            assert_eq!(
                backend.state().hosts.get(&target).unwrap().probe,
                crate::state::HostProbe::InFlight,
                "connecting marks the host in flight"
            );

            // Now online, the same row reads "Click to disconnect". Force the host reached and arm.
            backend.state_mut().hosts.get_mut(&target).unwrap().probe =
                crate::state::HostProbe::Reached;
            //   … 4 WINVM / "Click to disconnect" · 5 "No sessions here yet" · 6 "+ New…"
            backend.state_mut().sidebar.sessions.clear();
            backend
                .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
                .expect("arm disconnect");
            assert_eq!(
                backend.state().sidebar.pending_host_disconnect.as_ref(),
                Some(&target),
                "first click arms the confirmation"
            );
            backend.state_mut().hosts.get_mut(&target).unwrap().probe =
                crate::state::HostProbe::Reached;
            backend.state_mut().sidebar.sessions.clear();
            backend
                .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
                .expect("confirm disconnect");
            assert_eq!(
                backend.state().hosts.get(&target).unwrap().probe,
                crate::state::HostProbe::Idle,
                "confirming disconnect returns the host to offline"
            );
            assert!(backend.state().sidebar.pending_host_disconnect.is_none());
        });
    }

    /// The ✕ on a pane row takes two clicks: the first arms a confirmation, the second kills the
    /// pane. Clicking the row body in between abandons the arming rather than carrying it, so a
    /// confirmation can never be committed by a gesture that meant something else.
    #[test]
    fn the_close_affordance_takes_two_clicks_and_is_disarmed_by_acting_elsewhere() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
            }
            let id = backend
                .state()
                .current()
                .focused_pane
                .expect("the default attachment has a focused pane");
            // Row 0 is the workspace header; row 1 is the only pane.
            let pane_row = 1;

            backend
                .dispatch(crate::Msg::SidebarRowClose {
                    panel: 0,
                    index: pane_row,
                })
                .expect("arm the close");
            assert_eq!(
                backend.state().sidebar.pending_row_close,
                Some(crate::state::SidebarClose::Pane(id)),
                "the first click arms the confirmation"
            );

            // Activating the row instead of confirming abandons the arming.
            backend
                .dispatch(crate::Msg::SidebarRowActivate {
                    panel: 0,
                    index: pane_row,
                })
                .expect("activate the row");
            assert!(
                backend.state().sidebar.pending_row_close.is_none(),
                "acting on the row disarms the pending close"
            );
            assert!(
                crate::pane_lifecycle::find_pane(backend.state(), id)
                    .is_some_and(|pane| !pane.closing),
                "an abandoned confirmation leaves the pane alone"
            );

            backend
                .dispatch(crate::Msg::SidebarRowClose {
                    panel: 0,
                    index: pane_row,
                })
                .expect("re-arm the close");
            backend
                .dispatch(crate::Msg::SidebarRowClose {
                    panel: 0,
                    index: pane_row,
                })
                .expect("confirm the close");
            assert!(
                backend.state().sidebar.pending_row_close.is_none(),
                "committing consumes the arming"
            );
            assert!(
                crate::pane_lifecycle::find_pane(backend.state(), id)
                    .is_none_or(|pane| pane.closing),
                "the confirming click closes the pane"
            );
        });
    }

    /// An arming lapses on its own after [`crate::ops::confirm::CONFIRM_WINDOW`]. The expiry is
    /// matched by token rather than by wall time here: an expiry belonging to an arming that has
    /// already been replaced must leave the replacement alone, which is the case a bare timer would
    /// get wrong.
    #[test]
    fn a_lapsed_confirmation_clears_itself_and_never_disarms_a_later_one() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
            }
            let pane_row = 1;
            backend
                .dispatch(crate::Msg::SidebarRowClose {
                    panel: 0,
                    index: pane_row,
                })
                .expect("arm the close");
            let armed_epoch = backend.state().confirm_epoch;
            assert!(backend.state().sidebar.pending_row_close.is_some());

            // Re-arming (here, the same row after a disarm) advances the token, so the first
            // arming's expiry is now stale and must not clear what replaced it.
            backend
                .dispatch(crate::Msg::SidebarRowActivate {
                    panel: 0,
                    index: pane_row,
                })
                .expect("disarm");
            backend
                .dispatch(crate::Msg::SidebarRowClose {
                    panel: 0,
                    index: pane_row,
                })
                .expect("re-arm");
            assert_ne!(backend.state().confirm_epoch, armed_epoch);
            backend
                .dispatch(crate::Msg::ConfirmationExpired(armed_epoch))
                .expect("stale expiry");
            assert!(
                backend.state().sidebar.pending_row_close.is_some(),
                "a stale expiry leaves the current arming alone"
            );

            let current = backend.state().confirm_epoch;
            backend
                .dispatch(crate::Msg::ConfirmationExpired(current))
                .expect("the window lapses");
            assert!(
                backend.state().sidebar.pending_row_close.is_none(),
                "the arming lapses on its own"
            );
        });
    }

    /// Hover drives the ✕, and the row plus the ✕ nested inside it both report against the same
    /// index. Moving between them fires leave-then-enter for that one index, which has to settle on
    /// "hovered" rather than cancelling itself out.
    #[test]
    fn hover_survives_the_pointer_crossing_into_the_close_affordance() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend
                .dispatch(crate::Msg::SidebarRowHover {
                    panel: 0,
                    index: 1,
                    hovered: true,
                })
                .expect("enter the row");
            assert_eq!(backend.state().sidebar.panels[0].hovered_row, Some(1));

            // Crossing onto the ✕: the row leaves, then the ✕ enters, both naming row 1.
            backend
                .dispatch(crate::Msg::SidebarRowHover {
                    panel: 0,
                    index: 1,
                    hovered: false,
                })
                .expect("leave the row");
            backend
                .dispatch(crate::Msg::SidebarRowHover {
                    panel: 0,
                    index: 1,
                    hovered: true,
                })
                .expect("enter the ✕");
            assert_eq!(
                backend.state().sidebar.panels[0].hovered_row,
                Some(1),
                "the ✕ stays revealed under the pointer"
            );

            // A leave naming a row that is no longer the hovered one is stale and must not clear it.
            backend
                .dispatch(crate::Msg::SidebarRowHover {
                    panel: 0,
                    index: 4,
                    hovered: false,
                })
                .expect("stale leave");
            assert_eq!(backend.state().sidebar.panels[0].hovered_row, Some(1));

            backend
                .dispatch(crate::Msg::SidebarRowHover {
                    panel: 0,
                    index: 1,
                    hovered: false,
                })
                .expect("leave the sidebar");
            assert_eq!(backend.state().sidebar.panels[0].hovered_row, None);
        });
    }

    /// After a session switch bumps the sessions epoch — which kills the old refresh loop — the
    /// post-update chokepoint re-arms it while the tab is open, so the Sessions tab keeps updating
    /// instead of freezing on "No local sessions" until it is reopened.
    #[test]
    fn bumping_the_sessions_epoch_rearms_the_refresh_loop() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            // Let init settle so the command link is wired (the re-arm sends through it).
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("settle init");
            assert!(
                backend.state().command_link.is_some(),
                "command link should be wired after init"
            );
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
                // Simulate a session switch: epoch advanced, old loop's armed epoch left behind.
                state.sidebar.sessions_epoch = 99;
                state.sidebar.sessions_refresh_armed_epoch = Some(98);
            }
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("post-update runs");
            assert_eq!(
                backend.state().sidebar.sessions_refresh_armed_epoch,
                Some(99),
                "the refresh loop must re-arm for the new epoch"
            );

            // Leaving the sessions tab clears the arm so it re-arms cleanly on return.
            backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("tab left");
            assert_eq!(backend.state().sidebar.sessions_refresh_armed_epoch, None);
        });
    }

    /// Disconnecting the host the *current* session lives on must not quit and must not auto-attach.
    /// A parked local session remains as a choice, so the client lands sessionless with the picker
    /// open. The regression underneath: the sidebar dropped the update `disconnect_host` returns,
    /// so a hop command never ran and left a pending attach nothing would complete.
    #[test]
    fn disconnecting_the_current_host_opens_the_picker_instead_of_auto_attaching() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
            {
                let state = backend.state_mut();
                state.config.remote.hosts.insert(
                    "winvm".to_string(),
                    crate::config::RemoteHostConfig::default(),
                );
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            }
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("settle");

            {
                let state = backend.state_mut();
                // The local session used before hopping onto the remote one: parked, still live.
                let mut incoming = crate::state::fresh_default_attachment(&state.config);
                incoming.session_name = Some("build".to_string());
                incoming.session_attached = true;
                incoming.connection = crate::state::ConnectionState::Connected;
                incoming.remote_target = Some(target.clone());
                incoming.remote_host = Some("winvm".to_string());
                state.current_mut().session_name = Some("dev".to_string());
                state.current_mut().session_attached = true;
                // Settled, not mid-connect: the launch attach this test never completes would
                // otherwise leave it pending.
                state.current_mut().pending_session_attach = None;
                state.current_mut().connection = crate::state::ConnectionState::Connected;
                let parked_epoch = state.runtime_epoch;
                state.park_current(parked_epoch, incoming);
                state.runtime_epoch = state.mint_attachment_id();

                // Online, and armed, so the next activation of the host row commits the disconnect.
                state.hosts.get_mut(&target).unwrap().probe = crate::state::HostProbe::Reached;
                state.sidebar.pending_host_disconnect = Some(target.clone());
                state.sidebar.sessions.clear();
            }

            // Row 4 is the WINVM host row — see `host_connect_and_two_click_disconnect`.
            backend
                .dispatch(crate::Msg::SidebarRowActivate { panel: 0, index: 4 })
                .expect("confirm disconnect");

            let state = backend.state();
            assert!(
                state.is_launcher(),
                "disconnecting the active host leaves the foreground sessionless"
            );
            assert!(
                state.show_session_picker,
                "the parked local session remains as a choice, so the picker opens"
            );
            assert!(
                state.background.values().any(|attachment| {
                    attachment.session_name.as_deref() == Some("dev")
                        && attachment.remote_target.is_none()
                }),
                "the local parked session stays retained for an explicit picker choice"
            );
            assert!(
                state.current().pending_session_attach.is_none(),
                "nothing auto-attaches after a disconnect"
            );
        });
    }

    /// Killing the session on screen must not auto-attach a parked session and must not mint a
    /// fresh ephemeral. The killed one is gone; other choices stay available via the picker.
    #[test]
    fn killing_the_current_session_opens_the_picker_instead_of_auto_attaching() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
            }
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("settle");
            {
                let state = backend.state_mut();
                // `dev` is the session used before `build`: parked, settled, still live.
                let mut incoming = crate::state::fresh_default_attachment(&state.config);
                incoming.session_name = Some("build".to_string());
                incoming.session_attached = true;
                state.current_mut().session_name = Some("dev".to_string());
                state.current_mut().session_attached = true;
                state.current_mut().pending_session_attach = None;
                state.current_mut().connection = crate::state::ConnectionState::Connected;
                let parked_epoch = state.runtime_epoch;
                state.park_current(parked_epoch, incoming);
                state.runtime_epoch = state.mint_attachment_id();
                //   0 LOCAL header · 1 `build` · 2 "+ New session"
                state.sidebar.sessions = vec![discovered("build")];
                // Hide the sidebar so the recurring sweep stops replacing this fixed row list with
                // whatever sessions happen to be running on the machine the test runs on. Row
                // activation reads the active tab, not visibility, so the rows still resolve.
                state.sidebar_visible = false;
            }

            // The ✕ on the current session's row: arm, then confirm.
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
                .expect("arm the kill");
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
                .expect("confirm the kill");

            let state = backend.state();
            assert!(
                state.is_launcher(),
                "killing the active session leaves the foreground sessionless"
            );
            assert!(
                state.show_session_picker,
                "a parked session remains as a choice, so the picker opens"
            );
            assert!(
                state
                    .background
                    .values()
                    .any(|attachment| { attachment.session_name.as_deref() == Some("dev") }),
                "the parked session stays retained for an explicit picker choice"
            );
            assert!(
                state.current().pending_session_attach.is_none(),
                "nothing auto-attaches after a kill"
            );
        });
    }

    /// A host the user connected keeps being swept even after a probe fails. Connecting is an
    /// intent; a failure is one sweep's outcome. Dropping failed hosts from the sweep meant a single
    /// blip demoted a connected host to Offline for good, taking its sessions with it —
    /// only `Idle`, the disconnected state, is left alone.
    #[test]
    fn a_connected_host_is_still_swept_after_a_probe_fails() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
            {
                let state = backend.state_mut();
                state.config.remote.hosts.insert(
                    "winvm".to_string(),
                    crate::config::RemoteHostConfig::default(),
                );
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 7;
            }
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("settle");
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 7,
                    rows: Ok(Vec::new()),
                    host_status: vec![(target.clone(), Some("connection reset".to_string()))],
                })
                .expect("failed probe");
            assert!(matches!(
                backend.state().hosts.get(&target).unwrap().probe,
                crate::state::HostProbe::Failed(_)
            ));

            // The next sweep must still contact it, so the failure can clear on its own.
            assert!(
                probe_targets_for_test(backend.state()).contains(&target),
                "a failed-but-connected host stays in the sweep"
            );

            // Disconnecting is what takes it out.
            backend.state_mut().hosts.get_mut(&target).unwrap().probe =
                crate::state::HostProbe::Idle;
            assert!(!probe_targets_for_test(backend.state()).contains(&target));
        });
    }

    /// Mirrors the `probe_targets` filter in [`refresh_sessions`].
    fn probe_targets_for_test(
        state: &crate::state::State,
    ) -> Vec<crate::session::remote::RemoteTarget> {
        state
            .hosts
            .iter()
            .filter(|host| {
                !matches!(host.probe, crate::state::HostProbe::Idle)
                    || state
                        .background
                        .values()
                        .chain(std::iter::once(state.current()))
                        .any(|attachment| attachment.remote_target.as_ref() == Some(&host.target))
            })
            .map(|host| host.target.clone())
            .collect()
    }

    /// A probe failure records the reason on the host's registry entry (surfaced inline on its
    /// group header) as a failed probe; a subsequent success flips it to reached. The host is
    /// seeded from config by the handler, so the registry entry exists to receive the outcome.
    #[test]
    fn host_probe_errors_are_recorded_then_cleared() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let target = crate::session::remote::RemoteTarget::Alias("prod".to_string());
            {
                let state = backend.state_mut();
                state.config.remote.hosts.insert(
                    "prod".to_string(),
                    crate::config::RemoteHostConfig::default(),
                );
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 3;
            }
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 3,
                    rows: Ok(Vec::new()),
                    host_status: vec![(target.clone(), Some("no route to host".to_string()))],
                })
                .expect("failed probe");
            assert_eq!(
                backend.state().hosts.get(&target).unwrap().probe.error(),
                Some("no route to host")
            );

            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 3,
                    rows: Ok(Vec::new()),
                    host_status: vec![(target.clone(), None)],
                })
                .expect("recovered probe");
            assert_eq!(
                backend.state().hosts.get(&target).unwrap().probe,
                crate::state::HostProbe::Reached
            );
        });
    }

    #[test]
    fn command_rows_are_sanitized_bounded_and_keep_raw_separate_from_display() {
        let long = "x".repeat(COMMAND_RAW_ROW_CHARS + 20);
        let stdout = format!("\x1b[31mred\x1b[0m\n{long}\n{}", "row\n".repeat(600));
        let rows = command_rows(Ok(crate::platform::command::CommandOutput {
            stdout: stdout.into_bytes(),
            stderr: Vec::new(),
            status: Some(0),
            timed_out: false,
        }));
        assert_eq!(rows.len(), COMMAND_MAX_ROWS);
        assert_eq!(rows[0].raw, "red");
        assert_eq!(rows[1].raw.chars().count(), COMMAND_RAW_ROW_CHARS);
        assert_eq!(
            rows[1].display.chars().count(),
            COMMAND_DISPLAY_ROW_CHARS + 1
        );
    }

    #[test]
    fn command_errors_cover_timeout_nonzero_stderr_and_spawn_failure() {
        let timeout = command_rows(Ok(crate::platform::command::CommandOutput {
            stdout: b"ignored".to_vec(),
            stderr: Vec::new(),
            status: None,
            timed_out: true,
        }));
        assert!(timeout[0].error && timeout[0].raw.contains("timed out"));

        let nonzero = command_rows(Ok(crate::platform::command::CommandOutput {
            stdout: Vec::new(),
            stderr: b"\x1b[31mbad\x1b[0m".to_vec(),
            status: Some(7),
            timed_out: false,
        }));
        assert_eq!(nonzero[0].raw, "Error: bad");

        let spawn = command_rows(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        )));
        assert!(spawn[0].error && spawn[0].raw.contains("missing"));
    }

    #[test]
    fn line_substitution_is_literal_and_send_only() {
        assert_eq!(
            resolve_row_action(
                &UserCommandAction::Send("open [{line}] {line}\n".to_string()),
                "$(touch /tmp/nope); 'quoted'"
            ),
            UserCommandAction::Send(
                "open [$(touch /tmp/nope); 'quoted'] $(touch /tmp/nope); 'quoted'\n".to_string()
            )
        );
        assert_eq!(
            resolve_row_action(&UserCommandAction::run("show fixed".to_string()), "ignored"),
            UserCommandAction::run("show fixed".to_string())
        );
    }

    /// Build a one-entry launcher tab and activate it, returning the spawn request it queued. An
    /// in-flight attach (`hold_attach_open`) keeps the request in `pending_spawns` so the test can
    /// assert the payload without starting a real ephemeral session.
    fn activate_launcher(
        action: UserCommandAction,
        cwd: Option<&str>,
    ) -> crate::state::PendingPaneSpawn {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        let id = SidebarTabId::new("launch");
        backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Launcher {
            name: id.clone(),
            label: "Launch".to_string(),
            entries: vec![crate::config::SidebarLauncherEntry {
                label: "Entry".to_string(),
                action,
            }],
        }];
        backend.state_mut().sidebar.config_epoch = 1;
        if let Some(cwd) = cwd {
            let focused = backend.state().current().workspaces[0].panes[0].id;
            backend.state_mut().current_mut().workspaces[0].focused_pane = Some(focused);
            backend.state_mut().current_mut().focused_pane = Some(focused);
            backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .cwd = Some(cwd.to_string());
        }
        backend.state_mut().current_mut().pending_spawns.clear();
        hold_attach_open(&mut backend);
        backend
            .dispatch(crate::Msg::SidebarLauncherActivate {
                config_epoch: 1,
                tab_id: id,
                entry_index: 0,
            })
            .expect("launcher click");
        backend
            .state()
            .current()
            .pending_spawns
            .last()
            .cloned()
            .expect("launcher click queues a spawn")
    }

    /// A launcher `run` opens where the focused pane is, not where the session server was started:
    /// `cargo build` means "build the project I am looking at". It also holds the pane after the
    /// command exits, so a build that fails in milliseconds leaves its errors on screen.
    #[test]
    fn launcher_run_inherits_the_focused_pane_cwd_and_holds_the_pane_open() {
        on_test_thread(|| {
            let spawn = activate_launcher(
                UserCommandAction::run("cargo build"),
                Some("/home/x/work/hyprmux"),
            );
            assert_eq!(spawn.command.as_deref(), Some("cargo build"));
            assert_eq!(spawn.cwd.as_deref(), Some("/home/x/work/hyprmux"));
            assert!(spawn.keep_open);
        });
    }

    /// The popup carries the same two properties. Its `keep_open` used to be dropped between the
    /// identity and the wire request, so a popup running a fast command flashed and vanished.
    #[test]
    fn launcher_popup_inherits_cwd_and_keeps_its_identity_keep_open() {
        on_test_thread(|| {
            let spawn = activate_launcher(UserCommandAction::popup("date"), Some("/home/x/notes"));
            assert_eq!(spawn.pane_id, crate::state::POPUP_PANE_ID);
            assert_eq!(spawn.cwd.as_deref(), Some("/home/x/notes"));
            assert!(
                spawn.keep_open,
                "the wire request must agree with the pane identity"
            );

            let opt_out = activate_launcher(
                UserCommandAction::Popup {
                    command: "fzf".to_string(),
                    keep_open: false,
                },
                None,
            );
            assert!(!opt_out.keep_open);
        });
    }

    #[test]
    fn launcher_click_revalidates_config_epoch_tab_and_index() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let id = SidebarTabId::new("launch");
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Launcher {
                name: id.clone(),
                label: "Launch".to_string(),
                entries: vec![crate::config::SidebarLauncherEntry {
                    label: "Run".to_string(),
                    action: UserCommandAction::run("printf safe".to_string()),
                }],
            }];
            backend.state_mut().sidebar.config_epoch = 4;
            let initial = backend.state().current().workspaces[0].panes.len();
            backend
                .dispatch(crate::Msg::SidebarLauncherActivate {
                    config_epoch: 3,
                    tab_id: id.clone(),
                    entry_index: 0,
                })
                .expect("stale launcher click");
            backend
                .dispatch(crate::Msg::SidebarLauncherActivate {
                    config_epoch: 4,
                    tab_id: SidebarTabId::new("other"),
                    entry_index: 0,
                })
                .expect("wrong tab click");
            assert_eq!(backend.state().current().workspaces[0].panes.len(), initial);
            backend
                .dispatch(crate::Msg::SidebarLauncherActivate {
                    config_epoch: 4,
                    tab_id: id,
                    entry_index: 0,
                })
                .expect("current launcher click");
            assert_eq!(
                backend.state().current().workspaces[0].panes.len(),
                initial + 1
            );
        });
    }

    #[test]
    fn command_click_rejects_stale_output_epoch_and_changed_raw_line() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let id = SidebarTabId::new("rows");
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Command {
                name: id.clone(),
                label: "Rows".to_string(),
                command: "printf row".to_string(),
                interval_secs: 30,
                on_click: Some(UserCommandAction::run("printf fixed".to_string())),
            }];
            backend.state_mut().sidebar.config_epoch = 2;
            backend.state_mut().sidebar.command_output.insert(
                id.clone(),
                SidebarCommandOutput {
                    epoch: 9,
                    rows: vec![row("safe")],
                },
            );
            let initial = backend.state().current().workspaces[0].panes.len();
            for (output_epoch, line) in [(8, "safe"), (9, "changed")] {
                backend
                    .dispatch(crate::Msg::SidebarCommandRowActivate {
                        config_epoch: 2,
                        tab_id: id.clone(),
                        output_epoch,
                        line: line.to_string(),
                    })
                    .expect("stale row click");
            }
            assert_eq!(backend.state().current().workspaces[0].panes.len(), initial);
            backend
                .dispatch(crate::Msg::SidebarCommandRowActivate {
                    config_epoch: 2,
                    tab_id: id,
                    output_epoch: 9,
                    line: "safe".to_string(),
                })
                .expect("current row click");
            assert_eq!(
                backend.state().current().workspaces[0].panes.len(),
                initial + 1
            );
        });
    }

    #[test]
    fn stale_command_result_clears_only_its_run_and_cannot_replace_output() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let id = SidebarTabId::new("rows");
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(id.clone());
                state.sidebar.command_epoch = 8;
                state.sidebar.command_in_flight.insert(id.clone(), 7);
                state.sidebar.command_output.insert(
                    id.clone(),
                    SidebarCommandOutput {
                        epoch: 3,
                        rows: vec![row("current")],
                    },
                );
            }
            backend
                .dispatch(crate::Msg::SidebarCommandOutput {
                    epoch: 7,
                    tab_id: id.clone(),
                    rows: vec![row("stale")],
                })
                .expect("stale command result");
            assert!(!backend.state().sidebar.command_in_flight.contains_key(&id));
            assert_eq!(
                backend.state().sidebar.command_output[&id].rows,
                vec![row("current")]
            );
        });
    }

    #[test]
    fn polling_rejects_hidden_inactive_stale_and_overlapping_runs() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let id = SidebarTabId::new("rows");
            {
                let state = backend.state_mut();
                state.config.sidebar.tabs = vec![SidebarTab::Command {
                    name: id.clone(),
                    label: "Rows".to_string(),
                    command: "sleep 1".to_string(),
                    interval_secs: 5,
                    on_click: None,
                }];
                state.sidebar.panels[0].active_tab = Some(id.clone());
                state.sidebar.command_epoch = 6;
            }
            for (visible, epoch, active) in
                [(false, 6, "rows"), (true, 5, "rows"), (true, 6, "other")]
            {
                backend.state_mut().sidebar_visible = visible;
                backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new(active));
                backend
                    .dispatch(crate::Msg::SidebarCommandPoll {
                        epoch,
                        tab_id: id.clone(),
                    })
                    .expect("guarded poll");
                assert!(!backend.state().sidebar.command_in_flight.contains_key(&id));
            }

            let state = backend.state_mut();
            state.sidebar_visible = true;
            state.sidebar.panels[0].active_tab = Some(id.clone());
            state.sidebar.command_in_flight.insert(id.clone(), 5);
            backend
                .dispatch(crate::Msg::SidebarCommandPoll {
                    epoch: 6,
                    tab_id: id.clone(),
                })
                .expect("overlap guard");
            assert_eq!(backend.state().sidebar.command_in_flight.get(&id), Some(&5));
            backend.state_mut().sidebar_visible = false;
        });
    }

    #[test]
    fn sessions_and_command_panels_refresh_together() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend
                .dispatch(crate::Msg::SidebarPointerMoved(0))
                .expect("initialize command link");
            let command_id = SidebarTabId::new("rows");
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.tabs = vec![
                    SidebarTab::Sessions,
                    SidebarTab::Panes,
                    SidebarTab::Command {
                        name: command_id.clone(),
                        label: "Rows".to_string(),
                        command: "echo command-panel".to_string(),
                        interval_secs: 5,
                        on_click: None,
                    },
                ];
                state.sidebar.panels = vec![
                    crate::state::SidebarPanelState {
                        tabs: vec![SidebarTabId::new("sessions")],
                        active_tab: Some(SidebarTabId::new("sessions")),
                        ..Default::default()
                    },
                    crate::state::SidebarPanelState {
                        tabs: vec![SidebarTabId::new("panes"), command_id.clone()],
                        active_tab: Some(SidebarTabId::new("panes")),
                        ..Default::default()
                    },
                ];
            }

            backend
                .dispatch(crate::Msg::SidebarTabSelected { panel: 1, index: 1 })
                .expect("select command beside sessions");
            assert!(
                backend
                    .state()
                    .sidebar
                    .command_in_flight
                    .contains_key(&command_id)
                    || backend
                        .state()
                        .sidebar
                        .command_output
                        .contains_key(&command_id),
                "the command panel must start even while Sessions is visible"
            );
            backend.state_mut().sidebar_visible = false;
        });
    }
}
