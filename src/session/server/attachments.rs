//! Bounded uploads. Disk writes run off the session pump; only the pump can insert paths.

use super::*;
use crate::platform::attachments::StagedFile;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use protocol::{ATTACHMENT_CHUNK_BYTES, AttachmentOperation, MAX_ATTACHMENT_BYTES};
use std::result::Result;

type Key = (ClientId, u64);
const MAX_PENDING: usize = 8;
const MAX_RETAINED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RETAINED_FILES: usize = 256;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
struct Destination {
    pane: PaneId,
    generation: u64,
    local: bool,
}
struct Pending {
    destination: Destination,
    size: u64,
    received: u64,
    busy: bool,
    updated: Instant,
}
#[derive(Default)]
pub(super) struct Attachments {
    pending: HashMap<Key, Pending>,
    retained: Vec<StagedFile>,
    worker: Option<Worker>,
}
struct Job {
    key: Key,
    operation: AttachmentOperation,
}
struct Done {
    key: Key,
    result: Result<Option<StagedFile>, String>,
}
struct Writing {
    staged: StagedFile,
    file: fs::File,
    received: u64,
    updated: Instant,
}
struct Worker {
    jobs: mpsc::SyncSender<Job>,
    done: mpsc::Receiver<Done>,
}

impl Worker {
    fn new() -> io::Result<Self> {
        let (jobs, incoming) = mpsc::sync_channel::<Job>(MAX_PENDING * 2);
        let (completed, done) = mpsc::sync_channel(MAX_PENDING * 2);
        std::thread::Builder::new()
            .name("rozi-attachments".into())
            .spawn(move || {
                let mut writing = HashMap::<Key, Writing>::new();
                loop {
                    writing.retain(|_, file| file.updated.elapsed() < UPLOAD_TIMEOUT);
                    let job = match incoming.recv_timeout(Duration::from_secs(1)) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let cancel = matches!(job.operation, AttachmentOperation::Cancel);
                    let result = disk_step(&mut writing, job.key, job.operation)
                        .map_err(|err| err.to_string());
                    if result.is_err() {
                        writing.remove(&job.key);
                    }
                    if !cancel
                        && completed
                            .send(Done {
                                key: job.key,
                                result,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self { jobs, done })
    }
}

fn disk_step(
    writing: &mut HashMap<Key, Writing>,
    key: Key,
    operation: AttachmentOperation,
) -> io::Result<Option<StagedFile>> {
    match operation {
        AttachmentOperation::Begin { name, size, .. } => {
            let (staged, file) = StagedFile::create(&name, size)?;
            writing.insert(
                key,
                Writing {
                    staged,
                    file,
                    received: 0,
                    updated: Instant::now(),
                },
            );
            Ok(None)
        }
        AttachmentOperation::Chunk { data } => {
            let bytes = STANDARD.decode(data).map_err(io::Error::other)?;
            let file = writing
                .get_mut(&key)
                .ok_or_else(|| io::Error::other("upload not found"))?;
            file.file.write_all(&bytes)?;
            file.received += bytes.len() as u64;
            file.updated = Instant::now();
            Ok(None)
        }
        AttachmentOperation::Finish => {
            let file = writing
                .remove(&key)
                .ok_or_else(|| io::Error::other("upload not found"))?;
            if file.received != file.staged.size {
                return Err(io::Error::other("attachment size changed"));
            }
            drop(file.file);
            Ok(Some(file.staged))
        }
        AttachmentOperation::Cancel => {
            writing.remove(&key);
            Ok(None)
        }
    }
}

impl Attachments {
    fn can_reserve(&self, client: ClientId, size: u64) -> bool {
        let reserved: u64 = self.pending.values().map(|p| p.size).sum();
        let retained: u64 = self.retained.iter().map(|file| file.size).sum();
        self.pending.len() < MAX_PENDING
            && !self.pending.keys().any(|key| key.0 == client)
            && self.retained.len() + self.pending.len() < MAX_RETAINED_FILES
            && retained + reserved + size <= MAX_RETAINED_BYTES
    }
}

fn checked_chunk_size(data: &str, remaining: u64) -> Result<u64, String> {
    if data.len() > ATTACHMENT_CHUNK_BYTES.div_ceil(3) * 4 {
        return Err("attachment chunk is too large".into());
    }
    let size = STANDARD.decode(data).map_err(|err| err.to_string())?.len() as u64;
    if size == 0 || size > ATTACHMENT_CHUNK_BYTES as u64 || size > remaining {
        return Err("attachment size mismatch".into());
    }
    Ok(size)
}

impl SessionServer {
    fn attachment_destination_valid(&self, client: ClientId, destination: Destination) -> bool {
        let may_input = if destination.local {
            self.client_attached(client) && !self.client_read_only(client)
        } else {
            self.client_may_input(client)
        };
        may_input
            && self
                .pane(destination.local.then_some(client), destination.pane)
                .is_some_and(|pane| pane.generation == destination.generation && pane.pty.is_some())
    }

    fn begin_attachment(
        &mut self,
        key: Key,
        operation: &AttachmentOperation,
    ) -> Result<(), String> {
        let AttachmentOperation::Begin {
            pane_id,
            generation,
            local,
            name,
            size,
        } = operation
        else {
            return Ok(());
        };
        let destination = Destination {
            pane: *pane_id,
            generation: *generation,
            local: *local,
        };
        if !self.attachment_destination_valid(key.0, destination) {
            return Err("pane is unavailable or input is blocked".into());
        }
        if name.is_empty() || name.len() > 1024 || *size > MAX_ATTACHMENT_BYTES {
            return Err("attachment exceeds name or 16 MiB file limit".into());
        }
        if !self.attachments.can_reserve(key.0, *size) {
            return Err("session attachment storage or upload limit reached".into());
        }
        if self.attachments.worker.is_none() {
            self.attachments.worker = Some(Worker::new().map_err(|err| err.to_string())?);
        }
        self.attachments.pending.insert(
            key,
            Pending {
                destination,
                size: *size,
                received: 0,
                busy: false,
                updated: Instant::now(),
            },
        );
        Ok(())
    }

    fn validate_attachment_step(
        &mut self,
        key: Key,
        operation: &AttachmentOperation,
    ) -> Result<(), String> {
        if self.attachments.pending.contains_key(&key)
            && matches!(operation, AttachmentOperation::Begin { .. })
        {
            return Err("upload already exists".into());
        }
        self.begin_attachment(key, operation)?;
        let pending = self
            .attachments
            .pending
            .get(&key)
            .ok_or("upload not found")?;
        if pending.busy {
            return Err("wait for the previous upload acknowledgement".into());
        }
        if !self.attachment_destination_valid(key.0, pending.destination) {
            return Err("pane is unavailable or input is blocked".into());
        }
        let pending = self
            .attachments
            .pending
            .get_mut(&key)
            .ok_or("upload not found")?;
        if let AttachmentOperation::Chunk { data } = operation {
            let size = checked_chunk_size(data, pending.size - pending.received)?;
            pending.received += size;
        }
        if matches!(operation, AttachmentOperation::Finish) && pending.received != pending.size {
            return Err("attachment is incomplete".into());
        }
        pending.busy = true;
        pending.updated = Instant::now();
        Ok(())
    }

    pub(super) fn attachment_step(
        &mut self,
        client: ClientId,
        transfer_id: u64,
        operation: AttachmentOperation,
    ) -> Vec<(Target, ServerMessage)> {
        let key = (client, transfer_id);
        if matches!(operation, AttachmentOperation::Cancel) {
            self.cancel_attachment(key);
            return vec![];
        }
        let result = self
            .validate_attachment_step(key, &operation)
            .and_then(|()| {
                self.attachments
                    .worker
                    .as_ref()
                    .ok_or("attachment worker unavailable".to_string())?
                    .jobs
                    .try_send(Job { key, operation })
                    .map_err(|_| "attachment worker is busy".to_string())
            });
        if let Err(error) = result {
            self.cancel_attachment(key);
            return vec![(
                Target::Sender,
                ServerMessage::AttachmentAck {
                    transfer_id,
                    error: Some(error),
                },
            )];
        }
        vec![]
    }

    fn cancel_attachment(&mut self, key: Key) {
        self.attachments.pending.remove(&key);
        if let Some(worker) = &self.attachments.worker {
            let _ = worker.jobs.try_send(Job {
                key,
                operation: AttachmentOperation::Cancel,
            });
        }
    }

    pub(super) fn cancel_client_attachments(&mut self, client: ClientId) {
        let keys: Vec<_> = self
            .attachments
            .pending
            .keys()
            .filter(|k| k.0 == client)
            .copied()
            .collect();
        for key in keys {
            self.cancel_attachment(key);
        }
    }

    pub(super) fn poll_attachment_work(&mut self) -> bool {
        let expired: Vec<_> = self
            .attachments
            .pending
            .iter()
            .filter(|(_, p)| p.updated.elapsed() >= UPLOAD_TIMEOUT)
            .map(|(k, _)| *k)
            .collect();
        for key in expired {
            self.cancel_attachment(key);
        }
        let mut activity = false;
        loop {
            let done = self
                .attachments
                .worker
                .as_ref()
                .and_then(|w| w.done.try_recv().ok());
            let Some(done) = done else {
                break;
            };
            activity = true;
            if !self.attachments.pending.contains_key(&done.key) {
                continue;
            }
            let result = self.complete_attachment_step(done.key, done.result);
            self.enqueue(
                done.key.0,
                Target::Sender,
                ServerMessage::AttachmentAck {
                    transfer_id: done.key.1,
                    error: result.err(),
                },
            );
        }
        activity
    }

    fn complete_attachment_step(
        &mut self,
        key: Key,
        result: Result<Option<StagedFile>, String>,
    ) -> Result<(), String> {
        let file = match result {
            Ok(file) => file,
            Err(err) => {
                self.cancel_attachment(key);
                return Err(err);
            }
        };
        let pending = self
            .attachments
            .pending
            .get_mut(&key)
            .ok_or("upload not found")?;
        pending.busy = false;
        pending.updated = Instant::now();
        let destination = pending.destination;
        let Some(file) = file else {
            return Ok(());
        };
        self.attachments.pending.remove(&key);
        if !self.attachment_destination_valid(key.0, destination) {
            return Err("pane changed or input is blocked".into());
        }
        let pane = self
            .pane(destination.local.then_some(key.0), destination.pane)
            .ok_or("pane closed")?;
        let path = file.path.to_str().ok_or("attachment path is not UTF-8")?;
        let text = crate::platform::attachments::paste_path(path)?;
        let bytes = encode_paste(&text, pane.screen().key_modes());
        pane.pty
            .as_ref()
            .ok_or("pane exited")?
            .write(&bytes)
            .map_err(|err| err.to_string())?;
        self.attachments.retained.push(file);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(size: u64) -> AttachmentOperation {
        AttachmentOperation::Begin {
            pane_id: 7,
            generation: 3,
            local: false,
            name: "photo.png".into(),
            size,
        }
    }
    #[test]
    fn disk_upload_preserves_bytes_and_cancellation_removes_partial_files() {
        let mut files = HashMap::new();
        let key = (1, 2);
        disk_step(&mut files, key, begin(4)).unwrap();
        let path = files[&key].staged.path.clone();
        disk_step(
            &mut files,
            key,
            AttachmentOperation::Chunk {
                data: STANDARD.encode([0, 255, 13, 10]),
            },
        )
        .unwrap();
        let staged = disk_step(&mut files, key, AttachmentOperation::Finish)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(&staged.path).unwrap(), [0, 255, 13, 10]);
        assert_eq!(staged.path, path);
        assert!(files.is_empty());
        drop(staged);
        assert!(!path.exists());
        disk_step(&mut files, key, begin(4)).unwrap();
        let path = files[&key].staged.path.clone();
        disk_step(&mut files, key, AttachmentOperation::Cancel).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn chunks_reject_invalid_base64_empty_oversized_and_overrun_data() {
        assert!(checked_chunk_size("!", 100).is_err());
        assert!(checked_chunk_size("", 100).is_err());
        assert!(checked_chunk_size(&STANDARD.encode(b"abcd"), 3).is_err());
        assert!(
            checked_chunk_size(
                &STANDARD.encode(vec![0; ATTACHMENT_CHUNK_BYTES + 1]),
                MAX_ATTACHMENT_BYTES
            )
            .is_err()
        );
        assert_eq!(checked_chunk_size(&STANDARD.encode(b"abcd"), 4).unwrap(), 4);
    }

    #[test]
    fn disk_upload_rejects_incomplete_files() {
        let mut files = HashMap::new();
        let key = (1, 2);
        disk_step(&mut files, key, begin(4)).unwrap();
        let path = files[&key].staged.path.clone();
        assert!(disk_step(&mut files, key, AttachmentOperation::Finish).is_err());
        assert!(!path.exists());
    }
    #[cfg(unix)]
    #[test]
    fn upload_admission_rejects_stale_targets_and_read_only_clients() {
        let mut server = SessionServer::new_named("attachment-policy");
        let (client, _stream) = super::super::tests::attach_client(&mut server);
        assert!(server.begin_attachment((client, 1), &begin(4)).is_err());
        server.panes.insert(7, super::super::tests::test_pane(4));
        assert!(server.begin_attachment((client, 1), &begin(4)).is_err());
        server.client_mut(client).unwrap().read_only = true;
        assert!(server.begin_attachment((client, 1), &begin(4)).is_err());
        assert!(server.attachments.pending.is_empty());
    }
}

#[cfg(all(test, unix))]
mod delivery_tests {
    use super::super::tests::{attach_client, test_pane};
    use super::*;

    #[test]
    fn completed_upload_pastes_into_original_pane_and_survives_detach() {
        let mut server = SessionServer::new_named("attachment-delivery");
        let (client, _stream) = attach_client(&mut server);
        let (events, received) = mpsc::channel();
        let pty = TerminalPty::spawn(
            TerminalPtyConfig::new("/bin/sh")
                .arg("-c")
                .arg("stty raw -echo; printf ready; cat"),
            move |event| {
                let _ = events.send(event);
            },
        )
        .unwrap();
        let mut output = Vec::new();
        while !output.windows(5).any(|s| s == b"ready") {
            if let TerminalPtyEvent::Output(bytes) =
                received.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                output.extend_from_slice(&bytes);
            }
        }
        let mut pane = test_pane(3);
        pane.pty = Some(pty);
        pane.screen_mut().process_bytes(b"\x1b[?2004h");
        server.panes.insert(7, pane);
        let candidate = AttachmentOperation::Begin {
            pane_id: 7,
            generation: 3,
            local: false,
            name: "test.txt".into(),
            size: 4,
        };
        server.client_mut(client).unwrap().read_only = true;
        assert!(server.begin_attachment((client, 2), &candidate).is_err());
        server.client_mut(client).unwrap().read_only = false;
        server.panes.get_mut(&7).unwrap().generation = 4;
        assert!(server.begin_attachment((client, 2), &candidate).is_err());
        server.panes.get_mut(&7).unwrap().generation = 3;
        let begin = AttachmentOperation::Begin {
            pane_id: 7,
            generation: 3,
            local: false,
            name: "test.txt".into(),
            size: 4,
        };
        for operation in [
            begin,
            AttachmentOperation::Chunk {
                data: STANDARD.encode(b"data"),
            },
            AttachmentOperation::Finish,
        ] {
            assert!(server.attachment_step(client, 1, operation).is_empty());
            let deadline = Instant::now() + Duration::from_secs(5);
            while server
                .attachments
                .pending
                .get(&(client, 1))
                .is_some_and(|p| p.busy)
            {
                assert!(Instant::now() < deadline);
                server.poll_attachment_work();
                std::thread::yield_now();
            }
        }
        assert_eq!(server.attachments.retained.len(), 1);
        let path = server.attachments.retained[0].path.clone();
        assert_eq!(fs::read(&path).unwrap(), b"data");
        let expected = encode_paste(
            &crate::platform::attachments::paste_path(path.to_str().unwrap()).unwrap(),
            server.panes[&7].screen().key_modes(),
        );
        let mut actual = vec![];
        while actual.len() < expected.len() {
            if let TerminalPtyEvent::Output(bytes) =
                received.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                actual.extend_from_slice(&bytes);
            }
        }
        assert_eq!(actual, expected);
        server.remove_client(client);
        assert!(path.exists());
        server.panes[&7].pty.as_ref().unwrap().kill().unwrap();
        drop(server);
        assert!(!path.exists());
    }
}
