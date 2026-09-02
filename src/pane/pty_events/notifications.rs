use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::{PaneId, ToastChannel};

/// Why input was rejected, and what surfacing that rejection did to the screen.
pub(crate) struct BlockedInput {
    pub(crate) reason: String,
    pub(crate) notified: Notified,
}

pub(crate) fn input_blocked(ctx: &mut Context<AppRoot>) -> Option<BlockedInput> {
    let reason = ctx.state.pane_input_block_reason()?.to_string();
    // A held key against a read-only pane fires this at key-repeat rate. The first press pushes;
    // every repeat renews the same message in place, which costs no frame, so the toast stays up
    // for as long as the key is down without redrawing an identical view.
    let notified = notify_on(ctx, ToastChannel::InputState, None, reason.clone());
    Some(BlockedInput { reason, notified })
}

/// How long a tracked toast stays in [`crate::state::State::replaceable_toasts`] before it is
/// assumed expired and pruned. Comfortably longer than the 6s error toast, which is the longest
/// anything routed through [`notify`] can live.
const TOAST_TRACKING_TTL: std::time::Duration = std::time::Duration::from_secs(10);

/// A toast this app is still tracking, so a repeat of it can be recognized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackedToast {
    id: OverlayId,
    /// Refreshed on every push *and* renew, so pruning measures time since the toast was last
    /// known to be alive rather than since it first appeared.
    touched_at: std::time::Instant,
    /// The exact rendered text. Compared rather than hashed so a hash collision degrades to an
    /// ordinary replace instead of silently renewing an unrelated message.
    content: std::sync::Arc<str>,
}

#[cfg(test)]
impl TrackedToast {
    /// The overlay this entry points at. A renew keeps it; a replace mints a new one, which is how
    /// tests tell the two apart.
    pub(crate) fn id(&self) -> OverlayId {
        self.id
    }
}

/// Which slot a toast occupies for de-duplication purposes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToastKey {
    /// An explicit slot: the newest state wins even when its text differs, so `Layout controlled
    /// by client 2` supersedes `Layout controlled by client 1` instead of stacking beside it.
    Channel(ToastChannel),
    /// The implicit slot every unkeyed toast lands in: its own content. Only a byte-identical
    /// repeat collides, which is exactly when renewing is right.
    Content(u64),
}

/// What a [`notify`] did, and therefore whether the screen changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Notified {
    /// A new toast entered the stack.
    Pushed,
    /// An identical toast was already up; its countdown restarted in place. Nothing the renderer
    /// reads changed.
    Renewed,
}

impl Notified {
    /// The frame this notification needs. A renew is invisible by construction, so it asks for
    /// nothing — that is what keeps a held key from repainting.
    pub(crate) fn update(self) -> Update {
        match self {
            Self::Pushed => Update::full(),
            Self::Renewed => Update::none(),
        }
    }
}

