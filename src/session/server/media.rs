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
        let mut barrier_blocked = false;
        let selected_prefix = std::cell::Cell::new(None);
        events.try_pop_ready(
            |event| {
                if barrier_blocked {
                    return false;
                }
                if matches!(event, ServerEvent::MediaBarrier { .. }) {
                    barrier_blocked = self.media.pending();
                    return !barrier_blocked;
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

    pub(super) fn defer_media_barrier(
        &mut self,
        client_id: ClientId,
        message: ClientMessage,
    ) -> std::result::Result<ClientMessage, Vec<(Target, ServerMessage)>> {
        if !self.media.pending()
            || !matches!(
                message,
                ClientMessage::Resize { .. }
                    | ClientMessage::SetPalette { .. }
                    | ClientMessage::ConfigurePane { .. }
                    | ClientMessage::CommitLayout { .. }
                    | ClientMessage::SpawnPane { .. }
            )
        {
            return Ok(message);
        }
        let bytes = serde_json::to_vec(&message).map_or(MAX_PTY_INGRESS_BYTES, |bytes| bytes.len());
        let event = ServerEvent::MediaBarrier {
            client_id,
            message: Box::new(message),
        };
        let responses = if self.events.try_push(event, bytes).is_ok() {
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
