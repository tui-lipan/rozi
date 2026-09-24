use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::actions::execute_action;
use crate::control::{
    CaptureRender, CaptureScrollback, ControlCommand, ControlEnvelope, ControlErrorCode,
    ControlResponse,
};
use crate::input::Action;
use crate::input::send_keys::{SendKeysItem, parse_send_keys_arg};
use crate::ops::focus::{
    focus_pane_anywhere, move_focused_to_workspace, request_current_pane_focus, switch_workspace,
};
use crate::pane::lifecycle::{find_pane_mut, spawn_interactive_pane_with_focus};
use crate::pane::pty_events::terminal_key_event_bytes;
use crate::state::{PaneId, PaneIdentity};

/// The documents this endpoint answers with are the CLI's contract, not this module's, so their
/// shapes live in [`crate::control`] and both control surfaces fill the same types.
use crate::control::{NewPaneAccepted, PaneCapture, PaneInfo, UiCapture};

pub(crate) fn handle_control_request(
    ctx: &mut Context<AppRoot>,
    envelope: ControlEnvelope,
) -> Update {
    if envelope
        .request
        .extension
        .as_ref()
        .is_some_and(|provenance| {
            !crate::config::provenance_is_active(&ctx.state.extension_generations, provenance)
        })
    {
        let _ = envelope.reply.send(ControlResponse::error_with(
            ControlErrorCode::ExtensionInactive,
            "extension generation is not active",
        ));
        return Update::none();
    }
    if envelope.request.command.pane_wait().is_some() {
        return crate::ops::capture_wait::start(ctx, envelope);
    }
    let response = match envelope.request.command {
        ControlCommand::ListPanes => list_panes(ctx),
        ControlCommand::LayoutGet { workspace } => layout_report(ctx, workspace),
        ControlCommand::LayoutSet {
            workspace,
            layout,
            master_ratio,
            if_revision,
        } => match crate::control::LayoutEdit::validate(layout, master_ratio) {
            Ok(edit) => layout_set(ctx, workspace, edit, if_revision),
            Err(response) => response,
        },
        ControlCommand::PaneSet {
            target,
            floating,
            fullscreen,
            rect,
            rect_fraction,
            split_ratio,
            width_ratio,
            if_revision,
        } => match crate::control::PaneEdit::validate(
            floating,
            fullscreen,
            rect,
            rect_fraction,
            split_ratio,
            width_ratio,
        ) {
            Ok(edit) => pane_set(ctx, target, edit, if_revision),
            Err(response) => response,
        },
        ControlCommand::PaneMove {
            target,
            workspace,
            if_revision,
        } => pane_move(ctx, target, workspace, if_revision),
        ControlCommand::PaneSwap {
            target,
            with,
            if_revision,
        } => pane_swap(ctx, target, with, if_revision),
        ControlCommand::PaneClose {
            target,
            if_revision,
        } => {
            // Closing schedules the pane's prune once its close animation ends, so the update is
            // this arm's to return rather than the plain redraw every other reply gets.
            let (response, update) = pane_close(ctx, target, if_revision);
            let _ = envelope.reply.send(response);
            return update;
        }
        ControlCommand::AgentsList => {
            ControlResponse::ok(crate::control::AgentListPayload(list_agents(ctx)))
        }
        ControlCommand::AgentGet { target } => match resolve_agent(ctx, &target) {
            Ok(agent) => ControlResponse::ok(agent),
            Err(response) => response,
        },
        ControlCommand::AgentRead { target, scrollback } => match resolve_agent(ctx, &target) {
            Ok(agent) => match validate_agent_input_reference(ctx, &agent.reference) {
                Ok(()) => capture_pane(
                    ctx,
                    Some(agent.pane),
                    scrollback,
                    CaptureRender::Text,
                    None,
                    false,
                ),
                Err(response) => response,
            },
            Err(response) => response,
        },
        ControlCommand::Metrics => {
            let response = runtime_metrics(ctx);
            let _ = envelope.reply.send(response);
            return Update::none();
        }
        ControlCommand::Focus { target } => focus_target(ctx, target),
        ControlCommand::SendText { target, text, .. } => {
            send_text(ctx, target.or(envelope.request.source_pane), text)
        }
        ControlCommand::SendKeys {
            target,
            keys,
            literal,
            ..
        } => send_keys(ctx, target.or(envelope.request.source_pane), keys, literal),
        ControlCommand::NewPane {
            command,
            argv,
            cwd,
            title,
            keep_open,
            focus,
            workspace,
        } => {
            return new_pane(
                ctx,
                envelope.request.source_pane,
                command,
                argv,
                cwd,
                title,
                keep_open,
                focus,
                workspace,
                envelope.reply,
            );
        }
        ControlCommand::RunAction { action } => {
            return run_action(ctx, &action, envelope.reply);
        }
        ControlCommand::CapturePane {
            target,
            scrollback,
            render,
            scale,
            wait: _,
            image_pixels,
        } => capture_pane(
            ctx,
            target.or(envelope.request.source_pane),
            scrollback,
            render,
            scale,
            image_pixels,
        ),
        ControlCommand::CaptureUi {
            render,
            scale,
            image_pixels,
        } => {
            let form = crate::control::capture_scale(render, scale).and_then(|scale| {
                crate::control::capture_image_pixels(render, image_pixels)
                    .map(|image_pixels| (render, scale, image_pixels))
            });
            match form {
                Ok(form) => capture_ui(ctx, form, envelope.reply),
                Err(response) => {
                    let _ = envelope.reply.send(response);
                }
            }
            return Update::none();
        }
        ControlCommand::Notify {
            message,
            title,
            level,
        } => notify_command(ctx, message, title, level),
        ControlCommand::SwitchWorkspace { index } => switch_workspace_command(ctx, index),
        ControlCommand::MoveToWorkspace { index } => move_to_workspace_command(ctx, index),
        ControlCommand::Popup {
            command,
            cwd,
            width,
            height,
            title,
            keep_open,
        } => {
            let keep_open = keep_open.unwrap_or(true);
            if let Some(update) = crate::ops::session::ensure_session_for_pty(
                ctx,
                crate::state::PendingSessionAction::Popup {
                    command: command.clone(),
                    cwd: cwd.clone(),
                    width,
                    height,
                    title: title.clone(),
                    keep_open,
                },
            ) {
                ctx.state.pending_control_reply = Some(envelope.reply);
                return update;
            }
            match crate::ops::popup::open(
                ctx,
                command,
                cwd,
                width,
                height,
                title,
                keep_open,
                Vec::new(),
            ) {
                Ok(update) => {
                    let _ = envelope.reply.send(ControlResponse::empty());
                    return update;
                }
                Err(error) => ControlResponse::error(error),
            }
        }
        ControlCommand::Subscribe { .. } => {
            ControlResponse::error("subscribe is handled by the control listener")
        }
        ControlCommand::Publish => {
            ControlResponse::error("publish is handled by the control listener")
        }
        ControlCommand::Pick { .. } => {
            ControlResponse::error("pick is handled by the control listener")
        }
        ControlCommand::PaneLogging { target, enabled } => {
            let id = target
                .or(envelope.request.source_pane)
                .or(ctx.state.focused_pane());
            match id.and_then(|id| crate::pane::lifecycle::find_pane(&ctx.state, id)) {
                Some(pane) => {
                    if let Some(client) = ctx.state.pty_client_for_pane(pane.id) {
                        client.set_pane_logging(
                            pane.id,
                            pane.pty_generation,
                            crate::pane::lifecycle::pane_is_local(&ctx.state, pane.id),
                            enabled.unwrap_or(!pane.logging),
                        );
                    }
                    ControlResponse::empty()
                }
                None => {
                    ControlResponse::error_with(ControlErrorCode::PaneNotFound, "pane not found")
                }
            }
        }
        ControlCommand::SetStatus {
            target,
            status,
            reason,
        } => set_status(
            ctx,
            target
                .or(envelope.request.source_pane)
                .or(ctx.state.focused_pane()),
            status,
            reason,
        ),
        ControlCommand::AgentWait { .. } => ControlResponse::error_with(
            ControlErrorCode::Unsupported,
            "agent wait is server-owned; select a named session with --session",
        ),
        ControlCommand::RecordStart { .. }
        | ControlCommand::RecordStop { .. }
        | ControlCommand::RecordList
        | ControlCommand::RecordMark { .. } => ControlResponse::error_with(
            ControlErrorCode::Unsupported,
            "pane recording is server-owned; select a named session with --session",
        ),
        ControlCommand::AgentPrompt { .. } => ControlResponse::error_with(
            ControlErrorCode::Unsupported,
            "agent prompt is server-owned; select a named session with --session",
        ),
        ControlCommand::AgentReport {
            target,
            agent,
            integration,
            state,
            reason,
            native_session,
            seq,
        } => {
            return report_agent(
                ctx,
                target.or(envelope.request.source_pane),
                Some(agent),
                integration,
                Some(state),
                reason,
                native_session,
                seq,
                envelope.reply,
            );
        }
        ControlCommand::AgentRelease {
            target,
            integration,
            seq,
        } => {
            return report_agent(
                ctx,
                target.or(envelope.request.source_pane),
                None,
                integration,
                None,
                None,
                None,
                seq,
                envelope.reply,
            );
        }
    };
    let _ = envelope.reply.send(response);
    Update::full()
}

fn runtime_metrics(ctx: &Context<AppRoot>) -> ControlResponse {
    if let Some(client) = &ctx.state.current().session_client {
        // Refresh asynchronously for the next sample; this response always uses the cache.
        client.request_runtime_metrics();
    }
    ControlResponse::ok(crate::runtime_metrics::RuntimeMetrics::capture(
        ctx.state.current(),
    ))
}

impl PaneInfo {
    /// Scratch panes report workspace `0`; a tiled pane reports its one-based workspace number.
    fn new(
        pane: &crate::state::Pane,
        workspace: usize,
        session: &str,
        session_instance: Option<&crate::session::protocol::SessionInstanceId>,
    ) -> Self {
        let runtime = pane
            .agent_runtimes()
            .into_iter()
            .find(|runtime| runtime.reference.slot.is_none());
        Self {
            session: session.to_string(),
            id: pane.id,
            reference: session_instance.map(|session_instance| crate::session::protocol::PaneRef {
                session_instance: session_instance.clone(),
                pane_id: pane.id,
                generation: pane.pty_generation,
            }),
            agent_ref: session_instance
                .and_then(|_| runtime.as_ref().map(|runtime| runtime.reference.clone())),
            title: pane.display_title(pane.terminal.title()),
            workspace,
            command: pane
                .identity
                .launch
                .as_ref()
                .and_then(crate::pane::launch::PaneLaunch::shell_command)
                .map(str::to_string),
            argv: pane
                .identity
                .launch
                .as_ref()
                .and_then(crate::pane::launch::PaneLaunch::argv)
                .map(<[String]>::to_vec),
            foreground_program: pane.terminal.foreground_program.clone(),
            foreground_programs: pane.terminal.foreground_programs.clone(),
            foreground_arguments: pane.terminal.foreground_arguments.clone(),
            cwd: pane.live_cwd().or_else(|| pane.identity.cwd.clone()),
            status: pane.terminal.status_text(),
            reported_status: pane
                .terminal
                .reported_status
                .as_ref()
                .map(|status| status.value.clone()),
            status_reason: pane
                .terminal
                .reported_status
                .as_ref()
                .and_then(|status| status.reason.clone()),
            agent: runtime.as_ref().map(|runtime| runtime.identity.id.clone()),
            agent_state: runtime
                .as_ref()
                .map(|runtime| runtime.state.as_str().to_string()),
            recording: pane.terminal.recording,
        }
    }
}

/// The session name a report carries, qualified with its host when the attachment is remote so
/// two same-name sessions do not look interchangeable.
fn session_label(attachment: &crate::state::Attachment) -> String {
    let name = attachment.session_name.as_deref().unwrap_or("local");
    attachment
        .remote_host
        .as_deref()
        .map_or_else(|| name.to_string(), |host| format!("{name}@{host}"))
}

/// The canvas this UI's shared rects are measured against: the controller's canonical canvas for
/// a follower, this client's own pane canvas otherwise - the one it commits with.
fn layout_canvas(ctx: &Context<AppRoot>) -> (u16, u16) {
    ctx.state.follower_canonical_canvas().unwrap_or_else(|| {
        let bounds = ctx
            .state
            .canvas_bounds_from_terminal_viewport(ctx.viewport());
        (
            bounds.w.round().max(1.0) as u16,
            bounds.h.round().max(1.0) as u16,
        )
    })
}

/// Send any layout change this UI is still debouncing, then name the revision its layout has.
///
/// A controller pipelines commits ahead of the server's acknowledgement, so its layout's revision
/// is the one it last sent (`assumed_rev`); `committed` says whether the server has confirmed it.
/// Flushing first is what makes that revision describe the layout the report shows: without it a
/// change still inside the debounce window would have no revision at all. A follower has nothing
/// of its own to send and reports what it applied.
fn flushed_revision(ctx: &mut Context<AppRoot>) -> (Option<u64>, bool) {
    crate::ops::session::flush_layout_commit(ctx);
    match ctx.state.current().shared.as_ref() {
        None => (None, true),
        Some(shared) if shared.is_controller() => (
            Some(shared.assumed_rev),
            shared.assumed_rev == shared.layout_rev,
        ),
        Some(shared) => (Some(shared.layout_rev), true),
    }
}