pub(crate) fn content_key(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Drop tracking entries whose toasts must already have expired, bounding the map to whatever was
/// raised in the last [`TOAST_TRACKING_TTL`]. Without this, content-keyed entries would accumulate
/// one per distinct message for the life of the process.
fn prune_tracked_toasts(ctx: &mut Context<AppRoot>) {
    ctx.state
        .replaceable_toasts
        .retain(|_, tracked| tracked.touched_at.elapsed() < TOAST_TRACKING_TTL);
}

/// Raise `toast`, collapsing it into an identical one that is already on screen.
///
/// Three outcomes, in order of preference: an unchanged message in the same slot is *renewed* (the
/// existing toast keeps its place and its look, and just lives longer); a changed message in an
/// explicit channel *replaces* what was there; anything else is *pushed*.
fn notify(
    ctx: &mut Context<AppRoot>,
    key: ToastKey,
    content: std::sync::Arc<str>,
    toast: Toast,
) -> Notified {
    prune_tracked_toasts(ctx);
    if let Some(tracked) = ctx.state.replaceable_toasts.get(&key).cloned() {
        // `renew` reports false once the toast has expired or begun fading, neither of which can be
        // extended - then this falls through and pushes a fresh one.
        if tracked.content == content && ctx.toast().renew(tracked.id) {
            if let Some(tracked) = ctx.state.replaceable_toasts.get_mut(&key) {
                tracked.touched_at = std::time::Instant::now();
            }
            return Notified::Renewed;
        }
        ctx.toast().dismiss_immediately(tracked.id);
    }
    let id = ctx.toast().push(toast);
    ctx.state.replaceable_toasts.insert(
        key,
        TrackedToast {
            id,
            touched_at: std::time::Instant::now(),
            content,
        },
    );
    Notified::Pushed
}

/// The text two toasts must share to count as the same message. The separator is a byte that
/// cannot appear in either half, so a title/message split can never be forged by content.
fn toast_content(title: Option<&str>, message: &str) -> std::sync::Arc<str> {
    match title {
        Some(title) => format!("{title}\u{0}{message}").into(),
        None => message.into(),
    }
}

/// Report app state or a rejection. Identical repeats renew rather than stack.
pub(crate) fn notify_info(ctx: &mut Context<AppRoot>, message: impl Into<String>) -> Notified {
    let message = message.into();
    let content = toast_content(None, &message);
    let toast = info_toast(
        &ctx.state.theme,
        ctx.state.config.pane.toast_opacity,
        message,
    );
    notify(
        ctx,
        ToastKey::Content(content_key(&content)),
        content,
        toast,
    )
}

/// Report a failure. Identical repeats renew, which matters most for errors a loop can retry.
pub(crate) fn notify_error(
    ctx: &mut Context<AppRoot>,
    title: impl Into<String>,
    message: impl Into<String>,
) -> Notified {
    let (title, message) = (title.into(), message.into());
    let content = toast_content(Some(&title), &message);
    let toast = error_toast(
        &ctx.state.theme,
        ctx.state.config.pane.toast_opacity,
        title,
        message,
    );
    notify(
        ctx,
        ToastKey::Content(content_key(&content)),
        content,
        toast,
    )
}

/// Report a compatibility or other non-fatal risk with warning-colored chrome.
pub(crate) fn notify_warning(
    ctx: &mut Context<AppRoot>,
    title: impl Into<String>,
    message: impl Into<String>,
) -> Notified {
    let (title, message) = (title.into(), message.into());
    let content = toast_content(Some(&title), &message);
    let toast = warning_toast(
        &ctx.state.theme,
        ctx.state.config.pane.toast_opacity,
        title,
        message,
    );
    notify(
        ctx,
        ToastKey::Content(content_key(&content)),
        content,
        toast,
    )
}

/// Report the newest state of `channel`, superseding whatever that channel last showed.
pub(crate) fn notify_on(
    ctx: &mut Context<AppRoot>,
    channel: ToastChannel,
    title: Option<String>,
    message: impl Into<String>,
) -> Notified {
    let message = message.into();
    let content = toast_content(title.as_deref(), &message);
    let opacity = ctx.state.config.pane.toast_opacity;
    let toast = match title {
        Some(title) => error_toast(&ctx.state.theme, opacity, title, message),
        None => info_toast(&ctx.state.theme, opacity, message),
    };
    notify(ctx, ToastKey::Channel(channel), content, toast)
}

pub(crate) fn info_toast(theme: &Theme, opacity: f32, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .duration(3.0)
        .wrap(true)
        .min_width(Length::Px(10))
        .max_width(Length::Px(64))
        .frame_style(toast_frame_style(theme, theme.status.info, opacity))
        .title_style(toast_text_style(theme).bold())
        .message_style(toast_text_style(theme))
        .copyable(true)
        .copy_affordance(ToastCopyAffordance::None)
        .padding((0, 0, 0, 0))
}

/// Toast for an armed destructive action: error-colored chrome, visible for exactly the confirm
/// window so its dismissal coincides with the pending action expiring.
pub(crate) fn confirm_toast(theme: &Theme, opacity: f32, message: impl Into<String>) -> Toast {
    Toast::new(message.into())
        .duration(crate::ops::confirm::CONFIRM_WINDOW.as_secs_f64())
        .wrap(true)
        .min_width(Length::Px(10))
        .max_width(Length::Px(64))
        .frame_style(toast_frame_style(theme, theme.status.error, opacity))
        .message_style(toast_text_style(theme))
        .padding((0, 0, 0, 0))
}

pub(crate) fn warning_toast(
    theme: &Theme,
    opacity: f32,
    title: impl Into<String>,
    message: impl Into<String>,
) -> Toast {
    titled_toast(theme, theme.status.warning, opacity, title, message)
}

pub(crate) fn error_toast(
    theme: &Theme,
    opacity: f32,
    title: impl Into<String>,
    message: impl Into<String>,
) -> Toast {
    titled_toast(theme, theme.status.error, opacity, title, message)
}

fn titled_toast(
    theme: &Theme,
    accent: Color,
    opacity: f32,
    title: impl Into<String>,
    message: impl Into<String>,
) -> Toast {
    Toast::new(message.into())
        .title(Some(title.into()))
        .duration(6.0)
        .wrap(true)
        .min_width(Length::Px(10))
        .max_width(Length::Px(64))
        .border(true)
        .frame_style(toast_frame_style(theme, accent, opacity))
        .title_style(toast_text_style(theme).bold())
        .message_style(toast_text_style(theme))
        .copyable(true)
        .copy_affordance(ToastCopyAffordance::None)
        .padding((0, 0, 0, 0))
}

/// Chrome for a toast: `accent` over the theme's own panel color.
///
/// A toast sets no background of its own by default in the widget, so it would inherit whatever
/// pane output is behind it - unreadable over bright content. Painting the theme's panel color
/// makes it read as the same material as the palette and modals.
///
/// Below `1.0` the paint is alpha, which the overlay renderer composites per cell against the
/// content the toast covers - tinted glass rather than a flat wash. The trade is that text
/// contrast then depends on what is behind; see `[pane] toast_opacity` for the measured spread
/// across themes and for raising it on one that reads poorly.
///
/// The panel color is used rather than a fixed dark wash because the message text comes from
/// `theme.primary` - on a light theme that text is dark, and a dark background under it would
/// recreate the very problem this solves.
fn toast_frame_style(theme: &Theme, accent: Color, opacity: f32) -> Style {
    let style = Style::new().fg(accent);
    if opacity >= 1.0 {
        style.bg(theme.surface.panel)
    } else {
        style.bg_alpha(theme.surface.panel, opacity.clamp(0.0, 1.0))
    }
}

fn toast_text_style(theme: &Theme) -> Style {
    theme
        .primary
        .resolved_fg()
        .filter(|color| !color.is_sentinel())
        .map_or_else(Style::new, |text| Style::new().fg(text))
}

pub(crate) fn maybe_notify_pane_exit(config: &crate::config::Config, id: PaneId, code: i32) {
    if !should_notify_pane_exit(config, code) {
        return;
    }
    crate::platform::notifications::notify("rozi", &format!("Pane {id} exited with code {code}"));
}

pub(crate) fn should_notify_pane_exit(config: &crate::config::Config, code: i32) -> bool {
    config.notifications.enabled
        && if code == 0 {
            config.notifications.pane_exit
        } else {
            config.notifications.pane_exit_error
        }
}

pub(crate) struct PaneStatusNotification<'a> {
    pub blocked: bool,
    pub done: bool,
    pub reported_status: Option<&'a crate::session::protocol::PaneStatus>,
}

