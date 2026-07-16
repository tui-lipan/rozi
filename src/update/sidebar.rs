use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::{SidebarTab, SidebarTabId, UserCommandAction};
use crate::state::{SidebarCommandOutput, SidebarCommandRow};

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
        if sessions_active(ctx) {
            open_sessions(ctx)
        } else {
            start_active_command(ctx)
        }
    } else {
        Update::none()
    }
}

pub(crate) fn visibility_changed(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.sidebar.invalidate_sessions();
    ctx.state.sidebar.invalidate_commands();
    if sessions_active(ctx) {
        open_sessions(ctx)
    } else {
        start_active_command(ctx)
    }
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
        poll_command(ctx, ctx.state.sidebar.command_epoch, tab_id)
    } else {
        Update::full()
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
        return Update::command_only(Command::spawn(move |link: CommandLink<crate::Msg>| {
            std::thread::sleep(COMMAND_BUSY_RETRY);
            link.send(crate::Msg::SidebarCommandPoll { epoch, tab_id });
        }));
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
    let command = Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::sleep(std::time::Duration::from_secs(interval_secs));
        link.send(crate::Msg::SidebarCommandPoll { epoch, tab_id });
    });
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
    match action {
        UserCommandAction::Send(text) => UserCommandAction::Send(text.replace("{line}", line)),
        // Config validation rejects placeholders here; run/popup commands are always fixed.
        action => action.clone(),
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
    Update::with_command(Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::sleep(SESSION_REFRESH_INTERVAL);
        link.send(crate::Msg::SidebarSessionsRefresh { epoch });
    }))
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
            resolve_row_action(&UserCommandAction::Run("show fixed".to_string()), "ignored"),
            UserCommandAction::Run("show fixed".to_string())
        );
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
                    action: UserCommandAction::Run("printf safe".to_string()),
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
                on_click: Some(UserCommandAction::Run("printf fixed".to_string())),
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
