use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::{SidebarTab, SidebarTabId, UserCommandAction};
use crate::state::{SidebarCommandOutput, SidebarCommandRow};
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
            .active_tab
            .as_ref()
            .is_some_and(|id| id.as_str() == "sessions")
}

fn command_active(ctx: &Context<HyprmuxApp>, id: &SidebarTabId) -> bool {
    ctx.state.sidebar_visible && ctx.state.sidebar.active_tab.as_ref() == Some(id)
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

pub(crate) fn request_sessions_refresh(ctx: &Context<HyprmuxApp>) {
    if sessions_active(ctx)
        && let Some(link) = ctx.state.command_link.as_ref()
    {
        link.send(crate::Msg::SidebarSessionsRefresh {
            epoch: ctx.state.sidebar.sessions_epoch,
        });
    }
}

pub(crate) fn request_command_poll(ctx: &Context<HyprmuxApp>) {
    let Some(tab_id) = ctx.state.sidebar.active_tab.clone() else {
        return;
    };
    if command_active(ctx, &tab_id)
        && command_tab(ctx, &tab_id).is_some()
        && let Some(link) = ctx.state.command_link.as_ref()
    {
        link.send(crate::Msg::SidebarCommandPoll {
            epoch: ctx.state.sidebar.command_epoch,
            tab_id,
        });
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

pub(super) fn tab_selected(ctx: &mut Context<HyprmuxApp>, id: SidebarTabId) -> Update {
    if ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .any(|tab| tab.id() == id)
    {
        if ctx.state.sidebar.active_tab.as_ref() == Some(&id) {
            return Update::none();
        }
        ctx.state.sidebar.invalidate_sessions();
        ctx.state.sidebar.invalidate_commands();
        ctx.state.sidebar.active_tab = Some(id);
        // A different tab is a different row list; carrying the old index over would drop the
        // cursor somewhere arbitrary.
        ctx.state.sidebar.cursor = 0;
        // Clicking the tab strip does not move focus — the strip is not focusable and the sidebar
        // is outside click-to-focus — but the body it was on unmounts, and focus goes with it. The
        // file tree feels this worst: each tree keys on its root, so even Files -> Git is a
        // remount, and without this the keyboard would be left pointing at nothing.
        refocus_body(ctx);
        arm_agent_tick(ctx);
        if sessions_active(ctx) {
            open_sessions(ctx)
        } else {
            start_active_command(ctx)
        }
    } else {
        Update::none()
    }
}

/// Re-aim keyboard focus at the active tab's body after the previous one unmounted. A no-op unless
/// the sidebar already had the keyboard — switching tabs with the mouse must not steal it.
fn refocus_body(ctx: &mut Context<HyprmuxApp>) {
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
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.sidebar.invalidate_commands();
    arm_agent_tick(ctx);
    if sessions_active(ctx) {
        open_sessions(ctx)
    } else {
        start_active_command(ctx)
    }
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
    ctx.state.sidebar.suppress_row_hover = true;
    ctx.state.commands_dirty = true;
    Update::with_command(command)
}

/// Escape from the sidebar: give the keyboard back to the focused pane.
pub(crate) fn blur_body(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !ctx.state.sidebar.focused {
        return Update::none();
    }
    ctx.state.sidebar.focused = false;
    ctx.state.commands_dirty = true;
    release_focus(ctx);
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
    ctx.state.sidebar.cycle(&ctx.state.config.sidebar, forward);
    ctx.state.sidebar.cursor = 0;
    ctx.state.sidebar.suppress_row_hover = true;
    let update = visibility_changed(ctx);
    refocus_body(ctx);
    update
}

fn open_sessions(ctx: &mut Context<HyprmuxApp>) -> Update {
    let epoch = ctx.state.sidebar.sessions_epoch;
    match crate::ops::session::picker_rows(ctx) {
        Ok(rows) => sessions_discovered(ctx, epoch, Ok(rows)),
        Err(error) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Session list failed",
                error.to_string(),
            ));
            refresh_sessions(ctx, epoch)
        }
    }
}

fn start_active_command(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(tab_id) = ctx.state.sidebar.active_tab.clone() else {
        return Update::full();
    };
    if command_active(ctx, &tab_id) && command_tab(ctx, &tab_id).is_some() {
        let update = poll_command(ctx, ctx.state.sidebar.command_epoch, tab_id);
        Update::with_command(update.command)
    } else {
        Update::full()
    }
}

