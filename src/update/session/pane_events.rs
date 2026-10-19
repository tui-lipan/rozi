use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::lifecycle::{find_pane_in_namespace_mut, remove_pane_after_exit};
use crate::pane::pty_events::maybe_notify_pane_exit;
use crate::state::PaneId;

struct PaneOutputEffects {
    indicator_raised: bool,
    chrome_changed: bool,
    live_title_changed: bool,
    mutation: TerminalMutation,
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
    // Sidebar rows and modal pickers can derive labels from the raw terminal title too. Their
    // composition stays on the full refresh path until they also bind live sources.
    let title_is_local =
        !state.sidebar_shown() && state.sidebar_slide.get() == 0.0 && !state.has_modal_overlay();
    let remote_attached = state.current().remote_target.is_some();
    let show_titles = state.config.pane.show_titles;
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
    let was_ready = pane.terminal.is_ready();
    let output = pane.terminal.process_server_output(bytes);
    let mut chrome_changed = matches!(output.frame, crate::pane::OutputFrame::Rebuild);
    let mut live_title_changed = false;
    if chrome_changed
        && was_ready
        && title_is_local
        && (pane.live_title.bound.get() || !show_titles)
    {
        live_title_changed = show_titles && pane.live_title.bound.get();
        pane.live_title.refresh(pane.status_title(remote_attached));
        chrome_changed = false;
    }
    let bell_fired = pane.terminal.take_bell();
    pane.activity.last_activity = Some(std::time::Instant::now());
    // An agent rings the bell to say it finished or needs an answer, and the agent's tracked state
    // already raises exactly that mark and cue. Letting the bell mark the pane too would paint a
    // finished run as urgent and play a second sound for it.
    let bell_marks = bell_notifications && pane.terminal.agent_status().is_none();

    let mut indicator_raised = false;
    let mut bell_alert_raised = false;
    if !attended && (output.mutation.is_visual() || output.mutation.title_changed || bell_fired) {
        indicator_raised = !pane.activity.has_unseen_output;
        pane.activity.has_unseen_output = true;
        if bell_fired && bell_marks {
            indicator_raised |= !pane.activity.bell;
            bell_alert_raised = !pane.activity.bell;
            pane.activity.bell = true;
        }
    }
    Some(PaneOutputEffects {
        indicator_raised,
        chrome_changed,
        live_title_changed,
        mutation: output.mutation,
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
    mutation: &TerminalMutation,
    live_title_changed: bool,
) -> Update {
    if indicator_raised || chrome_changed {
        Update::full()
    } else if state.pane_is_rendered(pane_id) {
        if live_title_changed || mutation.cells != TerminalDamage::None || mutation.cursor_changed {
            Update::terminal_paint()
        } else if mutation.graphics != GraphicsDamage::None {
            Update::terminal_graphics()
        } else {
            Update::none()
        }
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
    metadata: crate::session::protocol::OutputMetadata,
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
    if metadata.sequence != 0
        && (effects.mutation.is_visual() || effects.chrome_changed || effects.live_title_changed)
        && let Some(pane) = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
    {
        pane.terminal.set_presentation_token(metadata.sequence);
    }
    if metadata.sequence != 0 {
        let terminal_id = find_pane_in_namespace_mut(&mut ctx.state, pane_id, local)
            .map(|pane| pane.terminal.stable_terminal_id());
        if let Some(terminal_id) = terminal_id {
            let bindings = &mut ctx.state.current_mut().presentation_bindings;
            let progress =
                bindings
                    .entry(terminal_id)
                    .or_insert(crate::state::PresentationProgress {
                        output: crate::session::protocol::PresentationAck {
                            local,
                            pane_id,
                            generation,
                            sequence: 0,
                        },
                        acknowledged: 0,
                    });
            if progress.output.generation != generation {
                progress.output.generation = generation;
                progress.acknowledged = 0;
            }
            progress.output.sequence = metadata.sequence;
        }
    }
    relay_output_clipboard(ctx, effects.clipboard_events);
    crate::ops::capture_wait::pane_output(ctx, pane_id, local);
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
    // Parser mutations select the narrowest frame after activity and chrome effects are known.
    // Native graphics can present directly; invisible responses and metadata need no frame.
    output_frame_update(
        &ctx.state,
        pane_id,
        effects.indicator_raised,
        effects.chrome_changed,
        &effects.mutation,
        effects.live_title_changed,
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

/// The server is about to replay a shared pane from its own screen: after an attach, or after this
/// client fell far enough behind that its output was shed. Start the pane over so the replay lands
/// on an empty parser.
pub(crate) fn pane_reset(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    pane_id: PaneId,
    generation: u64,
    cols: u16,
    rows: u16,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        if let Some(attachment) = ctx.state.background.get_mut(&epoch) {
            reset_attachment_pane(attachment, pane_id, generation, cols, rows);
        }
        return Update::none();
    }
    if !reset_attachment_pane(ctx.state.current_mut(), pane_id, generation, cols, rows) {
        return Update::none();
    }
    if let Some(update) = crate::ops::search::restart_search_after_pane_output(ctx, pane_id) {
        return update;
    }
    if ctx.state.pane_is_rendered(pane_id) {
        Update::full()
    } else {
        Update::none()
    }
}

fn reset_attachment_pane(
    attachment: &mut crate::state::Attachment,
    pane_id: PaneId,
    generation: u64,
    cols: u16,
    rows: u16,
) -> bool {
    match attachment.find_pane_mut(pane_id) {
        Some(pane) if pane.pty_generation == generation => {
            pane.terminal.reset_for_replay(cols, rows);
            true
        }
        Some(_) => false,
        None => {
            // Output buffered before the layout introduced this pane predates the replay.
            if let Some(shared) = attachment.shared.as_mut() {
                shared.discard_orphan_output(pane_id, generation);
            }
            false
        }
    }
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
    crate::ops::capture_wait::pane_exited(ctx, pane_id, local);
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
        return crate::ops::popup::handle_exit(ctx);
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
        crate::pane::pty_events::notify_error(
            ctx,
            format!("Pane {pane_id} exited"),
            format!("Status {code}"),
        );
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
            crate::pane::pty_events::notify_error(ctx, "Logging failed", error);
        }
        None if enabled => match path {
            Some(path) => {
                crate::pane::pty_events::notify_path_info(
                    ctx,
                    format!("Logging pane {pane_id}"),
                    path.clone(),
                    path,
                );
            }
            None => {
                crate::pane::pty_events::notify_info(
                    ctx,
                    format!("Logging pane {pane_id} to log file"),
                );
            }
        },
        None => {}
    }
    Update::full()
}

pub(crate) fn flush_pane_resizes(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return crate::pane::pty_events::flush_background_resizes(
            &mut ctx.state,
            epoch,
            generation,
        );
    }
    crate::pane::pty_events::flush_pending_resizes(ctx, generation);
    Update::none()
}

/// A pane the user already closed is expected to exit, so `hold_on_exit` must not keep its shell
/// around and the exit must not be surfaced.
pub(crate) fn should_hold_on_exit(hold_on_exit: bool, closing: bool) -> bool {
    hold_on_exit && !closing
}
