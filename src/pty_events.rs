use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::find_pane_mut;
use crate::state::{PaneId, ToastChannel};

/// Why input was rejected, and what surfacing that rejection did to the screen.
struct BlockedInput {
    reason: String,
    notified: Notified,
}

fn input_blocked(ctx: &mut Context<AppRoot>) -> Option<BlockedInput> {
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

/// What a [`notify`] call did, and therefore whether the screen changed.
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

pub(crate) fn error_toast(
    theme: &Theme,
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
        .frame_style(toast_frame_style(theme, theme.status.error, opacity))
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

pub(crate) fn forward_key_to_pane(ctx: &mut Context<AppRoot>, id: PaneId, key: KeyEvent) -> Update {
    if let Some(blocked) = input_blocked(ctx) {
        return blocked.notified.update();
    }
    let targets = synchronized_key_targets(&ctx.state, id);
    forward_key_to_targets(ctx, &targets, key)
}

fn forward_key_to_targets(ctx: &mut Context<AppRoot>, targets: &[PaneId], key: KeyEvent) -> Update {
    let mut repaint = false;
    let client = ctx.state.current().session_client.clone();
    ctx.state.current_mut().engaged = true;
    for id in targets {
        let Some(pane) = find_pane_mut(&mut ctx.state, *id) else {
            continue;
        };
        if let Some(client) = client.clone() {
            if send_key_to_session_client(
                &client,
                *id,
                pane.pty_generation,
                key,
                pane.terminal.snapshot().key_modes,
            )
            .is_ok()
                && pane.terminal.set_scrollback(0)
            {
                repaint = true;
            }
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            repaint = true;
        }
    }
    if repaint {
        Update::full()
    } else {
        Update::none()
    }
}

/// Send raw bytes (paste payloads, user `Send` commands, control-socket text) to a pane's shell
/// through the session server. Returns an error string when no client is connected.
pub(crate) fn send_pane_bytes(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    bytes: Vec<u8>,
) -> std::result::Result<(), String> {
    if let Some(blocked) = input_blocked(ctx) {
        return Err(blocked.reason);
    }
    let client = ctx.state.current().session_client.clone();
    ctx.state.current_mut().engaged = true;
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return Ok(());
    };
    let Some(client) = client else {
        pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
        return Err("session disconnected".to_string());
    };
    client.send_input(id, pane.pty_generation, bytes);
    Ok(())
}

pub(crate) fn synchronized_key_targets(state: &crate::state::State, source: PaneId) -> Vec<PaneId> {
    let workspace = &state.current().workspaces[state.current().active_workspace];
    if !workspace.synchronized {
        return vec![source];
    }
    if !workspace
        .panes
        .iter()
        .any(|pane| pane.id == source && !pane.floating && !pane.closing)
    {
        return vec![source];
    }
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.floating && !pane.closing)
        .map(|pane| pane.id)
        .collect()
}

pub(crate) fn maybe_notify_pane_exit(config: &crate::config::Config, id: PaneId, code: i32) {
    if !should_notify_pane_exit(config, code) {
        return;
    }
    crate::platform::notifications::notify("rozi", &format!("Pane {id} exited with code {code}"));
}

