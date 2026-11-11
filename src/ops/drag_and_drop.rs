//! What drag and drop does: writes to the host terminal and to panes, pastes, and registration.

use std::path::PathBuf;
use std::sync::Arc;

use tui_lipan::dnd::{Output, ProgramCommand, TEXT_PLAIN, TEXT_PLAIN_UTF8, URI_LIST};
use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::lifecycle::pane_is_local;
use crate::state::{DropPane, HostRegistration, Pane, State};

/// The pane whose terminal widget carries `key`.
pub(crate) fn pane_by_terminal_key<'a>(state: &'a State, key: &Key) -> Option<&'a Pane> {
    state
        .popup
        .iter()
        .chain(state.scratch.panes.iter())
        .chain(
            state
                .current()
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.panes.iter()),
        )
        .find(|pane| !pane.closing && &pane.keys.terminal == key)
}

pub(crate) fn drop_pane(state: &State, pane: &Pane) -> DropPane {
    DropPane {
        pane_id: pane.id,
        local: pane_is_local(state, pane.id),
        generation: pane.pty_generation,
    }
}

/// Start afresh when another attachment is on screen: the panes the router knew are gone.
pub(crate) fn forget_other_attachment(state: &mut State) {
    let epoch = state.runtime_epoch;
    let drag_and_drop = &mut state.drag_and_drop;
    if drag_and_drop.attachment_epoch != epoch {
        drag_and_drop.attachment_epoch = epoch;
        drag_and_drop.decoders.clear();
        drag_and_drop.hovered = None;
    }
}

/// Carry out what the router asked for.
pub(crate) fn perform(ctx: &mut Context<AppRoot>, outputs: Vec<Output<DropPane>>) -> Update {
    let mut update = Update::none();
    for output in outputs {
        match output {
            Output::Host(bytes) => {
                if ctx.state.config.clipboard.drag_and_drop {
                    ctx.host_drag_and_drop().send_encoded(&bytes);
                }
            }
            Output::Child(pane, bytes) => send_to_pane(ctx, pane, bytes),
            Output::Paste { key, mime, data } => {
                update = update.merge(paste_drop(ctx, key, &mime, data));
            }
            Output::Hover(hovered) => {
                if ctx.state.drag_and_drop.hovered != hovered {
                    ctx.state.drag_and_drop.hovered = hovered;
                    update = update.merge(Update::full());
                }
            }
        }
    }
    update
}

fn send_to_pane(ctx: &mut Context<AppRoot>, pane: DropPane, bytes: Vec<u8>) {
    let Some(client) = ctx.state.pty_client_for_pane(pane.pane_id) else {
        return;
    };
    if !client.supports(crate::session::protocol::Capability::DragAndDrop) {
        return;
    }
    // The escape codes are ASCII: base64 data, MIME types, and errors scrubbed of controls.
    let Ok(escape_codes) = String::from_utf8(bytes) else {
        return;
    };
    client.send_drag_and_drop(pane.pane_id, pane.generation, pane.local, escape_codes);
}

/// Paste a drop on a pane whose program does not speak the protocol, as a terminal would.
///
/// Dropped files become their paths, quoted for a shell prompt. A pane in a session on another
/// machine cannot open this machine's paths, so the files are uploaded and their copies' paths
/// pasted instead, as pasting copied files does.
fn paste_drop(ctx: &mut Context<AppRoot>, pane: DropPane, mime: &str, data: Vec<u8>) -> Update {
    let Some(target) =
        crate::pane::lifecycle::find_pane_in_namespace(&ctx.state, pane.pane_id, pane.local)
            .filter(|target| target.pty_generation == pane.generation)
    else {
        return Update::none();
    };
    let modes = target.terminal.snapshot().key_modes;
    if mime.eq_ignore_ascii_case(URI_LIST) {
        let uris = tui_lipan::dnd::parse_uri_list(&data);
        let files: Vec<PathBuf> = uris
            .iter()
            .filter_map(|uri| tui_lipan::dnd::file_uri_path(uri))
            .collect();
        if !files.is_empty()
            && files.len() == uris.len()
            && crate::ops::attachments::bridge_enabled(&ctx.state, pane.pane_id)
        {
            return crate::ops::attachments::paste(
                ctx,
                pane.pane_id,
                TerminalPasteEvent {
                    content: ClipboardContent::Files(files),
                    origin: PasteOrigin::Clipboard,
                },
            );
        }
        let text: String = uris
            .iter()
            .map(|uri| match tui_lipan::dnd::file_uri_path(uri) {
                Some(path) => crate::platform::attachments::quote_dropped_path(&path),
                None => format!("{uri} "),
            })
            .collect();
        crate::pane::pty_events::paste_pane_bytes(ctx, pane.pane_id, encode_paste(&text, modes))
    } else if mime.eq_ignore_ascii_case(TEXT_PLAIN) || mime.eq_ignore_ascii_case(TEXT_PLAIN_UTF8) {
        let text = String::from_utf8_lossy(&data);
        crate::pane::pty_events::paste_pane_bytes(ctx, pane.pane_id, encode_paste(&text, modes))
    } else {
        Update::none()
    }
}

