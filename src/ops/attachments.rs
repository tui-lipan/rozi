//! Paste local clipboard attachments into the session that owns the target pane.

use crate::pane::lifecycle::{find_pane, pane_is_local};
use crate::session::client::attachments::Source;
use crate::state::{PaneId, State};
use crate::{AppRoot, Msg};
use std::result::Result;
use tui_lipan::prelude::*;

pub(crate) fn bridge_enabled(state: &State, id: PaneId) -> bool {
    state.current().remote_target.is_some() && !crate::scratchpad::contains(state, id)
}

pub(crate) fn paste(ctx: &mut Context<AppRoot>, id: PaneId, event: TerminalPasteEvent) -> Update {
    let TerminalPasteEvent { content, origin } = event;
    let Some(pane) = find_pane(&ctx.state, id) else {
        return Update::none();
    };
    let generation = pane.pty_generation;
    let modes = pane.terminal.snapshot().key_modes;
    let local = pane_is_local(&ctx.state, id);
    if let ClipboardContent::Text(text) = &content {
        // Recognize an image path only in terminal-emitted text. Clipboard text
        // remains literal even when a local image exists at the copied path.
        if !bridge_enabled(&ctx.state, id) || dropped_image_path(text, origin).is_none() {
            let result =
                crate::pane::pty_events::send_pane_bytes(ctx, id, encode_paste(text, modes));
            if let Err(err) = result {
                crate::pane::pty_events::notify_error(ctx, "Paste failed", err);
            }
            crate::ops::focus::acknowledge_pane_input(&mut ctx.state, id);
            return Update::full();
        }
    }
    let Some(client) = ctx.state.pty_client_for_pane(id) else {
        return Update::none();
    };
    let fallback = match &content {
        ClipboardContent::Text(text) => Some((text.clone(), modes)),
        _ => None,
    };
    let sources = match sources(content, origin) {
        Ok(sources) => sources,
        Err(err) => {
            crate::pane::pty_events::notify_error(ctx, "Paste failed", err);
            return Update::full();
        }
    };
    crate::ops::focus::acknowledge_pane_input(&mut ctx.state, id);
    Update::with_command(Command::spawn(move |link| {
        let result = upload(&client, id, generation, local, sources, fallback);
        link.send(Msg::AttachmentUploadFinished(result));
    }))
}

fn upload(
    client: &crate::session::client::SessionClient,
    id: PaneId,
    generation: u64,
    local: bool,
    sources: Vec<Source>,
    fallback: Option<(String, TerminalKeyModes)>,
) -> Result<(), String> {
    // A terminal emits text for both file drops and ordinary path pastes. Check existence off
    // the UI thread, and preserve paths that can only exist on the remote host.
    if let Some((text, modes)) = fallback
        && let Some(Source::File(path)) = sources.first()
        && !path.try_exists().unwrap_or(true)
    {
        client.send_input(id, generation, local, encode_paste(&text, modes));
        return Ok(());
    }
    sources
        .into_iter()
        .try_for_each(|source| client.upload_attachment(id, generation, local, source))
}

fn sources(content: ClipboardContent, origin: PasteOrigin) -> Result<Vec<Source>, String> {
    match content {
        ClipboardContent::Image(image) => Ok(vec![Source::Image(image)]),
        ClipboardContent::Files(files) if files.len() <= 16 => {
            Ok(files.into_iter().map(Source::File).collect())
        }
        ClipboardContent::Files(_) => Err("paste at most 16 files at once".into()),
        ClipboardContent::Text(text) => dropped_image_path(&text, origin)
            .map(|path| vec![Source::File(path)])
            .ok_or("unsupported file drop".into()),
    }
}