fn should_notify_pane_exit(config: &crate::config::Config, code: i32) -> bool {
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

fn should_notify_pane_status(
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

pub(crate) fn handle_pane_input(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    if matches!(input.kind, TerminalInputKind::Key) {
        // Key input is routed through Msg::PaneKey so prefix and held-modifier
        // bindings can intercept before bytes reach the PTY. Keeping on_input
        // installed still enables bracketed paste and focus reports.
        return Update::none();
    }
    if let Some(blocked) = input_blocked(ctx) {
        return blocked.notified.update();
    }

    let client = ctx.state.current().session_client.clone();
    // Only a paste is the user putting something into this session. The focus notifications that
    // also arrive here are the terminal reporting on itself — counting those would mark a session
    // worked-in for having been looked at, which is the opposite of what engagement means.
    if matches!(input.kind, TerminalInputKind::Paste) {
        ctx.state.current_mut().engaged = true;
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            client.send_input(id, pane.pty_generation, input.bytes.to_vec());
            if matches!(input.kind, TerminalInputKind::Paste) && pane.terminal.set_scrollback(0) {
                return Update::full();
            }
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    Update::none()
}

pub(crate) fn handle_pane_mouse(ctx: &mut Context<AppRoot>, id: PaneId, bytes: Vec<u8>) -> Update {
    // A pane running mouse tracking consumes the event in the framework before this pane's
    // `MouseRegion` runs, so the `on_mouse_down` that normally raises `Msg::FocusPane` never fires
    // for a full-screen TUI. The framework has already moved its *own* focus for clicks, drags and
    // scrolls (but deliberately not for plain motion), so reconciling from it restores
    // click-to-focus without reintroducing hover-to-focus against the user's config.
    let before = ctx.state.current().focused_pane;
    crate::key_routing::sync_focus_from_framework(ctx);
    let focus_moved = ctx.state.current().focused_pane != before;
    // Forwarded activity also means the pointer is over this pane, so re-apply the hover policy.
    let hover = crate::ops::focus::hover_focus_pane(ctx, id);
    let focus_update = if focus_moved { Update::full() } else { hover };
    if let Some(blocked) = input_blocked(ctx) {
        // Pointer motion arrives continuously; a renewed rejection draws nothing new, so fall back
        // to whatever focus already asked for.
        return match blocked.notified {
            Notified::Pushed => Update::full(),
            Notified::Renewed => focus_update,
        };
    }

    let client = ctx.state.current().session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if let Some(client) = client {
            client.send_input(id, pane.pty_generation, bytes);
        } else {
            pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            return Update::full();
        }
    }
    focus_update
}

/// Trailing-edge debounce window for controller PTY resizes, coalescing a resize storm (drag,
/// tiling reflow) into one `pty.resize`/SIGWINCH per pane.
const RESIZE_DEBOUNCE_MS: u64 = 16;

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    // Followers never drive PTY size: they letterbox to the controller's canonical canvas and their
    // screens reshape only via the server's broadcast `Resized`. Suppress their local resize here.
    if !ctx.state.is_controller() {
        return Update::none();
    }
    // The pane rect updates immediately, but the client-side screen only reshapes on the server's
    // ordered `Resized` broadcast, so both parsers reshape at the same byte position.
    let client = ctx.state.current().session_client.clone();
    let generation = match find_pane_mut(&mut ctx.state, id) {
        Some(pane) => {
            if client.is_none() {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
                return Update::full();
            }
            pane.pty_generation
        }
        None => return Update::none(),
    };
    // Debounce through the shared bookkeeping when attached: record the latest size and arm a single
    // trailing-edge flush. Without shared state (a brief unattached window), send immediately.
    let epoch = ctx.state.runtime_epoch;
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared
            .pending_resizes
            .insert(id, (cols.max(1), rows.max(1)));
        if shared.resize_flush_scheduled {
            return Update::none();
        }
        shared.resize_flush_scheduled = true;
        return Update::with_command(schedule_pane_resize_flush(epoch));
    }
    if let Some(client) = client {
        client.resize(id, generation, cols.max(1), rows.max(1));
    }
    Update::none()
}

fn schedule_pane_resize_flush(epoch: u64) -> Command {
    Command::after(
        std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS),
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::FlushPaneResizes { epoch });
        },
    )
}

/// Send the latest debounced size for every pane that still exists (see the controller debounce in
/// [`handle_pane_resize`]). Clears the pending set and re-arms scheduling.
///
/// A pending size is the only record of that pane's geometry there is: `client.resize` is reached
/// from here and from [`handle_pane_resize`] alone, both driven by the terminal widget, and the
/// widget reports a viewport only when it *changes*. Nothing re-derives one. So a size dropped here
/// leaves the PTY wrong until the pane's geometry happens to change again - which for a pane the
/// user is not currently resizing may be never.
pub(crate) fn flush_pending_resizes(ctx: &mut Context<AppRoot>) {
    let Some(client) = ctx.state.current().session_client.clone() else {
        // Mid-attach or a reconnect window. Disarm so a later report can schedule a fresh flush,
        // but keep the sizes: `flush_pending_resizes` runs again once the client is installed.
        if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
            shared.resize_flush_scheduled = false;
        }
        return;
    };
    let pending: Vec<(PaneId, (u16, u16))> = match ctx.state.current_mut().shared.as_mut() {
        Some(shared) => {
            shared.resize_flush_scheduled = false;
            shared.pending_resizes.drain().collect()
        }
        None => return,
    };
    for (id, (cols, rows)) in pending {
        if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
            client.resize(id, pane.pty_generation, cols.max(1), rows.max(1));
        }
    }
}

