use tui_lipan::prelude::*;
use tui_lipan::style::ThemeRole;
use tui_lipan::utils::color_contrast::readable_text_color;

use crate::config::BadgeColor;
use crate::ops::focus::request_theme_picker_focus;
use crate::state::{AlertPaint, Mode, State, ThemePickerPreview, ThemePreset};
use crate::{AppRoot, Msg, schedule_theme_tick};

pub(crate) fn system_theme_from_host_colors(colors: HostTerminalColors) -> Theme {
    Theme::from_host_colors(colors)
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

/// Set hyprmux's subdued caret default while leaving explicit theme `[caret]` colors intact.
pub(crate) fn apply_default_caret_palette(theme: Theme) -> Theme {
    let accent_color = theme
        .role(ThemeRole::Accent)
        .resolved_fg()
        .filter(|color| !color.is_sentinel());
    if theme.caret.color != accent_color {
        return theme;
    }

    let caret_color = theme
        .role(ThemeRole::Base)
        .resolved_fg()
        .filter(|color| !color.is_sentinel())
        .map(|text| accent_color.map_or(text, |accent| text.blend_toward(accent, 0.40)))
        .or(accent_color);
    theme.caret_color(caret_color)
}

/// Apply the `pane.background_follows_terminal` preference on top of a freshly resolved theme,
/// then concretize its backdrop through the framework theme helper.
///
/// When `follow_terminal` is set, `surface.backdrop` is pinned to the transparency sentinel
/// regardless of what the active theme authored - including a preset or custom file that already
/// set a concrete color - so it always resolves to the host terminal's background. When unset,
/// the theme's own `backdrop` is left as authored (concrete, or a sentinel from a custom file);
/// `Theme::concretize_backdrop` still resolves any sentinel so nothing collapses to black.
pub(crate) fn apply_backdrop_policy(
    theme: Theme,
    host_bg: Option<Color>,
    follow_terminal: bool,
) -> Theme {
    let mut theme = apply_default_caret_palette(theme);
    if follow_terminal {
        theme.surface.backdrop = Color::Backdrop;
    }
    theme.surface.backdrop = theme.concretize_backdrop(host_bg);
    theme
}

/// Re-resolve the active theme from `config.theme.name` and reapply the current backdrop
/// policy. Used when flipping `pane.background_follows_terminal`, since the already-resolved
/// `state.theme` may have had its `surface.backdrop` overwritten by a previous policy pass and
/// can no longer tell us what the theme itself authored.
pub(crate) fn reapply_active_theme(ctx: &mut Context<AppRoot>) -> Update {
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

pub(crate) fn theme_tick(ctx: &mut Context<AppRoot>) -> Update {
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

pub(crate) fn open_theme_picker(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.theme_picker_preview.is_none() {
        ctx.state.theme_picker_preview = Some(ThemePickerPreview {
            theme: ctx.state.theme.clone(),
        });
    }
    // Open highlighting the active theme; from here the highlight is user-owned (see
    // `theme_picker_selected`), so filtering no longer snaps back to this row.
    let current = &ctx.state.config.theme.name;
    ctx.state.theme_picker_selected = crate::config::theme_choices()
        .iter()
        .position(|choice| &choice.id() == current);
    ctx.state.show_theme_picker = true;
    // Opened from the Appearance dialog's `Theme` row, cancelling or picking a theme returns
    // there; opened standalone (keybinding, palette) it leads back to the pane.
    ctx.state.overlay_return = ctx
        .state
        .show_appearance
        .then_some(crate::state::OverlayOrigin::Appearance);
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.show_appearance = false;
    ctx.state.pane_padding_editor = None;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    request_theme_picker_focus(ctx);
    Update::full()
}

pub(crate) fn preview_theme(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if !ctx.state.show_theme_picker {
        return Update::none();
    }
    let choices = crate::config::theme_choices();
    let Some(choice) = choices.get(index) else {
        return Update::none();
    };
    // Remember the highlighted row so the palette stays on it across query changes.
    ctx.state.theme_picker_selected = Some(index);
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

pub(crate) fn cancel_theme_picker(ctx: &mut Context<AppRoot>) {
    if let Some(preview) = ctx.state.theme_picker_preview.take() {
        ctx.state.theme = preview.theme;
        apply_terminal_palette_to_state(&mut ctx.state);
    }
    ctx.state.theme_picker_selected = None;
    ctx.state.show_theme_picker = false;
    ctx.state.commands_dirty = true;
}

/// Picking a theme finishes the errand it was opened for, so it leaves the whole dialog stack —
/// including the Appearance list it may have been raised from — rather than stepping back one level
/// into a dialog the user is done with. Cancelling still returns to Appearance.
fn close_after_theme_pick(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::overlay_return::leave(ctx);
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn select_theme(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let choices = crate::config::theme_choices();
    let Some(choice) = choices.get(index) else {
        ctx.state.theme_picker_selected = None;
        ctx.state.show_theme_picker = false;
        ctx.state.commands_dirty = true;
        return close_after_theme_pick(ctx);
    };
    let name = choice.id();
    let system_theme = ctx.state.system_theme.clone();
    let resolved = crate::config::resolve_theme(&name, system_theme.as_ref());
    for warning in &resolved.warnings {
        crate::pty_events::notify_error(ctx, "Theme warning", warning.clone());
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
                crate::pty_events::notify_error(ctx, "Theme watch failed", err.to_string());
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
    ctx.state.theme_picker_selected = None;
    apply_terminal_palette_to_state(&mut ctx.state);
    ctx.state.show_theme_picker = false;
    ctx.state.commands_dirty = true;
    let closed = close_after_theme_pick(ctx);
    if let Err(err) = crate::config::persist_theme_name(&name) {
        crate::pty_events::notify_error(ctx, "Theme not saved", err);
    }

    if start_tick {
        Update::with_command(schedule_theme_tick())
    } else {
        closed
    }
}

pub(crate) fn apply_terminal_palette_to_state(state: &mut State) -> bool {
    // Bound to a local clone rather than `&state.theme`: the loop below mutably borrows the whole
    // `State` through `current_mut()`, so a live `&state.theme` borrow would conflict.
    let theme_owned = state.theme.clone();
    let theme = &theme_owned;
    let client = state.current().session_client.clone();
    let highlight_focused_background = state.config.pane.highlight_focused_background;
    let mut changed = false;
    let active_index = state.current().active_workspace;
    let active_focus = state.current().focused_pane;
    for (index, workspace) in state.current_mut().workspaces.iter_mut().enumerate() {
        let focused_pane = if index == active_index {
            active_focus
        } else {
            workspace.focused_pane
        };
        for pane in &mut workspace.panes {
            let background = pane_frame_background(
                theme,
                focused_pane == Some(pane.id),
                highlight_focused_background,
            );
            let palette = TerminalColorPalette::from_theme(theme, background);
            let pane_changed = pane.terminal.set_palette(palette);
            changed |= pane_changed;
            if pane_changed && let Some(client) = &client {
                client.set_palette(pane.id, pane.pty_generation, palette);
            }
        }
    }
    if let Some(scratch) = state.scratch.as_mut() {
        changed |= scratch
            .terminal
            .set_palette(TerminalColorPalette::from_theme(
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
        theme
            .border
            .resolved_fg()
            .filter(|color| !color.is_sentinel())
            .or_else(|| {
                theme
                    .muted
                    .resolved_fg()
                    .filter(|color| !color.is_sentinel())
            })
            .unwrap_or(theme.surface.menu),
        pane_frame_background(theme, false, false),
        theme
            .primary
            .resolved_fg()
            .filter(|color| !color.is_sentinel())
            .or_else(|| {
                theme
                    .muted
                    .resolved_fg()
                    .filter(|color| !color.is_sentinel())
            })
            .unwrap_or(Color::Gray),
    )
}

/// Resolve the shared `BadgeColor` vocabulary to its theme role. Workbar badges and pane alerts
/// intentionally use this one mapping so a role means the same thing on both surfaces.
pub(crate) fn badge_role_color(theme: &Theme, color: BadgeColor) -> Color {
    match color {
        BadgeColor::Accent => theme.border_active,
        BadgeColor::Info => theme.status.info,
        BadgeColor::Success => theme.status.success,
        BadgeColor::Warning => theme.status.warning,
        BadgeColor::Error => theme.status.error,
        BadgeColor::Neutral => theme.surface.menu,
        BadgeColor::Panel => theme.surface.panel,
    }
}

/// Readable peak color for an unfocused pane alert border.
pub(crate) fn pane_frame_alert_foreground(theme: &Theme, color: BadgeColor) -> Color {
    readable_chrome_color(
        badge_role_color(theme, color),
        pane_frame_background(theme, false, false),
        theme.status.warning,
    )
}

/// Readable alert trough, blended partway toward `neutral` so an alert never becomes quiet.
pub(crate) fn alert_trough(peak: Color, neutral: Color, background: Color, blend: f32) -> Color {
    readable_chrome_color(peak.blend_toward(neutral, blend), background, peak)
}

pub(crate) fn pane_frame_alert_trough(theme: &Theme, color: BadgeColor) -> Color {
    let peak = pane_frame_alert_foreground(theme, color);
    alert_trough(
        peak,
        pane_frame_foreground(theme, false, false),
        pane_frame_background(theme, false, false),
        crate::anim::ALERT_PULSE_BLEND,
    )
}

pub(crate) fn pane_frame_alert_can_pulse(theme: &Theme, color: BadgeColor) -> bool {
    crate::app::chrome_colors_animate(
        pane_frame_alert_foreground(theme, color),
        pane_frame_alert_trough(theme, color),
    )
}

pub(crate) fn pane_frame_alert_color(
    theme: &Theme,
    color: BadgeColor,
    pulse: bool,
    trough_phase: bool,
) -> Color {
    if pulse && trough_phase {
        pane_frame_alert_trough(theme, color)
    } else {
        pane_frame_alert_foreground(theme, color)
    }
}

/// The peak background of a marked workspace tab: the panel surface tinted toward the alert role.
///
/// Tabs mark on background where pane borders mark on foreground. A pane border *is* a foreground
/// glyph, so colouring it is the only option; a tab is a filled region in a permanently visible bar,
/// where a recoloured glyph is too quiet to catch peripheral vision. Tinting rather than filling
/// keeps it short of the active tab's solid pill.
pub(crate) fn tab_alert_background(theme: &Theme, color: BadgeColor) -> Color {
    theme
        .surface
        .panel
        .blend_toward(badge_role_color(theme, color), crate::anim::ALERT_TAB_TINT)
}

/// The trough background: the plain panel surface, so the tab breathes between neutral and its role
/// colour. Unlike a pane border, reaching neutral loses nothing - the label is still there and the
/// tint is the whole signal, so only the tint comes and goes.
pub(crate) fn tab_alert_background_trough(theme: &Theme) -> Color {
    theme.surface.panel
}

/// The label colour a marked tab ends the breathe on: what the renderer's contrast policy would
/// pick for the *peak* tint, computed once here instead of per frame.
///
/// Leaving the policy to the renderer re-derives the foreground against whatever the background
/// holds mid-fade, so a label flips white then black as the tint crosses the readability threshold.
/// Resolving against the final background and animating toward that value turns the flip into the
/// same fade the background is already doing. The tab style pairs this with
/// `ContrastPolicy::Off` - without that the renderer would overwrite it again every frame.
pub(crate) fn tab_alert_foreground(theme: &Theme, color: BadgeColor) -> Color {
    tab_label_on(theme, tab_alert_background(theme, color))
}

/// The tab label colour resolved for `background`.
///
/// Every endpoint of a marked tab's fade goes through this, not just the tinted one: the tab opts
/// out of the renderer's contrast policy so the label can fade instead of flipping, which also means
/// nothing downstream will rescue an unreadable pair. Some themes (Lipan) put `surface.menu` below
/// the readable threshold on `surface.panel` already, so even the untinted end needs resolving.
fn tab_label_on(theme: &Theme, background: Color) -> Color {
    readable_text_color(Some(theme.surface.menu), background)
}

/// The label colour of an unmarked tab, and so the resting end of a marked tab's fade.
pub(crate) fn tab_foreground(theme: &Theme) -> Color {
    tab_label_on(theme, tab_alert_background_trough(theme))
}

/// Whether a marked tab's breathe can actually fade, for the channel that moves.
///
/// Derived from the endpoint pairs rather than hardcoding the background, because the two paints
/// move different channels: `background` fades the fill, `text` fades only the label. Requiring the
/// *unmoved* channel to be animatable would disable the breathe for no reason, and checking only the
/// background would let a palette-theme label snap mid-breathe under `text`.
pub(crate) fn tab_alert_can_pulse(theme: &Theme, color: BadgeColor, paint: AlertPaint) -> bool {
    let (peak_fg, peak_bg) = tab_alert_colors(theme, color, paint, true, false);
    let (trough_fg, trough_bg) = tab_alert_colors(theme, color, paint, true, true);
    let channel_ok = |peak: Color, trough: Color| {
        peak == trough || crate::app::chrome_colors_animate(peak, trough)
    };
    (peak_fg != trough_fg || peak_bg != trough_bg)
        && channel_ok(peak_fg, trough_fg)
        && channel_ok(peak_bg, trough_bg)
}

/// The `(foreground, background)` a marked tab paints this frame.
///
/// Returned as a pair rather than two calls because the foreground is only correct *for its own
/// background*: computing them against different ends of the breathe is exactly the mismatch that
/// produces an unreadable label. A static tab rests on the peak, so it takes the peak's foreground.
pub(crate) fn tab_alert_colors(
    theme: &Theme,
    color: BadgeColor,
    paint: AlertPaint,
    pulse: bool,
    trough_phase: bool,
) -> (Color, Color) {
    let resting = (tab_foreground(theme), tab_alert_background_trough(theme));
    if pulse && trough_phase {
        return resting;
    }
    match paint {
        AlertPaint::Background => (
            tab_alert_foreground(theme, color),
            tab_alert_background(theme, color),
        ),
        // The label carries the alert and the tab stays flat, so the colour is read against the
        // untinted panel - the same contrast question, just a different background.
        AlertPaint::Text => (
            readable_text_color(
                Some(badge_role_color(theme, color)),
                tab_alert_background_trough(theme),
            ),
            resting.1,
        ),
    }
}

pub(crate) fn pane_title_foreground(theme: &Theme, focused: bool, background: Color) -> Color {
    let fallback = theme
        .primary
        .resolved_fg()
        .filter(|color| !color.is_sentinel())
        .unwrap_or_else(|| fallback_text_color(background));
    let preferred = if focused {
        theme.surface.backdrop
    } else {
        fallback
    };
    readable_chrome_color(preferred, background, fallback)
}

pub(crate) fn pane_border_title_foreground(
    theme: &Theme,
    focused: bool,
    background: Color,
) -> Color {
    if focused {
        return readable_chrome_color(
            theme.border_active,
            background,
            theme
                .primary
                .resolved_fg()
                .filter(|color| !color.is_sentinel())
                .unwrap_or_else(|| fallback_text_color(background)),
        );
    }

    pane_title_foreground(theme, false, background)
}

fn readable_chrome_color(preferred: Color, background: Color, fallback: Color) -> Color {
    let preferred = preferred.resolve(Color::Reset);
    if is_readable_chrome_pair(preferred, background) {
        return preferred;
    }

    let fallback = fallback.resolve(Color::Reset);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::{Pane, PaneId};

    /// Both ends of a marked tab's breathe must be readable *on their own background*. The bug this
    /// pins is pairing a foreground with the wrong end: the renderer's own contrast policy is off
    /// for these tabs, so nothing downstream will rescue a mismatched pair.
    #[test]
    fn tab_alert_colors_stay_readable_at_both_ends_of_the_breathe() {
        for preset in ThemePreset::all() {
            let theme = crate::config::resolve_theme(preset.id(), None).theme;
            for role in [BadgeColor::Error, BadgeColor::Success, BadgeColor::Info] {
                for paint in AlertPaint::all().iter().copied() {
                    for (pulse, trough) in
                        [(true, false), (true, true), (false, false), (false, true)]
                    {
                        let (fg, bg) = tab_alert_colors(&theme, role, paint, pulse, trough);
                        assert_eq!(
                            fg,
                            readable_text_color(Some(fg), bg),
                            "{preset:?}/{role:?}/{paint:?} pulse={pulse} trough={trough}: label is \
                             not readable on the background it is painted on"
                        );
                    }
                }
            }
        }
    }

    /// A static tab never reaches the trough, so it must take the peak's foreground rather than the
    /// resting one — the mismatch that would otherwise appear only outside `pulse` mode.
    #[test]
    fn a_static_tab_takes_the_peak_pair() {
        let theme = crate::config::resolve_theme(ThemePreset::TokyoNight.id(), None).theme;
        for paint in AlertPaint::all().iter().copied() {
            let peak = tab_alert_colors(&theme, BadgeColor::Error, paint, true, false);
            assert_eq!(
                tab_alert_colors(&theme, BadgeColor::Error, paint, false, false),
                peak
            );
            assert_eq!(
                tab_alert_colors(&theme, BadgeColor::Error, paint, false, true),
                peak
            );
        }
    }

    /// The two paints must move different channels, or `text` would just be a second name for the
    /// background variant.
    #[test]
    fn the_two_paints_move_different_channels() {
        let theme = crate::config::resolve_theme(ThemePreset::TokyoNight.id(), None).theme;
        let role = BadgeColor::Error;
        let (bg_fg, bg_bg) = tab_alert_colors(&theme, role, AlertPaint::Background, true, false);
        let (text_fg, text_bg) = tab_alert_colors(&theme, role, AlertPaint::Text, true, false);
        let (rest_fg, rest_bg) = tab_alert_colors(&theme, role, AlertPaint::Background, true, true);

        assert_ne!(bg_bg, rest_bg, "background paint must tint the fill");
        assert_eq!(
            text_bg, rest_bg,
            "text paint must leave the fill at the resting panel colour"
        );
        assert_ne!(text_fg, rest_fg, "text paint must colour the label");
        assert_ne!(text_fg, bg_fg, "the two paints resolve different labels");
    }

    /// The pulse gate must follow the channel each paint actually moves, or `text` would be judged
    /// on a background that never changes.
    #[test]
    fn each_paint_can_pulse_on_its_own_channel() {
        let theme = crate::config::resolve_theme(ThemePreset::TokyoNight.id(), None).theme;
        for paint in AlertPaint::all().iter().copied() {
            assert!(
                tab_alert_can_pulse(&theme, BadgeColor::Error, paint),
                "{paint:?} cannot pulse on a truecolor theme"
            );
        }
    }

    fn pane_palette_background(state: &State, id: PaneId) -> Option<Color> {
        state.current().workspaces[0]
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
    fn default_caret_palette_blends_text_toward_accent() {
        let theme = Theme::custom(
            Color::rgb(100, 100, 100),
            Color::Black,
            Color::rgb(200, 50, 0),
        );

        let resolved = apply_default_caret_palette(theme);

        assert_eq!(resolved.caret.color, Some(Color::rgb(140, 80, 60)));
    }

    #[test]
    fn explicit_caret_color_is_preserved() {
        let explicit = Color::rgb(12, 34, 56);
        let theme = Theme::custom(Color::White, Color::Black, Color::Cyan).caret_color(explicit);

        let resolved = apply_default_caret_palette(theme);

        assert_eq!(resolved.caret.color, Some(explicit));
    }

    #[test]
    fn terminal_palette_background_respects_focused_background_config() {
        let theme = ThemePreset::OneDark.theme();
        let mut state = State::new(Config::default(), theme.clone());
        let scrollback = state.config.scrollback;
        state.current_mut().workspaces[0].panes.push(Pane::new(
            2,
            scrollback,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        ));

        state.current_mut().focused_pane = Some(1);
        state.current_mut().workspaces[0].focused_pane = Some(1);
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

        state.current_mut().focused_pane = Some(2);
        state.current_mut().workspaces[0].focused_pane = Some(2);
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

        let palette = TerminalColorPalette::from_theme(&theme, pane_background);

        assert_eq!(palette.foreground, Some(colors.fg));
        assert_eq!(palette.background, Some(pane_background));
        assert_eq!(palette.ansi, colors.ansi);
    }

    #[test]
    fn host_terminal_palette_background_still_follows_pane_background() {
        let colors = host_colors();
        let theme = system_theme_from_host_colors(colors);
        let mut state = State::new(Config::default(), theme.clone());
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
    fn alert_peak_and_trough_are_readable_and_stay_alert_colored() {
        for preset in ThemePreset::all() {
            let theme = preset.theme();
            let background = pane_frame_background(&theme, false, false);
            let error_peak = pane_frame_alert_foreground(&theme, BadgeColor::Error);
            let success_peak = pane_frame_alert_foreground(&theme, BadgeColor::Success);
            for role in [BadgeColor::Error, BadgeColor::Success] {
                let peak = pane_frame_alert_foreground(&theme, role);
                let trough = pane_frame_alert_trough(&theme, role);
                assert!(
                    is_readable_chrome_pair(peak, background),
                    "{preset:?} {role:?}"
                );
                assert!(
                    is_readable_chrome_pair(trough, background),
                    "{preset:?} {role:?}"
                );
                if pane_frame_alert_can_pulse(&theme, role) {
                    assert_ne!(peak, trough, "{preset:?} {role:?}");
                }
            }
            if pane_frame_alert_can_pulse(&theme, BadgeColor::Error)
                && pane_frame_alert_can_pulse(&theme, BadgeColor::Success)
            {
                assert_ne!(error_peak, success_peak, "{preset:?}");
            }
        }
    }

    #[test]
    fn transparent_backdrop_theme_never_paints_panes_black() {
        // Regression: extending nord with `backdrop = "backdrop"` used to force unfocused (and, in
        // spawn animations, freshly created) pane backgrounds to pitch black. After concretizing to
        // the host bg, an unfocused pane must render on that concrete surface, never black.
        let host_bg = Color::rgb(10, 11, 12);
        let mut theme = ThemePreset::Nord.theme();
        theme.surface.backdrop = Color::Backdrop;
        theme.surface.backdrop = theme.concretize_backdrop(Some(host_bg));

        let mut state = State::new(Config::default(), theme.clone());
        state.current_mut().focused_pane = Some(1);
        state.current_mut().workspaces[0].focused_pane = Some(1);

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

        let resolved = theme.concretize_backdrop(Some(host_bg));

        assert_eq!(resolved, host_bg);
        assert!(resolved.to_rgb().is_some());
    }

    #[test]
    fn concretize_backdrop_falls_back_to_panel_without_host_colors() {
        let mut theme = ThemePreset::Nord.theme();
        theme.surface.backdrop = Color::Transparent;
        let panel = theme.surface.panel;

        let resolved = theme.concretize_backdrop(None);

        assert_eq!(resolved, panel);
    }

    #[test]
    fn concretize_backdrop_leaves_concrete_backdrops_untouched() {
        let theme = ThemePreset::Nord.theme();
        let original = theme.surface.backdrop;

        let resolved = theme.concretize_backdrop(Some(Color::rgb(1, 2, 3)));

        assert_eq!(resolved, original);
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

    #[test]
    fn unfocused_title_prefers_primary_text_over_muted() {
        let mut theme = ThemePreset::Ansi.theme();
        theme.primary = Style::new().fg(Color::LightBlue);
        theme.muted = Style::new().fg(Color::Yellow);

        assert_eq!(
            pane_title_foreground(&theme, false, Color::Black),
            Color::LightBlue
        );
    }

    #[test]
    fn focused_border_title_prefers_active_border_color() {
        let mut theme = ThemePreset::Ansi.theme();
        theme.border_active = Color::rgb(220, 170, 45);

        assert_eq!(
            pane_border_title_foreground(&theme, true, Color::Black),
            theme.border_active
        );
    }

    #[test]
    fn disabled_focused_titlebar_keeps_border_title_unfocused() {
        let theme = ThemePreset::Ansi.theme();

        assert_eq!(
            pane_border_title_foreground(&theme, false, Color::Black),
            pane_title_foreground(&theme, false, Color::Black)
        );
    }
}
