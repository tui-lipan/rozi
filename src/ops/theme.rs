use tui_lipan::prelude::*;

use crate::ops::focus::request_theme_picker_focus;
use crate::state::{Mode, State, ThemePickerPreview, ThemePreset};
use crate::{HyprmuxApp, Msg, schedule_theme_tick};

pub(crate) fn system_theme_from_host_colors(colors: HostTerminalColors) -> Theme {
    Theme::from_host_colors(colors).with_extension(colors)
}

/// The host terminal's probed default background, if hyprmux queried it at startup. Carried on the
/// derived `system_theme` as a [`HostTerminalColors`] extension so it stays available regardless of
/// which theme is active.
pub(crate) fn host_background(state: &State) -> Option<Color> {
    state
        .system_theme
        .as_ref()
        .and_then(|theme| theme.extension::<HostTerminalColors>())
        .map(|colors| colors.bg)
}

/// Resolve a transparency-sentinel `surface.backdrop` to a concrete color.
///
/// A custom theme that extends a preset with `backdrop = "backdrop"` (or `"transparent"`/`"reset"`)
/// lands a sentinel [`Color`] with no RGB. That leaks into every consumer that needs a real color -
/// the terminal default background reported to OSC 11 background queries, embedded-pane default-bg
/// cells, workbar badge text and end caps - each of which then falls back to pitch black. Pin the
/// backdrop to the host terminal's own background so those surfaces track the real terminal bg
/// instead of collapsing to black; fall back to the theme's panel surface when the host bg is
/// unknown (e.g. the startup color query failed).
///
/// This deliberately snapshots the sentinel to a *concrete* color rather than preserving literal
/// pass-through transparency: a queried color keeps every consumer (including OSC 11 replies and
/// contrast math, which cannot be transparent) on the terminal's background, and leaves no unset
/// channel that a future consumer could accidentally leak black through again. The cost is that a
/// live wallpaper/blur behind the terminal is matched by color, not shown through the panes. If
/// real pass-through is ever wanted, add it as a *separate* token (e.g. `backdrop = "transparent"`)
/// that this resolver skips - keep `"backdrop"` meaning "the terminal's background color". Do not
/// "fix" this back into leaving the channel unset; that reintroduces the black-surface bug.
pub(crate) fn concretize_backdrop(mut theme: Theme, host_bg: Option<Color>) -> Theme {
    if theme.surface.backdrop.to_rgb().is_none() {
        theme.surface.backdrop = host_bg
            .filter(|color| color.to_rgb().is_some())
            .unwrap_or(theme.surface.panel);
    }
    theme
}

/// Apply the `pane.background_follows_terminal` preference on top of a freshly resolved theme,
/// then run it through [`concretize_backdrop`].
///
/// When `follow_terminal` is set, `surface.backdrop` is pinned to the transparency sentinel
/// regardless of what the active theme authored - including a preset or custom file that already
/// set a concrete color - so it always resolves to the host terminal's background. When unset,
/// the theme's own `backdrop` is left as authored (concrete, or a sentinel from a custom file);
/// `concretize_backdrop` still resolves any sentinel so nothing collapses to black.
pub(crate) fn apply_backdrop_policy(
    mut theme: Theme,
    host_bg: Option<Color>,
    follow_terminal: bool,
) -> Theme {
    if follow_terminal {
        theme.surface.backdrop = Color::Backdrop;
    }
    concretize_backdrop(theme, host_bg)
}

/// Re-resolve the active theme from `config.theme.name` and reapply the current backdrop
/// policy. Used when flipping `pane.background_follows_terminal`, since the already-resolved
/// `state.theme` may have had its `surface.backdrop` overwritten by a previous policy pass and
/// can no longer tell us what the theme itself authored.
pub(crate) fn reapply_active_theme(ctx: &mut Context<HyprmuxApp>) -> Update {
    let system_theme = ctx.state.system_theme.clone();
    let resolved =
        crate::config::resolve_theme(&ctx.state.config.theme.name, system_theme.as_ref());
    let host_bg = host_background(&ctx.state);
    ctx.state.theme = apply_backdrop_policy(
        resolved.theme,
        host_bg,
        ctx.state.config.pane.background_follows_terminal,
    );
    apply_terminal_palette_to_state(&mut ctx.state);
    Update::full()
}

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
        let host_bg = host_background(&ctx.state);
        ctx.state.theme = apply_backdrop_policy(
            theme,
            host_bg,
            ctx.state.config.pane.background_follows_terminal,
        );
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
    ctx.state.show_appearance = false;
    ctx.state.pane_padding_editor = None;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_theme_picker_focus(ctx);
    Update::full()
}