/// Move the keyboard cursor by `delta` selectable rows, stopping at the ends rather than wrapping —
/// the row list is a panel, not a carousel, and wrapping past the last agent back to the first reads
/// as a glitch. Headers and spacers are stepped over rather than landed on.
pub(crate) fn move_cursor(ctx: &mut Context<HyprmuxApp>, delta: isize) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab(ctx).cloned() else {
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
    let current = crate::view::sidebar::resolve_cursor(ctx.state.sidebar.cursor, &rows);
    let position = current
        .and_then(|current| selectable.iter().position(|index| *index == current))
        .unwrap_or(0);
    let next = position
        .saturating_add_signed(delta)
        .min(selectable.len() - 1);
    let cursor = selectable[next];
    if ctx.state.sidebar.cursor == cursor {
        return Update::none();
    }
    ctx.state.sidebar.cursor = cursor;
    ctx.state.sidebar.suppress_row_hover = true;
    Update::full()
}

/// A real pointer move ends keyboard modality and lets row hover follow the pointer again.
pub(crate) fn pointer_moved(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !ctx.state.sidebar.suppress_row_hover {
        return Update::none();
    }
    ctx.state.sidebar.suppress_row_hover = false;
    Update::full()
}

/// Enter: run whatever the row under the cursor does — the same path a click on it takes.
pub(crate) fn activate_cursor(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab(ctx).cloned() else {
        return Update::none();
    };
    let rows = crate::view::sidebar::body_rows(ctx, &tab);
    match crate::view::sidebar::resolve_cursor(ctx.state.sidebar.cursor, &rows) {
        Some(index) => row_activate(ctx, index),
        None => Update::none(),
    }
}

