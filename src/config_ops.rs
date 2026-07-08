use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use notify::Watcher;
use tui_lipan::prelude::*;

use crate::pane_lifecycle::spawn_pane_in_workspace;
use crate::state::{PaneIdentity, ThemePreset};
use crate::{HyprmuxApp, Msg};

/// Watches the config file's directory and requests a live reload when `hyprmux.toml` changes
/// on disk. Watching the parent directory (like tui-lipan's `ThemeWatcher`) catches editors
/// that save via write-to-temp + rename; hyprmux's own persistence writes are filtered out in
/// the `Msg::ConfigFileChanged` handler by comparing against the last text this process read
/// or wrote. Fire-and-forget for the life of the app, like the bar-command pollers.
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
/// what hyprmux last read or wrote, so self-persistence and no-op saves stay silent.
pub(crate) fn config_file_changed(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !crate::config::config_text_changed_on_disk() {
        return Update::none();
    }
    // A reload can change `[keys]` bindings and user commands; resync the palette.
    ctx.state.commands_dirty = true;
    reload_config(ctx)
}

/// Re-reads `hyprmux.toml` and applies it live: config fields, keymap/user commands, theme
/// (including switching the theme file watcher), and pane chrome - the same result a restart
/// would produce, without losing running panes/workspaces/session state.
pub(crate) fn reload_config(ctx: &mut Context<HyprmuxApp>) -> Update {
    let loaded = crate::config::load_config();
    let new_config = loaded.config;

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
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Theme Watcher",
                    format!("Can't watch theme file: {err}"),
                ));
            }
        }
    }
    ctx.state.theme = resolved.theme;

    // The bar-command poller loop never stops itself, so only spawn one for commands that
    // aren't already running rather than restarting everything on every reload.
    let new_bar_commands: Vec<(String, u64)> = new_config
        .bar
        .command_specs()
        .into_iter()
        .filter(|(command, _)| !ctx.state.bar_commands_running.contains(command))
        .collect();
    for (command, _) in &new_bar_commands {
        ctx.state.bar_commands_running.insert(command.clone());
    }
    // Same trick for the clock repaint loop: it reschedules itself only while a clock segment
    // is configured, so it needs a kick here exactly when it wasn't already running.
    let had_bar_tick = ctx.state.config.bar.has_clock();
    let start_bar_tick = !had_bar_tick && new_config.bar.has_clock();

    ctx.state.config = new_config;
    crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state);

    for warning in loaded.warnings.iter().chain(&resolved.warnings) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Config",
            warning.clone(),
        ));
    }
    if loaded.warnings.is_empty() && resolved.warnings.is_empty() {
        ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            "Config reloaded",
        ));
    }

    if start_theme_tick || start_bar_tick || !new_bar_commands.is_empty() {
        Update::with_command(Command::spawn(move |link: CommandLink<Msg>| {
            if start_theme_tick {
                std::thread::sleep(Duration::from_millis(150));
                link.send(Msg::ThemeTick);
            }
            if start_bar_tick {
                link.send(Msg::BarTick);
            }
            crate::pane_lifecycle::spawn_bar_command_pollers(new_bar_commands, &link);
        }))
    } else {
        Update::full()
    }
}

/// Opens `hyprmux.toml` in `$EDITOR` (falling back to `$VISUAL`, then `vi`) in a new pane, so
/// hand-editing the config doesn't require remembering or typing its path.
pub(crate) fn open_config_file(ctx: &mut Context<HyprmuxApp>) -> Update {
    let path = crate::config::config_path();
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    let command = format!("{editor} {}", quote_shell_arg(&path.to_string_lossy()));

    let workspace_index = ctx.state.active_workspace;
    let previous_focused = ctx.state.workspaces[workspace_index].focused_pane;
    let identity = PaneIdentity {
        command: Some(command),
        ..PaneIdentity::default()
    };
    spawn_pane_in_workspace(ctx, workspace_index, previous_focused, identity).1
}

/// Single-quotes a shell argument so a config path containing spaces (or other shell
/// metacharacters) survives being spliced into a `sh -c` command string.
fn quote_shell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_shell_arg_wraps_plain_paths() {
        assert_eq!(
            quote_shell_arg("/home/me/.config/hyprmux.toml"),
            "'/home/me/.config/hyprmux.toml'"
        );
    }

    #[test]
    fn quote_shell_arg_escapes_embedded_single_quotes() {
        assert_eq!(quote_shell_arg("it's/a/path"), "'it'\\''s/a/path'");
    }
}
