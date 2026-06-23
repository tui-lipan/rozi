use tui_lipan::prelude::*;

use crate::focus_ops::request_theme_picker_focus;
use crate::state::{Mode, State, ThemePickerPreview, ThemePreset};
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
    if ctx.state.theme_picker_preview.is_none() {
        ctx.state.theme_picker_preview = Some(ThemePickerPreview {
            theme: ctx.state.theme.clone(),
        });
    }
    ctx.state.show_theme_picker = true;
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_theme_picker_focus(ctx);
    Update::full()
}

pub(crate) fn preview_theme(ctx: &mut Context<HyprmuxApp>, preset: ThemePreset) -> Update {
    if !ctx.state.show_theme_picker {
        return Update::none();
    }
    if ctx.state.theme_picker_preview.is_none() {
        ctx.state.theme_picker_preview = Some(ThemePickerPreview {
            theme: ctx.state.theme.clone(),
        });
    }
    ctx.state.theme = preset.theme();
    apply_terminal_palette_to_state(&mut ctx.state);
    Update::full()
}

pub(crate) fn cancel_theme_picker(ctx: &mut Context<HyprmuxApp>) {
    if let Some(preview) = ctx.state.theme_picker_preview.take() {
        ctx.state.theme = preview.theme;
        apply_terminal_palette_to_state(&mut ctx.state);
    }
    ctx.state.show_theme_picker = false;
}

pub(crate) fn select_theme(ctx: &mut Context<HyprmuxApp>, preset: ThemePreset) {
    ctx.state.config.theme.preset = preset;
    ctx.state.config.theme.path = None;
    ctx.state.theme_watcher = None;
    ctx.state.theme = preset.theme();
    ctx.state.theme_picker_preview = None;
    apply_terminal_palette_to_state(&mut ctx.state);
    ctx.state.show_theme_picker = false;
    ctx.toast().push(crate::pty_events::info_toast(format!(
        "Theme: {}",
        preset.label()
    )));
}

pub(crate) fn apply_terminal_palette_to_state(state: &mut State) -> bool {
    let theme = &state.theme;
    let mut changed = false;
    for (index, workspace) in state.workspaces.iter_mut().enumerate() {
        let focused_pane = if index == state.active_workspace {
            state.focused_pane
        } else {
            workspace.focused_pane
        };
        for pane in &mut workspace.panes {
            let background = pane_frame_background(theme, focused_pane == Some(pane.id));
            changed |= pane
                .terminal
                .set_palette(terminal_palette(theme, background));
        }
    }
    if let Some(scratch) = state.scratch.as_mut() {
        changed |= scratch
            .terminal
            .set_palette(terminal_palette(theme, pane_frame_background(theme, true)));
    }
    changed
}

pub(crate) fn pane_frame_background(theme: &Theme, focused: bool) -> Color {
    if focused {
        theme.surface.panel
    } else {
        theme.surface.backdrop
    }
}

pub(crate) fn pane_frame_foreground(theme: &Theme, focused: bool) -> Color {
    if focused {
        return theme.border_active;
    }

    readable_chrome_color(
        style_fg(theme.border)
            .or_else(|| style_fg(theme.muted))
            .unwrap_or(theme.surface.menu),
        pane_frame_background(theme, false),
        style_fg(theme.primary)
            .or_else(|| style_fg(theme.muted))
            .unwrap_or(Color::Gray),
    )
}

pub(crate) fn pane_title_foreground(theme: &Theme, focused: bool, background: Color) -> Color {
    let preferred = if focused {
        theme.surface.backdrop
    } else {
        style_fg(theme.muted)
            .or_else(|| style_fg(theme.primary))
            .unwrap_or(Color::Gray)
    };
    readable_chrome_color(
        preferred,
        background,
        style_fg(theme.primary).unwrap_or_else(|| fallback_text_color(background)),
    )
}

pub(crate) fn terminal_palette(theme: &Theme, background: Color) -> TerminalColorPalette {
    let foreground = style_fg(theme.primary).unwrap_or(Color::White);
    let background = clean_terminal_color(background, Color::Black);
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

fn readable_chrome_color(preferred: Color, background: Color, fallback: Color) -> Color {
    let preferred = clean_terminal_color(preferred, Color::Reset);
    if is_readable_chrome_pair(preferred, background) {
        return preferred;
    }

    let fallback = clean_terminal_color(fallback, Color::Reset);
    if is_readable_chrome_pair(fallback, background) {
        return fallback;
    }

    fallback_text_color(background)
}

fn is_readable_chrome_pair(foreground: Color, background: Color) -> bool {
    if foreground == Color::Reset || background == Color::Reset {
        return false;
    }
    if foreground == background {
        return false;
    }
    (foreground.luminance() - background.luminance()).abs() >= 0.18
}

fn fallback_text_color(background: Color) -> Color {
    if background.is_dark() {
        Color::White
    } else {
        Color::Black
    }
}

fn clean_terminal_color(color: Color, fallback: Color) -> Color {
    match color {
        Color::Reset | Color::Backdrop | Color::Transparent => fallback,
        _ => color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{HyprmuxConfig, Pane, PaneId};

    fn pane_palette_background(state: &State, id: PaneId) -> Option<Color> {
        state.workspaces[0]
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .expect("pane should exist")
            .terminal
            .screen
            .palette()
            .background
    }

    #[test]
    fn terminal_palette_background_matches_frame_focus() {
        let theme = ThemePreset::OneDark.theme();
        let mut state = State::new(HyprmuxConfig::default(), theme.clone());
        state.workspaces[0].panes.push(Pane::new(
            2,
            state.config.scrollback,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));

        state.focused_pane = Some(1);
        state.workspaces[0].focused_pane = Some(1);
        assert!(apply_terminal_palette_to_state(&mut state));
        assert_eq!(
            pane_palette_background(&state, 1),
            Some(theme.surface.panel)
        );
        assert_eq!(
            pane_palette_background(&state, 2),
            Some(theme.surface.backdrop)
        );

        state.focused_pane = Some(2);
        state.workspaces[0].focused_pane = Some(2);
        assert!(apply_terminal_palette_to_state(&mut state));
        assert_eq!(
            pane_palette_background(&state, 1),
            Some(theme.surface.backdrop)
        );
        assert_eq!(
            pane_palette_background(&state, 2),
            Some(theme.surface.panel)
        );
    }

    #[test]
    fn ansi_inactive_chrome_is_visible_on_black_surfaces() {
        let theme = ThemePreset::Ansi.theme();
        assert_ne!(pane_frame_foreground(&theme, false), Color::Black);
        assert_ne!(
            pane_title_foreground(&theme, false, theme.surface.element),
            Color::Black
        );
    }

    #[test]
    fn unreadable_chrome_falls_back_to_primary_text() {
        let mut theme = ThemePreset::Ansi.theme();
        theme.border = Style::new().fg(Color::Black);
        theme.muted = Style::new().fg(Color::Black);
        theme.primary = Style::new().fg(Color::Gray).bg(Color::Black);

        assert_eq!(pane_frame_foreground(&theme, false), Color::Gray);
        assert_eq!(
            pane_title_foreground(&theme, false, Color::Black),
            Color::Gray
        );
    }
}