pub(crate) fn preview_theme(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    if !ctx.state.show_theme_picker {
        return Update::none();
    }
    let choices = crate::config::theme_choices();
    let Some(choice) = choices.get(index) else {
        return Update::none();
    };
    if ctx.state.theme_picker_preview.is_none() {
        ctx.state.theme_picker_preview = Some(ThemePickerPreview {
            theme: ctx.state.theme.clone(),
        });
    }
    let system_theme = ctx.state.system_theme.clone();
    let resolved = crate::config::resolve_theme(&choice.id(), system_theme.as_ref()).theme;
    let host_bg = host_background(&ctx.state);
    ctx.state.theme = apply_backdrop_policy(
        resolved,
        host_bg,
        ctx.state.config.pane.background_follows_terminal,
    );
    apply_terminal_palette_to_state(&mut ctx.state);
    Update::full()
}

pub(crate) fn cancel_theme_picker(ctx: &mut Context<HyprmuxApp>) {
    if let Some(preview) = ctx.state.theme_picker_preview.take() {
        ctx.state.theme = preview.theme;
        apply_terminal_palette_to_state(&mut ctx.state);
    }
    ctx.state.show_theme_picker = false;
    ctx.state.commands_dirty = true;
}

pub(crate) fn select_theme(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let choices = crate::config::theme_choices();
    let Some(choice) = choices.get(index) else {
        ctx.state.show_theme_picker = false;
        ctx.state.commands_dirty = true;
        return Update::full();
    };
    let name = choice.id();
    let system_theme = ctx.state.system_theme.clone();
    let resolved = crate::config::resolve_theme(&name, system_theme.as_ref());
    for warning in &resolved.warnings {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Theme warning",
            warning.clone(),
        ));
    }

    // Watch the active theme's file only when it is a custom file; start the reload tick loop
    // if it was not already running (switching between custom files keeps it running).
    let had_watcher = ctx.state.theme_watcher.is_some();
    ctx.state.theme_watcher = None;
    let mut start_tick = false;
    if let Some(path) = &resolved.watch_path {
        match ThemeWatcher::new(path.clone(), ThemePreset::Lipan.theme()) {
            Ok(watcher) => {
                ctx.state.theme_watcher = Some(watcher);
                start_tick = !had_watcher;
            }
            Err(err) => {
                ctx.toast().push(crate::pty_events::error_toast(
                    &ctx.state.theme,
                    "Theme watch failed",
                    err.to_string(),
                ));
            }
        }
    }

    ctx.state.config.theme.name = name.clone();
    let host_bg = host_background(&ctx.state);
    ctx.state.theme = apply_backdrop_policy(
        resolved.theme,
        host_bg,
        ctx.state.config.pane.background_follows_terminal,
    );
    ctx.state.theme_picker_preview = None;
    apply_terminal_palette_to_state(&mut ctx.state);
    ctx.state.show_theme_picker = false;
    ctx.state.commands_dirty = true;
    if let Err(err) = crate::config::persist_theme_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Theme not saved",
            err,
        ));
    }

    if start_tick {
        Update::with_command(schedule_theme_tick())
    } else {
        Update::full()
    }
}

