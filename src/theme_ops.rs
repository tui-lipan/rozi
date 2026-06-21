use tui_lipan::prelude::*;

use crate::focus_ops::request_theme_picker_focus;
use crate::state::{Mode, State, ThemePreset};
use crate::{HyprmuxApp, Msg, schedule_theme_tick};

pub(crate) fn theme_tick(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(watcher) = ctx.state.theme_watcher.as_ref() else {
        return Update::none();
    };

    let mut newest_theme = None;
    while let Some(theme) = watcher.try_recv() {
        newest_theme = Some(theme);
    }
    let mut errors = Vec::new();
    while let Some(err) = watcher.try_recv_error() {
        errors.push(err);
    }

    for err in errors {
        ctx.link().send(Msg::ThemeError(err));
    }

    if let Some(theme) = newest_theme {
        ctx.state.theme = theme;
        apply_terminal_palette_to_state(&mut ctx.state);
        return Update::with_command(schedule_theme_tick());
    }
    Update::command_only(schedule_theme_tick())
}

pub(crate) fn open_theme_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_theme_picker = true;
    ctx.state.theme_picker_selected = ctx.state.config.theme.preset.index();
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_theme_picker_focus(ctx);
    Update::full()
}

pub(crate) fn select_theme(ctx: &mut Context<HyprmuxApp>, preset: ThemePreset) {
    ctx.state.config.theme.preset = preset;
    ctx.state.config.theme.path = None;
    ctx.state.theme_watcher = None;
    ctx.state.theme = preset.theme();
    apply_terminal_palette_to_state(&mut ctx.state);
    ctx.state.show_theme_picker = false;
    ctx.toast().push(crate::pty_events::info_toast(format!(
        "Theme: {}",
        preset.label()
    )));
}

pub(crate) fn apply_terminal_palette_to_state(state: &mut State) {
    let palette = terminal_palette(&state.theme);
    for workspace in &mut state.workspaces {
        for pane in &mut workspace.panes {
            pane.terminal.set_palette(palette);
        }
    }
}

pub(crate) fn terminal_palette(theme: &Theme) -> TerminalColorPalette {
    let foreground = style_fg(theme.primary).unwrap_or(Color::White);
    let background = clean_terminal_color(theme.surface.backdrop, Color::Black);
    let muted = style_fg(theme.muted).unwrap_or(theme.surface.menu);
    let accent = style_fg(theme.accent).unwrap_or(theme.border_active);
    let purple = theme.file_icons.purple;
    let cyan = theme.file_icons.cyan;

    TerminalColorPalette::new(
        foreground,
        background,
        [
            background,
            theme.status.error,
            theme.status.success,
            theme.status.warning,
            theme.status.info,
            purple,
            cyan,
            foreground,
            muted,
            theme.status.error.lighten_by(0.18),
            theme.status.success.lighten_by(0.18),
            theme.status.warning.lighten_by(0.18),
            accent.lighten_by(0.12),
            purple.lighten_by(0.18),
            cyan.lighten_by(0.18),
            foreground.lighten_by(0.12),
        ],
    )
}

pub(crate) fn style_fg(style: Style) -> Option<Color> {
    style
        .fg
        .map(|paint| clean_terminal_color(paint.color(), Color::Reset))
        .filter(|color| *color != Color::Reset)
}

pub(crate) fn clean_terminal_color(color: Color, fallback: Color) -> Color {
    match color {
        Color::Reset | Color::Backdrop | Color::Transparent => fallback,
        _ => color,
    }
}