/// Describe `layout`'s workspaces, adding where this UI draws each pane of the one it shows.
fn workspace_reports(
    ctx: &Context<AppRoot>,
    layout: &crate::layout::shared::SharedLayout,
    only: Option<usize>,
) -> Vec<crate::control::WorkspaceLayout> {
    let attachment = ctx.state.current();
    let mut workspaces = crate::control::WorkspaceLayout::from_shared(
        layout,
        only,
        attachment.session_instance.as_ref(),
    );
    let active_workspace = attachment.active_workspace + 1;
    if let Some(active) = workspaces
        .iter_mut()
        .find(|workspace| workspace.index == active_workspace)
    {
        let view_rects = crate::view::settled_active_pane_rects(&ctx.state, ctx.viewport());
        for pane in &mut active.panes {
            pane.view_rect = view_rects
                .iter()
                .find(|(id, _)| *id == pane.id)
                .map(|(_, rect)| crate::control::CellRect::from_float(*rect));
        }
    }
    workspaces
}

/// The shared document `layout get` describes from this UI.
///
/// A follower reports the document it last applied, exactly as the server holds it. Rebuilding one
/// from its own `State` would be wrong wherever the two differ: a document the server started for a
/// headless `split` names only the workspaces it placed panes in, and the follower's other
/// workspaces carry its local defaults, which nobody shared. A controller's own `State` *is* the
/// layout, and it commits that document on its next update whether or not anyone asks - so that
/// is the one it reports.
fn reported_layout(ctx: &Context<AppRoot>) -> crate::layout::shared::SharedLayout {
    ctx.state
        .current()
        .shared
        .as_ref()
        .filter(|shared| !shared.is_controller())
        .and_then(|shared| shared.last_committed_layout.clone())
        .unwrap_or_else(|| {
            crate::layout::shared::shared_layout_from_state(&ctx.state, layout_canvas(ctx))
        })
}

/// `layout get` from this UI.
///
/// The shared half is built from the document this client would commit, measured against
/// [`layout_canvas`]. That is the same document a session endpoint reports, so the two answers
/// agree once the client's changes are committed. The client half adds what only this UI knows:
/// focus, the workspace it shows, and where it draws each pane.
fn layout_report(ctx: &mut Context<AppRoot>, workspace: Option<usize>) -> ControlResponse {
    if let Err(response) = crate::control::validate_layout_workspace(workspace) {
        return response;
    }
    let (revision, committed) = flushed_revision(ctx);
    let layout = reported_layout(ctx);
    let workspaces = workspace_reports(ctx, &layout, workspace);
    let viewport = ctx.viewport();
    let attachment = ctx.state.current();
    ControlResponse::ok(crate::control::LayoutReport {
        session: session_label(attachment),
        revision,
        canvas: Some(crate::control::CellSize {
            cols: layout.canvas_cols,
            rows: layout.canvas_rows,
        }),
        workspaces,
        unplaced_panes: Vec::new(),
        client: Some(crate::control::ClientLayoutView {
            active_workspace: attachment.active_workspace + 1,
            focused_pane: attachment.focused_pane,
            controller: ctx.state.is_controller(),
            committed,
            viewport: crate::control::CellSize {
                cols: viewport.w,
                rows: viewport.h,
            },
        }),
    })
}

/// Refuse a layout write from a UI that may not make one, before anything is checked or changed.
///
/// The same lease that stops a follower's keyboard stops its socket: a script driving a follower
/// would otherwise reshape a session someone else is arranging.
fn layout_write_gate(ctx: &Context<AppRoot>) -> std::result::Result<(), ControlResponse> {
    if ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only)
    {
        return Err(ControlResponse::error_with(
            ControlErrorCode::ReadOnly,
            "this UI is attached read-only and cannot change the layout",
        ));
    }
    if !ctx.state.is_controller() {
        return Err(ControlResponse::error_with(
            ControlErrorCode::NotController,
            "this UI does not hold layout control; take control first, or change the layout from the UI that has it",
        ));
    }
    Ok(())
}

/// The checks every layout write makes before looking at its target: authority, then revision.
fn layout_write_preflight(
    ctx: &mut Context<AppRoot>,
    if_revision: Option<u64>,
) -> std::result::Result<(), ControlResponse> {
    layout_write_gate(ctx)?;
    let (revision, _) = flushed_revision(ctx);
    crate::control::check_if_revision(if_revision, revision)
}

/// The zero-based workspace holding live pane `target`, and whether it floats.
fn locate_layout_pane(
    ctx: &Context<AppRoot>,
    target: PaneId,
) -> std::result::Result<(usize, bool), ControlResponse> {
    let found = ctx
        .state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .find_map(|(index, workspace)| {
            workspace
                .panes
                .iter()
                .find(|pane| pane.id == target && !pane.closing)
                .map(|pane| (index, pane.floating))
        });
    found.ok_or_else(|| {
        if ctx.state.scratch.panes.iter().any(|pane| pane.id == target) {
            ControlResponse::error_with(
                ControlErrorCode::Unsupported,
                format!(
                    "pane {target} is a scratch pane, which is client-local and has no shared layout"
                ),
            )
        } else {
            ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {target} not found"),
            )
        }
    })
}

/// Commit a layout write and describe the workspace it touched.
fn layout_change_reply(
    ctx: &mut Context<AppRoot>,
    changed: bool,
    workspace: usize,
) -> ControlResponse {
    if changed {
        // Arranging a session is using it: from here on it is not a disposable one.
        ctx.state.current_mut().engaged = true;
    }
    let (revision, committed) = flushed_revision(ctx);
    let layout = crate::layout::shared::shared_layout_from_state(&ctx.state, layout_canvas(ctx));
    let Some(workspace) = workspace_reports(ctx, &layout, Some(workspace + 1)).pop() else {
        return ControlResponse::error(format!("workspace {} is missing", workspace + 1));
    };
    ControlResponse::ok(crate::control::LayoutChange {
        changed,
        revision,
        committed,
        workspace,
    })
}

/// `layout set` from this UI. `workspace` is one-based.
fn layout_set(
    ctx: &mut Context<AppRoot>,
    workspace: usize,
    edit: crate::control::LayoutEdit,
    if_revision: Option<u64>,
) -> ControlResponse {
    let index = workspace.wrapping_sub(1);
    if let Err(response) = crate::control::validate_layout_workspace(Some(workspace))
        .and_then(|()| layout_write_preflight(ctx, if_revision))
        .and_then(|()| edit.check(ctx.state.current().workspaces[index].layout_kind))
    {
        return response;
    }
    let target = &mut ctx.state.current_mut().workspaces[index];
    let mut changed = edit
        .kind
        .is_some_and(|kind| crate::ops::resize_move::set_workspace_layout(target, kind));
    if let Some(ratio) = edit.master_ratio {
        changed |= crate::ops::resize_move::set_master_ratio(target, ratio);
    }
    if changed && index == ctx.state.current().active_workspace {
        ctx.state.animation = crate::layout::anim::GeometryAnimation::AxisChange;
    }
    layout_change_reply(ctx, changed, index)
}

/// `pane set` from this UI.
///
/// Rects are resolved from the shared document through the same function a session server uses,
/// then applied to the live workspace through the primitives the interactive toggles use - so the
/// request lands where it would have landed headlessly, and the pane gets there the way a pane a
/// person floats does.
fn pane_set(
    ctx: &mut Context<AppRoot>,
    target: PaneId,
    edit: crate::control::PaneEdit,
    if_revision: Option<u64>,
) -> ControlResponse {
    let (index, floating_now) = match layout_write_preflight(ctx, if_revision)
        .and_then(|()| locate_layout_pane(ctx, target))
    {
        Ok(found) => found,
        Err(response) => return response,
    };
    let in_split = {
        let workspace = &ctx.state.current().workspaces[index];
        crate::layout::effective_tile_tree(workspace, None)
            .is_some_and(|tree| crate::layout::tiling::leaf_share(&tree, target).is_some())
    };
    if let Err(response) = edit.check_rect_target(floating_now).and_then(|()| {
        edit.check_sizes(
            floating_now,
            ctx.state.current().workspaces[index].layout_kind,
            in_split,
        )
    }) {
        return response;
    }

    let canvas = layout_canvas(ctx);
    let before = crate::layout::shared::shared_layout_from_state(&ctx.state, canvas);
    let floats = edit.floats(floating_now);
    let float_rect = floats.then(|| {
        before
            .workspaces
            .iter()
            .find(|workspace| workspace.index == index)
            .and_then(|workspace| {
                crate::layout::shared::automation_float_rect(
                    workspace,
                    canvas,
                    target,
                    edit.rect.map(|rect| rect.to_cells(canvas)),
                )
            })
            .unwrap_or_else(|| {
                crate::layout::geometry::default_floating_rect(
                    crate::layout::shared::canvas_bounds(canvas),
                    0,
                )
            })
    });

    let workspace = &mut ctx.state.current_mut().workspaces[index];
    let mut moved = false;
    match float_rect {
        // A float that stays put keeps its rect exactly, rather than a round trip through cells.
        Some(rect) if !floating_now || edit.rect.is_some() => {
            moved = crate::ops::resize_move::float_pane(workspace, target, rect);
        }
        Some(_) => {}
        None => moved = crate::ops::resize_move::tile_pane(workspace, target, None),
    }
    let mut fullscreen_changed = false;
    if let Some(fullscreen) = edit.fullscreen {
        fullscreen_changed =
            crate::ops::resize_move::set_pane_fullscreen(workspace, target, fullscreen);
    }
    // Sizes snap rather than animate: a terminal resized over several frames reflows its program
    // once per frame.
    if let Some(width) = edit.width_ratio
        && let Some(pane) = workspace.panes.iter_mut().find(|pane| pane.id == target)
    {
        pane.scrollable_width = width;
    }
    if let Some(share) = edit.split_ratio {
        crate::ops::resize_move::set_pane_split_share(workspace, target, share);
    }

    // Judged on the document, so `changed` means exactly "this commits a new revision".
    let changed = crate::layout::shared::shared_layout_from_state(&ctx.state, canvas) != before;
    if changed && index == ctx.state.current().active_workspace {
        ctx.state.animation = if moved {
            crate::layout::anim::GeometryAnimation::TileFloat
        } else if fullscreen_changed {
            crate::layout::anim::GeometryAnimation::Fullscreen
        } else {
            ctx.state.animation
        };
    }
    layout_change_reply(ctx, changed, index)
}

/// `pane move` from this UI. `workspace` is one-based.
///
/// Focus stays where it is unless the moved pane had it, and then it falls back inside the
/// workspace the pane left, as it would if the pane had closed.
fn pane_move(
    ctx: &mut Context<AppRoot>,
    target: PaneId,
    workspace: usize,
    if_revision: Option<u64>,
) -> ControlResponse {
    let source = match crate::control::validate_layout_workspace(Some(workspace))
        .and_then(|()| layout_write_preflight(ctx, if_revision))
        .and_then(|()| locate_layout_pane(ctx, target))
    {
        Ok((source, _)) => source,
        Err(response) => return response,
    };
    let destination = workspace - 1;
    let changed = crate::ops::focus::transfer_pane(&mut ctx.state, target, source, destination);
    if changed {
        let active = ctx.state.current().active_workspace;
        if source == active && !ctx.state.scratch_visible {
            crate::ops::focus::choose_fallback_focus(&mut ctx.state);
        } else {
            let left = &mut ctx.state.current_mut().workspaces[source];
            if left.focused_pane == Some(target) {
                left.focused_pane = crate::ops::focus::first_visible_pane(left);
            }
        }
        if source == active || destination == active {
            ctx.state.animation = crate::layout::anim::GeometryAnimation::TileFloat;
        }
    }
    layout_change_reply(ctx, changed, destination)
}

/// `pane swap` from this UI.
fn pane_swap(
    ctx: &mut Context<AppRoot>,
    target: PaneId,
    with: PaneId,
    if_revision: Option<u64>,
) -> ControlResponse {
    let found = layout_write_preflight(ctx, if_revision)
        .and_then(|()| locate_layout_pane(ctx, target))
        .and_then(|first| locate_layout_pane(ctx, with).map(|second| (first, second)));
    let index = match found {
        Ok(((index, false), (other_index, false))) if index == other_index && target != with => {
            index
        }
        Ok(_) => {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                format!(
                    "panes {target} and {with} cannot swap; a swap exchanges two different tiled panes of one workspace"
                ),
            );
        }
        Err(response) => return response,
    };
    let changed = crate::ops::resize_move::swap_tiled_panes(
        &mut ctx.state.current_mut().workspaces[index],
        target,
        with,
    );
    if changed && index == ctx.state.current().active_workspace {
        ctx.state.animation = crate::layout::anim::GeometryAnimation::TileFloat;
    }
    layout_change_reply(ctx, changed, index)
}

/// `pane close` from this UI: the request is the confirmation, so `[confirm]` is not consulted.
fn pane_close(
    ctx: &mut Context<AppRoot>,
    target: PaneId,
    if_revision: Option<u64>,
) -> (ControlResponse, Update) {
    let index = match layout_write_preflight(ctx, if_revision)
        .and_then(|()| locate_layout_pane(ctx, target))
    {
        Ok((index, _)) => index,
        Err(response) => return (response, Update::none()),
    };
    ctx.state.current_mut().engaged = true;
    let update = crate::pane::lifecycle::close_pane(ctx, target);
    let (revision, committed) = flushed_revision(ctx);
    let layout = crate::layout::shared::shared_layout_from_state(&ctx.state, layout_canvas(ctx));
    let workspace = workspace_reports(ctx, &layout, Some(index + 1)).pop();
    (
        ControlResponse::ok(crate::control::PaneClosed {
            id: target,
            revision,
            committed,
            workspace,
        }),
        update,
    )
}