fn dropped_image_path(text: &str, origin: PasteOrigin) -> Option<std::path::PathBuf> {
    if origin != PasteOrigin::Terminal {
        return None;
    }
    let text = text.trim_end_matches(['\r', '\n']);
    if text.contains(['\r', '\n', '\0']) {
        return None;
    }
    let path = crate::platform::attachments::drop_path(text)?;
    let extension = path.extension()?.to_str()?;
    if !["png", "jpg", "jpeg", "gif", "webp", "bmp"]
        .iter()
        .any(|ext| extension.eq_ignore_ascii_case(ext))
    {
        return None;
    }
    // No synchronous disk access on the UI thread. The worker reports unreadable local drops.
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copied_literal_path_is_text_even_when_the_image_exists_locally() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.png");
        std::fs::write(&path, b"local image bytes").unwrap();
        let text = path.to_string_lossy().to_string();
        assert!(dropped_image_path(&text, PasteOrigin::Clipboard).is_none());
        assert_eq!(
            dropped_image_path(&text, PasteOrigin::Terminal),
            Some(path.clone())
        );
        assert!(sources(ClipboardContent::Text(text.clone()), PasteOrigin::Clipboard).is_err());
        assert!(
            matches!(sources(ClipboardContent::Text(text), PasteOrigin::Terminal).unwrap().as_slice(), [Source::File(file)] if *file == path)
        );
    }

    #[test]
    fn clipboard_path_event_sends_literal_text_to_the_remote_pane() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                crate::test_support::isolate_user_dirs();
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("photo.png");
                std::fs::write(&path, b"local image bytes").unwrap();
                let text = path.to_string_lossy().to_string();
                let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
                let (client, outbound) = crate::session::client::SessionClient::test_channel();
                let (id, generation, modes) = {
                    let state = backend.state_mut();
                    state.current_mut().session_client = Some(client);
                    state.current_mut().remote_target = Some(
                        crate::session::remote::RemoteTarget::Alias("test-host".into()),
                    );
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.opening = false;
                    pane.terminal_active = true;
                    (pane.id, pane.pty_generation, pane.terminal.snapshot().key_modes)
                };
                while outbound.try_recv().is_ok() {}
                backend.dispatch(Msg::PanePaste(id, TerminalPasteEvent {
                    content: ClipboardContent::Text(text.clone()),
                    origin: PasteOrigin::Clipboard,
                })).unwrap();
                let messages: Vec<_> = outbound.try_iter().filter(|message| matches!(message,
                    crate::session::client::ClientOutbound::PaneInput { .. }
                    | crate::session::client::ClientOutbound::Control(
                        crate::session::protocol::ClientMessage::Attachment { .. }
                    )
                )).collect();
                assert_eq!(messages.len(), 1, "a text paste must not start an upload");
                assert!(matches!(&messages[0], crate::session::client::ClientOutbound::PaneInput {
                    pane_id, generation: actual_generation, bytes, ..
                } if *pane_id == id && *actual_generation == generation && *bytes == encode_paste(&text, modes)));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn missing_local_image_path_remains_a_remote_text_paste() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("remote-only.png");
        let text = missing.to_string_lossy().to_string();
        let (client, outbound) = crate::session::client::SessionClient::test_channel();
        let modes = TerminalKeyModes {
            bracketed_paste: true,
            ..Default::default()
        };
        upload(
            &client,
            7,
            9,
            false,
            vec![Source::File(missing)],
            Some((text.clone(), modes)),
        )
        .unwrap();
        assert!(
            matches!(outbound.recv().unwrap(), crate::session::client::ClientOutbound::PaneInput { pane_id: 7, generation: 9, bytes, .. } if bytes == encode_paste(&text, modes))
        );
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn image_drops_are_single_absolute_image_paths_and_documents_remain_text() {
        assert!(dropped_image_path("/tmp/notes.txt", PasteOrigin::Terminal).is_none());
        assert!(dropped_image_path("photo.png", PasteOrigin::Terminal).is_none());
        assert!(dropped_image_path("/tmp/photo.png\n/other.png", PasteOrigin::Terminal).is_none());
        assert_eq!(
            dropped_image_path("'/tmp/paste test.png'", PasteOrigin::Terminal),
            Some("/tmp/paste test.png".into())
        );
        assert_eq!(
            dropped_image_path("/tmp/paste\\ test.png", PasteOrigin::Terminal),
            Some("/tmp/paste test.png".into())
        );
    }
}
