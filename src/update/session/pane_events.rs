use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane_lifecycle::{find_pane_in_namespace_mut, remove_pane_after_exit};
use crate::pty_events::maybe_notify_pane_exit;
use crate::state::PaneId;

struct PaneOutputEffects {
    indicator_raised: bool,
    chrome_changed: bool,
    clipboard_events: Vec<TerminalClipboardEvent>,
    bell_fired: bool,
    bell_alert_raised: bool,
}

fn apply_current_pane_output(
    state: &mut crate::state::State,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    bytes: &[u8],
) -> Option<PaneOutputEffects> {
    let attended = state.is_pane_attended(pane_id);
    let bell_notifications = state.config.notifications.bell;
    // Reassert media policy on every path that can create or feed a pane.
    let policy = if local && crate::scratchpad::contains(state, pane_id) {
        GraphicsMediaPolicy::SHARED
    } else {
        state.current().image_media_policy()
    };
    let pane = find_pane_in_namespace_mut(state, pane_id, local)?;
    if pane.pty_generation != generation {
        return None;
    }
    pane.terminal.set_media_policy(policy);
    let output = pane.terminal.process_server_output(bytes);
    let chrome_changed = matches!(output.frame, crate::pane::OutputFrame::Rebuild);
    let bell_fired = pane.terminal.take_bell();
    pane.activity.last_activity = Some(std::time::Instant::now());

    let mut indicator_raised = false;
    let mut bell_alert_raised = false;
    if !attended {
        indicator_raised = !pane.activity.has_unseen_output;
        pane.activity.has_unseen_output = true;
        if bell_fired && bell_notifications {
            indicator_raised |= !pane.activity.bell;
            bell_alert_raised = !pane.activity.bell;
            pane.activity.bell = true;
        }
    }
    Some(PaneOutputEffects {
        indicator_raised,
        chrome_changed,
        clipboard_events: output.clipboard_events,
        bell_fired,
        bell_alert_raised,
    })
}

fn relay_output_clipboard(ctx: &mut Context<AppRoot>, events: Vec<TerminalClipboardEvent>) {
    if !ctx.state.config.clipboard.enable_osc52 {
        return;
    }
    for event in events {
        if matches!(event.target, TerminalClipboardTarget::Clipboard) {
            ctx.clipboard().relay_osc52(&event.text);
        }
    }
}

fn emit_output_bell(
    ctx: &mut Context<AppRoot>,
    pane_id: PaneId,
    focused: Option<PaneId>,
    alert_raised: bool,
) {
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::Bell,
            vec![
                ("pane", pane_id.to_string()),
                ("focused", (focused == Some(pane_id)).to_string()),
            ],
        ),
    );
    if alert_raised && !ctx.state.do_not_disturb {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Bell);
    }
}

fn apply_stale_output(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    bytes: &[u8],
) {
    // A popup belongs to the outgoing attachment. Scratch frames are retagged when drained.
    if local {
        return;
    }
    if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
        attachment.apply_background_output(pane_id, generation, bytes);
    }
}

fn buffer_orphan_output(
    state: &mut crate::state::State,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    bytes: &[u8],
) {
    if !local && let Some(shared) = state.current_mut().shared.as_mut() {
        shared.buffer_orphan_output(pane_id, generation, bytes);
    }
}

fn output_frame_update(
    state: &crate::state::State,
    pane_id: PaneId,
    indicator_raised: bool,
    chrome_changed: bool,
) -> Update {
    if indicator_raised || chrome_changed {
        Update::full()
    } else if state.pane_is_rendered(pane_id) {
        Update::paint()
    } else {
        Update::none()
    }
}

pub(crate) fn output(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    bytes: Vec<u8>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        // A retained background attachment: keep its screens live so switching back is instant, but
        // never draw them (nothing background is on screen).
        apply_stale_output(ctx, epoch, pane_id, local, generation, &bytes);
        return Update::none();
    }
    let focused = ctx.state.focused_pane();
    // Activity/bell indicators are workspace-agnostic (the workbar counts them across every
    // workspace), so an off-screen pane still needs a frame on the chunk that first raises one.
    // Both flags only ever go false -> true here, so that is a single frame per quiet period
    // rather than one per output chunk.
    let Some(effects) =
        apply_current_pane_output(&mut ctx.state, pane_id, local, generation, &bytes)
    else {
        // Output arrived before the layout commit that introduces this pane (or its new generation).
        // Buffer it so the reconciler can replay it when the pane appears; dropping it would leave
        // a follower's fresh pane blank until the next redraw. Nothing draws it yet, so no frame.
        // Local output never joins that shared-layout race: the owner created the pane itself.
        buffer_orphan_output(&mut ctx.state, pane_id, local, generation, &bytes);
        return Update::none();
    };
    relay_output_clipboard(ctx, effects.clipboard_events);
    if effects.bell_fired {
        emit_output_bell(ctx, pane_id, focused, effects.bell_alert_raised);
    }
    if effects.indicator_raised {
        // Session output bypasses the global post-update sweep. An unseen-output or bell edge is
        // the only output-side mutation that can start the shared alert animation, so arm it here
        // once on that edge instead of scanning all panes after every ordinary chunk.
        crate::update::panes::arm_alert_pulse(ctx);
    }
    // Search coordinates are absolute within the retained terminal grid. Apply output first, drop
    // the pane borrow, then rebuild any affected scan so no result can address the pre-output grid.
    if !bytes.is_empty()
        && let Some(update) = crate::ops::search::restart_search_after_pane_output(ctx, pane_id)
    {
        return update;
    }
    // The screen is already updated above; only ask for a frame when the result reaches the
    // display. A chatty pane on an inactive workspace would otherwise drive the renderer at full
    // rate painting a view its output never appears in (see `State::pane_is_rendered`).
    //
    // Output is a *repaint*: the view hands the widget the screen itself (`TerminalPane::screen_handle`),
    // so nothing about the element tree depends on what the child program just drew and the runtime
    // picks the new contents up on its way to the buffer. A full frame here would instead re-run
    // `view()` and layout for every pane, workbar segment and sidebar row in the window on every
    // chunk. That is a real saving but not the dominant one while a pane streams - parsing the bytes
    // and diffing the buffer cost more - so the reason to keep this at `paint` is that the work is
    // unnecessary, not that it is the bottleneck.
    //
    // A raised activity or bell indicator is different: those *are* view state (the workbar counts
    // them), as is a changed OSC title, so those frames have to be full ones.
    output_frame_update(
        &ctx.state,
        pane_id,
        effects.indicator_raised,
        effects.chrome_changed,
    )
}