fn list_panes(ctx: &Context<AppRoot>) -> ControlResponse {
    let mut panes = Vec::new();
    let attachment = ctx.state.current();
    let session = session_label(attachment);
    let session_instance = attachment.session_instance.as_ref();
    for (workspace_index, workspace) in attachment.workspaces.iter().enumerate() {
        for pane in workspace.panes.iter().filter(|pane| !pane.closing) {
            panes.push(PaneInfo::new(
                pane,
                workspace_index + 1,
                &session,
                session_instance,
            ));
        }
    }
    for pane in ctx.state.scratch.panes.iter().filter(|pane| !pane.closing) {
        panes.push(PaneInfo::new(pane, 0, &session, None));
    }
    ControlResponse::ok(crate::control::PaneListPayload(panes))
}

fn list_agents(ctx: &Context<AppRoot>) -> Vec<crate::control::AgentInfo> {
    let attachment = ctx.state.current();
    let session = session_label(attachment);
    let mut agents = Vec::new();
    for (workspace_index, workspace) in attachment.workspaces.iter().enumerate() {
        for pane in workspace.panes.iter().filter(|pane| !pane.closing) {
            for runtime in pane.agent_runtimes() {
                agents.push(crate::control::AgentInfo {
                    session: session.clone(),
                    pane: pane.id,
                    workspace: workspace_index + 1,
                    agent: runtime.identity.id,
                    label: runtime.label,
                    state: runtime.state,
                    reason: runtime.reason,
                    cwd: pane.live_cwd().or_else(|| pane.identity.cwd.clone()),
                    native_session: pane
                        .terminal
                        .agent_integration
                        .as_ref()
                        .and_then(|report| report.native_session.clone()),
                    reference: runtime.reference,
                    source: runtime.source,
                });
            }
        }
    }
    agents.sort_by(|left, right| {
        left.pane
            .cmp(&right.pane)
            .then_with(|| left.reference.slot.cmp(&right.reference.slot))
    });
    agents
}

fn resolve_agent(
    ctx: &Context<AppRoot>,
    target: &crate::control::AgentTarget,
) -> std::result::Result<crate::control::AgentInfo, ControlResponse> {
    let agents = list_agents(ctx);
    match target {
        crate::control::AgentTarget::Pane(pane_id) => {
            let mut matching = agents.into_iter().filter(|agent| agent.pane == *pane_id);
            let Some(agent) = matching.next() else {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::AgentGone,
                    format!("pane {pane_id} has no agent"),
                ));
            };
            if matching.next().is_some() {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::TargetRequired,
                    format!(
                        "pane {pane_id} has multiple agent activities; use an exact agent reference"
                    ),
                ));
            }
            Ok(agent)
        }
        crate::control::AgentTarget::Ref(reference) => {
            let current_instance = ctx.state.current().session_instance.as_ref();
            if current_instance != Some(&reference.pane.session_instance) {
                return Err(ControlResponse::error_with(
                    ControlErrorCode::StaleReference,
                    "agent reference belongs to a different session server instance",
                ));
            }
            agents
                .into_iter()
                .find(|agent| agent.reference == *reference)
                .ok_or_else(|| {
                    ControlResponse::error_with(
                        ControlErrorCode::AgentReplaced,
                        "agent reference no longer names the current incarnation",
                    )
                })
        }
    }
}

fn validate_agent_input_reference(
    ctx: &Context<AppRoot>,
    reference: &crate::session::protocol::AgentRef,
) -> std::result::Result<(), ControlResponse> {
    let Some(slot) = reference.slot.as_deref() else {
        return Ok(());
    };
    let active = crate::pane::lifecycle::find_pane(&ctx.state, reference.pane.pane_id)
        .and_then(|pane| {
            pane.terminal
                .published_rows
                .iter()
                .find(|row| row.id == slot)
        })
        .is_some_and(|row| row.active);
    if active {
        Ok(())
    } else {
        Err(ControlResponse::error_with(
            ControlErrorCode::Conflict,
            format!(
                "published agent slot `{slot}` is not active; read only targets the visible activity"
            ),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn report_agent(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    agent: Option<String>,
    integration: String,
    state: Option<crate::session::protocol::AgentState>,
    reason: Option<String>,
    native_session: Option<String>,
    seq: u64,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let Some(id) = target else {
        let _ = reply.send(ControlResponse::error_with(
            ControlErrorCode::TargetRequired,
            "agents report requires --target or ROZI_PANE",
        ));
        return Update::none();
    };
    let Some(pane) = crate::pane::lifecycle::find_pane(&ctx.state, id).filter(|pane| !pane.closing)
    else {
        let _ = reply.send(ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {id} not found"),
        ));
        return Update::none();
    };
    let pane_id = pane.id;
    let generation = pane.pty_generation;
    let local = crate::pane::lifecycle::pane_is_local(&ctx.state, pane.id);
    let scratch = crate::scratchpad::contains(&ctx.state, pane.id);
    let Some(client) = ctx.state.pty_client_for_pane(pane.id) else {
        let _ = reply.send(ControlResponse::error_with(
            ControlErrorCode::SessionNotConnected,
            format!("pane {id} has no session server"),
        ));
        return Update::none();
    };
    let request_id = ctx.state.next_agent_report_request_id;
    ctx.state.next_agent_report_request_id = ctx
        .state
        .next_agent_report_request_id
        .wrapping_add(1)
        .max(1);
    ctx.state.pending_agent_report_replies.insert(
        request_id,
        crate::state::PendingAgentReportReply {
            origin_epoch: (!scratch).then_some(ctx.state.runtime_epoch),
            reply,
        },
    );
    client.report_agent(
        request_id,
        pane_id,
        generation,
        local,
        agent,
        integration,
        state,
        reason,
        native_session,
        seq,
    );
    Update::none()
}

fn set_status(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    status: Option<String>,
    reason: Option<String>,
) -> ControlResponse {
    let Some(id) = target else {
        return ControlResponse::error_with(
            ControlErrorCode::TargetRequired,
            "no target pane and no focused pane",
        );
    };
    let Some(pane) = crate::pane::lifecycle::find_pane(&ctx.state, id).filter(|pane| !pane.closing)
    else {
        return ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {id} not found"),
        );
    };
    let generation = pane.pty_generation;
    let local = crate::pane::lifecycle::pane_is_local(&ctx.state, id);
    let scratch = crate::scratchpad::contains(&ctx.state, id);
    if !scratch && !ctx.state.current().session_attached {
        return ControlResponse::error_with(
            ControlErrorCode::SessionNotAttached,
            format!("pane {id} session is not attached"),
        );
    }
    if !scratch
        && ctx
            .state
            .current()
            .shared
            .as_ref()
            .is_some_and(|shared| shared.read_only)
    {
        return ControlResponse::error_with(ControlErrorCode::ReadOnly, "attached read-only");
    }
    let Some(client) = ctx.state.pty_client_for_pane(id) else {
        return ControlResponse::error_with(
            ControlErrorCode::SessionNotConnected,
            format!("pane {id} session is not connected"),
        );
    };
    client.set_pane_status(id, generation, local, status, reason);
    ControlResponse::empty()
}

fn focus_target(ctx: &mut Context<AppRoot>, target: PaneId) -> ControlResponse {
    if crate::scratchpad::contains(&ctx.state, target) {
        if !ctx.state.scratch_visible {
            return ControlResponse::error("scratchpad is hidden");
        }
        crate::ops::focus::focus_pane(&mut ctx.state, target);
        crate::ops::focus::request_pane_focus(ctx, target);
        return ControlResponse::empty();
    }
    if ctx.state.scratch_visible {
        return ControlResponse::error("scratchpad is open");
    }
    if !focus_pane_anywhere(ctx, target) {
        return ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {target} not found"),
        );
    }
    ControlResponse::empty()
}

/// A resolved `send-text` / `send-keys` destination.
pub(super) struct InputTarget {
    pub(super) id: PaneId,
    pub(super) generation: u64,
    pub(super) local: bool,
    modes: TerminalKeyModes,
    /// The PTY accepts input now. When false the spawn is still in flight and bytes are queued as
    /// type-ahead instead.
    pub(super) starting: bool,
    pub(super) client: Option<crate::session::client::SessionClient>,
}

/// Resolve and validate the pane a control input request targets.
///
/// A pane whose PTY is still starting is a valid target: its bytes are queued rather than rejected,
/// which is what a person typing into a freshly split pane already gets. A pane that has exited or
/// failed is not — those keep failing loudly, because nothing will ever read the input.
fn control_input_target(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
) -> std::result::Result<InputTarget, ControlResponse> {
    if let Some(reason) = ctx.state.pane_input_block_reason() {
        return Err(ControlResponse::error(reason));
    }
    let Some(id) = target.or(ctx.state.focused_pane()) else {
        return Err(ControlResponse::error_with(
            ControlErrorCode::TargetRequired,
            "no target pane and no focused pane",
        ));
    };
    let client = ctx.state.pty_client_for_pane(id);
    let local = crate::pane::lifecycle::pane_is_local(&ctx.state, id);
    let Some(pane) = find_pane_mut(&mut ctx.state, id).filter(|pane| !pane.closing) else {
        return Err(ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {id} not found"),
        ));
    };
    let ready = pane.terminal.accepts_input();
    if !ready && !pane.terminal.is_running() {
        return Err(ControlResponse::error_with(
            ControlErrorCode::PaneNotRunning,
            format!("pane {id} PTY is not running"),
        ));
    }
    if ready && client.is_none() {
        return Err(ControlResponse::error_with(
            ControlErrorCode::SessionNotConnected,
            format!("pane {id} session is not connected"),
        ));
    }
    Ok(InputTarget {
        id,
        generation: pane.pty_generation,
        local,
        modes: pane.terminal.snapshot().key_modes,
        starting: !ready,
        client,
    })
}

/// Write control input to the PTY, or append it to the pane's type-ahead queue while the spawn is
/// still in flight (see [`crate::state::State::pending_control_input`]).
///
/// A `mark` asks the server to answer it in the same step as writing the bytes (see
/// [`crate::session::protocol::ClientMessage::MarkedInput`]). Queued input carries no mark; its
/// waits are marked when the queue is written.
fn deliver_control_input(
    ctx: &mut Context<AppRoot>,
    target: &InputTarget,
    bytes: Vec<u8>,
    mark: Option<u64>,
) {
    if target.starting {
        ctx.state
            .pending_control_input
            .entry((target.local, target.id, target.generation))
            .or_default()
            .extend(bytes);
        return;
    }
    if let Some(client) = target.client.as_ref() {
        match mark {
            Some(token) => {
                client.send_marked_input(target.id, target.generation, target.local, bytes, token);
            }
            None => client.send_input(target.id, target.generation, target.local, bytes),
        }
    }
}

fn send_text(ctx: &mut Context<AppRoot>, target: Option<PaneId>, text: String) -> ControlResponse {
    let target = match control_input_target(ctx, target) {
        Ok(target) => target,
        Err(response) => return response,
    };
    deliver_control_input(ctx, &target, text.into_bytes(), None);
    ControlResponse::empty()
}

fn send_keys(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    keys: Vec<String>,
    literal: bool,
) -> ControlResponse {
    match send_keys_to(ctx, target, &keys, literal, None) {
        Ok(_) => ControlResponse::empty(),
        Err(response) => response,
    }
}

fn send_keys_to(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    keys: &[String],
    literal: bool,
    mark: Option<u64>,
) -> std::result::Result<InputTarget, ControlResponse> {
    let target = control_input_target(ctx, target)?;

    // Encode every argument before writing any so invalid input never reaches the PTY, and so a
    // queued batch is all-or-nothing rather than half-written when a later key is unrepresentable.
    let mut bytes = Vec::new();
    for key in keys {
        match parse_send_keys_arg(key, literal).map_err(ControlResponse::error)? {
            SendKeysItem::Text(text) => bytes.extend(text.into_bytes()),
            SendKeysItem::Key(event) => {
                let Some(encoded) = terminal_key_event_bytes(event, target.modes) else {
                    return Err(ControlResponse::error(
                        "key is not representable for session forwarding yet",
                    ));
                };
                bytes.extend(encoded);
            }
        }
    }

    deliver_control_input(ctx, &target, bytes, mark);
    Ok(target)
}

/// Write a `send-text` or `send-keys` request's input as it would be without a wait, marked with
/// `mark` unless the pane is still starting, and say where it went.
pub(super) fn deliver_send(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    command: &ControlCommand,
    mark: u64,
) -> std::result::Result<InputTarget, ControlResponse> {
    match command {
        ControlCommand::SendText { text, .. } => {
            let target = control_input_target(ctx, target)?;
            deliver_control_input(ctx, &target, text.clone().into_bytes(), Some(mark));
            Ok(target)
        }
        ControlCommand::SendKeys { keys, literal, .. } => {
            send_keys_to(ctx, target, keys, *literal, Some(mark))
        }
        _ => Err(ControlResponse::error("not a send command")),
    }
}

