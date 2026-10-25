//! Connection-scoped acquisition watermarks. Acknowledgements permit unlinking, never reuse.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const CONNECTION_BYTES: usize = 128 * 1024 * 1024;
const CONNECTION_FILES: usize = 128;
const SERVER_BYTES: usize = 256 * 1024 * 1024;
const SERVER_FILES: usize = 1024;
static RETAINED_BYTES: AtomicUsize = AtomicUsize::new(0);
static RETAINED_FILES: AtomicUsize = AtomicUsize::new(0);

struct Obligation {
    scope: PresentationKey,
    sequence: u64,
    output: Option<Arc<EncodedOutput>>,
    bytes: usize,
    files: usize,
}
impl Drop for Obligation {
    fn drop(&mut self) {
        // Release accounting only after this obligation's owning handles are released.
        drop(self.output.take());
        RETAINED_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
        RETAINED_FILES.fetch_sub(self.files, Ordering::AcqRel);
    }
}

#[derive(Default)]
pub(super) struct Acquisitions {
    issued: HashMap<PresentationKey, u64>,
    acknowledged: HashMap<PresentationKey, u64>,
    pending: VecDeque<Obligation>,
    bytes: usize,
    files: usize,
}
impl Acquisitions {
    fn issue(&mut self, output: Arc<EncodedOutput>) -> Option<u64> {
        let scope = output.scope?;
        let bytes = output
            .media
            .iter()
            .map(|m| m.file.byte_len())
            .sum::<usize>();
        let files = output.media.len();
        if self.bytes.checked_add(bytes)? > CONNECTION_BYTES
            || self.files.checked_add(files)? > CONNECTION_FILES
        {
            return None;
        }
        if !self.issued.contains_key(&scope) && self.issued.len() >= 4096 {
            return None;
        }
        let next = self
            .issued
            .get(&scope)
            .copied()
            .unwrap_or(0)
            .checked_add(1)?;
        RETAINED_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bytes).filter(|n| *n <= SERVER_BYTES)
            })
            .ok()?;
        if RETAINED_FILES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(files).filter(|n| *n <= SERVER_FILES)
            })
            .is_err()
        {
            RETAINED_BYTES.fetch_sub(bytes, Ordering::AcqRel);
            return None;
        }
        self.issued.insert(scope, next);
        self.pending.push_back(Obligation {
            scope,
            sequence: next,
            output: Some(output),
            bytes,
            files,
        });
        self.bytes += bytes;
        self.files += files;
        Some(next)
    }

    pub(super) fn acknowledge(&mut self, acks: &[protocol::AcquisitionAck]) -> bool {
        for ack in acks {
            let scope = PresentationKey {
                local: ack.local,
                pane_id: ack.pane_id,
                generation: ack.generation,
            };
            let Some(issued) = self.issued.get(&scope) else {
                return false;
            };
            if ack.sequence == 0 || ack.sequence > *issued {
                return false;
            }
            let watermark = self.acknowledged.entry(scope).or_default();
            *watermark = (*watermark).max(ack.sequence);
        }
        true
    }

    fn reap_one(&mut self) -> bool {
        // A later pane must not be held behind an unacknowledged acquisition in another pane.
        let Some(index) = self.pending.iter().position(|o| {
            self.acknowledged
                .get(&o.scope)
                .is_some_and(|n| *n >= o.sequence)
        }) else {
            return false;
        };
        let obligation = self.pending.remove(index).expect("located obligation");
        self.bytes -= obligation.bytes;
        self.files -= obligation.files;
        drop(obligation);
        true
    }
}

pub(super) fn encode_output(
    pane_id: PaneId,
    generation: u64,
    local: bool,
    metadata: protocol::OutputMetadata,
    bytes: &[u8],
    media: &[tui_lipan::TerminalOutputMedia],
) -> Option<Arc<EncodedOutput>> {
    if media.is_empty() {
        return encode_pane_output(pane_id, generation, local, metadata, bytes);
    }
    let bindings = protocol::OutputMedia {
        acquisition_sequence: 1, // Replaced by the connection writer after coalescing.
        files: media
            .iter()
            .map(|m| protocol::ImmutableFileBinding {
                command_range: m.command_range.clone(),
                path: m.file.path().to_owned(),
                byte_len: m.file.byte_len(),
                format: m.file.format(),
            })
            .collect(),
        acquired: Vec::new(),
    };
    let mut frame = Vec::new();
    protocol::write_pane_output_media_frame(
        &mut frame,
        pane_id,
        generation,
        local,
        metadata,
        Some(&bindings),
        bytes,
    )
    .ok()?;
    Some(Arc::new(EncodedOutput {
        bytes: frame,
        media: media.to_vec(),
        scope: Some(PresentationKey {
            pane_id,
            generation,
            local,
        }),
    }))
}

