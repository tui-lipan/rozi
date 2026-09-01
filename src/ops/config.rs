use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use notify::Watcher;
use tui_lipan::prelude::*;

use crate::state::{PaneIdentity, ThemePreset};
use crate::{AppRoot, Msg};

/// Watches the config file's directory and requests a live reload when `config.toml` changes
/// on disk. Watching the parent directory (like tui-lipan's `ThemeWatcher`) catches editors
/// that save via write-to-temp + rename; rozi's own persistence writes are filtered out in
/// the `Msg::ConfigFileChanged` handler by comparing against the last text this process read
/// or wrote. Fire-and-forget for the life of the app.
pub(crate) fn spawn_config_watcher(link: &CommandLink<Msg>) {
    let link = link.clone();
    std::thread::spawn(move || {
        let path = crate::config::config_path();
        let Some(dir) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(event_tx) else {
            return;
        };
        if watcher
            .watch(&dir, notify::RecursiveMode::NonRecursive)
            .is_err()
        {
            return;
        }

        loop {
            let event = match event_rx.recv() {
                Ok(Ok(event)) => event,
                Ok(Err(_)) => continue,
                Err(_) => return,
            };
            if !event_touches_config(&event, &path) {
                continue;
            }
            // Editors save in bursts (create + data + rename events); coalesce them into one
            // reload by draining until the events go quiet.
            loop {
                match event_rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            link.send(Msg::ConfigFileChanged);
        }
    });
}

fn event_touches_config(event: &notify::Event, target: &std::path::Path) -> bool {
    use notify::EventKind;
    if !matches!(
        event.kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_)
    ) {
        return false;
    }
    let Some(target_name) = target.file_name() else {
        return false;
    };
    event
        .paths
        .iter()
        .any(|path| path.file_name().is_some_and(|name| name == target_name))
}

/// Handles a config-watcher wakeup: reloads only when the file content actually differs from
/// what rozi last read or wrote, so self-persistence and no-op saves stay silent.
pub(crate) fn config_file_changed(ctx: &mut Context<AppRoot>) -> Update {
    if !crate::config::config_text_changed_on_disk() {
        return Update::none();
    }
    // A reload can change `[keys]` bindings and user commands; resync the palette.
    ctx.state.commands_dirty = true;
    reload_config(ctx)
}

/// Re-reads `config.toml` and applies it live: config fields, keymap/user commands, theme
/// (including switching the theme file watcher), and pane chrome - the same result a restart
/// would produce, without losing running panes/workspaces/session state.
pub(crate) fn reload_config(ctx: &mut Context<AppRoot>) -> Update {
    reload(ctx, "Config reloaded")
}

/// Rescans installed extension manifests and reconciles their runtime contributions.
///
/// Extension discovery is part of config loading, so this necessarily re-reads `config.toml` too.
pub(crate) fn reload_extensions(ctx: &mut Context<AppRoot>) -> Update {
    reload(ctx, "Extensions reloaded")
}