/// Run any keybindable action by its stable id, the same names used in `[keys]` config and the
/// command palette (see `Action::id`/`Action::from_id`).
fn run_action(
    ctx: &mut Context<AppRoot>,
    action_id: &str,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let action = action_from_runtime_id(&ctx.state, action_id);
    let Some(action) = action else {
        let error = if crate::config::is_extension_scoped_id(action_id) {
            let extension = action_id
                .split_once('.')
                .map(|(id, _)| id)
                .unwrap_or_default();
            if ctx.state.config.active_extensions.contains(extension) {
                format!("extension `{extension}` does not define command `{action_id}`")
            } else {
                format!(
                    "extension command `{action_id}` is unavailable; run `rozi extensions list --verbose`"
                )
            }
        } else {
            format!("unknown action `{action_id}`")
        };
        let _ = reply.send(ControlResponse::error(error));
        return Update::full();
    };
    if crate::actions::is_layout_mutating(&ctx.state, action)
        && !ctx.state.scratch_visible
        && !ctx.state.is_controller()
    {
        let _ = reply.send(ControlResponse::error_with(
            ControlErrorCode::NotController,
            "not controller",
        ));
        return Update::full();
    }
    if crate::actions::is_blocked_by_scratchpad(&ctx.state, action) {
        let _ = reply.send(ControlResponse::error("scratchpad is open"));
        return Update::none();
    }
    // Leaving is interactive: it can raise the prompt that asks whether to keep a temporary
    // session, and there is nobody on this socket to answer it. A scripted exit takes the
    // preserving path instead, so automation can never be what closes a session.
    if matches!(action, Action::Quit | Action::Detach) {
        let update = crate::ops::exit::leave_client_unattended(ctx);
        let _ = reply.send(ControlResponse::empty());
        return update;
    }
    let update = execute_action(ctx, action);
    let _ = reply.send(ControlResponse::empty());
    update
}

fn action_from_runtime_id(state: &crate::state::State, action_id: &str) -> Option<Action> {
    Action::from_id(action_id).or_else(|| {
        state
            .config
            .commands
            .iter()
            .position(|command| command.id == action_id)
            .map(Action::RunNamedCommand)
    })
}

fn capture_pane(
    ctx: &mut Context<AppRoot>,
    target: Option<PaneId>,
    scrollback: Option<CaptureScrollback>,
    render: CaptureRender,
    scale: Option<u8>,
    image_pixels: bool,
) -> ControlResponse {
    let Some(id) = target.or(ctx.state.focused_pane()) else {
        return ControlResponse::error_with(
            ControlErrorCode::TargetRequired,
            "no target pane and no focused pane",
        );
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, id) else {
        return ControlResponse::error_with(
            ControlErrorCode::PaneNotFound,
            format!("pane {id} not found"),
        );
    };
    let content = match pane.terminal.with_screen_mut(|screen| {
        crate::pane::capture_screen(screen, scrollback, render, scale, image_pixels)
    }) {
        Ok(content) => content,
        Err(response) => return response,
    };
    let title = pane.terminal.title();
    ControlResponse::ok(PaneCapture { id, title, content })
}

/// What a `capture-ui` request asks the frame to be encoded as: its render, checked PNG scale, and
/// checked `image_pixels`.
type UiCaptureForm = (CaptureRender, u8, bool);

/// `capture-ui` requests that will be answered from the same painted frame.
#[derive(Default)]
pub(crate) struct UiCaptureBatch {
    waiters: Vec<(UiCaptureForm, std::sync::mpsc::Sender<ControlResponse>)>,
    /// The frame has arrived; a request after this waits for the next one.
    served: bool,
}

/// Answer `capture-ui` with the next frame the client paints.
///
/// The request forces that paint, so an idle client answers too. Requests that arrive before it
/// join one batch: tui-lipan hands the frame to a single callback, which encodes each form asked
/// for once and answers every waiter from it, so four agents asking for a PNG at once cost one
/// encode rather than four.
fn capture_ui(
    ctx: &mut Context<AppRoot>,
    form: UiCaptureForm,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) {
    if let Some(batch) = &ctx.state.pending_ui_capture {
        let mut batch = batch.borrow_mut();
        if !batch.served {
            batch.waiters.push((form, reply));
            return;
        }
    }

    let batch = std::rc::Rc::new(std::cell::RefCell::new(UiCaptureBatch {
        waiters: vec![(form, reply)],
        served: false,
    }));
    ctx.state.pending_ui_capture = Some(std::rc::Rc::clone(&batch));
    let theme = &ctx.state.theme;
    let palette = TerminalColorPalette::from_theme(theme, theme.surface.backdrop);
    ctx.request_ui_snapshot(Callback::new(move |snapshot: tui_lipan::UiSnapshot| {
        let waiters = {
            let mut batch = batch.borrow_mut();
            batch.served = true;
            std::mem::take(&mut batch.waiters)
        };
        answer_ui_captures(snapshot.frame, palette, waiters);
    }));
}

/// Encode `frame` once per form the waiters asked for, and answer each of them.
///
/// A PNG of the whole client takes long enough to encode that it would stall the next frame, so
/// the work runs off the UI thread.
fn answer_ui_captures(
    frame: tui_lipan::CapturedFrame,
    palette: TerminalColorPalette,
    waiters: Vec<(UiCaptureForm, std::sync::mpsc::Sender<ControlResponse>)>,
) {
    let fallback: Vec<_> = waiters.iter().map(|(_, reply)| reply.clone()).collect();
    let spawned = std::thread::Builder::new()
        .name("rozi-capture-ui".into())
        .spawn(move || {
            let mut encoded: Vec<(UiCaptureForm, ControlResponse)> = Vec::new();
            for (form, reply) in waiters {
                let response = match encoded.iter().find(|(done, _)| *done == form) {
                    Some((_, response)) => response.clone(),
                    None => {
                        let response = encode_ui_capture(&frame, form, palette);
                        encoded.push((form, response.clone()));
                        response
                    }
                };
                let _ = reply.send(response);
            }
        });
    if let Err(error) = spawned {
        let response = ControlResponse::error(format!("cannot start the capture encoder: {error}"));
        for reply in fallback {
            let _ = reply.send(response.clone());
        }
    }
}

fn encode_ui_capture(
    frame: &tui_lipan::CapturedFrame,
    (render, scale, image_pixels): UiCaptureForm,
    palette: TerminalColorPalette,
) -> ControlResponse {
    match crate::pane::capture_ui_frame(frame, render, scale, image_pixels, palette) {
        Ok(content) => ControlResponse::ok(UiCapture {
            width: frame.width,
            height: frame.height,
            content,
        }),
        Err(response) => response,
    }
}

/// Raise a toast on behalf of a script.
///
/// Empty messages are rejected rather than shown: a blank toast is a bug in the caller, and it
/// would still occupy the slot a real message needs.
fn notify_command(
    ctx: &mut Context<AppRoot>,
    message: String,
    title: Option<String>,
    level: crate::control::NotifyLevel,
) -> ControlResponse {
    let message = message.trim().to_string();
    if message.is_empty() {
        return ControlResponse::error("notify requires a message");
    }
    match level {
        // `title` is meaningful only here: a titled toast is drawn in the error style, so an
        // `info` carrying one would read as a failure. Info stays the single-line form.
        crate::control::NotifyLevel::Error => {
            crate::pane::pty_events::notify_error(
                ctx,
                title.unwrap_or_else(|| "Error".to_string()),
                message,
            );
        }
        crate::control::NotifyLevel::Info => {
            crate::pane::pty_events::notify_info(ctx, message);
        }
    }
    ControlResponse::empty()
}

fn switch_workspace_command(ctx: &mut Context<AppRoot>, index: usize) -> ControlResponse {
    if ctx.state.scratch_visible {
        return ControlResponse::error("scratchpad is open");
    }
    let Some(response) = validate_workspace_index(index) else {
        switch_workspace(&mut ctx.state, index - 1);
        request_current_pane_focus(ctx);
        return ControlResponse::empty();
    };
    response
}

fn move_to_workspace_command(ctx: &mut Context<AppRoot>, index: usize) -> ControlResponse {
    if ctx.state.scratch_visible {
        return ControlResponse::error("scratchpad is open");
    }
    if !ctx.state.is_controller() {
        return ControlResponse::error_with(ControlErrorCode::NotController, "not controller");
    }
    let Some(response) = validate_workspace_index(index) else {
        move_focused_to_workspace(&mut ctx.state, index - 1);
        request_current_pane_focus(ctx);
        return ControlResponse::empty();
    };
    response
}

/// `Some(error response)` when `index` (1-based) is out of the `1..=WORKSPACE_COUNT` range,
/// `None` when it is valid.
fn validate_workspace_index(index: usize) -> Option<ControlResponse> {
    crate::pane::spawn_policy::workspace_index(index)
        .err()
        .map(ControlResponse::error)
}

struct PreparedNewPane {
    workspace: Option<usize>,
    launch: Option<crate::pane::launch::PaneLaunch>,
    scratch_source: bool,
}

fn prepare_new_pane(
    state: &crate::state::State,
    source: Option<PaneId>,
    command: Option<String>,
    argv: Option<Vec<String>>,
    workspace: Option<usize>,
) -> std::result::Result<PreparedNewPane, ControlResponse> {
    let workspace = match workspace {
        Some(index) => Some(
            crate::pane::spawn_policy::workspace_index(index).map_err(ControlResponse::error)?,
        ),
        None => None,
    };
    let launch = crate::pane::spawn_policy::requested_launch(command, argv)
        .map_err(ControlResponse::error)?;
    let scratch_source = source.is_some_and(|id| crate::scratchpad::contains(state, id));
    if state.scratch_visible && source.is_some() && !scratch_source {
        return Err(ControlResponse::error(
            "source pane is hidden behind scratchpad",
        ));
    }
    if !scratch_source && !state.is_controller() {
        return Err(ControlResponse::error_with(
            ControlErrorCode::NotController,
            "not controller",
        ));
    }
    Ok(PreparedNewPane {
        workspace,
        launch,
        scratch_source,
    })
}

#[allow(clippy::too_many_arguments)]
fn new_pane(
    ctx: &mut Context<AppRoot>,
    source: Option<PaneId>,
    command: Option<String>,
    argv: Option<Vec<String>>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
    workspace: Option<usize>,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) -> Update {
    let PreparedNewPane {
        workspace,
        launch,
        scratch_source,
    } = match prepare_new_pane(&ctx.state, source, command, argv, workspace) {
        Ok(prepared) => prepared,
        Err(response) => {
            let _ = reply.send(response);
            return Update::full();
        }
    };
    if let Some(update) = crate::ops::session::ensure_session_for_pty(
        ctx,
        crate::state::PendingSessionAction::NewPane {
            source,
            launch: launch.clone(),
            cwd: cwd.clone(),
            title: title.clone(),
            keep_open,
            focus,
            workspace,
        },
    ) {
        ctx.state.pending_control_reply = Some(reply);
        return update;
    }
    if scratch_source || (ctx.state.scratch_visible && source.is_none()) {
        let mut identity = PaneIdentity {
            launch,
            cwd,
            keep_open,
            ..PaneIdentity::default()
        };
        if let Some(title) = title {
            identity.set_custom_title(title);
        }
        let previous = source.or(ctx.state.scratch.focused_pane);
        let (id, update) = crate::pane::lifecycle::spawn_pane_in_scratch(ctx, previous, identity);
        if !focus && let Some(previous) = previous {
            crate::ops::focus::focus_pane(&mut ctx.state, previous);
        }
        hold_spawn_reply(ctx, id, reply);
        return update;
    }
    let source_workspace = match workspace_for_source(&ctx.state, source) {
        Ok(index) => index,
        Err(message) => {
            let _ = reply.send(ControlResponse::error(message));
            return Update::full();
        }
    };
    let (id, update) = spawn_new_pane(
        ctx,
        source_workspace,
        source,
        launch,
        cwd,
        title,
        keep_open,
        focus,
        workspace,
    );
    hold_spawn_reply(ctx, id, reply);
    update
}

/// How long a held `new-pane` reply waits for the PTY before answering `pty_ready:false` anyway.
/// The control connection gives up at 10s (see [`crate::control::handle_connection`]), so this has
/// to leave room for the reply to travel back; a remote spawn over a slow SSH link is the case that
/// actually uses the budget.
const SPAWN_READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Park a `new-pane` reply until the pane's PTY reports ready, so the answer describes readiness
/// rather than acceptance. Falls back to answering immediately when there is no command link to arm
/// the deadline with — without one nothing would ever release the reply.
pub(crate) fn hold_spawn_reply(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    reply: std::sync::mpsc::Sender<ControlResponse>,
) {
    let local = crate::pane::lifecycle::pane_is_local(&ctx.state, id);
    let generation = crate::pane::lifecycle::find_pane_in_namespace(&ctx.state, id, local)
        .map(|pane| pane.pty_generation);
    let (Some(generation), Some(link)) = (generation, ctx.state.command_link.clone()) else {
        let _ = reply.send(ControlResponse::ok(NewPaneAccepted {
            id,
            accepted: true,
            pty_ready: false,
        }));
        return;
    };
    let epoch = ctx.state.runtime_epoch;
    ctx.state
        .pending_spawn_replies
        .insert((epoch, local, id, generation), reply);
    link.send_after(
        SPAWN_READY_DEADLINE,
        crate::Msg::SpawnReplyDeadline {
            epoch,
            pane_id: id,
            local,
            generation,
        },
    );
}