pub(crate) fn handle_pane_scroll(ctx: &mut Context<AppRoot>, id: PaneId, offset: usize) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && pane.terminal.set_scrollback(offset)
    {
        return Update::full();
    }
    Update::none()
}

pub(crate) fn terminal_key_event_bytes(key: KeyEvent, modes: TerminalKeyModes) -> Option<Vec<u8>> {
    key_event_to_bytes(key, modes)
}

pub(crate) fn send_key_to_session_client(
    client: &crate::session::client::SessionClient,
    pane_id: PaneId,
    generation: u64,
    key: KeyEvent,
    modes: TerminalKeyModes,
) -> std::result::Result<(), String> {
    let bytes = terminal_key_event_bytes(key, modes)
        .ok_or_else(|| "key is not representable for session forwarding yet".to_string())?;
    client.send_input(pane_id, generation, bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Pane, State};

    fn rect() -> FloatRect {
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        }
    }

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    #[test]
    fn terminal_key_encoding_matches_local_terminal_encoder_representatives() {
        let cases = [
            (key(KeyCode::Char('x'), KeyMods::NONE), b"x".to_vec()),
            (key(KeyCode::Char('c'), KeyMods::CTRL), vec![3]),
            (key(KeyCode::Char('x'), KeyMods::ALT), b"\x1bx".to_vec()),
            (key(KeyCode::Enter, KeyMods::NONE), b"\r".to_vec()),
            (key(KeyCode::BackTab, KeyMods::NONE), b"\x1b[Z".to_vec()),
            (key(KeyCode::Delete, KeyMods::NONE), b"\x1b[3~".to_vec()),
            (key(KeyCode::Home, KeyMods::NONE), b"\x1b[H".to_vec()),
            (key(KeyCode::End, KeyMods::NONE), b"\x1b[F".to_vec()),
            (key(KeyCode::PageUp, KeyMods::NONE), b"\x1b[5~".to_vec()),
            (key(KeyCode::F(12), KeyMods::NONE), b"\x1b[24~".to_vec()),
            // Modified cursor keys must carry the xterm parameter so word-wise motion
            // (Ctrl+Left/Right) and shifted selection reach TUIs instead of a bare arrow.
            (key(KeyCode::Left, KeyMods::CTRL), b"\x1b[1;5D".to_vec()),
            (key(KeyCode::Right, KeyMods::CTRL), b"\x1b[1;5C".to_vec()),
            (key(KeyCode::End, KeyMods::SHIFT), b"\x1b[1;2F".to_vec()),
        ];

        for (key, expected) in cases {
            assert_eq!(
                terminal_key_event_bytes(key, TerminalKeyModes::default()),
                Some(expected)
            );
        }
    }

    #[test]
    fn server_key_forwarding_enqueues_session_input_bytes() {
        let (client, rx) = crate::session::client::SessionClient::test_channel();

        send_key_to_session_client(
            &client,
            7,
            9,
            key(KeyCode::F(5), KeyMods::ALT),
            TerminalKeyModes::default(),
        )
        .expect("modified navigation key forwards");
        send_key_to_session_client(
            &client,
            7,
            9,
            key(KeyCode::Char('c'), KeyMods::CTRL),
            TerminalKeyModes::default(),
        )
        .expect("control key forwards");

        assert_eq!(
            rx.recv().expect("first message"),
            crate::session::client::ClientOutbound::PaneInput {
                pane_id: 7,
                generation: 9,
                bytes: b"\x1b\x1b[15~".to_vec(),
            }
        );
        assert_eq!(
            rx.recv().expect("second message"),
            crate::session::client::ClientOutbound::PaneInput {
                pane_id: 7,
                generation: 9,
                bytes: vec![3],
            }
        );
    }

    /// The prefix is an explicit entry into rozi's command state, so an unbound key there resolves
    /// to nothing rather than being replayed into the shell. Without this, a mistyped chord types a
    /// stray character into whatever is running in the pane.
    #[test]
    fn an_unbound_key_after_the_prefix_reaches_no_pane() {
        use crate::session::client::{ClientOutbound, SessionClient};
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let app = App::new()
                    .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
                    .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
                    .chord_mismatch_policy(ChordMismatchPolicy::CancelOnly);
                let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_client = Some(client);
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.opening = false;
                    pane.terminal_active = true;
                }
                backend.render();
                backend.focus_next();
                while rx.try_recv().is_ok() {}

                let prefix = key(KeyCode::Char('a'), KeyMods::CTRL);
                backend.send_key(prefix).expect("prefix enters chord");
                // `y` is deliberately unbound as a prefix chord: nothing should reach the pane.
                backend
                    .send_key(key(KeyCode::Char('y'), KeyMods::NONE))
                    .expect("unbound key resolves");

                let inputs: Vec<_> = rx
                    .try_iter()
                    .filter_map(|message| match message {
                        ClientOutbound::PaneInput { bytes, .. } => Some(bytes),
                        ClientOutbound::Control(_) => None,
                    })
                    .collect();
                assert!(
                    inputs.is_empty(),
                    "the prefix and the unbound key must both stay in rozi: {inputs:?}"
                );

                // The chord is over, so the next key is ordinary input again.
                backend
                    .send_key(key(KeyCode::Char('y'), KeyMods::NONE))
                    .expect("plain key forwards");
                let inputs: Vec<_> = rx
                    .try_iter()
                    .filter_map(|message| match message {
                        ClientOutbound::PaneInput { bytes, .. } => Some(bytes),
                        ClientOutbound::Control(_) => None,
                    })
                    .collect();
                assert_eq!(inputs, vec![b"y".to_vec()]);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn double_prefix_forwards_one_prefix_key() {
        use crate::session::client::{ClientOutbound, SessionClient};
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let app = App::new()
                    .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
                    .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
                    .chord_mismatch_policy(ChordMismatchPolicy::CancelOnly);
                let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
                let (client, rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_client = Some(client);
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.opening = false;
                    pane.terminal_active = true;
                }
                backend.render();
                backend.focus_next();
                while rx.try_recv().is_ok() {}

                let prefix = key(KeyCode::Char('a'), KeyMods::CTRL);
                backend.send_key(prefix).expect("first prefix enters chord");
                backend
                    .send_key(prefix)
                    .expect("second prefix forwards the first");

                let inputs: Vec<_> = rx
                    .try_iter()
                    .filter_map(|message| match message {
                        ClientOutbound::PaneInput { .. } => Some(message),
                        ClientOutbound::Control(_) => None,
                    })
                    .collect();
                assert_eq!(
                    inputs,
                    vec![ClientOutbound::PaneInput {
                        pane_id: 1,
                        generation: 0,
                        bytes: vec![1],
                    }]
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    /// Engagement is what keeps a startup temporary session alive across a switch, so it has to
    /// mean the user put something into the session. A focus report is the terminal talking about
    /// itself — a session that was merely looked at is still untouched.
    #[test]
    fn focus_reports_do_not_mark_a_session_as_worked_in() {
        use crate::Msg;
        use crate::session::client::SessionClient;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, _rx) = SessionClient::test_channel();
                backend.state_mut().current_mut().session_client = Some(client);
                backend.state_mut().current_mut().engaged = false;
                backend.render();

                for kind in [TerminalInputKind::FocusIn, TerminalInputKind::FocusOut] {
                    backend
                        .dispatch(Msg::PaneInput(
                            1,
                            TerminalInputEvent {
                                kind,
                                key: None,
                                bytes: Vec::new().into(),
                            },
                        ))
                        .expect("dispatch focus report");
                    assert!(
                        !backend.state().current().engaged,
                        "{kind:?} must not count as working in the session"
                    );
                }

                backend
                    .dispatch(Msg::PaneInput(
                        1,
                        TerminalInputEvent {
                            kind: TerminalInputKind::Paste,
                            key: None,
                            bytes: b"work".to_vec().into(),
                        },
                    ))
                    .expect("dispatch paste");
                assert!(
                    backend.state().current().engaged,
                    "a paste puts the user's own content into the session"
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn terminal_keyboard_and_paste_input_return_scrolled_pane_to_live_view() {
        use crate::Msg;
        use crate::session::client::SessionClient;
        use tui_lipan::TestBackend;

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, _rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_client = Some(client);
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.terminal
                        .process_server_output("history\n".repeat(80).as_bytes());
                    assert!(pane.terminal.set_scrollback(10));
                }
                backend.render();

                backend
                    .dispatch(Msg::PaneKey(1, key(KeyCode::Char('x'), KeyMods::NONE)))
                    .expect("dispatch terminal key");
                assert_eq!(
                    backend.state().current().workspaces[0].panes[0]
                        .terminal
                        .scrollback_offset(),
                    0
                );

                assert!(
                    backend.state_mut().current_mut().workspaces[0].panes[0]
                        .terminal
                        .set_scrollback(10)
                );
                backend
                    .dispatch(Msg::PaneInput(
                        1,
                        TerminalInputEvent {
                            kind: TerminalInputKind::Paste,
                            key: None,
                            bytes: b"pasted".to_vec().into(),
                        },
                    ))
                    .expect("dispatch terminal paste");
                assert_eq!(
                    backend.state().current().workspaces[0].panes[0]
                        .terminal
                        .scrollback_offset(),
                    0
                );
            })
            .expect("spawn terminal input test thread")
            .join()
            .expect("terminal input test thread completes");
    }

    #[test]
    fn synchronized_targets_default_to_source_only() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0]
            .panes
            .push(Pane::new(2, 100, rect()));

        assert_eq!(synchronized_key_targets(&state, 1), vec![1]);
    }

    #[test]
    fn pane_status_notification_policy_is_controller_only_and_configurable() {
        let mut config = crate::config::Config::default();
        config.notifications.enabled = true;

        assert!(should_notify_pane_status(&config, true, false, true, false));
        assert!(!should_notify_pane_status(
            &config, false, false, true, false
        ));
        assert!(!should_notify_pane_status(&config, true, true, true, false));
        assert!(!should_notify_pane_status(
            &config, true, false, false, true
        ));
        config.notifications.pane_done = true;
        assert!(should_notify_pane_status(&config, true, false, false, true));
        config.notifications.enabled = false;
        assert!(!should_notify_pane_status(
            &config, true, false, true, false
        ));
    }

    #[test]
    fn status_notification_treats_a_focused_background_window_as_unattended() {
        let mut config = crate::config::Config::default();
        config.notifications.enabled = true;
        let mut state = State::new(config.clone(), Theme::default());
        let pane_id = state.current().focused_pane.expect("fresh pane focus");
        state.window_focused = false;

        assert!(!state.is_pane_attended(pane_id));
        assert!(should_notify_pane_status(
            &config,
            true,
            state.is_pane_attended(pane_id),
            true,
            false
        ));
    }

    #[test]
    fn pane_exit_notification_splits_clean_and_error_codes() {
        let mut config = crate::config::Config::default();
        config.notifications.enabled = true;
        // Enabling notifications is not on its own a reason to announce a clean exit.
        assert!(!should_notify_pane_exit(&config, 0));
        assert!(should_notify_pane_exit(&config, 1));
        config.notifications.pane_exit = true;
        assert!(should_notify_pane_exit(&config, 0));
        assert!(should_notify_pane_exit(&config, 1));
        config.notifications.pane_exit_error = false;
        assert!(should_notify_pane_exit(&config, 0));
        assert!(!should_notify_pane_exit(&config, 1));
    }

    #[test]
    fn follower_resize_is_suppressed_and_controller_resize_debounces() {
        use crate::AppRoot;
        use crate::Msg;
        use crate::session::client::{ClientOutbound, SessionClient};
        use crate::session::protocol::ClientMessage;
        use crate::state::SharedSessionState;
        use tui_lipan::TestBackend;

        fn resizes(rx: &std::sync::mpsc::Receiver<ClientOutbound>) -> Vec<(u16, u16)> {
            rx.try_iter()
                .filter_map(|msg| match msg {
                    ClientOutbound::Control(ClientMessage::Resize { cols, rows, .. }) => {
                        Some((cols, rows))
                    }
                    _ => None,
                })
                .collect()
        }

        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let viewport = Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                };

                // Follower: a resize forwards nothing (it letterboxes to the canonical size).
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(viewport);
                let (client, follower_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(2);
                    state.current_mut().shared = Some(shared);
                }
                backend.render();
                backend
                    .dispatch(Msg::PaneResize(1, 40, 12))
                    .expect("dispatch follower resize");
                assert!(
                    resizes(&follower_rx).is_empty(),
                    "a follower must not forward pane resizes"
                );

                // Controller: rapid resizes coalesce; the flush sends only the latest size.
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(viewport);
                let (client, controller_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(1);
                    state.current_mut().shared = Some(shared);
                }
                backend.render();
                // Pretend a flush is already armed. A real one is a 16 ms wall-clock timer, and
                // everything asserted below is state that firing it drains - so left to run, how
                // loaded the machine is decides the outcome. Arming it by hand keeps the flush in
                // this test's hands instead of the clock's.
                //
                // What that gives up is the arming branch itself, which cannot be asserted either
                // way: the flag is set synchronously and cleared 16 ms later, so any read of it
                // races the same timer. What is left is the part worth pinning down - resizes
                // coalesce into one pending size, and only a flush puts it on the wire.
                backend
                    .state_mut()
                    .current_mut()
                    .shared
                    .as_mut()
                    .expect("controller has shared state")
                    .resize_flush_scheduled = true;
                backend
                    .dispatch(Msg::PaneResize(1, 40, 12))
                    .expect("dispatch first resize");
                backend
                    .dispatch(Msg::PaneResize(1, 50, 20))
                    .expect("dispatch second resize");
                assert!(
                    resizes(&controller_rx).is_empty(),
                    "debounced resizes are not sent until the flush"
                );
                assert_eq!(
                    backend
                        .state()
                        .current()
                        .shared
                        .as_ref()
                        .expect("controller has shared state")
                        .pending_resizes
                        .get(&1),
                    Some(&(50, 20)),
                    "both resizes coalesce into the latest pending size"
                );
                backend
                    .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                    .expect("dispatch flush");
                assert_eq!(
                    resizes(&controller_rx),
                    vec![(50, 20)],
                    "flush sends only the latest size per pane"
                );

                // A live sidebar drag follows the same debounce path as every other geometry
                // change. Preview state must not hold PTY resizes until pointer release.
                backend.state_mut().sidebar.width_preview = Some(40);
                backend
                    .dispatch(Msg::PaneResize(1, 60, 22))
                    .expect("dispatch preview resize");
                backend
                    .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                    .expect("dispatch flush during preview");
                assert_eq!(resizes(&controller_rx), vec![(60, 22)]);
                backend.state_mut().sidebar.width_preview = None;

                // A flush with no client must hold the size rather than discard it: nothing
                // re-derives one, so dropping it leaves the PTY wrong until the pane's geometry
                // happens to change again.
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(viewport);
                let (client, reconnect_rx) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client.clone());
                    let mut shared = SharedSessionState::new(1);
                    shared.controller = Some(1);
                    state.current_mut().shared = Some(shared);
                }
                backend.render();
                backend
                    .dispatch(Msg::PaneResize(1, 50, 20))
                    .expect("dispatch resize");
                // The link drops between the report and the trailing-edge flush it armed.
                backend.state_mut().current_mut().session_client = None;
                backend
                    .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                    .expect("dispatch flush while disconnected");
                assert_eq!(
                    backend
                        .state()
                        .current()
                        .shared
                        .as_ref()
                        .expect("controller has shared state")
                        .pending_resizes
                        .get(&1),
                    Some(&(50, 20)),
                    "a flush with no client keeps the size for the next one"
                );

                // The client arriving is what delivers it.
                backend.state_mut().current_mut().session_client = Some(client);
                backend
                    .dispatch(Msg::FlushPaneResizes { epoch: 0 })
                    .expect("dispatch flush after reconnect");
                assert_eq!(
                    resizes(&reconnect_rx),
                    vec![(50, 20)],
                    "the held size reaches the PTY once a client is back"
                );
            })
            .expect("spawn resize test thread")
            .join()
            .expect("resize test thread completes");
    }

    #[test]
    fn synchronized_targets_exclude_floating_and_scratch() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().workspaces[0].synchronized = true;
        state.current_mut().workspaces[0]
            .panes
            .push(Pane::new(2, 100, rect()));
        let mut floating = Pane::new(3, 100, rect());
        floating.floating = true;
        state.current_mut().workspaces[0].panes.push(floating);
        state.current_mut().workspaces[0]
            .panes
            .push(Pane::new(4, 100, rect()));
        state.scratch = Some(Pane::new(crate::state::SCRATCH_PANE_ID, 100, rect()));

        assert_eq!(synchronized_key_targets(&state, 1), vec![1, 2, 4]);
        assert_eq!(synchronized_key_targets(&state, 3), vec![3]);
    }
}
