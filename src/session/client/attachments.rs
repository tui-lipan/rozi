//! Acknowledged attachment uploads. Call only from a background command.

use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use protocol::{ATTACHMENT_CHUNK_BYTES, AttachmentOperation, MAX_ATTACHMENT_BYTES};
use std::io::Read;
use std::result::Result;
use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub(super) struct Replies {
    next: AtomicU64,
    upload: Mutex<()>,
    waiter: Mutex<Option<ReplyWaiter>>,
}
type ReplyWaiter = (u64, mpsc::Sender<Result<(), String>>);

impl Replies {
    pub(super) fn acknowledge(&self, id: u64, error: Option<String>) {
        if let Ok(waiter) = self.waiter.lock()
            && let Some((expected, sender)) = &*waiter
            && *expected == id
        {
            let _ = sender.send(error.map_or(Ok(()), Err));
        }
    }
    pub(super) fn disconnect(&self) {
        if let Ok(mut waiter) = self.waiter.lock() {
            *waiter = None;
        }
    }
}

pub(crate) enum Source {
    Image(tui_lipan::ImageContent),
    File(std::path::PathBuf),
}

impl SessionClient {
    pub(crate) fn upload_attachment(
        &self,
        pane_id: PaneId,
        generation: u64,
        local: bool,
        source: Source,
    ) -> Result<(), String> {
        let _guard = self
            .attachment_replies
            .upload
            .try_lock()
            .map_err(|_| "another attachment upload is in progress".to_string())?;
        let (name, size, reader) = open_source(source)?;
        let id = self.attachment_replies.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        *self
            .attachment_replies
            .waiter
            .lock()
            .map_err(|_| "upload reply lock failed")? = Some((id, sender));
        let result = self.upload_reader(
            id,
            AttachmentOperation::Begin {
                pane_id,
                generation,
                local,
                name,
                size,
            },
            reader,
            &receiver,
        );
        self.attachment_replies.disconnect();
        if result.is_err() {
            self.send_control(ClientMessage::Attachment {
                transfer_id: id,
                operation: AttachmentOperation::Cancel,
            });
        }
        result
    }

    fn upload_reader(
        &self,
        id: u64,
        begin: AttachmentOperation,
        mut reader: Box<dyn Read>,
        receiver: &mpsc::Receiver<Result<(), String>>,
    ) -> Result<(), String> {
        self.attachment_request(id, begin, receiver)?;
        let mut bytes = vec![0; ATTACHMENT_CHUNK_BYTES];
        loop {
            let read = reader.read(&mut bytes).map_err(|err| err.to_string())?;
            if read == 0 {
                break;
            }
            self.attachment_request(
                id,
                AttachmentOperation::Chunk {
                    data: STANDARD.encode(&bytes[..read]),
                },
                receiver,
            )?;
        }
        self.attachment_request(id, AttachmentOperation::Finish, receiver)
    }

    fn attachment_request(
        &self,
        id: u64,
        operation: AttachmentOperation,
        receiver: &mpsc::Receiver<Result<(), String>>,
    ) -> Result<(), String> {
        if self.outbound.stats().closed {
            return Err("session disconnected".into());
        }
        self.send_control(ClientMessage::Attachment {
            transfer_id: id,
            operation,
        });
        receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "attachment upload disconnected or timed out".to_string())?
    }
}

fn open_source(source: Source) -> Result<(String, u64, Box<dyn Read>), String> {
    let (name, size, reader): (_, _, Box<dyn Read>) = match source {
        Source::Image(image) => {
            let name = match image.mime {
                "image/png" => "clipboard.png",
                "image/jpeg" => "clipboard.jpg",
                _ => return Err("unsupported clipboard image format".into()),
            };
            let bytes = image.to_bytes().map_err(|err| err.to_string())?;
            (
                name.to_string(),
                bytes.len() as u64,
                Box::new(io::Cursor::new(bytes)),
            )
        }
        Source::File(path) => {
            let file = crate::platform::attachments::open_source(&path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            let size = file.metadata().map_err(|err| err.to_string())?.len();
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("attachment filename is not UTF-8")?
                .to_owned();
            (name, size, Box::new(file))
        }
    };
    if size > MAX_ATTACHMENT_BYTES {
        return Err("attachment exceeds the 16 MiB file limit".into());
    }
    // A growing local file cannot keep an upload alive indefinitely.
    Ok((name, size, Box::new(reader.take(MAX_ATTACHMENT_BYTES + 1))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_waits_for_each_ack_and_streams_exact_source_bytes() {
        let (client, outbound) = SessionClient::test_channel();
        let replies = client.attachment_replies.clone();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        let expected = vec![b'x'; ATTACHMENT_CHUNK_BYTES + 17];
        std::fs::write(&path, &expected).unwrap();
        let task =
            std::thread::spawn(move || client.upload_attachment(7, 9, false, Source::File(path)));
        let mut received = vec![];
        let mut steps = vec![];
        loop {
            let ClientOutbound::Control(ClientMessage::Attachment {
                transfer_id,
                operation,
            }) = outbound.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("unexpected message");
            };
            assert!(
                outbound.try_recv().is_err(),
                "next step must wait for its acknowledgement"
            );
            let finish = matches!(operation, AttachmentOperation::Finish);
            match operation {
                AttachmentOperation::Begin {
                    pane_id,
                    generation,
                    size,
                    ..
                } => {
                    assert_eq!((pane_id, generation, size), (7, 9, expected.len() as u64));
                    steps.push("begin");
                }
                AttachmentOperation::Chunk { data } => {
                    received.extend(STANDARD.decode(data).unwrap());
                    steps.push("chunk");
                }
                AttachmentOperation::Finish => steps.push("finish"),
                AttachmentOperation::Cancel => panic!("unexpected cancel"),
            }
            replies.acknowledge(transfer_id, None);
            if finish {
                break;
            }
        }
        assert!(task.join().unwrap().is_ok());
        assert_eq!(received, expected);
        assert_eq!(steps, ["begin", "chunk", "chunk", "finish"]);
    }
    #[test]
    fn server_rejection_cancels_upload_without_sending_file_bytes() {
        let (client, outbound) = SessionClient::test_channel();
        let replies = client.attachment_replies.clone();
        let image =
            tui_lipan::ImageContent::from_bytes(b"image bytes", tui_lipan::ImageFormat::Png);
        let task =
            std::thread::spawn(move || client.upload_attachment(7, 9, false, Source::Image(image)));
        let ClientOutbound::Control(ClientMessage::Attachment { transfer_id, .. }) =
            outbound.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("expected begin");
        };
        replies.acknowledge(transfer_id, Some("read-only client".into()));
        assert_eq!(task.join().unwrap().unwrap_err(), "read-only client");
        assert!(matches!(
            outbound.recv_timeout(Duration::from_secs(5)).unwrap(),
            ClientOutbound::Control(ClientMessage::Attachment {
                operation: AttachmentOperation::Cancel,
                ..
            })
        ));
        assert!(outbound.try_recv().is_err());
    }
}