/// Answer a held `new-pane` reply. `ready` is the pane's real PTY state; `error` replaces the
/// success payload when the spawn failed outright.
pub(crate) fn resolve_spawn_reply(
    state: &mut crate::state::State,
    epoch: u64,
    pane_id: PaneId,
    local: bool,
    generation: u64,
    ready: bool,
    error: Option<&str>,
) {
    let Some(reply) = state
        .pending_spawn_replies
        .remove(&(epoch, local, pane_id, generation))
    else {
        return;
    };
    let _ = reply.send(match error {
        Some(message) => ControlResponse::error(message),
        None => ControlResponse::ok(NewPaneAccepted {
            id: pane_id,
            accepted: true,
            pty_ready: ready,
        }),
    });
}

/// Spawn a pane once a session client is available (shared by the live control path and the
/// deferred launcher replay).
///
/// The arguments are one control command's fields, kept flat to match `spawn_new_pane` below and
/// the `PendingSessionAction::NewPane` variant they arrive in.
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_pane_after_session(
    ctx: &mut Context<AppRoot>,
    source: Option<PaneId>,
    launch: Option<crate::pane::launch::PaneLaunch>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
    workspace: Option<usize>,
) -> (PaneId, Update) {
    if !ctx.state.is_controller() {
        return (0, Update::full());
    }
    let Ok(source_workspace) = workspace_for_source(&ctx.state, source) else {
        return (0, Update::full());
    };
    spawn_new_pane(
        ctx,
        source_workspace,
        source,
        launch,
        cwd,
        title,
        keep_open,
        focus,
        workspace,
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_new_pane(
    ctx: &mut Context<AppRoot>,
    source_workspace: usize,
    source: Option<PaneId>,
    launch: Option<crate::pane::launch::PaneLaunch>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
    workspace: Option<usize>,
) -> (PaneId, Update) {
    let mut identity = PaneIdentity {
        launch,
        cwd,
        keep_open,
        ..PaneIdentity::default()
    };
    if let Some(title) = title {
        identity.set_custom_title(title);
    }
    spawn_interactive_pane_with_focus(
        ctx,
        source_workspace,
        source,
        identity,
        Some(focus),
        workspace,
    )
}

fn workspace_for_source(
    state: &crate::state::State,
    source: Option<PaneId>,
) -> std::result::Result<usize, String> {
    match source {
        Some(id) => state
            .current()
            .workspaces
            .iter()
            .position(|ws| ws.panes.iter().any(|p| p.id == id && !p.closing))
            .ok_or_else(|| format!("source pane {id} not found")),
        None => Ok(state.current().active_workspace),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ControlCommand, ControlEnvelope, ControlRequest};
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::{Pane, State};
    use std::sync::mpsc;
    use tui_lipan::TestBackend;

    fn capture_ui_request(render: CaptureRender) -> (crate::Msg, mpsc::Receiver<ControlResponse>) {
        capture_ui_request_at(render, None)
    }

    fn capture_ui_request_at(
        render: CaptureRender,
        scale: Option<u8>,
    ) -> (crate::Msg, mpsc::Receiver<ControlResponse>) {
        capture_ui_request_with(render, scale, false)
    }

    fn capture_ui_request_with(
        render: CaptureRender,
        scale: Option<u8>,
        image_pixels: bool,
    ) -> (crate::Msg, mpsc::Receiver<ControlResponse>) {
        let (reply, response) = mpsc::channel();
        let message = crate::Msg::ControlRequest(ControlEnvelope {
            request: ControlRequest {
                command: ControlCommand::CaptureUi {
                    render,
                    scale,
                    image_pixels,
                },
                source_pane: None,
                extension: None,
            },
            reply,
        });
        (message, response)
    }

    fn capture_ui_reply(
        response: &mpsc::Receiver<ControlResponse>,
        render: CaptureRender,
    ) -> UiCapture {
        // Windows may discover and load a system font on the PNG encoder thread while the
        // parallel test suite is busy. Keep the bound, with room for that first render.
        let timeout = if cfg!(windows) { 30 } else { 10 };
        let response = response
            .recv_timeout(std::time::Duration::from_secs(timeout))
            .unwrap_or_else(|error| panic!("capture-ui {render:?} should be answered: {error}"));
        assert!(response.ok, "capture-ui failed: {:?}", response.error);
        serde_json::from_value(response.data.expect("capture-ui carries data")).unwrap()
    }

    #[test]
    fn capture_ui_answers_every_request_from_the_next_painted_frame() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.render();
                let drawn = backend.capture_frame();

                // Both arrive before the next paint, and nothing else asks for one.
                let (text_request, text_reply) = capture_ui_request(CaptureRender::Text);
                let (png_request, png_reply) = capture_ui_request(CaptureRender::Png);
                backend.enqueue(text_request);
                backend.enqueue(png_request);
                backend.pump().unwrap();

                let text = capture_ui_reply(&text_reply, CaptureRender::Text);
                assert_eq!((text.width, text.height), (drawn.width, drawn.height));
                assert_eq!(
                    text.content,
                    crate::control::CaptureContent::Text {
                        text: drawn.plain_text()
                    }
                );

                let png = capture_ui_reply(&png_reply, CaptureRender::Png);
                let crate::control::CaptureContent::Png { png_base64 } = png.content else {
                    panic!("expected a png capture, got {:?}", png.content);
                };
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(png_base64)
                    .unwrap();
                assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn capture_ui_encodes_each_scale_in_a_batch_and_refuses_a_bad_one_at_once() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.render();

                // Refused before any paint, so nothing is left waiting.
                let (request, reply) = capture_ui_request_at(CaptureRender::Text, Some(2));
                backend.update_level(request).unwrap();
                let refused = reply.try_recv().expect("answered at once");
                assert_eq!(refused.code, Some(ControlErrorCode::InvalidArgument));
                assert!(backend.state().pending_ui_capture.is_none());

                let (one, one_reply) = capture_ui_request_at(CaptureRender::Png, None);
                let (two, two_reply) = capture_ui_request_at(CaptureRender::Png, Some(2));
                backend.update_level(one).unwrap();
                backend.update_level(two).unwrap();
                backend.pump().unwrap();

                let size = |capture: UiCapture| {
                    let crate::control::CaptureContent::Png { png_base64 } = capture.content else {
                        panic!("expected a png");
                    };
                    use base64::Engine as _;
                    let png = base64::engine::general_purpose::STANDARD
                        .decode(png_base64)
                        .unwrap();
                    let field = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
                    (field(16), field(20))
                };
                let (w1, h1) = size(capture_ui_reply(&one_reply, CaptureRender::Png));
                let (w2, h2) = size(capture_ui_reply(&two_reply, CaptureRender::Png));
                assert_eq!((w2, h2), (w1 * 2, h1 * 2), "each scale gets its own encode");
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn capture_ui_answers_spans_of_the_painted_frame_and_refuses_pixels_elsewhere() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.render();
                let drawn = backend.capture_frame();

                let (request, reply) = capture_ui_request_with(CaptureRender::Png, None, true);
                backend.update_level(request).unwrap();
                let refused = reply.try_recv().expect("answered at once");
                assert_eq!(refused.code, Some(ControlErrorCode::InvalidArgument));
                assert!(backend.state().pending_ui_capture.is_none());

                let (spans, spans_reply) = capture_ui_request(CaptureRender::Spans);
                let (pixels, pixels_reply) =
                    capture_ui_request_with(CaptureRender::Spans, None, true);
                backend.update_level(spans).unwrap();
                backend.update_level(pixels).unwrap();
                backend.pump().unwrap();

                let capture = capture_ui_reply(&spans_reply, CaptureRender::Spans);
                let crate::control::CaptureContent::Spans { frame } = capture.content else {
                    panic!("expected spans, got {:?}", capture.content);
                };
                assert_eq!((frame.width, frame.height), (drawn.width, drawn.height));
                let text: Vec<String> = frame
                    .rows
                    .iter()
                    .map(|row| {
                        let covered: u16 = row.iter().map(|run| run.width).sum();
                        let text: String = row.iter().map(|run| run.text.as_str()).collect();
                        text + &" ".repeat(usize::from(frame.width - covered))
                    })
                    .collect();
                assert_eq!(
                    text,
                    drawn.to_fixed_grid_lines(),
                    "the runs read as the painted frame"
                );
                assert!(
                    frame.rows.iter().flatten().any(|run| run.fg.is_some()),
                    "the UI's resolved colors are kept"
                );
                // No image is drawn, so asking for pixels answers with the same frame.
                let with_pixels = capture_ui_reply(&pixels_reply, CaptureRender::Spans);
                let crate::control::CaptureContent::Spans { frame: again } = with_pixels.content
                else {
                    panic!("expected spans");
                };
                assert_eq!(again, frame);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn capture_ui_requests_before_one_paint_share_its_encodes() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.render();

                // Handled without a paint in between, as requests arriving together would be.
                let mut replies = Vec::new();
                for render in [CaptureRender::Png, CaptureRender::Png, CaptureRender::Text] {
                    let (request, reply) = capture_ui_request(render);
                    backend.update_level(request).unwrap();
                    replies.push(reply);
                }
                let batch = backend.state().pending_ui_capture.clone().expect("a batch");
                assert_eq!(
                    batch.borrow().waiters.len(),
                    3,
                    "one batch, and so one snapshot callback, for all three"
                );

                backend.pump().unwrap();
                let captures = [
                    capture_ui_reply(&replies[0], CaptureRender::Png),
                    capture_ui_reply(&replies[1], CaptureRender::Png),
                    capture_ui_reply(&replies[2], CaptureRender::Text),
                ];
                assert_eq!(
                    captures[0], captures[1],
                    "both PNG waiters get the one encode"
                );
                assert!(matches!(
                    captures[2].content,
                    crate::control::CaptureContent::Text { .. }
                ));
                assert!(batch.borrow().served && batch.borrow().waiters.is_empty());

                // A request after the paint waits for a frame of its own.
                let (request, reply) = capture_ui_request(CaptureRender::Text);
                backend.dispatch(request).unwrap();
                capture_ui_reply(&reply, CaptureRender::Text);
                let next = backend.state().pending_ui_capture.clone().expect("a batch");
                assert!(!std::rc::Rc::ptr_eq(&batch, &next));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn stale_extension_generation_is_rejected_at_execution_time() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::Notify {
                                message: "stale".to_string(),
                                title: None,
                                level: crate::control::NotifyLevel::Info,
                            },
                            source_pane: None,
                            extension: Some(crate::config::ExtensionProvenance {
                                id: "tools".to_string(),
                                generation: "retired".to_string(),
                            }),
                        },
                        reply,
                    }))
                    .unwrap();

                let response = response.recv().unwrap();
                assert!(!response.ok);
                assert_eq!(
                    response.error.as_deref(),
                    Some("extension generation is not active")
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn unavailable_extension_commands_get_extension_specific_guidance() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::RunAction {
                                action: "git-tools.branches".to_string(),
                            },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .unwrap();

                let response = response.recv().unwrap();
                assert_eq!(
                    response.error.as_deref(),
                    Some(
                        "extension command `git-tools.branches` is unavailable; run `rozi extensions list --verbose`"
                    )
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn rect() -> FloatRect {
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        }
    }

    #[test]
    fn workspace_for_source_errors_on_invalid_explicit_source() {
        let state = State::new(crate::config::Config::default(), Theme::default());
        assert_eq!(
            workspace_for_source(&state, Some(999)),
            Err("source pane 999 not found".to_string())
        );
    }

    #[test]
    fn workspace_for_source_falls_back_only_without_source() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().active_workspace = 2;
        state.current_mut().workspaces[1]
            .panes
            .push(Pane::new(7, 100, rect()));
        assert_eq!(workspace_for_source(&state, None), Ok(2));
        assert_eq!(workspace_for_source(&state, Some(7)), Ok(1));
    }

    #[test]
    fn validate_workspace_index_rejects_out_of_range() {
        assert!(validate_workspace_index(0).is_some());
        assert!(validate_workspace_index(1).is_none());
        assert!(validate_workspace_index(crate::state::WORKSPACE_COUNT).is_none());
        assert!(validate_workspace_index(crate::state::WORKSPACE_COUNT + 1).is_some());
    }

    #[test]
    fn runtime_action_ids_include_named_commands() {
        let mut config = crate::config::Config::default();
        config.commands.push(crate::config::NamedCommand {
            default_key: None,
            id: "branches".to_string(),
            label: None,
            action: crate::config::UserCommandAction::Send("git branch\n".to_string()),
            category: "Custom".to_string(),
            env: Vec::new(),
        });
        let state = State::new(config, Theme::default());
        assert_eq!(
            action_from_runtime_id(&state, "branches"),
            Some(Action::RunNamedCommand(0))
        );
        assert_eq!(action_from_runtime_id(&state, "not-there"), None);
    }

    #[test]
    fn writable_follower_can_queue_status_and_read_only_client_cannot() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (client, outbound) = SessionClient::test_channel();
                {
                    let state = backend.state_mut();
                    state.current_mut().session_attached = true;
                    state.current_mut().session_client = Some(client);
                    state.current_mut().workspaces[0].panes[0].pty_generation = 9;
                    let mut shared = crate::state::SharedSessionState::new(1);
                    shared.controller = Some(2);
                    state.current_mut().shared = Some(shared);
                }
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SetStatus {
                                target: Some(1),
                                status: Some("blocked".into()),
                                reason: Some("waiting".into()),
                            },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("dispatch writable status request");
                assert!(response.recv().unwrap().ok);
                assert!(outbound.try_iter().any(|message| matches!(
                        message,
                        ClientOutbound::Control(ClientMessage::SetPaneStatus {
                            pane_id: 1,
                local: false,
                            generation: 9,
                            status: Some(status),
                            reason: Some(reason),
                        }) if status == "blocked" && reason == "waiting"
                    )));

                backend
                    .state_mut()
                    .current_mut()
                    .shared
                    .as_mut()
                    .unwrap()
                    .read_only = true;
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SetStatus {
                                target: Some(1),
                                status: None,
                                reason: None,
                            },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("dispatch read-only status request");
                let response = response.recv().unwrap();
                assert!(!response.ok);
                assert_eq!(response.error.as_deref(), Some("attached read-only"));
                assert!(outbound.try_recv().is_err());
            })
            .expect("spawn control status test thread")
            .join()
            .expect("control status test thread completes");
    }

    /// A backend whose mount has delivered the command link, which `hold_spawn_reply` needs to arm
    /// its deadline; without one it answers immediately and the held-reply behavior never runs.
    fn settled_backend() -> TestBackend<crate::AppRoot> {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the mount never delivered the command link"
            );
            backend.pump().expect("settle the mount");
            std::thread::yield_now();
        }
        backend
    }

    fn attach_test_session(
        backend: &mut TestBackend<crate::AppRoot>,
    ) -> mpsc::Receiver<ClientOutbound> {
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        outbound
    }

    fn new_pane_request(
        focus: bool,
    ) -> (
        ControlEnvelope,
        mpsc::Receiver<crate::control::ControlResponse>,
    ) {
        let (reply, response) = mpsc::channel();
        (
            ControlEnvelope {
                request: ControlRequest {
                    command: ControlCommand::NewPane {
                        command: None,
                        argv: None,
                        cwd: None,
                        title: None,
                        keep_open: false,
                        focus,
                        workspace: None,
                    },
                    source_pane: None,
                    extension: None,
                },
                reply,
            },
            response,
        )
    }

    #[test]
    fn new_pane_leaves_focus_put_and_answers_only_once_the_pty_is_ready() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                let focused_before = backend.state().current().focused_pane;

                let (envelope, response) = new_pane_request(false);
                backend
                    .dispatch(crate::Msg::ControlRequest(envelope))
                    .expect("dispatch new-pane");

                // Acceptance alone must not answer: the caller is told about readiness, not about
                // the request having been received.
                assert!(
                    response.try_recv().is_err(),
                    "reply was sent before the PTY reported ready"
                );
                assert_eq!(
                    backend.state().current().focused_pane,
                    focused_before,
                    "an automation spawn moved focus"
                );

                let spawned = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .map(|pane| (pane.id, pane.pty_generation))
                    .max_by_key(|(id, _)| *id)
                    .expect("the pane was created");
                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: spawned.0,
                        local: false,
                        generation: spawned.1,
                        pid: Some(4242),
                        ok: true,
                        error: None,
                    })
                    .expect("dispatch spawn result");

                let response = response.try_recv().expect("reply released by spawn result");
                assert!(response.ok);
                let data = response.data.unwrap();
                assert_eq!(data["id"], spawned.0);
                assert_eq!(data["pty_ready"], true);
                assert_eq!(
                    backend.state().current().focused_pane,
                    focused_before,
                    "focus moved when the spawn completed"
                );
            })
            .expect("spawn new-pane readiness test thread")
            .join()
            .expect("new-pane readiness test thread completes");
    }

    #[test]
    fn new_pane_with_focus_moves_focus_to_the_new_pane() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                let focused_before = backend.state().current().focused_pane;

                let (envelope, _response) = new_pane_request(true);
                backend
                    .dispatch(crate::Msg::ControlRequest(envelope))
                    .expect("dispatch new-pane --focus");

                let focused_after = backend.state().current().focused_pane;
                assert_ne!(focused_after, focused_before);
                assert!(focused_after.is_some());
            })
            .expect("spawn new-pane focus test thread")
            .join()
            .expect("new-pane focus test thread completes");
    }

    /// A blank toast would occupy the slot a real message needs, so it is refused rather than
    /// drawn empty.
    #[test]
    fn notify_refuses_an_empty_message() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                for blank in ["", "   "] {
                    let (tx, rx) = std::sync::mpsc::channel();
                    backend
                        .dispatch(crate::Msg::ControlRequest(
                            crate::control::ControlEnvelope {
                                request: crate::control::ControlRequest {
                                    command: crate::control::ControlCommand::Notify {
                                        message: blank.to_string(),
                                        title: None,
                                        level: crate::control::NotifyLevel::Info,
                                    },
                                    source_pane: None,
                                    extension: None,
                                },
                                reply: tx,
                            },
                        ))
                        .expect("dispatch notify");
                    let response = rx.try_recv().expect("answered");
                    assert!(!response.ok, "an empty message was accepted");
                    assert_eq!(response.error.as_deref(), Some("notify requires a message"));
                }

                let (tx, rx) = std::sync::mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(
                        crate::control::ControlEnvelope {
                            request: crate::control::ControlRequest {
                                command: crate::control::ControlCommand::Notify {
                                    message: "deploy finished".into(),
                                    title: None,
                                    level: crate::control::NotifyLevel::Info,
                                },
                                source_pane: None,
                                extension: None,
                            },
                            reply: tx,
                        },
                    ))
                    .expect("dispatch notify");
                assert!(rx.try_recv().expect("answered").ok);
            })
            .expect("spawn notify test thread")
            .join()
            .expect("notify test thread completes");
    }

    #[test]
    fn a_failed_spawn_answers_the_held_new_pane_reply_with_an_error() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);

                let (envelope, response) = new_pane_request(false);
                backend
                    .dispatch(crate::Msg::ControlRequest(envelope))
                    .expect("dispatch new-pane");
                let spawned = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .map(|pane| (pane.id, pane.pty_generation))
                    .max_by_key(|(id, _)| *id)
                    .expect("the pane was created");
                let epoch = backend.state().runtime_epoch;

                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: spawned.0,
                        local: false,
                        generation: spawned.1,
                        pid: None,
                        ok: false,
                        error: Some("no such file".into()),
                    })
                    .expect("dispatch failed spawn result");

                let response = response.try_recv().expect("reply released by spawn result");
                assert!(!response.ok);
                assert_eq!(response.error.as_deref(), Some("no such file"));
            })
            .expect("spawn failed-spawn test thread")
            .join()
            .expect("failed-spawn test thread completes");
    }

    #[test]
    fn input_for_a_starting_pane_is_queued_and_flushed_once_the_pty_is_ready() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                let outbound = attach_test_session(&mut backend);
                {
                    let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                    pane.pty_generation = 4;
                    pane.terminal.status = ManagedTerminalStatus::Starting;
                }

                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SendText {
                                target: Some(1),
                                text: "cargo test\n".into(),
                                wait: None,
                                capture: None,
                                scale: None,
                            },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("dispatch send-text at a starting pane");
                assert!(response.recv().unwrap().ok);
                assert!(
                    outbound.try_recv().is_err(),
                    "input reached the PTY before it was ready"
                );

                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: 1,
                        local: false,
                        generation: 4,
                        pid: Some(11),
                        ok: true,
                        error: None,
                    })
                    .expect("dispatch spawn result");

                assert!(outbound.try_iter().any(|message| matches!(
                    message,
                    ClientOutbound::PaneInput {
                        pane_id: 1,
                        generation: 4,
                        ref bytes,
                        ..
                    } if bytes == b"cargo test\n"
                )));
            })
            .expect("spawn queued-input test thread")
            .join()
            .expect("queued-input test thread completes");
    }

    #[test]
    fn input_for_a_pane_that_is_not_running_still_fails() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                backend.state_mut().current_mut().workspaces[0].panes[0]
                    .terminal
                    .status = ManagedTerminalStatus::Exited(0);

                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::SendText {
                                target: Some(1),
                                text: "hi".into(),
                                wait: None,
                                capture: None,
                                scale: None,
                            },
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("dispatch send-text at an exited pane");
                let response = response.recv().unwrap();
                assert!(!response.ok);
                assert_eq!(response.error.as_deref(), Some("pane 1 PTY is not running"));
            })
            .expect("spawn exited-pane input test thread")
            .join()
            .expect("exited-pane input test thread completes");
    }

    #[test]
    fn list_panes_keeps_terminal_status_and_adds_reported_status_fields() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.state_mut().current_mut().session_name = Some("dev".into());
                let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                pane.terminal.title = Some("build".into());
                pane.terminal.foreground_program = Some("cargo".into());
                pane.terminal.foreground_programs = vec!["cargo".into(), "rustc".into()];
                pane.terminal.foreground_arguments = vec!["test".into()];
                pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
                    value: "working".into(),
                    reason: Some("building".into()),
                    set_at: 1,
                });
                pane.terminal.status = ManagedTerminalStatus::Ready;
                let (reply, response) = mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::ListPanes,
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("dispatch list panes");
                let data = response.recv().unwrap().data.unwrap();
                assert!(data[0]["status"].is_string());
                assert_ne!(data[0]["status"], "working");
                assert_eq!(data[0]["reported_status"], "working");
                assert_eq!(data[0]["status_reason"], "building");
                assert_eq!(data[0]["session"], "dev");
                assert_eq!(data[0]["title"], "build");
                assert_eq!(data[0]["foreground_program"], "cargo");
                assert_eq!(data[0]["foreground_programs"][1], "rustc");
                assert_eq!(data[0]["foreground_arguments"][0], "test");
            })
            .expect("spawn list panes test thread")
            .join()
            .expect("list panes test thread completes");
    }

    #[test]
    fn layout_get_reports_the_shared_arrangement_and_where_this_ui_draws_it() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                backend.state_mut().current_mut().session_name = Some("dev".into());
                let ask = |backend: &mut TestBackend<crate::AppRoot>, workspace| {
                    let (reply, response) = mpsc::channel();
                    backend
                        .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                            request: ControlRequest {
                                command: ControlCommand::LayoutGet { workspace },
                                source_pane: None,
                                extension: None,
                            },
                            reply,
                        }))
                        .expect("dispatch layout get");
                    let response = response.recv().unwrap();
                    assert!(response.ok, "{:?}", response.error);
                    serde_json::from_value::<crate::control::LayoutReport>(
                        response.data.expect("layout data"),
                    )
                    .expect("a LayoutReport")
                };

                let report = ask(&mut backend, None);
                assert_eq!(report.session, "dev");
                assert_eq!(
                    report.workspaces.len(),
                    crate::state::WORKSPACE_COUNT,
                    "empty workspaces are still reported, with their layouts"
                );
                let client = report.client.expect("a UI reports its own view");
                assert_eq!(client.active_workspace, 1);
                // No shared session: this UI owns its layout outright and has no server to lag.
                assert!(client.controller);
                assert!(client.committed);
                assert_eq!(report.revision, None);
                let canvas = report.canvas.expect("a UI always has a canvas");
                let [pane] = report.workspaces[0].panes.as_slice() else {
                    panic!("expected the seeded pane");
                };
                assert_eq!(client.focused_pane, Some(pane.id));
                assert_eq!(pane.order, Some(0));
                assert_eq!(
                    (pane.rect.width, pane.rect.height),
                    (u32::from(canvas.cols), u32::from(canvas.rows)),
                    "a lone tiled pane covers the canonical canvas"
                );
                let view = pane.view_rect.expect("the active workspace is on screen");
                assert!(view.width > 0 && view.width <= u32::from(client.viewport.cols));
                assert!(view.height > 0 && view.height <= u32::from(client.viewport.rows));

                let narrowed = ask(&mut backend, Some(2));
                assert_eq!(narrowed.workspaces.len(), 1);
                assert_eq!(narrowed.workspaces[0].index, 2);
                assert!(narrowed.workspaces[0].panes.is_empty());

                // A follower describes the revision it applied and never has changes of its own.
                let mut shared = crate::state::SharedSessionState::new(1);
                shared.controller = Some(2);
                shared.layout_rev = 3;
                shared.assumed_rev = 3;
                backend.state_mut().current_mut().shared = Some(shared);
                let follower = ask(&mut backend, None);
                assert_eq!(follower.revision, Some(3));
                let client = follower.client.expect("client view");
                assert!(!client.controller);
                assert!(client.committed);

                // A controller whose commit the server has not echoed yet is ahead of `revision`.
                let shared = backend.state_mut().current_mut().shared.as_mut().unwrap();
                shared.controller = Some(1);
                shared.assumed_rev = 4;
                let pending = ask(&mut backend, None).client.expect("client view");
                assert!(pending.controller);
                assert!(!pending.committed);
            })
            .expect("spawn layout get test thread")
            .join()
            .expect("layout get test thread completes");
    }

    fn dispatch(
        backend: &mut TestBackend<crate::AppRoot>,
        command: ControlCommand,
    ) -> ControlResponse {
        let (reply, response) = mpsc::channel();
        backend
            .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                request: ControlRequest {
                    command,
                    source_pane: None,
                    extension: None,
                },
                reply,
            }))
            .expect("dispatch control request");
        response.recv().unwrap()
    }

    fn pane_set(
        target: PaneId,
        floating: Option<bool>,
        fullscreen: Option<bool>,
        rect: Option<crate::control::CellRect>,
    ) -> ControlCommand {
        ControlCommand::PaneSet {
            target,
            floating,
            fullscreen,
            rect,
            rect_fraction: None,
            split_ratio: None,
            width_ratio: None,
            if_revision: None,
        }
    }

    /// A UI with three tiled panes in workspace 1, and the canvas its shared rects are measured on.
    fn three_tiled_panes() -> (TestBackend<crate::AppRoot>, (u16, u16)) {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        {
            let workspace = &mut backend.state_mut().current_mut().workspaces[0];
            for id in [2, 3] {
                workspace
                    .panes
                    .push(crate::state::Pane::new(id, 100, FloatRect::default()));
                crate::layout::tiling::append_tiled_window(workspace, id);
            }
            for pane in &mut workspace.panes {
                pane.pty_generation = 1;
            }
        }
        let report = dispatch(&mut backend, ControlCommand::LayoutGet { workspace: None });
        let canvas = serde_json::from_value::<crate::control::LayoutReport>(
            report.data.expect("layout data"),
        )
        .expect("a LayoutReport")
        .canvas
        .expect("canvas");
        (backend, (canvas.cols, canvas.rows))
    }

    /// Moves and swaps, like `pane set`, must commit the document a session server would write.
    #[test]
    fn a_ui_moves_and_swaps_panes_into_the_document_a_session_server_would_write() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let (mut backend, canvas) = three_tiled_panes();
                let focused_before = backend.state().current().focused_pane;
                type Edit = fn(&mut crate::layout::shared::SharedLayout) -> bool;
                let steps: [(ControlCommand, Edit); 4] = [
                    (
                        ControlCommand::PaneSwap {
                            target: 1,
                            with: 3,
                            if_revision: None,
                        },
                        |layout| layout.swap_panes(1, 3).expect("swap"),
                    ),
                    (
                        ControlCommand::PaneMove {
                            target: 2,
                            workspace: 4,
                            if_revision: None,
                        },
                        |layout| layout.move_pane(2, 3).expect("move"),
                    ),
                    (
                        ControlCommand::PaneMove {
                            target: 3,
                            workspace: 4,
                            if_revision: None,
                        },
                        |layout| layout.move_pane(3, 3).expect("move"),
                    ),
                    (
                        ControlCommand::PaneMove {
                            target: 3,
                            workspace: 4,
                            if_revision: None,
                        },
                        |layout| layout.move_pane(3, 3).expect("move"),
                    ),
                ];
                for (command, apply) in steps {
                    let mut expected =
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas);
                    let expected_change = apply(&mut expected);
                    let response = dispatch(&mut backend, command.clone());
                    assert!(response.ok, "{command:?}: {:?}", response.error);
                    let change: crate::control::LayoutChange =
                        serde_json::from_value(response.data.expect("change")).expect("change");
                    assert_eq!(change.changed, expected_change, "{command:?}");
                    assert_eq!(
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas),
                        expected,
                        "{command:?}"
                    );
                }
                let attachment = backend.state().current();
                assert_eq!(
                    attachment.active_workspace, 0,
                    "a move does not follow the pane"
                );
                if focused_before == Some(1) {
                    assert_eq!(attachment.focused_pane, Some(1), "unmoved focus stays put");
                }
                assert!(
                    attachment
                        .focused_pane
                        .is_none_or(|id| attachment.workspaces[0]
                            .panes
                            .iter()
                            .any(|pane| pane.id == id)),
                    "focus never points at a pane that left the workspace"
                );
            })
            .expect("spawn move test thread")
            .join()
            .expect("move test thread completes");
    }

    /// Absolute sizes land in the same document from a UI as from a session server, read back
    /// through `layout get`, and are refused for layouts that do not use them.
    #[test]
    fn a_ui_sets_ratios_into_the_document_a_session_server_would_write() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let (mut backend, canvas) = three_tiled_panes();
                let sized = |split_ratio, width_ratio| ControlCommand::PaneSet {
                    target: 3,
                    floating: None,
                    fullscreen: None,
                    rect: None,
                    rect_fraction: None,
                    split_ratio,
                    width_ratio,
                    if_revision: None,
                };
                let layout_set = |layout, master_ratio| ControlCommand::LayoutSet {
                    workspace: 1,
                    layout,
                    master_ratio,
                    if_revision: None,
                };
                type Edit = fn(&mut crate::layout::shared::SharedLayout) -> bool;
                let steps: [(ControlCommand, Edit); 3] = [
                    (sized(Some(0.7), None), |layout| {
                        let edit = crate::control::PaneEdit::validate(
                            None,
                            None,
                            None,
                            None,
                            Some(0.7),
                            None,
                        )
                        .unwrap();
                        layout.edit_pane(3, edit).unwrap()
                    }),
                    (
                        layout_set(Some(crate::control::ControlLayoutKind::Master), Some(0.65)),
                        |layout| {
                            layout.set_layout_kind(0, crate::state::LayoutKind::Master)
                                | layout.set_master_ratio(0, 0.65)
                        },
                    ),
                    (
                        layout_set(Some(crate::control::ControlLayoutKind::Scrollable), None),
                        |layout| layout.set_layout_kind(0, crate::state::LayoutKind::Scrollable),
                    ),
                ];
                for (command, apply) in steps {
                    let mut expected =
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas);
                    let expected_change = apply(&mut expected);
                    let response = dispatch(&mut backend, command.clone());
                    assert!(response.ok, "{command:?}: {:?}", response.error);
                    let change: crate::control::LayoutChange =
                        serde_json::from_value(response.data.expect("change")).expect("change");
                    assert_eq!(change.changed, expected_change, "{command:?}");
                    assert_eq!(
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas),
                        expected,
                        "{command:?}"
                    );
                }

                let response = dispatch(&mut backend, sized(None, Some(0.4)));
                let change: crate::control::LayoutChange =
                    serde_json::from_value(response.data.expect("change")).expect("change");
                let pane = change
                    .workspace
                    .panes
                    .iter()
                    .find(|pane| pane.id == 3)
                    .expect("pane 3");
                assert_eq!(pane.width_ratio, Some(0.4), "read back as it was set");
                assert_eq!(pane.split_ratio, None, "no split ratio outside Dwindle");
                assert_eq!(change.workspace.master_ratio, None);

                for (command, code) in [
                    (sized(Some(0.6), None), ControlErrorCode::Unsupported),
                    (layout_set(None, Some(0.6)), ControlErrorCode::Unsupported),
                    (sized(None, Some(0.9)), ControlErrorCode::InvalidArgument),
                    (
                        ControlCommand::PaneSet {
                            target: 3,
                            floating: Some(false),
                            fullscreen: None,
                            rect: None,
                            rect_fraction: None,
                            split_ratio: None,
                            width_ratio: Some(0.5),
                            if_revision: None,
                        },
                        ControlErrorCode::InvalidArgument,
                    ),
                ] {
                    let before =
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas);
                    let response = dispatch(&mut backend, command.clone());
                    assert_eq!(response.code, Some(code), "{command:?}");
                    assert_eq!(
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas),
                        before,
                        "a refused {command:?} changes nothing"
                    );
                }
            })
            .expect("spawn ratio test thread")
            .join()
            .expect("ratio test thread completes");
    }

    /// The one-fullscreen-pane rule holds for explicit targets, and the UI keeps it exactly as the
    /// document edit does.
    #[test]
    fn explicit_fullscreen_keeps_one_fullscreen_pane_per_workspace_on_both_endpoints() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let (mut backend, canvas) = three_tiled_panes();
                let fullscreen = |target| pane_set(target, None, Some(true), None);
                type Edit = fn(&mut crate::layout::shared::SharedLayout) -> bool;
                let steps: [(ControlCommand, Edit); 4] = [
                    (fullscreen(1), |layout| {
                        layout
                            .edit_pane(
                                1,
                                crate::control::PaneEdit::validate(
                                    None,
                                    Some(true),
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .unwrap(),
                            )
                            .unwrap()
                    }),
                    (fullscreen(2), |layout| {
                        layout
                            .edit_pane(
                                2,
                                crate::control::PaneEdit::validate(
                                    None,
                                    Some(true),
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .unwrap(),
                            )
                            .unwrap()
                    }),
                    (
                        ControlCommand::PaneMove {
                            target: 1,
                            workspace: 2,
                            if_revision: None,
                        },
                        |layout| layout.move_pane(1, 1).unwrap(),
                    ),
                    (
                        ControlCommand::PaneMove {
                            target: 2,
                            workspace: 2,
                            if_revision: None,
                        },
                        |layout| layout.move_pane(2, 1).unwrap(),
                    ),
                ];
                for (command, apply) in steps {
                    let mut expected =
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas);
                    apply(&mut expected);
                    let response = dispatch(&mut backend, command.clone());
                    assert!(response.ok, "{command:?}: {:?}", response.error);
                    assert_eq!(
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas),
                        expected,
                        "{command:?}"
                    );
                }
                let fullscreen: Vec<(usize, PaneId)> = backend
                    .state()
                    .current()
                    .workspaces
                    .iter()
                    .enumerate()
                    .flat_map(|(index, workspace)| {
                        workspace
                            .panes
                            .iter()
                            .filter(|pane| pane.fullscreen)
                            .map(move |pane| (index, pane.id))
                    })
                    .collect();
                assert_eq!(
                    fullscreen,
                    vec![(1, 2)],
                    "exactly one, and the arriving pane won"
                );
            })
            .expect("spawn fullscreen test thread")
            .join()
            .expect("fullscreen test thread completes");
    }

    /// A follower reports the document the server holds, not one rebuilt from its own workspaces:
    /// a sparse document must not grow the follower's local defaults.
    #[test]
    fn a_follower_reports_the_document_it_applied() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let (mut backend, canvas) = three_tiled_panes();
                let mut sparse =
                    crate::layout::shared::shared_layout_from_state(backend.state(), canvas);
                sparse.workspaces.retain(|workspace| workspace.index == 0);
                let mut follower = crate::state::SharedSessionState::new(1);
                follower.controller = Some(2);
                follower.layout_rev = 7;
                follower.assumed_rev = 7;
                follower.last_committed_layout = Some(sparse.clone());
                backend.state_mut().current_mut().shared = Some(follower);

                let response =
                    dispatch(&mut backend, ControlCommand::LayoutGet { workspace: None });
                let report: crate::control::LayoutReport =
                    serde_json::from_value(response.data.expect("layout data")).expect("report");
                let mut shared = report.workspaces.clone();
                for workspace in &mut shared {
                    for pane in &mut workspace.panes {
                        pane.view_rect = None;
                    }
                }
                assert_eq!(
                    shared,
                    crate::control::WorkspaceLayout::from_shared(
                        &sparse,
                        None,
                        backend.state().current().session_instance.as_ref(),
                    ),
                    "exactly the session's document"
                );
                assert_eq!(report.revision, Some(7));
                let shared = backend.state().current().shared.as_ref().unwrap();
                assert_eq!(
                    (shared.layout_rev, shared.assumed_rev),
                    (7, 7),
                    "reading commits nothing"
                );
            })
            .expect("spawn follower test thread")
            .join()
            .expect("follower test thread completes");
    }

    #[test]
    fn moving_the_focused_pane_leaves_focus_behind_and_close_needs_no_confirmation() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let (mut backend, _) = three_tiled_panes();
                let focused = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("a focused pane");
                let moved = dispatch(
                    &mut backend,
                    ControlCommand::PaneMove {
                        target: focused,
                        workspace: 5,
                        if_revision: None,
                    },
                );
                assert!(moved.ok, "{:?}", moved.error);
                let attachment = backend.state().current();
                assert_eq!(attachment.active_workspace, 0, "the view does not follow");
                let now = attachment.focused_pane.expect("focus fell back");
                assert_ne!(now, focused);
                assert!(
                    attachment.workspaces[0]
                        .panes
                        .iter()
                        .any(|pane| pane.id == now)
                );

                let closed = dispatch(
                    &mut backend,
                    ControlCommand::PaneClose {
                        target: now,
                        if_revision: None,
                    },
                );
                assert!(closed.ok, "{:?}", closed.error);
                let closed: crate::control::PaneClosed =
                    serde_json::from_value(closed.data.expect("closed")).expect("PaneClosed");
                assert_eq!(closed.id, now);
                let workspace = closed.workspace.expect("the workspace it left");
                assert!(workspace.panes.iter().all(|pane| pane.id != now));
                assert!(
                    backend.state().pending_destructive.is_none(),
                    "no second-press confirmation was armed"
                );

                let gone = dispatch(
                    &mut backend,
                    ControlCommand::PaneClose {
                        target: now,
                        if_revision: None,
                    },
                );
                assert_eq!(gone.code, Some(ControlErrorCode::PaneNotFound));
            })
            .expect("spawn close test thread")
            .join()
            .expect("close test thread completes");
    }

    /// A UI applies `pane set` to its live workspace and a session server applies it to the shared
    /// document. The document the UI then commits has to be the one the server would have written,
    /// or the same script reshapes a session differently depending on who is attached.
    #[test]
    fn a_ui_edits_panes_into_exactly_the_document_a_session_server_would_write() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let (mut backend, canvas) = three_tiled_panes();

                let rect = crate::control::CellRect {
                    x: 7,
                    y: 3,
                    width: 30,
                    height: 9,
                };
                for (target, floating, fullscreen, rect) in [
                    (2, Some(true), None, None),
                    (2, None, None, Some(rect)),
                    (3, Some(true), Some(true), None),
                    (2, Some(false), None, None),
                    (3, Some(false), Some(false), None),
                ] {
                    let mut expected =
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas);
                    let edit = crate::control::PaneEdit::validate(floating, fullscreen, rect, None, None, None)
                        .expect("valid edit");
                    let expected_change = expected.edit_pane(target, edit).expect("server edit");

                    let response = dispatch(
                        &mut backend,
                        pane_set(target, floating, fullscreen, rect),
                    );
                    assert!(response.ok, "{:?}", response.error);
                    let change: crate::control::LayoutChange =
                        serde_json::from_value(response.data.expect("change")).expect("change");
                    assert_eq!(change.changed, expected_change);
                    assert_eq!(
                        crate::layout::shared::shared_layout_from_state(backend.state(), canvas),
                        expected,
                        "pane {target}: floating {floating:?}, fullscreen {fullscreen:?}, rect {rect:?}"
                    );
                }

                // The same request twice is a no-op the second time.
                let again = dispatch(&mut backend, pane_set(3, Some(false), Some(false), None));
                let again: crate::control::LayoutChange =
                    serde_json::from_value(again.data.expect("change")).expect("change");
                assert!(!again.changed);

                let set = dispatch(
                    &mut backend,
                    ControlCommand::LayoutSet {
                        workspace: 1,
                        layout: Some(crate::control::ControlLayoutKind::Grid),
                        master_ratio: None,
                        if_revision: None,
                    },
                );
                let set: crate::control::LayoutChange =
                    serde_json::from_value(set.data.expect("change")).expect("change");
                assert!(set.changed);
                assert_eq!(set.workspace.layout, crate::control::ControlLayoutKind::Grid);
                assert!(
                    set.workspace.panes.iter().all(|pane| pane.view_rect.is_some()),
                    "the shown workspace reports where it is drawn"
                );
            })
            .expect("spawn pane set test thread")
            .join()
            .expect("pane set test thread completes");
    }

    #[test]
    fn a_ui_refuses_layout_writes_it_may_not_make_before_changing_anything() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let code = |response: ControlResponse| {
                    assert!(!response.ok);
                    response.code
                };

                assert_eq!(
                    code(dispatch(&mut backend, pane_set(99, None, Some(true), None))),
                    Some(ControlErrorCode::PaneNotFound)
                );
                assert_eq!(
                    code(dispatch(
                        &mut backend,
                        pane_set(
                            1,
                            None,
                            None,
                            Some(crate::control::CellRect {
                                x: 0,
                                y: 0,
                                width: 10,
                                height: 5,
                            })
                        )
                    )),
                    Some(ControlErrorCode::InvalidArgument),
                    "a rect on a pane that stays tiled"
                );
                assert_eq!(
                    code(dispatch(
                        &mut backend,
                        ControlCommand::LayoutSet {
                            workspace: 1,
                            layout: Some(crate::control::ControlLayoutKind::Rows),
                            master_ratio: None,
                            if_revision: Some(4),
                        }
                    )),
                    Some(ControlErrorCode::Conflict),
                    "a UI without a shared session has no revision to match"
                );

                let scratch = 1 << 31;
                backend
                    .state_mut()
                    .scratch
                    .panes
                    .push(crate::state::Pane::new(scratch, 100, FloatRect::default()));
                assert_eq!(
                    code(dispatch(
                        &mut backend,
                        pane_set(scratch, None, Some(true), None)
                    )),
                    Some(ControlErrorCode::Unsupported)
                );

                let mut follower = crate::state::SharedSessionState::new(1);
                follower.controller = Some(2);
                backend.state_mut().current_mut().shared = Some(follower);
                assert_eq!(
                    code(dispatch(&mut backend, pane_set(1, Some(true), None, None))),
                    Some(ControlErrorCode::NotController)
                );
                let pane = &backend.state().current().workspaces[0].panes[0];
                assert!(!pane.floating, "a refused write changes nothing");
            })
            .expect("spawn refusal test thread")
            .join()
            .expect("refusal test thread completes");
    }

    #[test]
    fn metrics_control_is_render_neutral_and_returns_cached_shape_without_waiting() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(crate::AppRoot::default());
                let (reply, response) = mpsc::channel();
                let level = backend
                    .update_level(crate::Msg::ControlRequest(ControlEnvelope {
                        request: ControlRequest {
                            command: ControlCommand::Metrics,
                            source_pane: None,
                            extension: None,
                        },
                        reply,
                    }))
                    .expect("update metrics");
                assert_eq!(level, tui_lipan::UpdateLevel::None);
                let response = response
                    .recv_timeout(std::time::Duration::from_millis(100))
                    .expect("metrics response is immediate");
                let data = response.data.expect("metrics data");
                assert!(response.ok);
                assert!(data["sampled_at_unix_ms"].is_number());
                assert!(data["client_inbound"].is_null());
                assert!(data["client_outbound"].is_null());
                assert!(data["piped_remote"].is_null());
                assert_eq!(
                    data["orphan_output"]["capacity_bytes"],
                    crate::state::ORPHAN_OUTPUT_GLOBAL_CAP as u64
                );
                assert_eq!(
                    data["orphan_output"]["capacity_keys"],
                    crate::state::ORPHAN_OUTPUT_KEY_CAP as u64
                );
                assert!(data["server"].is_null());
            })
            .expect("spawn metrics control test")
            .join()
            .expect("metrics control test completes");
    }

    fn pane_wait(text: Option<&str>, timeout_ms: u64) -> Option<crate::control::PaneWait> {
        Some(crate::control::PaneWait {
            text: text.map(str::to_string),
            settle_ms: None,
            timeout_ms,
        })
    }

    fn feed(backend: &mut TestBackend<crate::AppRoot>, bytes: &[u8]) {
        let epoch = backend.state().runtime_epoch;
        let generation = backend.state().current().workspaces[0].panes[0].pty_generation;
        backend
            .dispatch(crate::Msg::SessionOutput {
                epoch,
                pane_id: 1,
                local: false,
                generation,
                bytes: bytes.to_vec(),
            })
            .expect("dispatch pane output");
    }

    fn ask(
        backend: &mut TestBackend<crate::AppRoot>,
        command: ControlCommand,
    ) -> mpsc::Receiver<crate::control::ControlResponse> {
        let (reply, response) = mpsc::channel();
        backend
            .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                request: ControlRequest {
                    command,
                    source_pane: None,
                    extension: None,
                },
                reply,
            }))
            .expect("dispatch control request");
        response
    }

    fn captured_text(response: &crate::control::ControlResponse) -> String {
        let capture: PaneCapture =
            serde_json::from_value(response.data.clone().expect("a capture")).unwrap();
        match capture.content {
            crate::control::CaptureContent::Text { text } => text,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_ui_send_wait_matches_only_output_after_the_server_marks_its_input() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                let outbound = attach_test_session(&mut backend);
                feed(&mut backend, b"$ ");

                let response = ask(
                    &mut backend,
                    ControlCommand::SendKeys {
                        target: Some(1),
                        keys: vec!["make".into(), "Enter".into()],
                        literal: false,
                        wait: pane_wait(Some("$ "), 5_000),
                        capture: Some(CaptureRender::Text),
                        scale: None,
                    },
                );
                let sent: Vec<_> = outbound.try_iter().collect();
                let [
                    ClientOutbound::Control(ClientMessage::MarkedInput {
                        pane_id: 1, token, ..
                    }),
                ] = sent.as_slice()
                else {
                    panic!("the input carries its own mark, got {sent:?}");
                };
                let token = *token;

                // The server answers the mark in the same step as writing the input, so anything
                // ahead of the answer is older than the input - not even a fresh-looking prompt
                // counts.
                feed(&mut backend, b"make\r\nold output\r\n$ ");
                assert!(response.try_recv().is_err());
                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionInputMarked { epoch, token })
                    .expect("dispatch the mark");
                assert!(response.try_recv().is_err(), "the prompt was already there");

                feed(&mut backend, b"\r\nbuilt\r\n$ ");
                let answer = response
                    .try_recv()
                    .expect("the new prompt answers the wait");
                assert!(answer.ok, "{answer:?}");
                assert!(captured_text(&answer).contains("built"));
                assert!(backend.state().capture_waits_are_empty());
            })
            .expect("spawn ui send-wait test")
            .join()
            .expect("ui send-wait test completes");
    }

    #[test]
    fn a_ui_send_settle_counts_quiet_only_from_its_mark() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                let outbound = attach_test_session(&mut backend);
                feed(&mut backend, b"$ ");
                let response = ask(
                    &mut backend,
                    ControlCommand::SendText {
                        target: Some(1),
                        text: "go\r".into(),
                        wait: Some(crate::control::PaneWait {
                            text: None,
                            settle_ms: Some(40),
                            timeout_ms: 5_000,
                        }),
                        capture: None,
                        scale: None,
                    },
                );
                let token = outbound
                    .try_iter()
                    .find_map(|sent| match sent {
                        ClientOutbound::Control(ClientMessage::MarkedInput { token, .. }) => {
                            Some(token)
                        }
                        _ => None,
                    })
                    .expect("the input carries its own mark");

                // The mark is slow to come back: longer than the whole settle period.
                std::thread::sleep(std::time::Duration::from_millis(80));
                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionInputMarked { epoch, token })
                    .expect("dispatch the mark");
                assert!(
                    response.try_recv().is_err(),
                    "nothing was watched before the mark, so none of it is quiet"
                );

                std::thread::sleep(std::time::Duration::from_millis(60));
                backend
                    .dispatch(crate::Msg::CaptureWaitTick)
                    .expect("dispatch tick");
                let answer = response
                    .try_recv()
                    .expect("the quiet after the mark answers");
                assert!(answer.ok, "{answer:?}");
            })
            .expect("spawn ui settle test")
            .join()
            .expect("ui settle test completes");
    }

    #[test]
    fn ui_send_waits_queued_behind_a_starting_pane_share_the_mark_on_its_first_write() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                let outbound = attach_test_session(&mut backend);
                {
                    let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                    pane.pty_generation = 4;
                    pane.terminal.status = ManagedTerminalStatus::Starting;
                }
                let send = |text: &str, wait: &str| ControlCommand::SendText {
                    target: Some(1),
                    text: text.into(),
                    wait: pane_wait(Some(wait), 5_000),
                    capture: None,
                    scale: None,
                };
                let first = ask(&mut backend, send("one\r", "ONE"));
                let second = ask(&mut backend, send("two\r", "TWO"));
                assert!(outbound.try_recv().is_err(), "input waits for the PTY");

                let epoch = backend.state().runtime_epoch;
                backend
                    .dispatch(crate::Msg::SessionSpawnResult {
                        epoch,
                        pane_id: 1,
                        local: false,
                        generation: 4,
                        pid: Some(11),
                        ok: true,
                        error: None,
                    })
                    .expect("dispatch spawn result");
                let sent: Vec<_> = outbound.try_iter().collect();
                let [
                    ClientOutbound::Control(ClientMessage::MarkedInput {
                        pane_id: 1,
                        generation: 4,
                        bytes,
                        token,
                        ..
                    }),
                ] = sent.as_slice()
                else {
                    panic!("the queue goes out as one marked write, got {sent:?}");
                };
                assert_eq!(bytes, b"one\rtwo\r");

                backend
                    .dispatch(crate::Msg::SessionInputMarked {
                        epoch,
                        token: *token,
                    })
                    .expect("dispatch the mark");
                let generation = backend.state().current().workspaces[0].panes[0].pty_generation;
                backend
                    .dispatch(crate::Msg::SessionOutput {
                        epoch,
                        pane_id: 1,
                        local: false,
                        generation,
                        bytes: b"ONE\r\nTWO\r\n".to_vec(),
                    })
                    .expect("dispatch pane output");
                assert!(first.try_recv().expect("first answered").ok);
                assert!(second.try_recv().expect("second answered").ok);
            })
            .expect("spawn queued ui wait test")
            .join()
            .expect("queued ui wait test completes");
    }

    #[test]
    fn a_ui_capture_wait_times_out_with_the_screen_it_had() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                feed(&mut backend, b"still working");
                let response = ask(
                    &mut backend,
                    ControlCommand::CapturePane {
                        target: Some(1),
                        scrollback: None,
                        render: CaptureRender::Text,
                        scale: None,
                        image_pixels: false,
                        wait: pane_wait(Some("done"), 20),
                    },
                );
                assert!(response.try_recv().is_err());
                std::thread::sleep(std::time::Duration::from_millis(30));
                backend
                    .dispatch(crate::Msg::CaptureWaitTick)
                    .expect("dispatch tick");
                let answer = response.try_recv().expect("the deadline answers");
                assert_eq!(answer.code, Some(ControlErrorCode::Timeout));
                assert!(captured_text(&answer).contains("still working"));
            })
            .expect("spawn ui timeout test")
            .join()
            .expect("ui timeout test completes");
    }

    #[test]
    fn a_ui_wait_ends_when_its_pane_exits() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = settled_backend();
                attach_test_session(&mut backend);
                let response = ask(
                    &mut backend,
                    ControlCommand::CapturePane {
                        target: Some(1),
                        scrollback: None,
                        render: CaptureRender::Text,
                        scale: None,
                        image_pixels: false,
                        wait: pane_wait(Some("done"), 5_000),
                    },
                );
                feed(&mut backend, b"crashed");
                let epoch = backend.state().runtime_epoch;
                let generation = backend.state().current().workspaces[0].panes[0].pty_generation;
                backend
                    .dispatch(crate::Msg::SessionExited {
                        epoch,
                        pane_id: 1,
                        local: false,
                        generation,
                        code: 1,
                    })
                    .expect("dispatch exit");
                let answer = response.try_recv().expect("the exit answers");
                assert_eq!(answer.code, Some(ControlErrorCode::PaneNotRunning));
                assert!(captured_text(&answer).contains("crashed"));
            })
            .expect("spawn ui exit test")
            .join()
            .expect("ui exit test completes");
    }
}