pub(crate) fn maybe_notify_pane_status(
    config: &crate::config::Config,
    is_controller: bool,
    is_attended: bool,
    id: PaneId,
    title: &str,
    alert: PaneStatusNotification<'_>,
) {
    if !should_notify_pane_status(
        config,
        is_controller,
        is_attended,
        alert.blocked,
        alert.done,
    ) {
        return;
    }
    let reported = alert.reported_status.filter(|status| {
        (alert.blocked
            && status
                .value
                .trim()
                .eq_ignore_ascii_case(crate::session::protocol::pane_status::BLOCKED))
            || (alert.done
                && status
                    .value
                    .trim()
                    .eq_ignore_ascii_case(crate::session::protocol::pane_status::DONE))
    });
    let body = reported.map_or_else(
        || {
            format!(
                "Pane {id} ({title}) is {}",
                if alert.blocked { "blocked" } else { "done" }
            )
        },
        |status| {
            status.reason.as_deref().map_or_else(
                || format!("Pane {id} ({title}) is {}", status.value),
                |reason| format!("Pane {id} ({title}) is {}: {reason}", status.value),
            )
        },
    );
    crate::platform::notifications::notify("rozi", &body);
}

pub(crate) fn should_notify_pane_status(
    config: &crate::config::Config,
    is_controller: bool,
    is_attended: bool,
    blocked: bool,
    done: bool,
) -> bool {
    if !config.notifications.enabled || !is_controller || is_attended {
        return false;
    }
    (config.notifications.pane_blocked && blocked) || (config.notifications.pane_done && done)
}
