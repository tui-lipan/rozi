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
        if independent {
            self.output_times
                .push_back((self.output_sequence, Instant::now()));
            if self.output_times.len() > 32 {
                self.output_times.pop_front();
            }
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
        let transactions = pane.recorded_output(bytes);
        let mut staging = OutputStaging::default();
        for transaction in transactions {
            let metadata = pane.next_output_metadata(output_class(transaction.class));
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