fn reload(ctx: &mut Context<AppRoot>, success_message: &'static str) -> Update {
    // Extension reloads must refresh command registration even when config.toml itself did not
    // change.
    ctx.state.commands_dirty = true;
    let loaded = crate::config::load_config();
    let mut new_config = loaded.config;
    // The framework clipboard service is configured when the runtime starts. Keep the state-side
    // gate aligned with it until restart so child OSC 52 and rozi-originated copies cannot disagree.
    new_config.clipboard.enable_osc52 = ctx.state.config.clipboard.enable_osc52;

    let system_theme = ctx.state.system_theme.clone();
    let resolved = crate::config::resolve_theme(&new_config.theme.name, system_theme.as_ref());

    let had_theme_watcher = ctx.state.theme_watcher.is_some();
    ctx.state.theme_watcher = None;
    let mut start_theme_tick = false;
    if let Some(path) = &resolved.watch_path {
        match ThemeWatcher::new(path.clone(), ThemePreset::Lipan.theme()) {
            Ok(watcher) => {
                ctx.state.theme_watcher = Some(watcher);
                start_theme_tick = !had_theme_watcher;
            }
            Err(err) => {
                crate::pane::pty_events::notify_error(ctx, "Theme watch failed", err.to_string());
            }
        }
    }
    let host_bg = crate::ops::theme::host_background(&ctx.state);
    ctx.state.theme = crate::ops::theme::apply_backdrop_policy(
        resolved.theme,
        host_bg,
        new_config.pane.background_follows_terminal,
    );

    // Same trick for the clock repaint loop: it reschedules itself only while a clock segment
    // is configured, so it needs a kick here exactly when it wasn't already running.
    let had_workbar_tick = ctx.state.config.workbar.has_clock();
    let start_workbar_tick = !had_workbar_tick && new_config.workbar.has_clock();

    // `[sidebar] visible` is a startup default only. A reload deliberately does not reapply it:
    // visibility is client-local view chrome, so the file must not reach in and open or close a
    // running client's sidebar - not on an unrelated edit, and not on an edit to the key itself.
    ctx.state.sidebar.reconcile(&new_config.sidebar);
    // Every reload invalidates scheduled/running results, including interval-only and
    // command-shell-only changes. Keep matching in-flight guards until their old results arrive so
    // the replacement polls cannot overlap them.
    ctx.state.workbar.reconcile(&new_config.workbar);
    // The reveal delay lives in the runtime rather than in `State`, so a reload has to push it
    // across explicitly - otherwise it is the one `[input]` key that silently needs a restart.
    ctx.set_command_chord_reveal_delay(new_config.input.which_key.reveal_delay());
    let (extension_generations, stale_extensions) = crate::config::reconcile_generations(
        Some(&ctx.state.config),
        &mut new_config,
        &ctx.state.extension_generations,
    );
    ctx.state.extension_generations = extension_generations;
    ctx.state.config = new_config;
    // Agent detection runs in the session server against the server's own config load, so the
    // reload has to be forwarded rather than applied here. Only the controller may: `detected_agent`
    // is one answer shared by every attached client, not a per-client view.
    if ctx.state.is_controller()
        && let Some(client) = ctx.state.current().session_client.as_ref()
    {
        client.reload_agents();
    }
    if let Some(client) = ctx.state.scratch_client() {
        client.reload_agents();
    }
    let _ = crate::ops::pick::unload_extensions(ctx, &stale_extensions);
    let _ = crate::ops::published_rows::unload_extensions(ctx, &stale_extensions);
    crate::ops::extensions::unload(ctx, &stale_extensions);
    // A reload cannot change visibility, but it can change the tab set and panel split, so the
    // caches and active-tab refresh still run. Refresh work is kicked explicitly below, so only
    // the synchronous focus/cache effects are needed here.
    let _ = crate::update::sidebar::visibility_changed(ctx);
    if ctx.state.sidebar_visible && ctx.state.sidebar.focused {
        crate::update::sidebar::refocus_body(ctx);
    }
    crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state);
    crate::update::workbar::request_command_polls(ctx);
    crate::ops::services::reconcile_services(ctx);
    let start_services_tick =
        !ctx.state.services.running.is_empty() || !ctx.state.services.pending.is_empty();

    for warning in loaded.warnings.iter().chain(&resolved.warnings) {
        crate::pane::pty_events::notify_error(ctx, "Config warning", warning.clone());
    }
    if loaded.warnings.is_empty() && resolved.warnings.is_empty() {
        crate::pane::pty_events::notify_info(ctx, success_message);
        crate::events::emit(
            &ctx.state,
            crate::events::Event::new(
                crate::events::EventKind::ConfigReloaded,
                vec![("path", crate::config::config_path().display().to_string())],
            ),
        );
    }

    if start_theme_tick || start_workbar_tick || start_services_tick {
        let services_epoch = ctx.state.services.epoch;
        Update::with_command(Command::spawn(move |link: CommandLink<Msg>| {
            if start_theme_tick {
                // Arming the first tick must not hold this worker for 150ms while the pollers
                // below are still waiting to start.
                link.send_after(Duration::from_millis(150), Msg::ThemeTick);
            }
            if start_workbar_tick {
                link.send(Msg::WorkbarTick);
            }
            if start_services_tick {
                link.send_after(
                    Duration::from_secs(1),
                    Msg::ServicesTick {
                        epoch: services_epoch,
                    },
                );
            }
        }))
    } else {
        Update::full()
    }
}

