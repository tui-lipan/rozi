use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{SidebarTab, SidebarTabId};
use crate::state::{SidebarCommandOutput, SidebarCommandRow};

pub(crate) const SESSION_REFRESH_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(1500);
pub(crate) const TREE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
pub(crate) const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub(crate) const COMMAND_CAPTURE_BYTES: usize = 64 * 1024;
pub(crate) const COMMAND_MAX_ROWS: usize = 500;
pub(crate) const COMMAND_RAW_ROW_CHARS: usize = 4096;
pub(crate) const COMMAND_DISPLAY_ROW_CHARS: usize = 160;
pub(crate) const COMMAND_BUSY_RETRY: std::time::Duration = std::time::Duration::from_millis(50);

pub(crate) fn sessions_active(ctx: &Context<AppRoot>) -> bool {
    ctx.state.sidebar_visible
        && ctx
            .state
            .sidebar
            .active_tabs()
            .any(|id| id.as_str() == "sessions")
}

pub(crate) fn command_active(ctx: &Context<AppRoot>, id: &SidebarTabId) -> bool {
    ctx.state.sidebar_visible && ctx.state.sidebar.active_tabs().any(|active| active == id)
}

pub(crate) fn tree_active(ctx: &Context<AppRoot>) -> bool {
    ctx.state.sidebar_visible
        && ctx.state.sidebar.active_tabs().any(|id| {
            ctx.state
                .config
                .sidebar
                .tabs
                .iter()
                .any(|tab| matches!(tab, SidebarTab::Tree { view, .. } if view.id() == id.as_str()))
        })
}

/// The environment an extension's command tab polls with. Empty for a `config.toml` tab.
pub(crate) fn command_tab_env(ctx: &Context<AppRoot>, id: &SidebarTabId) -> Vec<(String, String)> {
    ctx.state
        .config
        .sidebar
        .tabs
        .iter()
        .find(|tab| &tab.id() == id)
        .map(|tab| tab.env().to_vec())
        .unwrap_or_default()
}

pub(crate) fn command_tab(ctx: &Context<AppRoot>, id: &SidebarTabId) -> Option<(String, u64)> {
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

/// The tab's section marker, read at poll time so a config reload takes effect on the next run
/// rather than on the next restart.
pub(crate) fn command_group_prefix(ctx: &Context<AppRoot>, id: &SidebarTabId) -> Option<String> {
    ctx.state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Command {
                name, group_prefix, ..
            } if name == id => group_prefix.clone(),
            _ => None,
        })
}

