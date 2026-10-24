//! One bounded media worker, with ingress entries retained until parser commit.
use super::*;
use tui_lipan::{TerminalMediaWork, TerminalPreparedInput};

type MediaKey = (Option<ClientId>, PaneId, u64);

type MediaSplit = (ServerEvent, Option<(ServerEvent, usize)>);

#[derive(Default)]
pub(super) struct MediaPipeline {
    worker: Option<MediaWorker>,
    active: Option<MediaKey>,
    ready: Option<TerminalPreparedInput>,
    stage_key: Option<MediaKey>,
    output: Option<PendingMediaOutput>,
    client_barriers: HashMap<ClientId, usize>,
}

const MEDIA_OUTPUT_BYTES_PER_TICK: usize = 256 * 1024;

struct PendingMediaOutput {
    transactions: VecDeque<tui_lipan::TerminalOutputTransaction>,
    offset: usize,
}

impl PendingMediaOutput {
    fn take_batch(&mut self) -> Vec<tui_lipan::TerminalOutputTransaction> {
        let mut batch = Vec::new();
        let mut budget = MEDIA_OUTPUT_BYTES_PER_TICK;
        while budget > 0 {
            let Some(front) = self.transactions.front() else {
                break;
            };
            let remaining = front.bytes.len() - self.offset;
            // A graphics replacement stays atomic. Splitting its independence metadata would
            // allow a downstream coalescer to discard pieces of the same transmission.
            let length = if front.class == tui_lipan::TerminalOutputClass::Ordered {
                remaining.min(budget)
            } else {
                remaining
            };
            let transaction = if self.offset == 0 && length == remaining {
                self.transactions.pop_front().expect("pending output")
            } else {
                let transaction = tui_lipan::TerminalOutputTransaction {
                    class: front.class,
                    bytes: front.bytes[self.offset..self.offset + length].to_vec(),
                };
                self.offset += length;
                if self.offset == front.bytes.len() {
                    self.transactions.pop_front();
                    self.offset = 0;
                }
                transaction
            };
            budget = budget.saturating_sub(length);
            batch.push(transaction);
        }
        batch
    }
}