pub(crate) fn apply_terminal_palette_to_state(state: &mut State) -> bool {
    let theme = &state.theme;
    let client = state.session_client.clone();
    let highlight_focused_background = state.config.pane.highlight_focused_background;
    let mut changed = false;
    for (index, workspace) in state.workspaces.iter_mut().enumerate() {
        let focused_pane = if index == state.active_workspace {
            state.focused_pane
        } else {
            workspace.focused_pane
        };
        for pane in &mut workspace.panes {
            let background = pane_frame_background(
                theme,
                focused_pane == Some(pane.id),
                highlight_focused_background,
            );
            let palette = terminal_palette(theme, background);
            let pane_changed = pane.terminal.set_palette(palette);
            changed |= pane_changed;
            if pane_changed && let Some(client) = &client {
                client.set_palette(pane.id, pane.pty_generation, palette);
            }
        }
    }
    if let Some(scratch) = state.scratch.as_mut() {
        changed |= scratch.terminal.set_palette(terminal_palette(
            theme,
            pane_frame_background(theme, true, highlight_focused_background),
        ));
    }
    changed
}

pub(crate) fn pane_frame_background(
    theme: &Theme,
    focused: bool,
    highlight_focused_background: bool,
) -> Color {
    if focused && highlight_focused_background {
        theme.surface.panel
    } else {
        theme.surface.backdrop
    }
}