/// A row was activated by Enter or by a click. The index is resolved against a freshly rebuilt row
/// list — the same pure function of `State` the view rendered from — so both gestures land on the
/// same handler and a row list that changed underneath simply resolves to nothing.
pub(super) fn row_activate(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(tab) = crate::view::sidebar::active_tab(ctx).cloned() else {
        return Update::none();
    };
    let mut rows = crate::view::sidebar::body_rows(ctx, &tab);
    if index >= rows.len() {
        return Update::none();
    }
    match rows.swap_remove(index).target {
        RowTarget::Inert => Update::none(),
        RowTarget::Pane(id) => focus_pane(ctx, id),
        RowTarget::Session(entry) => activate_session(ctx, *entry),
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
fn discover_repo_root(cwd: &str) -> Option<String> {
    std::path::Path::new(cwd)
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(|dir| dir.to_string_lossy().into_owned())
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
    if crate::pane_lifecycle::focused_local_cwd_ref(&ctx.state)
        != ctx.state.sidebar.tree_cwd.as_deref()
    {
        let cwd = crate::pane_lifecycle::focused_local_cwd(&ctx.state);
        ctx.state.sidebar.tree_repo = cwd.as_deref().and_then(discover_repo_root);
        ctx.state.sidebar.tree_cwd = cwd;
        bump_git_refresh(&mut ctx.state.sidebar);
    }

    // A command finishing is the moment the working tree most likely changed, and it is a far
    // better refresh trigger than a timer: no polling while the user reads, immediate feedback
    // after a build, commit, or checkout.
    let phase = ctx.state.focused_pane.and_then(|id| {
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
            let sanitized = crate::plain_text::sanitize(&String::from_utf8_lossy(bounded));
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

pub(super) fn refresh_sessions(ctx: &Context<HyprmuxApp>, epoch: u64) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    let current_name = ctx.state.session_name.clone();
    let current = crate::ops::session::current_session_row(&ctx.state);
    Update::with_command(Command::spawn(move |link: CommandLink<crate::Msg>| {
        let rows = crate::session::discovery::discover_selectable_sessions(current_name.as_deref())
            .map(|mut rows| {
                if let Some(current) = current {
                    rows.push(current);
                    rows.sort_by(|a, b| a.name.cmp(&b.name));
                }
                rows
            })
            .map_err(|error| error.to_string());
        link.send(crate::Msg::SidebarSessionsDiscovered { epoch, rows });
    }))
}

pub(super) fn sessions_discovered(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    rows: std::result::Result<Vec<crate::session::discovery::DiscoveredSession>, String>,
) -> Update {
    if !sessions_active(ctx) || epoch != ctx.state.sidebar.sessions_epoch {
        return Update::none();
    }
    if let Ok(rows) = rows {
        if ctx
            .state
            .sidebar
            .pending_session_open
            .as_ref()
            .is_some_and(|pending| !rows.iter().any(|entry| &entry.name == pending))
        {
            ctx.state.sidebar.pending_session_open = None;
        }
        ctx.state.sidebar.sessions = rows;
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
    crate::ops::session::activate_discovered_session(
        ctx,
        entry,
        crate::ops::session::SessionActivationSource::Sidebar,
    )
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

    fn discovered(name: &str) -> crate::session::discovery::DiscoveredSession {
        crate::session::discovery::DiscoveredSession {
            name: name.to_string(),
            ephemeral: false,
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: 1,
                clients: 0,
                has_layout: true,
                created_from_profile: None,
            },
        }
    }

    fn on_test_thread(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn sidebar test")
            .join()
            .expect("sidebar test completes");
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
            discover_repo_root(&nested.to_string_lossy()),
            Some(repo_str.clone())
        );
        assert_eq!(discover_repo_root(&repo_str), Some(repo_str));
        assert_eq!(discover_repo_root(&dir.path().to_string_lossy()), None);
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
            let pane = backend.state().workspaces[0].panes[0].id;
            {
                let state = backend.state_mut();
                state.focused_pane = Some(pane);
                state.workspaces[0].focused_pane = Some(pane);
                state.workspaces[0].panes[0].terminal.cwd = Some(nested.clone());
            }
            backend
                .dispatch(crate::Msg::SidebarTabSelected(SidebarTabId::new("files")))
                .expect("sync runs after any message");
            assert_eq!(backend.state().sidebar.tree_cwd.as_deref(), Some(&*nested));
            assert_eq!(backend.state().sidebar.tree_repo.as_deref(), Some(&*repo));
            let settled = backend.state().sidebar.git_refresh_token;
            assert!(settled > 0, "resolving a root schedules a git refresh");

            // Same directory reported again: no walk, no refresh.
            backend
                .dispatch(crate::Msg::SidebarTabSelected(SidebarTabId::new("files")))
                .expect("repeat sync");
            assert_eq!(backend.state().sidebar.git_refresh_token, settled);

            // Leaving the repository clears the repo root but keeps the working directory.
            backend.state_mut().workspaces[0].panes[0].terminal.cwd = Some(outside.clone());
            backend
                .dispatch(crate::Msg::SidebarTabSelected(SidebarTabId::new("files")))
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
            let pane = backend.state().workspaces[0].panes[0].id;
            {
                let state = backend.state_mut();
                state.focused_pane = Some(pane);
                state.workspaces[0].focused_pane = Some(pane);
                state.workspaces[0].panes[0].terminal.command_phase =
                    crate::session::protocol::PaneCommandPhase::Executing;
            }
            backend
                .dispatch(crate::Msg::SidebarTabSelected(SidebarTabId::new("git")))
                .expect("observe executing");
            let running = backend.state().sidebar.git_refresh_token;

            backend.state_mut().workspaces[0].panes[0]
                .terminal
                .command_phase = crate::session::protocol::PaneCommandPhase::Completed {
                exit_status: Some(0),
            };
            backend
                .dispatch(crate::Msg::SidebarTabSelected(SidebarTabId::new("git")))
                .expect("observe completion");
            let finished = backend.state().sidebar.git_refresh_token;
            assert_eq!(finished, running + 1, "one refresh per finished command");

            // Still completed on the next message: no repeat refresh.
            backend
                .dispatch(crate::Msg::SidebarTabSelected(SidebarTabId::new("git")))
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
            backend.state_mut().pending_spawns.clear();

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
                backend.state_mut().workspaces[1].panes.push(pane);
                backend
                    .dispatch(crate::Msg::SidebarFocusPane(2))
                    .expect("focus sidebar pane");
                assert_eq!(backend.state().active_workspace, 1);
                assert_eq!(backend.state().focused_pane, Some(2));
                assert!(
                    !backend.state().workspaces[1].panes[0]
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
                state.sidebar.active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 10;
            }
            let stale = vec![discovered("old")];

            backend.state_mut().sidebar_visible = false;
            backend.state_mut().sidebar.invalidate_sessions();
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 10,
                    rows: Ok(stale.clone()),
                })
                .expect("stale close result");
            assert!(backend.state().sidebar.sessions.is_empty());

            backend.state_mut().sidebar_visible = true;
            backend.state_mut().sidebar.active_tab = Some(SidebarTabId::new("panes"));
            backend.state_mut().sidebar.invalidate_sessions();
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 11,
                    rows: Ok(stale.clone()),
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
                })
                .expect("stale reload result");
            assert!(backend.state().sidebar.sessions.is_empty());
        });
    }

    #[test]
    fn current_session_results_apply_and_missing_confirmation_is_cleared() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.active_tab = Some(SidebarTabId::new("sessions"));
                state.sidebar.sessions_epoch = 7;
                state.sidebar.pending_session_open = Some("gone".to_string());
            }
            backend
                .dispatch(crate::Msg::SidebarSessionsDiscovered {
                    epoch: 7,
                    rows: Ok(vec![discovered("dev")]),
                })
                .expect("current result");
            assert_eq!(backend.state().sidebar.sessions, vec![discovered("dev")]);
            assert_eq!(backend.state().sidebar.pending_session_open, None);
        });
    }

    #[test]
    fn sidebar_ephemeral_confirmation_is_independent_from_picker_confirmation() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            {
                let state = backend.state_mut();
                state.session_attached = true;
                state.session_name = Some("eph-test".to_string());
                let mut picker = crate::state::SessionPickerState::new(vec![discovered("picker")]);
                picker.pending_open = Some(0);
                state.session_picker = Some(picker);
            }
            backend
                .dispatch(crate::Msg::SidebarSessionActivate(discovered("dev")))
                .expect("arm sidebar activation");
            assert_eq!(
                backend.state().sidebar.pending_session_open.as_deref(),
                Some("dev")
            );
            assert_eq!(
                backend
                    .state()
                    .session_picker
                    .as_ref()
                    .and_then(|picker| picker.pending_open),
                Some(0)
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

    /// Build a one-entry launcher tab and activate it, returning the spawn request it queued. With
    /// no session client attached the request lands in `pending_spawns` instead of going out on the
    /// wire, which is exactly the payload worth asserting on.
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
            let focused = backend.state().workspaces[0].panes[0].id;
            backend.state_mut().workspaces[0].focused_pane = Some(focused);
            backend.state_mut().focused_pane = Some(focused);
            backend.state_mut().workspaces[0].panes[0].terminal.cwd = Some(cwd.to_string());
        }
        backend.state_mut().pending_spawns.clear();
        backend
            .dispatch(crate::Msg::SidebarLauncherActivate {
                config_epoch: 1,
                tab_id: id,
                entry_index: 0,
            })
            .expect("launcher click");
        backend
            .state()
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
            let initial = backend.state().workspaces[0].panes.len();
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
            assert_eq!(backend.state().workspaces[0].panes.len(), initial);
            backend
                .dispatch(crate::Msg::SidebarLauncherActivate {
                    config_epoch: 4,
                    tab_id: id,
                    entry_index: 0,
                })
                .expect("current launcher click");
            assert_eq!(backend.state().workspaces[0].panes.len(), initial + 1);
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
            let initial = backend.state().workspaces[0].panes.len();
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
            assert_eq!(backend.state().workspaces[0].panes.len(), initial);
            backend
                .dispatch(crate::Msg::SidebarCommandRowActivate {
                    config_epoch: 2,
                    tab_id: id,
                    output_epoch: 9,
                    line: "safe".to_string(),
                })
                .expect("current row click");
            assert_eq!(backend.state().workspaces[0].panes.len(), initial + 1);
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
                state.sidebar.active_tab = Some(id.clone());
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
                state.sidebar.active_tab = Some(id.clone());
                state.sidebar.command_epoch = 6;
            }
            for (visible, epoch, active) in
                [(false, 6, "rows"), (true, 5, "rows"), (true, 6, "other")]
            {
                backend.state_mut().sidebar_visible = visible;
                backend.state_mut().sidebar.active_tab = Some(SidebarTabId::new(active));
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
            state.sidebar.active_tab = Some(id.clone());
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
}
