use std::time::Duration;

use tui_lipan::prelude::*;

use crate::pane_lifecycle::spawn_pane_in_workspace;
use crate::state::{PaneIdentity, ThemePreset};
use crate::{HyprmuxApp, Msg};

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
        ctx.toast()
            .push(crate::pty_events::info_toast("Config reloaded"));
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