pub(crate) fn pane_frame_foreground(
    theme: &Theme,
    focused: bool,
    highlight_focused_border: bool,
) -> Color {
    if focused && highlight_focused_border {
        return theme.border_active;
    }

    readable_chrome_color(
        style_fg(theme.border)
            .or_else(|| style_fg(theme.muted))
            .unwrap_or(theme.surface.menu),
        pane_frame_background(theme, false, false),
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
    if let Some(host_colors) = theme.extension::<HostTerminalColors>() {
        return TerminalColorPalette::from_host_colors(*host_colors, background);
    }

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
    use crate::config::HyprmuxConfig;
    use crate::state::{Pane, PaneId};

    fn pane_palette_background(state: &State, id: PaneId) -> Option<Color> {
        state.workspaces[0]
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .expect("pane should exist")
            .terminal
            .last_palette
            .as_ref()
            .expect("palette should be cached")
            .background
    }

    fn host_colors() -> HostTerminalColors {
        let ansi = std::array::from_fn(|i| Color::rgb(i as u8, 10 + i as u8, 20 + i as u8));
        HostTerminalColors {
            ansi,
            fg: Color::rgb(230, 231, 232),
            bg: Color::rgb(10, 11, 12),
        }
    }

    #[test]
    fn terminal_palette_background_respects_focused_background_config() {
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
            Some(theme.surface.backdrop)
        );
        assert_eq!(
            pane_palette_background(&state, 2),
            Some(theme.surface.backdrop)
        );

        state.config.pane.highlight_focused_background = true;
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
    fn system_theme_terminal_palette_preserves_host_ansi_slots() {
        let colors = host_colors();
        let theme = system_theme_from_host_colors(colors);
        let pane_background = Color::rgb(1, 2, 3);

        let palette = terminal_palette(&theme, pane_background);

        assert_eq!(palette.foreground, Some(colors.fg));
        assert_eq!(palette.background, Some(pane_background));
        assert_eq!(palette.ansi, colors.ansi);
    }

    #[test]
    fn host_terminal_palette_background_still_follows_pane_background() {
        let colors = host_colors();
        let theme = system_theme_from_host_colors(colors);
        let mut state = State::new(HyprmuxConfig::default(), theme.clone());
        state.config.pane.highlight_focused_background = true;

        assert!(apply_terminal_palette_to_state(&mut state));

        assert_eq!(
            pane_palette_background(&state, 1),
            Some(theme.surface.panel)
        );
    }

    #[test]
    fn ansi_inactive_chrome_is_visible_on_black_surfaces() {
        let theme = ThemePreset::Ansi.theme();
        assert_ne!(pane_frame_foreground(&theme, false, false), Color::Black);
        assert_ne!(
            pane_title_foreground(&theme, false, theme.surface.element),
            Color::Black
        );
    }

    #[test]
    fn focused_border_highlight_is_opt_in() {
        let theme = ThemePreset::OneDark.theme();

        assert_eq!(
            pane_frame_foreground(&theme, true, false),
            pane_frame_foreground(&theme, false, false)
        );
        assert_eq!(
            pane_frame_foreground(&theme, true, true),
            theme.border_active
        );
    }

    #[test]
    fn transparent_backdrop_theme_never_paints_panes_black() {
        // Regression: extending nord with `backdrop = "backdrop"` used to force unfocused (and, in
        // spawn animations, freshly created) pane backgrounds to pitch black. After concretizing to
        // the host bg, an unfocused pane must render on that concrete surface, never black.
        let host_bg = Color::rgb(10, 11, 12);
        let mut theme = ThemePreset::Nord.theme();
        theme.surface.backdrop = Color::Backdrop;
        let theme = concretize_backdrop(theme, Some(host_bg));

        let mut state = State::new(HyprmuxConfig::default(), theme.clone());
        state.focused_pane = Some(1);
        state.workspaces[0].focused_pane = Some(1);

        assert!(apply_terminal_palette_to_state(&mut state));
        assert_eq!(pane_palette_background(&state, 1), Some(host_bg));
        assert_ne!(pane_palette_background(&state, 1), Some(Color::Black));
    }

    #[test]
    fn concretize_backdrop_pins_transparent_backdrop_to_host_background() {
        // A custom theme that extends a preset with `backdrop = "backdrop"` lands a sentinel with
        // no RGB; it must resolve to the real host terminal bg so terminal palettes, OSC 11 bg
        // queries, and the workbar stop collapsing to pitch black.
        let mut theme = ThemePreset::Nord.theme();
        theme.surface.backdrop = Color::Backdrop;
        let host_bg = Color::rgb(10, 11, 12);

        let resolved = concretize_backdrop(theme, Some(host_bg));

        assert_eq!(resolved.surface.backdrop, host_bg);
        assert!(resolved.surface.backdrop.to_rgb().is_some());
    }

    #[test]
    fn concretize_backdrop_falls_back_to_panel_without_host_colors() {
        let mut theme = ThemePreset::Nord.theme();
        theme.surface.backdrop = Color::Transparent;
        let panel = theme.surface.panel;

        let resolved = concretize_backdrop(theme, None);

        assert_eq!(resolved.surface.backdrop, panel);
    }

    #[test]
    fn concretize_backdrop_leaves_concrete_backdrops_untouched() {
        let theme = ThemePreset::Nord.theme();
        let original = theme.surface.backdrop;

        let resolved = concretize_backdrop(theme, Some(Color::rgb(1, 2, 3)));

        assert_eq!(resolved.surface.backdrop, original);
    }

    #[test]
    fn backdrop_policy_follow_terminal_overrides_a_concrete_preset_backdrop() {
        // `pane.background_follows_terminal` must win even over a preset's own concrete
        // backdrop, not just an unresolved sentinel from a custom theme file.
        let theme = ThemePreset::Nord.theme();
        let preset_backdrop = theme.surface.backdrop;
        let host_bg = Color::rgb(10, 11, 12);

        let resolved = apply_backdrop_policy(theme, Some(host_bg), true);

        assert_eq!(resolved.surface.backdrop, host_bg);
        assert_ne!(resolved.surface.backdrop, preset_backdrop);
    }

    #[test]
    fn backdrop_policy_leaves_theme_backdrop_alone_when_not_following() {
        let theme = ThemePreset::Nord.theme();
        let preset_backdrop = theme.surface.backdrop;

        let resolved = apply_backdrop_policy(theme, Some(Color::rgb(10, 11, 12)), false);

        assert_eq!(resolved.surface.backdrop, preset_backdrop);
    }

    #[test]
    fn backdrop_policy_falls_back_to_panel_when_following_without_host_colors() {
        let theme = ThemePreset::Nord.theme();
        let panel = theme.surface.panel;

        let resolved = apply_backdrop_policy(theme, None, true);

        assert_eq!(resolved.surface.backdrop, panel);
    }

    #[test]
    fn unreadable_chrome_falls_back_to_primary_text() {
        let mut theme = ThemePreset::Ansi.theme();
        theme.border = Style::new().fg(Color::Black);
        theme.muted = Style::new().fg(Color::Black);
        theme.primary = Style::new().fg(Color::Gray).bg(Color::Black);

        assert_eq!(pane_frame_foreground(&theme, false, false), Color::Gray);
        assert_eq!(
            pane_title_foreground(&theme, false, Color::Black),
            Color::Gray
        );
    }
}