impl ClientConn {
    pub(super) fn prepare_acquisition(&mut self) -> bool {
        let Some(front) = self.outbox.front() else {
            return true;
        };
        if front.bytes.media.is_empty() {
            return true;
        }
        if !self.shares_filesystem {
            return false;
        }
        let Some(sequence) = self.acquisitions.issue(Arc::clone(&front.bytes)) else {
            return false;
        };
        let front = self.outbox.front_mut().expect("front exists");
        let output = Arc::make_mut(&mut front.bytes);
        let start = protocol::PANE_OUTPUT_ACQUISITION_OFFSET;
        output.bytes[start..start + size_of::<u64>()].copy_from_slice(&sequence.to_be_bytes());
        // Obligation owns the resource handles; the serialized front no longer needs duplicates.
        output.media.clear();
        true
    }

    fn queued_media_within_budget(&self) -> bool {
        let outbox = self.outbox.iter().map(|f| &f.bytes);
        let catch_up = self
            .seed
            .iter()
            .flat_map(|s| s.catch_up.iter().map(|f| &f.bytes));
        let (mut bytes, mut files) = (self.acquisitions.bytes, self.acquisitions.files);
        for output in outbox.chain(catch_up) {
            for media in &output.media {
                bytes = bytes.saturating_add(media.file.byte_len());
                files += 1;
                if bytes > CONNECTION_BYTES || files > CONNECTION_FILES {
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn push_delta(
        &mut self,
        bytes: Arc<EncodedOutput>,
        delta: SeedDelta,
        default_cap: usize,
        replacement: Option<ReplacementKey>,
    ) -> ClientFrameResult {
        let carries_media = !bytes.media.is_empty();
        let result = self.push_delta_inner(bytes, delta, default_cap, replacement);
        if !carries_media || self.queued_media_within_budget() {
            result
        } else {
            ClientFrameResult::Overflow
        }
    }
}

/// Disconnected queues retain ownership and budget charges until bounded retirement runs.
#[derive(Default)]
pub(super) struct RetiredMedia {
    pending: VecDeque<Obligation>,
    outbox: VecDeque<OutboxFrame>,
    catch_up: VecDeque<CatchUpFrame>,
}
impl RetiredMedia {
    pub(super) fn take(client: &mut ClientConn) -> Self {
        Self {
            pending: std::mem::take(&mut client.acquisitions.pending),
            outbox: std::mem::take(&mut client.outbox),
            catch_up: client
                .seed
                .as_mut()
                .map(|s| std::mem::take(&mut s.catch_up))
                .unwrap_or_default(),
        }
    }
    fn reap_one(&mut self) -> bool {
        self.pending.pop_front().is_some()
            || self.outbox.pop_front().is_some()
            || self.catch_up.pop_front().is_some()
    }
}
impl SessionServer {
    pub(super) fn reap_acquisitions(&mut self) -> bool {
        let mut activity = false;
        let count = self.clients.len();
        for offset in 0..count {
            let index = (self.acquisition_reaper_cursor + offset) % count;
            if self.clients[index].acquisitions.reap_one() {
                self.acquisition_reaper_cursor = (index + 1) % count;
                activity = true;
                break;
            }
        }
        // One live obligation and one disconnected queue entry per turn, regardless of client
        // count. Keep reservations charged until these owning handles are actually destroyed.
        if let Some(retired) = self.retired_media.front_mut() {
            if retired.reap_one() {
                activity = true;
            } else {
                self.retired_media.pop_front();
                activity = true;
            }
        }
        activity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn test_output() -> Arc<EncodedOutput> {
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(source.path(), [1, 2, 3, 255]).unwrap();
        let name = base64::engine::general_purpose::STANDARD
            .encode(source.path().to_string_lossy().as_bytes());
        let mut screen = tui_lipan::prelude::TerminalScreen::new(5, 20, 100);
        screen.set_image_file_storage_enabled(true);
        screen.set_output_transaction_recording(true);
        screen.set_image_relay_policy(tui_lipan::GraphicsRelayPolicy::NamedAsFiles);
        screen.process_bytes(
            format!("\x1b_Ga=T,t=f,f=32,s=1,v=1,i=9,C=1,q=2;{name}\x1b\\").as_bytes(),
        );
        let transaction = screen.take_output_transactions().remove(0);
        assert_eq!(transaction.media.len(), 1);
        encode_output(
            7,
            2,
            false,
            protocol::OutputMetadata::default(),
            &transaction.bytes,
            &transaction.media,
        )
        .unwrap()
    }

    fn ack(sequence: u64) -> protocol::AcquisitionAck {
        protocol::AcquisitionAck {
            pane_id: 7,
            generation: 2,
            local: false,
            sequence,
        }
    }

    #[test]
    fn acquisition_watermark_owns_files_until_ack_and_bounded_retirement() {
        let output = test_output();
        let path = output.media[0].file.path().to_owned();
        let mut acquisitions = Acquisitions::default();
        assert_eq!(acquisitions.issue(Arc::clone(&output)), Some(1));
        assert_eq!(acquisitions.issue(output), Some(2));
        assert!(path.exists());
        assert!(!acquisitions.reap_one());
        assert!(!acquisitions.acknowledge(&[ack(3)]));
        assert!(acquisitions.acknowledge(&[ack(2)]));
        assert!(path.exists(), "ack is not synchronous reclamation");
        assert!(acquisitions.reap_one());
        assert_eq!(acquisitions.pending.len(), 1);
        assert!(path.exists());
        assert!(acquisitions.reap_one());
        assert!(!path.exists());
        assert_eq!(
            acquisitions.issue(test_output()),
            Some(3),
            "replay must not restart a generation counter"
        );
    }

    #[test]
    fn stopped_connection_hits_file_bound_while_another_can_acquire() {
        let output = test_output();
        let mut stopped = Acquisitions::default();
        for sequence in 1..=CONNECTION_FILES as u64 {
            assert_eq!(stopped.issue(Arc::clone(&output)), Some(sequence));
        }
        assert_eq!(stopped.issue(Arc::clone(&output)), None);
        let mut healthy = Acquisitions::default();
        assert_eq!(healthy.issue(output), Some(1));
        assert!(healthy.acknowledge(&[ack(1)]));
        assert!(healthy.reap_one());
        assert_eq!(stopped.pending.len(), CONNECTION_FILES);
    }

    #[test]
    fn acknowledgements_are_scoped_to_connection_namespace_and_generation() {
        let mut first = Acquisitions::default();
        let mut second = Acquisitions::default();
        first.issue(test_output()).unwrap();
        assert!(!second.acknowledge(&[ack(1)]));
        assert!(!first.acknowledge(&[protocol::AcquisitionAck {
            generation: 3,
            ..ack(1)
        }]));
        assert!(!first.acknowledge(&[protocol::AcquisitionAck {
            local: true,
            ..ack(1)
        }]));
        assert!(!first.acknowledge(&[ack(0)]));
        assert!(first.acknowledge(&[ack(1)]));
    }
}

#[cfg(all(test, unix))]
mod queue_tests {
    use super::*;
    use base64::Engine as _;

    fn transaction() -> tui_lipan::TerminalOutputTransaction {
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(source.path(), [1, 2, 3, 255]).unwrap();
        let name = base64::engine::general_purpose::STANDARD
            .encode(source.path().to_string_lossy().as_bytes());
        let mut screen = tui_lipan::prelude::TerminalScreen::new(5, 20, 100);
        screen.set_image_file_storage_enabled(true);
        screen.set_output_transaction_recording(true);
        screen.set_image_relay_policy(tui_lipan::GraphicsRelayPolicy::NamedAsFiles);
        screen.process_bytes(
            format!("\x1b_Ga=T,t=f,f=32,s=1,v=1,i=9,C=1,q=2;{name}\x1b\\").as_bytes(),
        );
        screen.take_output_transactions().remove(0)
    }

    #[test]
    fn coalescing_releases_only_the_discarded_files_before_issuing_sequences() {
        let mut server = SessionServer::new_named("acquisitions");
        let (id, _stream) = super::super::tests::attach_client(&mut server);
        let mut paths = Vec::new();
        for sequence in 1..=2 {
            let transaction = transaction();
            paths.push(transaction.media[0].file.path().to_owned());
            server.enqueue_outbound(
                id,
                &ServerOutbound::PaneOutput {
                    pane_id: 7,
                    generation: 2,
                    local: false,
                    metadata: protocol::OutputMetadata {
                        sequence,
                        coalesce_epoch: 0,
                        class: protocol::OutputClass::IndependentGraphics {
                            image: 9,
                            incarnation: 1,
                            revision: sequence,
                        },
                    },
                    bytes: transaction.bytes,
                    media: transaction.media,
                },
            );
        }
        assert!(!paths[0].exists());
        assert!(paths[1].exists());
        let client = server.client_mut(id).unwrap();
        assert!(client.acquisitions.issued.is_empty());
        assert_eq!(client.outbox.len(), 1);
        assert!(client.prepare_acquisition());
        assert_eq!(client.acquisitions.pending.len(), 1);
        assert_eq!(client.acquisitions.pending[0].sequence, 1);
        server.remove_client(id);
        assert!(paths[1].exists(), "disconnect defers destruction");
        while server.reap_acquisitions() {}
        assert!(!paths[1].exists());
    }

    #[test]
    fn orphan_envelope_owns_imported_pixels_after_server_output_is_released() {
        let transaction = transaction();
        let binding = &transaction.media[0];
        let file = tui_lipan::TerminalImmutableFile::acquire(binding.file.path(), 4, 32).unwrap();
        let imported_path = file.path().to_owned();
        let imported = tui_lipan::TerminalOutputMedia {
            command_range: binding.command_range.clone(),
            file,
        };
        let mut store = crate::state::OrphanOutputStore::default();
        assert!(store.insert_media(7, 2, &transaction.bytes, vec![imported]));
        drop(transaction);
        assert!(imported_path.exists());
        let (bytes, media) = store.take_owned_for_generation(7, 2).unwrap();
        let mut client = tui_lipan::prelude::TerminalScreen::new(5, 20, 100);
        client.set_image_file_storage_enabled(true);
        let prepared = client
            .prepare_immutable_media_input(Arc::from(bytes), media)
            .unwrap();
        client.process_prepared_media_input(prepared).unwrap();
        assert!(client.has_images());
        drop(client);
        assert!(!imported_path.exists());
    }
}