/// Opens `config.toml` in `$EDITOR` (falling back to `$VISUAL`, then `vi`) in a new pane, so
/// hand-editing the config doesn't require remembering or typing its path.
pub(crate) fn open_config_file(ctx: &mut Context<AppRoot>) -> Update {
    let editor = config_editor();
    if let Some(command) = missing_editor_command(&editor) {
        crate::pane::pty_events::notify_error(
            ctx,
            "Editor not found",
            format!("`{command}` is not available\nSet $EDITOR"),
        );
        return Update::none();
    }
    if let Some(update) = crate::ops::session::ensure_session_for_pty(
        ctx,
        crate::state::PendingSessionAction::OpenConfigFile,
    ) {
        return update;
    }
    let path = crate::config::config_path();
    let launch = match editor_launch(&editor, &path) {
        Ok(launch) => launch,
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Editor not found", error);
            return Update::none();
        }
    };

    let identity = PaneIdentity {
        launch: Some(launch),
        ..PaneIdentity::default()
    };
    crate::pane::lifecycle::spawn_interactive_pane(
        ctx,
        ctx.state.current().active_workspace,
        None,
        identity,
    )
    .1
}

/// Builds the direct argv that opens `path` in `editor`.
///
/// Direct execution rather than a shell command string: a `PaneLaunch::Shell` command runs through
/// the command shell, which on Windows defaults to `cmd.exe /D /S /C`, and `cmd.exe` does not strip
/// the POSIX single quotes a `sh -c` string needs. Quoting the path for one shell corrupts it in
/// the other - the editor opens a file whose name still carries the quotes - so the path is passed
/// as its own process argument and never quoted at all.
pub(crate) fn editor_launch(
    editor: &str,
    path: &std::path::Path,
) -> std::result::Result<crate::pane::launch::PaneLaunch, String> {
    let mut argv = editor_words(editor);
    argv.push(path.to_string_lossy().into_owned());
    crate::pane::launch::PaneLaunch::direct(argv)
}

/// Splits an `$EDITOR` value into argv words, honoring the quotes people put around a program path
/// that contains spaces (`"/opt/my editor/bin/edit" --wait`).
fn editor_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote = None;

    for ch in value.chars() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

pub(crate) fn config_editor() -> String {
    std::env::var("EDITOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("VISUAL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string())
}

pub(crate) fn missing_editor_command(editor: &str) -> Option<String> {
    let words = editor_words(editor);
    let command = words.first().map_or("", String::as_str);
    if command.is_empty() || command_exists(command) {
        None
    } else {
        Some(command.to_string())
    }
}

fn command_exists(command: &str) -> bool {
    crate::platform::command::program_exists(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_words_reads_bare_command() {
        assert_eq!(editor_words("nvim --clean"), ["nvim", "--clean"]);
    }

    #[test]
    fn editor_words_reads_quoted_command() {
        assert_eq!(
            editor_words("'/opt/my editor/bin/edit' --wait"),
            ["/opt/my editor/bin/edit", "--wait"]
        );
    }

    #[test]
    fn editor_launch_passes_the_path_as_its_own_argument() {
        let launch = editor_launch(
            "nvim",
            std::path::Path::new(r"C:\Users\me\rozi\config.toml"),
        )
        .expect("editor launch");
        assert_eq!(
            launch.argv(),
            Some(
                &[
                    "nvim".to_string(),
                    r"C:\Users\me\rozi\config.toml".to_string()
                ][..]
            )
        );
    }

    #[test]
    fn editor_launch_keeps_spaced_paths_unquoted() {
        let launch = editor_launch(
            "code -w",
            std::path::Path::new("/home/me/my dir/config.toml"),
        )
        .expect("editor launch");
        assert_eq!(
            launch.argv(),
            Some(
                &[
                    "code".to_string(),
                    "-w".to_string(),
                    "/home/me/my dir/config.toml".to_string(),
                ][..]
            )
        );
    }
}
