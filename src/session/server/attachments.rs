//! Bounded uploads. Disk writes run off the session pump; only the pump can insert paths.

use super::headless::ReplyTo;
use super::*;
use crate::control::{ControlErrorCode, ControlResponse};
use crate::platform::attachments::StagedFile;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use protocol::{ATTACHMENT_CHUNK_BYTES, AttachmentOperation, MAX_ATTACHMENT_BYTES};
use std::result::Result;

type Key = (ClientId, u64);
const MAX_PENDING: usize = 8;
const MAX_RETAINED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RETAINED_FILES: usize = 256;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a finished `agent-attach` upload waits for the request that claims it. Long enough
/// for the rest of a slow multi-file upload; an asker that fails cancels its uploads at once.
const UNCLAIMED_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Owns the uploads that arrive as `agent-attach` control commands. Each chunk comes over a
/// connection of its own, so the upload cannot belong to one; no client is ever given this id.
const CONTROL_UPLOADS: ClientId = ClientId::MAX;

#[derive(Clone, Copy)]
struct Destination {
    pane: PaneId,
    generation: u64,
    local: bool,
}
/// What a finished upload is for.
enum Purpose {
    /// Paste its path into a pane, as a client's paste or drop does.
    Paste(Destination),
    /// Keep it for an agent request, answering the control caller with its path. `reply` is the
    /// chunk whose answer waits on the disk worker.
    Keep { reply: Option<(ClientId, ReplyTo)> },
}
struct Pending {
    purpose: Purpose,
    size: u64,
    received: u64,
    busy: bool,
    updated: Instant,
}
#[derive(Default)]
pub(super) struct Attachments {
    pending: HashMap<Key, Pending>,
    retained: Vec<StagedFile>,
    /// Finished `agent-attach` uploads no request has claimed yet, by transfer id, with when
    /// they finished. They count against the same limits as retained files.
    unclaimed: HashMap<u64, (StagedFile, Instant)>,
    worker: Option<Worker>,
}
/// Disk steps for one upload, run in order; the first failure ends the job.
struct Job {
    key: Key,
    operations: Vec<AttachmentOperation>,
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
                    let cancel = matches!(job.operations[..], [AttachmentOperation::Cancel]);
                    let mut result = Ok(None);
                    for operation in job.operations {
                        result = disk_step(&mut writing, job.key, operation)
                            .map_err(|err| err.to_string());
                        if result.is_err() {
                            break;
                        }
                    }
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
        AttachmentOperation::Unknown => Err(io::Error::other(
            "this session's rozi does not support that attachment step",
        )),
        AttachmentOperation::Cancel => {
            writing.remove(&key);
            Ok(None)
        }
    }
}

impl Attachments {
    #[cfg(all(test, unix))]
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    #[cfg(all(test, unix))]
    pub(super) fn is_busy(&self, key: Key) -> bool {
        self.pending.get(&key).is_some_and(|pending| pending.busy)
    }

    fn can_reserve(&self, client: ClientId, size: u64) -> bool {
        let reserved: u64 = self.pending.values().map(|p| p.size).sum();
        let kept: u64 = self
            .retained
            .iter()
            .chain(self.unclaimed.values().map(|(file, _)| file))
            .map(|file| file.size)
            .sum();
        let files = self.retained.len() + self.unclaimed.len() + self.pending.len();
        // A client pastes one upload at a time; control uploads are many callers' at once.
        self.pending.len() < MAX_PENDING
            && (client == CONTROL_UPLOADS || !self.pending.keys().any(|key| key.0 == client))
            && files < MAX_RETAINED_FILES
            && kept + reserved + size <= MAX_RETAINED_BYTES
    }
}

/// One `agent-attach` chunk, as the control command carries it.
#[derive(Clone, Copy)]
pub(super) struct AgentAttachmentChunk<'a> {
    pub(super) transfer: u64,
    pub(super) name: &'a str,
    pub(super) size: u64,
    pub(super) offset: u64,
    pub(super) data: &'a str,
    pub(super) last: bool,
}

fn checked_chunk_size(data: &str, remaining: u64) -> Result<u64, String> {
    checked_chunk_size_up_to(data, remaining, ATTACHMENT_CHUNK_BYTES)
}

