//! Parser-established output transactions and bounded latest-frame staging.
use super::*;
use crate::runtime_metrics::GraphicsSessionMetrics;
use tui_lipan::{GraphicsDependency, TerminalOutputClass, TerminalOutputTransaction};

fn output_class(class: TerminalOutputClass) -> protocol::OutputClass {
    match class {
        TerminalOutputClass::Ordered => protocol::OutputClass::Ordered,
        TerminalOutputClass::Graphics {
            source,
            revision,
            dependency,
        } => match dependency {
            GraphicsDependency::Independent => protocol::OutputClass::IndependentGraphics {
                image: source.producer_image_id,
                incarnation: source.incarnation,
                revision: revision.0,
            },
            GraphicsDependency::DependsOnPrevious => protocol::OutputClass::DependentGraphics {
                image: source.producer_image_id,
                incarnation: source.incarnation,
                revision: revision.0,
            },
        },
    }
}

#[derive(Default)]
struct OutputStaging {
    output: Vec<ServerOutbound>,
    replacements: HashMap<ReplacementKey, usize>,
    metrics: GraphicsSessionMetrics,
}

impl OutputStaging {
    fn push(
        &mut self,
        pane_id: PaneId,
        local: bool,
        generation: u64,
        metadata: protocol::OutputMetadata,
        bytes: Vec<u8>,
    ) {
        match metadata.class {
            protocol::OutputClass::IndependentGraphics { .. } => {
                self.metrics.graphics_transactions += 1;
                self.metrics.replaceable_transactions += 1;
            }
            protocol::OutputClass::DependentGraphics { .. } => {
                self.metrics.graphics_transactions += 1;
                self.metrics.ordered_barriers += 1;
            }
            protocol::OutputClass::Ordered => self.metrics.ordered_barriers += 1,
        }
        let key = metadata.replacement_key(pane_id, local, generation);
        let output = ServerOutbound::PaneOutput {
            pane_id,
            local,
            generation,
            metadata,
            bytes,
        };
        if let Some(index) = key.and_then(|key| self.replacements.get(&key).copied()) {
            if let ServerOutbound::PaneOutput { bytes, .. } = &self.output[index] {
                self.metrics.staged_coalesced_frames += 1;
                self.metrics.staged_coalesced_bytes += bytes.len() as u64;
            }
            self.output[index] = output;
        } else {
            if let Some(key) = key {
                self.replacements.insert(key, self.output.len());
            }
            self.output.push(output);
        }
    }
}

impl ServerPane {
    fn recorded_output(&mut self, bytes: &[u8]) -> Vec<TerminalOutputTransaction> {
        let transactions = self.screen_without_change().take_output_transactions();
        if !transactions.is_empty()
            || bytes.is_empty()
            || self.screen_without_change().output_transaction_pending()
        {
            return transactions;
        }
        // Internal/test screens which have not opted into recording remain conservatively ordered.
        vec![TerminalOutputTransaction {
            class: TerminalOutputClass::Ordered,
            bytes: bytes.to_vec(),
        }]
    }

    fn next_output_metadata(&mut self, class: protocol::OutputClass) -> protocol::OutputMetadata {
        let independent = matches!(class, protocol::OutputClass::IndependentGraphics { .. });
        if !independent {
            self.output_coalesce_epoch = self.output_coalesce_epoch.saturating_add(1);
        }
        self.output_sequence = self.output_sequence.saturating_add(1);
        // Every sequence is timed, so a presenter's oldest unacknowledged output has a known age.
        self.output_times
            .push_back((self.output_sequence, Instant::now()));
        if self.output_times.len() > 32 {
            self.output_times.pop_front();
        }
        protocol::OutputMetadata {
            sequence: self.output_sequence,
            coalesce_epoch: self.output_coalesce_epoch,
            class,
        }
    }
}

impl SessionServer {
    pub(super) fn send_parsed_output(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
        bytes: &[u8],
    ) {
        let Some(pane) = self
            .pane_mut(owner, pane_id)
            .filter(|pane| pane.generation == generation)
        else {
            return;
        };
        let mut transactions = pane.recorded_output(bytes);
        let completed = !transactions.is_empty();
        let continuation = !completed && pane.screen_without_change().output_transaction_pending();
        if self
            .media
            .stage_output((owner, pane_id, generation), &mut transactions)
        {
            return;
        }
        self.deliver_output_transactions(owner, pane_id, generation, transactions, continuation);
    }

    pub(super) fn deliver_output_transactions(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
        transactions: Vec<TerminalOutputTransaction>,
        continuation: bool,
    ) {
        let Some(pane) = self
            .pane_mut(owner, pane_id)
            .filter(|pane| pane.generation == generation)
        else {
            return;
        };
        let mut staging = OutputStaging::default();
        let now = Instant::now();
        pane.graphics_pacer.read_completed(continuation);
        for transaction in transactions {
            let metadata = pane.next_output_metadata(output_class(transaction.class));
            pane.graphics_pacer
                .observe(now, metadata.sequence, metadata.class);
            staging.push(
                pane_id,
                wire_local(owner),
                generation,
                metadata,
                transaction.bytes,
            );
        }
        self.graphics_metrics.graphics_transactions += staging.metrics.graphics_transactions;
        self.graphics_metrics.replaceable_transactions += staging.metrics.replaceable_transactions;
        self.graphics_metrics.staged_coalesced_frames += staging.metrics.staged_coalesced_frames;
        self.graphics_metrics.staged_coalesced_bytes += staging.metrics.staged_coalesced_bytes;
        self.graphics_metrics.ordered_barriers += staging.metrics.ordered_barriers;
        for output in staging.output {
            self.send_outbound(owner, &output);
        }
    }
}