pub(crate) fn request_command_poll(ctx: &Context<AppRoot>) {
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
pub(crate) fn arm_agent_tick(ctx: &mut Context<AppRoot>) {
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
pub(crate) fn agent_tick(ctx: &mut Context<AppRoot>) -> Update {
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

pub(crate) fn poll_command(ctx: &mut Context<AppRoot>, epoch: u64, tab_id: SidebarTabId) -> Update {
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
    let group_prefix = command_group_prefix(ctx, &tab_id);
    let env = command_tab_env(ctx, &tab_id);
    // A command tab describes the project in front of you, so it runs where the focused pane is —
    // the same rule extension commands follow. Under `--remote` the pane's path belongs to the
    // server and this poll runs on the client, so nothing is set and the client's own directory
    // stands.
    let cwd = ctx
        .state
        .sidebar
        .command_cwd
        .clone()
        .map(std::path::PathBuf::from);
    Update::command_only(Command::spawn(move |link: CommandLink<crate::Msg>| {
        let rows = command_rows(
            crate::platform::command::run_bounded_shell_command_with_env(
                &shell,
                &command_line,
                &env,
                cwd.as_deref(),
                COMMAND_TIMEOUT,
                COMMAND_CAPTURE_BYTES,
            ),
            group_prefix.as_deref(),
        );
        link.send(crate::Msg::SidebarCommandOutput {
            epoch,
            tab_id,
            rows,
        });
    }))
}

pub(crate) fn command_output(
    ctx: &mut Context<AppRoot>,
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
        .is_none_or(|output| output.rows != rows || output.cwd != ctx.state.sidebar.command_cwd);
    if changed {
        ctx.state.sidebar.next_output_epoch = ctx.state.sidebar.next_output_epoch.wrapping_add(1);
        ctx.state.sidebar.command_output.insert(
            tab_id.clone(),
            SidebarCommandOutput {
                epoch: ctx.state.sidebar.next_output_epoch,
                rows,
                cwd: ctx.state.sidebar.command_cwd.clone(),
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

/// Follow the focused pane's directory, so a `cd` re-lists a command tab instead of leaving it
/// describing the project you were in half a minute ago.
///
/// Runs after every message like the tree's own root sync, so the unchanged case is one borrowed
/// string comparison. A change invalidates any poll already in flight — its output describes the
/// old directory — and starts a new one straight away rather than waiting out the interval.
pub(crate) fn sync_command_cwd(ctx: &mut Context<AppRoot>) {
    if ctx.state.current().remote_host.is_some() {
        return;
    }
    let cwd = crate::pane::lifecycle::focused_local_cwd_ref(&ctx.state);
    if cwd == ctx.state.sidebar.command_cwd.as_deref() {
        return;
    }
    ctx.state.sidebar.command_cwd = cwd.map(str::to_string);
    ctx.state.sidebar.command_epoch = ctx.state.sidebar.command_epoch.wrapping_add(1);
    ctx.state.sidebar.command_in_flight.clear();
    request_command_poll(ctx);
}

pub(crate) fn refresh_active_tabs(ctx: &mut Context<AppRoot>) -> Update {
    if sessions_active(ctx) {
        crate::update::sidebar::sessions::open_sessions(ctx);
    }
    request_command_poll(ctx);
    Update::full()
}

pub(crate) fn command_rows(
    result: std::io::Result<crate::platform::command::CommandOutput>,
    group_prefix: Option<&str>,
) -> Vec<SidebarCommandRow> {
    let output = match result {
        Ok(output) => output,
        Err(error) => return vec![error_row(&format!("command failed: {error}"))],
    };
    if output.timed_out {
        return vec![error_row("command timed out after 5 seconds")];
    }
    let mut rows = text_rows(&output.stderr, true, None);
    if output.status != Some(0) && !rows.iter().any(|row| row.error) {
        rows.push(error_row(&format!(
            "command exited with status {}",
            output
                .status
                .map_or_else(|| "unknown".to_string(), |status| status.to_string())
        )));
    }
    rows.extend(text_rows(&output.stdout, false, group_prefix));
    rows.truncate(COMMAND_MAX_ROWS);
    rows
}

pub(crate) fn text_rows(
    bytes: &[u8],
    error: bool,
    group_prefix: Option<&str>,
) -> Vec<SidebarCommandRow> {
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
                match group_prefix.and_then(|prefix| sanitized.strip_prefix(prefix)) {
                    // The marker is rozi's chrome, not part of the label; a line carrying nothing
                    // but the marker drops out the same way a blank line does.
                    Some(label) => Some(label.trim())
                        .filter(|label| !label.is_empty())
                        .map(header_row),
                    None => Some(row(&sanitized, false)),
                }
            }
        })
        .collect()
}

/// A section header carrying no action, built through [`row`] so it truncates like every other row.
pub(crate) fn header_row(text: &str) -> SidebarCommandRow {
    SidebarCommandRow {
        header: true,
        ..row(text, false)
    }
}

pub(crate) fn error_row(text: &str) -> SidebarCommandRow {
    row(&format!("Error: {text}"), true)
}

pub(crate) fn row(text: &str, error: bool) -> SidebarCommandRow {
    let raw: String = text.chars().take(COMMAND_RAW_ROW_CHARS).collect();
    let mut display: String = raw.chars().take(COMMAND_DISPLAY_ROW_CHARS).collect();
    if raw.chars().count() > COMMAND_DISPLAY_ROW_CHARS {
        display.push('…');
    }
    SidebarCommandRow {
        raw,
        display,
        error,
        header: false,
    }
}