fn checked_chunk_size_up_to(data: &str, remaining: u64, max: usize) -> Result<u64, String> {
    if data.len() > max.div_ceil(3) * 4 {
        return Err("attachment chunk is too large".into());
    }
    let size = STANDARD.decode(data).map_err(|err| err.to_string())?.len() as u64;
    if size == 0 || size > max as u64 || size > remaining {
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
        // The history reader owns the pane's input until it has restored the viewport.
        let history_leased =
            !destination.local && self.agent_history.contains_key(&destination.pane);
        may_input
            && !history_leased
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
            return Err(
                "pane is unavailable, input is blocked, or agent history is being read; retry"
                    .into(),
            );
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
                purpose: Purpose::Paste(destination),
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
        let Purpose::Paste(destination) = pending.purpose else {
            return Err("upload not found".into());
        };
        if !self.attachment_destination_valid(key.0, destination) {
            return Err(
                "pane is unavailable, input is blocked, or agent history is being read; retry"
                    .into(),
            );
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

    /// One chunk of an `agent-attach` upload. `None` holds the answer until the disk worker has
    /// written the chunk; a refusal answers at once and drops the upload.
    pub(super) fn agent_attachment_chunk(
        &mut self,
        client: ClientId,
        reply: ReplyTo,
        chunk: AgentAttachmentChunk<'_>,
    ) -> Option<ControlResponse> {
        let key = (CONTROL_UPLOADS, chunk.transfer);
        match self.queue_control_chunk(key, chunk) {
            Ok(operations) => {
                let queued =
                    self.attachments.worker.as_ref().is_some_and(|worker| {
                        worker.jobs.try_send(Job { key, operations }).is_ok()
                    });
                if !queued {
                    self.cancel_attachment(key);
                    return Some(ControlResponse::error_with(
                        ControlErrorCode::Unavailable,
                        "the session is busy storing other files; retry",
                    ));
                }
                if let Some(pending) = self.attachments.pending.get_mut(&key) {
                    pending.purpose = Purpose::Keep {
                        reply: Some((client, reply)),
                    };
                }
                None
            }
            Err((code, message)) => {
                if chunk.offset > 0 || code != ControlErrorCode::Conflict {
                    self.cancel_attachment(key);
                }
                Some(ControlResponse::error_with(code, message))
            }
        }
    }

    /// Check a chunk against its upload, starting the upload at offset 0, and say which disk
    /// steps it takes.
    fn queue_control_chunk(
        &mut self,
        key: Key,
        chunk: AgentAttachmentChunk<'_>,
    ) -> Result<Vec<AttachmentOperation>, (ControlErrorCode, String)> {
        let invalid = |message: String| (ControlErrorCode::InvalidArgument, message);
        let mut operations = Vec::new();
        if chunk.offset == 0 {
            if self.attachments.pending.contains_key(&key)
                || self.attachments.unclaimed.contains_key(&chunk.transfer)
            {
                return Err((
                    ControlErrorCode::Conflict,
                    format!("upload {} is already in progress", chunk.transfer),
                ));
            }
            if chunk.name.is_empty() || chunk.name.len() > 1024 || chunk.size > MAX_ATTACHMENT_BYTES
            {
                return Err(invalid(
                    "a file needs a name and may be up to 16 MiB".into(),
                ));
            }
            if !self.attachments.can_reserve(CONTROL_UPLOADS, chunk.size) {
                return Err((
                    ControlErrorCode::Conflict,
                    "the session's file storage or upload limit is reached".into(),
                ));
            }
            if self.attachments.worker.is_none() {
                let worker = Worker::new()
                    .map_err(|err| (ControlErrorCode::Unavailable, err.to_string()))?;
                self.attachments.worker = Some(worker);
            }
            self.attachments.pending.insert(
                key,
                Pending {
                    purpose: Purpose::Keep { reply: None },
                    size: chunk.size,
                    received: 0,
                    busy: false,
                    updated: Instant::now(),
                },
            );
            operations.push(AttachmentOperation::Begin {
                pane_id: 0,
                generation: 0,
                local: false,
                name: chunk.name.to_string(),
                size: chunk.size,
            });
        }
        let pending = self.attachments.pending.get_mut(&key).ok_or_else(|| {
            invalid(format!(
                "no upload {} in progress; start it at offset 0",
                chunk.transfer
            ))
        })?;
        if pending.busy {
            return Err(invalid("wait for the previous chunk's answer".into()));
        }
        if chunk.offset != pending.received || chunk.size != pending.size {
            return Err(invalid(format!(
                "this chunk starts at byte {} of {}, but the session holds {} of {}",
                chunk.offset, chunk.size, pending.received, pending.size
            )));
        }
        if !chunk.data.is_empty() {
            pending.received += checked_chunk_size_up_to(
                chunk.data,
                pending.size - pending.received,
                crate::control::AGENT_ATTACH_CHUNK_BYTES,
            )
            .map_err(invalid)?;
            operations.push(AttachmentOperation::Chunk {
                data: chunk.data.to_string(),
            });
        }
        if chunk.last {
            if pending.received != pending.size {
                return Err(invalid("the file is incomplete".into()));
            }
            operations.push(AttachmentOperation::Finish);
        }
        if operations.is_empty() {
            return Err(invalid("a chunk needs data or `last`".into()));
        }
        pending.busy = true;
        pending.updated = Instant::now();
        Ok(operations)
    }

    /// `prompt` with the paths of the uploads a request claims listed after it. Refused unless
    /// every one has finished and no request has claimed it yet.
    pub(super) fn attached_prompt(
        &self,
        mut prompt: String,
        transfers: &[u64],
    ) -> Result<String, ControlResponse> {
        if transfers.is_empty() {
            return Ok(prompt);
        }
        prompt.push_str("\n\nAttached files:");
        for transfer in transfers {
            let (file, _) = self.attachments.unclaimed.get(transfer).ok_or_else(|| {
                ControlResponse::error_with(
                    ControlErrorCode::InvalidArgument,
                    format!(
                        "upload {transfer} is not finished, was already used, or expired; attach the file again"
                    ),
                )
            })?;
            prompt.push_str("\n- ");
            prompt.push_str(file.path.to_str().unwrap_or_default());
        }
        Ok(prompt)
    }

    /// Keep the uploads a request was asked with, until the session stops.
    pub(super) fn claim_attachments(&mut self, transfers: &[u64]) {
        for transfer in transfers {
            if let Some((file, _)) = self.attachments.unclaimed.remove(transfer) {
                self.attachments.retained.push(file);
            }
        }
    }

    /// Delete uploads no request claimed, finished or still arriving.
    pub(super) fn cancel_agent_attachments(&mut self, transfers: &[u64]) -> ControlResponse {
        for transfer in transfers {
            self.attachments.unclaimed.remove(transfer);
            self.cancel_attachment((CONTROL_UPLOADS, *transfer));
        }
        ControlResponse::empty()
    }

    /// Upload `bytes` as `agent-attach` upload `transfer`, as a caller would, and return where
    /// the finished file waits to be claimed.
    #[cfg(all(test, unix))]
    pub(super) fn upload_for_test(&mut self, transfer: u64, bytes: &[u8]) -> PathBuf {
        let data = STANDARD.encode(bytes);
        let reply = ReplyTo::Headless {
            capabilities: protocol::Capabilities::current(),
            effective_protocol: protocol::PROTOCOL_VERSION,
        };
        let chunk = AgentAttachmentChunk {
            transfer,
            name: "file.txt",
            size: bytes.len() as u64,
            offset: 0,
            data: &data,
            last: true,
        };
        assert!(self.agent_attachment_chunk(0, reply, chunk).is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        while self
            .attachments
            .pending
            .contains_key(&(CONTROL_UPLOADS, transfer))
        {
            assert!(Instant::now() < deadline);
            self.poll_attachment_work();
            std::thread::yield_now();
        }
        self.attachments.unclaimed[&transfer].0.path.clone()
    }

    /// Uploads still arriving, waiting to be claimed, and kept, in that order.
    #[cfg(all(test, unix))]
    pub(super) fn attachment_counts(&self) -> (usize, usize, usize) {
        (
            self.attachments.pending.len(),
            self.attachments.unclaimed.len(),
            self.attachments.retained.len(),
        )
    }

    /// Whether `client` waits on a chunk of an `agent-attach` upload, which keeps a headless
    /// connection open until the chunk is written.
    pub(super) fn holds_attachment_reply(&self, client: ClientId) -> bool {
        self.attachments.pending.values().any(|pending| {
            matches!(
                &pending.purpose,
                Purpose::Keep { reply: Some((holder, _)) } if *holder == client
            )
        })
    }

    /// Answer the control caller waiting on a chunk the disk worker has written. The last chunk
    /// answers with where the session keeps the file.
    fn complete_control_chunk(&mut self, key: Key, result: Result<Option<StagedFile>, String>) {
        let Some(pending) = self.attachments.pending.get_mut(&key) else {
            return;
        };
        let reply = match &mut pending.purpose {
            Purpose::Keep { reply } => reply.take(),
            Purpose::Paste(_) => None,
        };
        pending.busy = false;
        pending.updated = Instant::now();
        let received = pending.received;
        let response = match result {
            Err(error) => {
                self.cancel_attachment(key);
                ControlResponse::error_with(ControlErrorCode::Unavailable, error)
            }
            Ok(None) => ControlResponse::ok(crate::control::AgentAttachmentUpload {
                received,
                path: None,
            }),
            Ok(Some(file)) => {
                self.attachments.pending.remove(&key);
                match file.path.to_str().map(str::to_string) {
                    Some(path) => {
                        self.attachments
                            .unclaimed
                            .insert(key.1, (file, Instant::now()));
                        ControlResponse::ok(crate::control::AgentAttachmentUpload {
                            received,
                            path: Some(path),
                        })
                    }
                    None => ControlResponse::error_with(
                        ControlErrorCode::Unavailable,
                        "the stored file's path is not UTF-8",
                    ),
                }
            }
        };
        if let Some((client, reply)) = reply {
            self.answer_held(client, &reply, response);
        }
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
                    .try_send(Job {
                        key,
                        operations: vec![operation],
                    })
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
        let removed = self.attachments.pending.remove(&key);
        if let Some(worker) = &self.attachments.worker {
            let _ = worker.jobs.try_send(Job {
                key,
                operations: vec![AttachmentOperation::Cancel],
            });
        }
        // A control caller still waiting on a chunk hears that its upload is gone.
        if let Some(Pending {
            purpose: Purpose::Keep {
                reply: Some((client, reply)),
            },
            ..
        }) = removed
        {
            self.answer_held(
                client,
                &reply,
                ControlResponse::error_with(ControlErrorCode::Unavailable, "upload cancelled"),
            );
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
        // Dropping an unclaimed file deletes it.
        self.attachments
            .unclaimed
            .retain(|_, (_, finished)| finished.elapsed() < UNCLAIMED_TIMEOUT);
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
            if done.key.0 == CONTROL_UPLOADS {
                self.complete_control_chunk(done.key, done.result);
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
        let Purpose::Paste(destination) = pending.purpose else {
            return Err("upload not found".into());
        };
        let Some(file) = file else {
            return Ok(());
        };
        self.attachments.pending.remove(&key);
        if !self.attachment_destination_valid(key.0, destination) {
            return Err(
                "pane changed, input is blocked, or agent history is being read; retry".into(),
            );
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

    #[test]
    fn a_control_upload_continues_across_connections_and_answers_with_its_path() {
        use super::super::tests::add_client;
        let mut server = SessionServer::new_named("agent-attachments");
        let reply = ReplyTo::Headless {
            capabilities: protocol::Capabilities::current(),
            effective_protocol: protocol::PROTOCOL_VERSION,
        };
        fn chunk(offset: u64, data: &str, last: bool) -> AgentAttachmentChunk<'_> {
            AgentAttachmentChunk {
                transfer: 9,
                name: "notes.txt",
                size: 5,
                offset,
                data,
                last,
            }
        }
        let (hel, o, lo) = (
            STANDARD.encode(b"hel"),
            STANDARD.encode(b"o"),
            STANDARD.encode(b"lo"),
        );
        // Each chunk comes over a connection of its own, as one `--remote` call per chunk does,
        // and that connection stays open until the chunk is written.
        let answer = |server: &mut SessionServer, chunk: AgentAttachmentChunk<'_>| {
            let (client, mut stream) = add_client(server);
            server.process_client_frame(
                client,
                protocol::Frame::Control(ClientMessage::SessionControl {
                    capabilities: Some(protocol::Capabilities::current()),
                    session: "agent-attachments".into(),
                    protocol_version: protocol::PROTOCOL_VERSION,
                    min_protocol_version: protocol::MIN_SUPPORTED_PROTOCOL,
                    request: crate::control::ControlRequest {
                        command: crate::control::ControlCommand::AgentAttach {
                            transfer: chunk.transfer,
                            name: chunk.name.into(),
                            size: chunk.size,
                            offset: chunk.offset,
                            data: chunk.data.into(),
                            last: chunk.last,
                        },
                        source_pane: None,
                        source_session: None,
                        extension: None,
                        caller_context: None,
                        credential: None,
                    },
                }),
            );
            // The server loop flushes before the disk worker is done.
            server.flush_clients();
            let deadline = Instant::now() + Duration::from_secs(5);
            while server
                .attachments
                .pending
                .get(&(CONTROL_UPLOADS, 9))
                .is_some_and(|pending| pending.busy)
            {
                assert!(Instant::now() < deadline);
                server.poll_attachment_work();
                std::thread::yield_now();
            }
            server.flush_clients();
            let ServerMessage::SessionControlResult { response, .. } =
                protocol::read_frame(&mut stream).unwrap()
            else {
                panic!("a control result")
            };
            serde_json::from_value::<crate::control::AgentAttachmentUpload>(response.data.unwrap())
                .unwrap()
        };
        assert_eq!(answer(&mut server, chunk(0, &hel, false)).received, 3);
        // A chunk that does not continue the file is refused, not guessed at.
        let (client, _stream) = add_client(&mut server);
        let gap = server.agent_attachment_chunk(client, reply.clone(), chunk(4, &o, true));
        assert_eq!(
            gap.and_then(|refusal| refusal.code),
            Some(ControlErrorCode::InvalidArgument)
        );
        assert_eq!(
            answer(&mut server, chunk(0, &hel, false)).received,
            3,
            "the refused chunk dropped the upload, so it starts again"
        );
        let done = answer(&mut server, chunk(3, &lo, true));
        let path = PathBuf::from(done.path.expect("the last chunk names the file"));
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert!(server.attachments.pending.is_empty());
        drop(server);
        assert!(!path.exists());
    }
}