pub(crate) fn resized(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    cols: u16,
    rows: u16,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if local {
            return Update::none();
        }
        // Keep a retained background attachment's screen at the server's size for an instant, correct
        // switch-back.
        if let Some(pane) = ctx
            .state
            .background
            .get_mut(&epoch)
            .and_then(|attachment| attachment.find_pane_mut(pane_id))
            && pane.pty_generation == generation
        {
            pane.terminal.apply_server_resize(cols, rows);
        }
        return Update::none();
    }
    if let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
        && pane.pty_generation == generation
        && pane.terminal.apply_server_resize(cols, rows)
        && ctx.state.pane_is_rendered(pane_id)
    {
        return Update::full();
    }
    Update::none()
}

pub(crate) fn exited(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    code: i32,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if local {
            return Update::none();
        }
        let hold_on_exit = ctx.state.config.pane.hold_on_exit;
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            let should_defer = attachment
                .find_pane_mut(pane_id)
                .filter(|pane| pane.pty_generation == generation)
                .map(|pane| {
                    pane.terminal.status = ManagedTerminalStatus::Exited(code);
                    !should_hold_on_exit(hold_on_exit, pane.closing)
                })
                .unwrap_or(false);
            if should_defer
                && !attachment
                    .pending_background_closes
                    .contains(&(pane_id, generation))
            {
                attachment
                    .pending_background_closes
                    .push((pane_id, generation));
            }
        }
        return Update::none();
    }
    let hold_on_exit = ctx.state.config.pane.hold_on_exit;
    let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local) else {
        // A pane closed by the app has already been removed; its later server exit frame is stale.
        return Update::none();
    };
    if pane.pty_generation != generation {
        return Update::none();
    }
    pane.terminal.status = ManagedTerminalStatus::Exited(code);
    let already_closing = pane.closing;
    let should_close = local || !should_hold_on_exit(hold_on_exit, already_closing);
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::PaneExited,
            vec![
                ("pane", pane_id.to_string()),
                ("code", code.to_string()),
                (
                    "focused",
                    (ctx.state.current().focused_pane == Some(pane_id)).to_string(),
                ),
            ],
        ),
    );
    ctx.state.commands_dirty = true;
    // A user-initiated close already tore this pane down; the exit is expected, so skip the exit
    // notification/toast and the redundant close call.
    if already_closing {
        return Update::full();
    }
    // The scratchpad is a local overlay (never in the shared layout), so every client that owns it
    // closes it directly. Shared exits never consult overlay membership: a colliding numeric id
    // must not close the owner's scratch or popup.
    if local && pane_id == crate::state::POPUP_PANE_ID {
        return crate::popup::handle_exit(ctx);
    }
    if local && crate::scratchpad::contains(&ctx.state, pane_id) {
        return remove_pane_after_exit(ctx, pane_id, true);
    }
    // Closing a tiled/floating pane is a structural layout change: only the controller acts on the
    // exit and commits the new layout; followers close it when that commit arrives.
    if !ctx.state.is_controller() {
        return Update::full();
    }
    if !ctx.state.do_not_disturb {
        maybe_notify_pane_exit(&ctx.state.config, pane_id, code);
    }
    if code != 0 {
        crate::ops::sound::cue(ctx, crate::platform::sound::Cue::Error);
    }
    if !should_close {
        return Update::full();
    }
    // A clean exit closes the pane on its own; only a failure code is worth surfacing.
    if code != 0 {
        crate::pty_events::notify_info(ctx, format!("Pane {pane_id} exited ({code})"));
    }
    remove_pane_after_exit(ctx, pane_id, false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pane_logging_changed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    enabled: bool,
    path: Option<String>,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
        && pane.pty_generation == generation
    {
        pane.logging = enabled;
    }
    // Starting logging is worth a toast because it reports the log path, which is chosen by the
    // server and knowable nowhere else. Stopping reveals nothing the user did not just ask for.
    match error {
        Some(error) => {
            crate::pty_events::notify_error(ctx, "Logging failed", error);
        }
        None if enabled => {
            crate::pty_events::notify_info(
                ctx,
                format!(
                    "Logging pane {pane_id} to {}",
                    path.as_deref().unwrap_or("log file")
                ),
            );
        }
        None => {}
    }
    Update::full()
}

pub(crate) fn flush_pane_resizes(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return crate::pty_events::flush_background_resizes(&mut ctx.state, epoch);
    }
    crate::pty_events::flush_pending_resizes(ctx);
    Update::none()
}

/// A pane the user already closed is expected to exit, so `hold_on_exit` must not keep its shell
/// around and the exit must not be surfaced.
pub(crate) fn should_hold_on_exit(hold_on_exit: bool, closing: bool) -> bool {
    hold_on_exit && !closing
}