/// A visible client whose oldest unacknowledged output is older than this is not presenting.
/// It exceeds the pacer's safety hold, so a client only loses the lead after the pacer has
/// already had to release without its acknowledgement.
const STALLED_PRESENTER: Duration = Duration::from_millis(250);

impl ServerPane {
    /// When the oldest output a client has not acknowledged was produced, if any is pending.
    fn oldest_unacknowledged(&self, presented: u64, acked_at: Option<Instant>) -> Option<Instant> {
        if presented >= self.output_sequence {
            return None;
        }
        match self
            .output_times
            .iter()
            .find(|(sequence, _)| *sequence > presented)
        {
            Some((sequence, at)) if *sequence == presented + 1 => Some(*at),
            // It fell behind the timed window, and has waited at least since its last advance.
            oldest => acked_at.or(oldest.map(|(_, at)| *at)),
        }
    }
}

impl SessionServer {
    pub(super) fn graphics_presenter(
        &self,
        pane_id: PaneId,
        generation: u64,
        owner: Option<ClientId>,
    ) -> Option<super::graphics_pacer::Presenter> {
        let key = PresentationKey {
            pane_id,
            generation,
            local: owner.is_some(),
        };
        let pane = self.pane(owner, pane_id)?;
        let now = Instant::now();
        self.clients
            .iter()
            .filter_map(|client| {
                if !client.attached
                    || client.parked
                    || client.close_after_flush
                    || owner.is_some_and(|owner| owner != client.id)
                {
                    return None;
                }
                let interest = client.presentation_interest.get(&key)?;
                if !interest.visible {
                    return None;
                }
                let priority = if interest.focused && self.controller == Some(client.id) {
                    3
                } else if !client.read_only {
                    2
                } else {
                    1
                };
                let presented = client.presented_sequences.get(&key).copied().unwrap_or(0);
                // A stopped or wedged client would hold the pane for its safety timeout on every
                // frame; any responsive visible client leads before it.
                let responsive = pane
                    .oldest_unacknowledged(presented, client.presented_at.get(&key).copied())
                    .is_none_or(|since| now.saturating_duration_since(since) < STALLED_PRESENTER);
                Some((
                    responsive,
                    priority,
                    interest.frames_per_second,
                    presented,
                    client.id,
                ))
            })
            .max()
            .map(
                |(_, _, frames_per_second, presented, client)| super::graphics_pacer::Presenter {
                    client,
                    frames_per_second: frames_per_second.clamp(1, 1000),
                    presented,
                },
            )
    }

    pub(super) fn update_graphics_pacing(&mut self) {
        let now = Instant::now();
        let presenters: HashMap<_, _> = self
            .panes
            .iter()
            .map(|(&id, pane)| {
                (
                    id,
                    self.graphics_presenter(id, pane.generation, None)
                        .or_else(|| self.recording_presenter(id, pane.generation)),
                )
            })
            .collect();
        let local_presenters: HashMap<_, _> = self
            .local_panes
            .iter()
            .map(|(&(owner, id), pane)| {
                (
                    (owner, id),
                    self.graphics_presenter(id, pane.generation, Some(owner)),
                )
            })
            .collect();
        for (&id, pane) in &mut self.panes {
            if let Some(pty) = &pane.pty {
                let policy = pane
                    .graphics_pacer
                    .policy(now, presenters.get(&id).copied().flatten());
                super::graphics_pacer::apply_policy(&pty.read_gate(), policy);
            }
        }
        for (&key, pane) in &mut self.local_panes {
            if let Some(pty) = &pane.pty {
                let policy = pane
                    .graphics_pacer
                    .policy(now, local_presenters.get(&key).copied().flatten());
                super::graphics_pacer::apply_policy(&pty.read_gate(), policy);
            }
        }
    }

    pub(super) fn graphics_pacing_timeout(&self, timeout: Duration) -> Duration {
        let now = Instant::now();
        self.panes
            .values()
            .chain(self.local_panes.values())
            .filter(|pane| pane.pty.is_some() && pane.exited.is_none())
            .filter_map(|pane| pane.graphics_pacer.wake_after(now))
            .fold(timeout, Duration::min)
    }
}

impl SessionServer {
    pub(super) fn graphics_pacing_metrics(&self) -> GraphicsSessionMetrics {
        let mut metrics = self.graphics_metrics;
        for pane in self.panes.values().chain(self.local_panes.values()) {
            if let Some(pty) = &pane.pty {
                let stats = pty.read_gate().stats();
                metrics.pacer_holds += stats.holds;
                metrics.pacer_hold_us +=
                    crate::runtime_metrics::duration_micros(stats.total_hold_time);
                metrics.pacer_forced_releases += stats.forced_releases;
            }
            metrics.frames_ahead_of_presenter = metrics
                .frames_ahead_of_presenter
                .max(pane.graphics_pacer.frames_ahead() as u64);
        }
        metrics
    }
}