struct MediaWorker {
    jobs: Option<mpsc::SyncSender<TerminalMediaWork>>,
    completed: mpsc::Receiver<TerminalPreparedInput>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MediaWorker {
    fn new() -> io::Result<Self> {
        let (jobs, incoming) = mpsc::sync_channel::<TerminalMediaWork>(1);
        let (completed, results) = mpsc::sync_channel(1);
        let handle = std::thread::Builder::new()
            .name("rozi-image-media".into())
            .spawn(move || {
                for work in incoming {
                    if completed.send(work.run()).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            completed: results,
            handle: Some(handle),
        })
    }
}

impl Drop for MediaWorker {
    fn drop(&mut self) {
        self.jobs.take();
        // At most one job is outstanding. Drain its bounded result before joining the worker.
        if let Some(handle) = self.handle.take() {
            while !handle.is_finished() {
                let _ = self.completed.recv_timeout(Duration::from_millis(10));
            }
            let _ = handle.join();
        }
    }
}

impl MediaPipeline {
    pub(super) fn client_fenced(&self, client_id: ClientId) -> bool {
        self.client_barriers.contains_key(&client_id)
    }

    pub(super) fn pending(&self) -> bool {
        self.active.is_some()
    }

    fn start(&mut self, key: MediaKey, work: TerminalMediaWork) -> bool {
        if self.active.is_some() {
            return false;
        }
        if self.worker.is_none() {
            match MediaWorker::new() {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    eprintln!("rozi: cannot start media worker: {error}");
                    return true;
                }
            }
        }
        let worker = self.worker.as_ref().expect("media worker installed");
        if worker
            .jobs
            .as_ref()
            .expect("live media sender")
            .try_send(work)
            .is_err()
        {
            return true;
        }
        self.active = Some(key);
        false
    }

    fn ready_length(&self, key: MediaKey) -> Option<usize> {
        (self.active == Some(key))
            .then(|| {
                self.ready
                    .as_ref()
                    .map(TerminalPreparedInput::consumed_bytes)
            })
            .flatten()
    }

    pub(super) fn take_ready(&mut self, key: MediaKey) -> Option<TerminalPreparedInput> {
        if self.active != Some(key) {
            return None;
        }
        let ready = self.ready.take()?;
        self.stage_key = Some(key);
        Some(ready)
    }

    pub(super) fn blocks_replay(&self, key: PaneSeedKey) -> bool {
        self.output.is_some() && self.active == Some((None, key.pane_id, key.generation))
    }

    pub(super) fn stage_output(
        &mut self,
        key: MediaKey,
        transactions: &mut Vec<tui_lipan::TerminalOutputTransaction>,
    ) -> bool {
        if self.stage_key != Some(key) {
            return false;
        }
        self.stage_key = None;
        if transactions.is_empty() {
            self.active = None;
        } else {
            self.output = Some(PendingMediaOutput {
                transactions: std::mem::take(transactions).into(),
                offset: 0,
            });
        }
        true
    }

    pub(super) fn discard_completed(&mut self, key: MediaKey) {
        if self.active == Some(key)
            && (self.ready.is_some() || self.output.is_some() || self.stage_key.is_some())
        {
            self.ready = None;
            self.output = None;
            self.stage_key = None;
            self.active = None;
        }
    }

    fn output_batch(
        &mut self,
    ) -> Option<(MediaKey, Vec<tui_lipan::TerminalOutputTransaction>, bool)> {
        let output = self.output.as_mut()?;
        let key = self.active.expect("pending media output has an owner");
        let batch = output.take_batch();
        let continuation = !output.transactions.is_empty();
        if !continuation {
            self.output = None;
            self.active = None;
        }
        Some((key, batch, continuation))
    }

    fn poll(&mut self, relay: tui_lipan::GraphicsRelayPolicy) {
        if self.output.is_some() || self.stage_key.is_some() {
            return;
        }
        let Some(worker) = &self.worker else {
            return;
        };
        if self.ready.is_none() {
            self.ready = worker.completed.try_recv().ok();
        }
        if self
            .ready
            .as_ref()
            .is_some_and(|ready| ready.relay_policy() != relay)
        {
            let work = self
                .ready
                .take()
                .expect("ready media")
                .with_relay_policy(relay);
            // The sole outstanding job just completed, so this bounded slot is available.
            if let Err(error) = worker
                .jobs
                .as_ref()
                .expect("live media sender")
                .try_send(work)
            {
                let work = match error {
                    mpsc::TrySendError::Full(work) | mpsc::TrySendError::Disconnected(work) => work,
                };
                self.ready = Some(work.run());
            }
        }
    }
}

impl SessionServer {
    pub(super) fn poll_media(&mut self) {
        let relay = self.image_media().relay;
        self.media.poll(relay);
        if let Some(key) = self.media.active
            && self
                .pane(key.0, key.1)
                .is_none_or(|pane| pane.generation != key.2)
        {
            self.media.discard_completed(key);
        }
    }

    pub(super) fn pump_media_output(&mut self) -> bool {
        let Some((key, transactions, continuation)) = self.media.output_batch() else {
            return false;
        };
        self.deliver_output_transactions(key.0, key.1, key.2, transactions, continuation);
        true
    }