/// Repairs after every update: what rozi has registered with the host, and the router's hold on
/// panes that have gone.
pub(crate) fn sync(ctx: &mut Context<AppRoot>) {
    forget_other_attachment(&mut ctx.state);
    let gone: Vec<DropPane> = ctx
        .state
        .drag_and_drop
        .router
        .children()
        .into_iter()
        .filter(|pane| {
            crate::pane::lifecycle::find_pane_in_namespace(&ctx.state, pane.pane_id, pane.local)
                .is_none_or(|found| found.pty_generation != pane.generation || found.closing)
        })
        .collect();
    let mut outputs = Vec::new();
    for pane in gone {
        ctx.state.drag_and_drop.decoders.remove(&pane);
        outputs.extend(ctx.state.drag_and_drop.router.forget_child(&pane));
    }
    if !outputs.is_empty() {
        let _ = perform(ctx, outputs);
    }
    sync_registration(ctx);
}

/// Register with the host for drops whenever drag and drop is on, and to offer drags while a pane's
/// program offers them.
fn sync_registration(ctx: &mut Context<AppRoot>) {
    let host = ctx.host_drag_and_drop();
    if !host.supported() {
        return;
    }
    let wanted = if ctx.state.config.clipboard.drag_and_drop {
        let panes = registered_panes(&ctx.state);
        let mut mimes: Vec<String> = panes
            .iter()
            .filter_map(|pane| pane.drops.as_ref())
            .flat_map(|drops| drops.mimes.iter().cloned())
            .filter(|mime| tui_lipan::dnd::is_valid_mime(mime))
            .collect();
        mimes.sort();
        mimes.dedup();
        HostRegistration {
            accepting: Some(mimes),
            offering: panes.iter().any(|pane| pane.drags.is_some()),
        }
    } else {
        HostRegistration::default()
    };
    let registered = &ctx.state.drag_and_drop.registered;
    if *registered == wanted {
        return;
    }
    if registered.accepting != wanted.accepting {
        match &wanted.accepting {
            Some(mimes) => {
                host.send(&ProgramCommand::AcceptDrops {
                    mimes: mimes.iter().map(|mime| Arc::from(mime.as_str())).collect(),
                });
                if let Some(id) = host.machine_id() {
                    host.send(&ProgramCommand::DropMachineId(Arc::from(id.as_str())));
                }
            }
            None => host.send(&ProgramCommand::StopAcceptingDrops),
        }
    }
    if registered.offering != wanted.offering {
        host.send(&if wanted.offering {
            ProgramCommand::OfferDrags {
                machine_id: host.machine_id().map(|id| Arc::from(id.as_str())),
            }
        } else {
            ProgramCommand::StopOfferingDrags
        });
    }
    ctx.state.drag_and_drop.registered = wanted;
}

/// What the panes on screen registered: the current attachment's, the scratchpad's and the popup's.
fn registered_panes(state: &State) -> Vec<crate::session::protocol::PaneDragAndDrop> {
    state
        .popup
        .iter()
        .chain(state.scratch.panes.iter())
        .chain(
            state
                .current()
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.panes.iter()),
        )
        .filter(|pane| !pane.closing && !pane.terminal.drag_and_drop.is_empty())
        .map(|pane| pane.terminal.drag_and_drop.clone())
        .collect()
}