    pub(super) fn finish_pending_media(&mut self) {
        while self.media.pending() {
            self.poll_media();
            self.pump_media_output();
            if !self.media.pending() {
                break;
            }
            if let Some(event) = self.next_ready_event() {
                if let Some(outbound) = self.handle_event(event) {
                    self.send_outbound(None, &outbound);
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    pub(super) fn next_ready_event(&mut self) -> Option<ServerEvent> {
        self.next_ready_event_phase(true)
    }

    pub(super) fn next_ready_event_phase(&mut self, commit_media: bool) -> Option<ServerEvent> {
        let events = Arc::clone(&self.events);
        let mut blocked = HashSet::new();
        let mut blocked_clients = HashSet::new();
        let selected_prefix = std::cell::Cell::new(None);
        events.try_pop_ready(
            |event| {
                if let ServerEvent::MediaBarrier { client_id, message } = event {
                    if blocked_clients.contains(client_id) {
                        return false;
                    }
                    if let Some(key) = self.media_barrier_key(*client_id, message) {
                        blocked_clients.insert(*client_id);
                        blocked.insert(key);
                        return false;
                    }
                    return true;
                }
                self.media_event_ready(event, &mut blocked, &selected_prefix, commit_media)
            },
            |event| split_media_event(event, selected_prefix.get()),
        )
    }

    fn media_event_ready(
        &mut self,
        event: &ServerEvent,
        blocked: &mut HashSet<MediaKey>,
        selected_prefix: &std::cell::Cell<Option<usize>>,
        commit_media: bool,
    ) -> bool {
        let ServerEvent::Pty(owner, id, generation, event) = event else {
            return true;
        };
        let key = (*owner, *id, *generation);
        if blocked.contains(&key) {
            return false;
        }
        if let Some(length) = self.media.ready_length(key) {
            if commit_media {
                selected_prefix.set(Some(length));
                return true;
            }
            blocked.insert(key);
            return false;
        }
        if self.media.active == Some(key) {
            blocked.insert(key);
            return false;
        }
        let ready = self.prepare_event_media(key, event);
        if !ready {
            blocked.insert(key);
        }
        ready
    }

    fn prepare_event_media(&mut self, key: MediaKey, event: &TerminalPtyEvent) -> bool {
        let TerminalPtyEvent::Output(bytes) = event else {
            return true;
        };
        let Some(pane) = self.pane_mut(key.0, key.1) else {
            return true;
        };
        if pane.generation != key.2 {
            return true;
        }
        let work = pane
            .screen_without_change()
            .prepare_media_input(Arc::clone(bytes));
        let Some(work) = work else {
            return true;
        };
        self.media.start(key, work)
    }

    /// Parser mutations wait only for the exact screen being prepared/emitted. Layout commits
    /// publish metadata without touching screens; a new pane also has no media dependency.
    /// Spawning over an existing pane must wait for that pane's current generation instead.
    fn media_barrier_key(&self, client_id: ClientId, message: &ClientMessage) -> Option<MediaKey> {
        if !self.client_attached(client_id) {
            return None;
        }
        let active = self.media.active?;
        let key = match message {
            ClientMessage::Resize {
                pane_id,
                local,
                generation,
                ..
            }
            | ClientMessage::SetPalette {
                pane_id,
                local,
                generation,
                ..
            }
            | ClientMessage::ConfigurePane {
                pane_id,
                local,
                generation,
                ..
            } => (local.then_some(client_id), *pane_id, *generation),
            ClientMessage::SpawnPane { pane_id, local, .. } => {
                let owner = local.then_some(client_id);
                (owner, *pane_id, self.pane(owner, *pane_id)?.generation)
            }
            _ => return None,
        };
        (key == active).then_some(key)
    }

    pub(super) fn release_media_barrier(&mut self, client_id: ClientId) {
        if let Some(remaining) = self.media.client_barriers.get_mut(&client_id) {
            *remaining -= 1;
            if *remaining == 0 {
                self.media.client_barriers.remove(&client_id);
            }
        }
        if !self.media.client_fenced(client_id)
            && self
                .client_mut(client_id)
                .is_some_and(|client| client.transport_closed)
        {
            self.remove_client(client_id);
        }
    }

    pub(super) fn forget_media_barriers(&mut self, client_id: ClientId) {
        self.media.client_barriers.remove(&client_id);
    }

    pub(super) fn defer_media_barrier(
        &mut self,
        client_id: ClientId,
        message: ClientMessage,
    ) -> std::result::Result<ClientMessage, Vec<(Target, ServerMessage)>> {
        if !self.media.client_barriers.contains_key(&client_id)
            && self.media_barrier_key(client_id, &message).is_none()
        {
            return Ok(message);
        }
        let bytes = serde_json::to_vec(&message).map_or(MAX_PTY_INGRESS_BYTES, |bytes| bytes.len());
        let event = ServerEvent::MediaBarrier {
            client_id,
            message: Box::new(message),
        };
        let responses = if self.events.try_push(event, bytes).is_ok() {
            *self.media.client_barriers.entry(client_id).or_default() += 1;
            Vec::new()
        } else {
            vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "media-busy".into(),
                    message: "terminal input is full; retry the change after image preparation"
                        .into(),
                },
            )]
        };
        Err(responses)
    }
}

fn split_media_event(event: ServerEvent, prefix: Option<usize>) -> MediaSplit {
    let Some(prefix) = prefix else {
        return (event, None);
    };
    let ServerEvent::Pty(owner, id, generation, TerminalPtyEvent::Output(bytes)) = event else {
        return (event, None);
    };
    if prefix >= bytes.len() {
        return (
            ServerEvent::Pty(owner, id, generation, TerminalPtyEvent::Output(bytes)),
            None,
        );
    }
    let tail: Arc<[u8]> = bytes[prefix..].into();
    let tail_len = tail.len();
    (
        ServerEvent::Pty(
            owner,
            id,
            generation,
            TerminalPtyEvent::Output(bytes[..prefix].into()),
        ),
        Some((
            ServerEvent::Pty(owner, id, generation, TerminalPtyEvent::Output(tail)),
            tail_len,
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn worker_consumes_a_source_once_and_relays_owned_pixels_to_readers_and_replay() {
        let server =
            SessionServer::new_named_with_settings("media-worker", ServerSettings::default());
        let policy = server.image_media();
        let mut source = TerminalScreen::new(5, 20, 100);
        source.set_output_transaction_recording(true);
        policy.apply(&mut source);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tty-graphics-protocol-worker");
        let pixel = [255, 0, 0, 255];
        fs::write(&path, pixel).unwrap();
        let name =
            base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_encoded_bytes());
        let bytes: Arc<[u8]> = format!("\x1b_Ga=T,t=t,f=32,s=1,v=1,i=7,p=1,C=1,q=2;{name}\x1b\\")
            .into_bytes()
            .into();
        let work = source
            .prepare_media_input(bytes)
            .expect("named media preparation");
        let key = (None, 7, 1);
        let mut pipeline = MediaPipeline::default();
        assert!(!pipeline.start(key, work));
        let deadline = Instant::now() + Duration::from_secs(5);
        while pipeline.ready_length(key).is_none() {
            pipeline.poll(policy.relay);
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::yield_now();
        }
        source
            .process_prepared_media_input(pipeline.take_ready(key).unwrap())
            .unwrap();
        assert!(!path.exists(), "the worker must consume the source");
        let mut transactions = source.take_output_transactions();
        assert!(pipeline.stage_output(key, &mut transactions));
        let mut wire = Vec::new();
        while let Some((owner, batch, _)) = pipeline.output_batch() {
            assert_eq!(owner, key);
            for transaction in batch {
                wire.extend_from_slice(&transaction.bytes);
            }
        }
        assert!(!wire.windows(3).any(|medium| medium == b"t=t"));
        // Readers cannot acquire any source, and both must receive the original owned pixels.
        for _ in 0..2 {
            let mut reader = TerminalScreen::new(5, 20, 100);
            reader.set_image_media_policy(GraphicsMediaPolicy::NONE);
            reader.process_bytes(&wire);
            assert_eq!(&reader.capture_frame().images[0].rgba[..4], &pixel);
        }
        // Reusing the consumed name must not let a replay read or delete the new source.
        let replacement = [0, 255, 0, 255];
        fs::write(&path, replacement).unwrap();
        let replay = source.export_replay_bytes();
        let mut late = TerminalScreen::new(5, 20, 100);
        late.set_image_media_policy(GraphicsMediaPolicy::NONE);
        late.process_bytes(&replay);
        assert_eq!(&late.capture_frame().images[0].rgba[..4], &pixel);
        assert_eq!(fs::read(&path).unwrap(), replacement);
    }
}
