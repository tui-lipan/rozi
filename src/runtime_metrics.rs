use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tui_lipan::DevToolsMetric;

use crate::state::{Attachment, ORPHAN_OUTPUT_GLOBAL_CAP, ORPHAN_OUTPUT_KEY_CAP};

/// A server sample older than three normal heartbeat intervals is stale.
pub const SERVER_METRICS_STALE_AFTER: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteBufferMetrics {
    pub current_bytes: u64,
    pub high_water_bytes: u64,
    pub capacity_bytes: u64,
}

impl ByteBufferMetrics {
    pub(crate) fn new(current: usize, high_water: usize, capacity: usize) -> Self {
        Self {
            current_bytes: current as u64,
            high_water_bytes: high_water as u64,
            capacity_bytes: capacity as u64,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueMetrics {
    #[serde(flatten)]
    pub bytes: ByteBufferMetrics,
    pub queued_items: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerOutboxMetrics {
    #[serde(flatten)]
    pub bytes: ByteBufferMetrics,
    pub clients: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanOutputMetrics {
    #[serde(flatten)]
    pub bytes: ByteBufferMetrics,
    pub keys: u64,
    pub capacity_keys: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResurrectionMetrics {
    pub attempts: u64,
    pub successes: u64,
    pub failures: u64,
    /// Complete attempt duration: the server-thread export plus the worker's write, sync, and
    /// rename. Comparable with the synchronous measurements recorded before snapshots moved off
    /// the server loop.
    pub last_duration_us: u64,
    pub max_duration_us: u64,
    /// The part of an attempt the server loop itself is blocked for: capturing pane replay bytes
    /// and metadata, before the durable write is handed to the snapshot worker. This is the figure
    /// that bounds input latency during a snapshot; `last_duration_us` no longer does.
    pub last_blocking_us: u64,
    pub max_blocking_us: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRuntimeMetrics {
    pub sampled_at_unix_ms: u64,
    pub pty_ingress: QueueMetrics,
    pub client_outboxes: ServerOutboxMetrics,
    pub resurrection: ResurrectionMetrics,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CachedServerRuntimeMetrics {
    #[serde(flatten)]
    pub sample: ServerRuntimeMetrics,
    pub age_ms: u64,
    pub stale: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeMetrics {
    pub sampled_at_unix_ms: u64,
    pub client_inbound: Option<QueueMetrics>,
    pub client_outbound: Option<QueueMetrics>,
    pub piped_remote: Option<ByteBufferMetrics>,
    pub orphan_output: OrphanOutputMetrics,
    pub server: Option<CachedServerRuntimeMetrics>,
}

impl RuntimeMetrics {
    pub fn capture(attachment: &Attachment) -> Self {
        let (client_inbound, client_outbound, piped_remote, server) = attachment
            .session_client
            .as_ref()
            .map(|client| {
                let stats = client.runtime_stats();
                (
                    stats.inbound,
                    Some(stats.outbound),
                    stats.piped_remote,
                    stats.server,
                )
            })
            .unwrap_or((None, None, None, None));
        let orphan = attachment
            .shared
            .as_ref()
            .map(|shared| shared.orphan_output_stats())
            .unwrap_or_default();

        Self {
            sampled_at_unix_ms: unix_time_millis(),
            client_inbound,
            client_outbound,
            piped_remote,
            orphan_output: OrphanOutputMetrics {
                bytes: ByteBufferMetrics::new(
                    orphan.retained,
                    orphan.high_water,
                    ORPHAN_OUTPUT_GLOBAL_CAP,
                ),
                keys: orphan.keys as u64,
                capacity_keys: ORPHAN_OUTPUT_KEY_CAP as u64,
            },
            server,
        }
    }

    pub fn devtools_rows(&self) -> Vec<DevToolsMetric> {
        let absent_queue = QueueMetrics::default();
        let inbound = self.client_inbound.as_ref().unwrap_or(&absent_queue);
        let outbound = self.client_outbound.as_ref().unwrap_or(&absent_queue);
        let (pty, server_out, snapshot, server_age) = self.server.as_ref().map_or_else(
            || {
                (
                    QueueMetrics::default(),
                    ServerOutboxMetrics::default(),
                    ResurrectionMetrics::default(),
                    None,
                )
            },
            |server| {
                (
                    server.sample.pty_ingress,
                    server.sample.client_outboxes,
                    server.sample.resurrection,
                    Some((server.age_ms, server.stale)),
                )
            },
        );

        vec![
            DevToolsMetric::new("PTY", format_queue(pty, server_age)),
            DevToolsMetric::new(
                "Srv out",
                format!(
                    "{} · {}c{}",
                    format_bytes(server_out.bytes),
                    server_out.clients,
                    format_age(server_age)
                ),
            ),
            DevToolsMetric::new("Cli in", format_queue(*inbound, None)),
            DevToolsMetric::new("Cli out", format_queue(*outbound, None)),
            DevToolsMetric::new(
                "Pipe",
                self.piped_remote
                    .map(format_bytes)
                    .unwrap_or_else(|| "-".to_string()),
            ),
            DevToolsMetric::new(
                "Orphan",
                format!(
                    "{} · {}k",
                    format_bytes(self.orphan_output.bytes),
                    self.orphan_output.keys
                ),
            ),
            DevToolsMetric::new(
                "Snapshot",
                format!(
                    "{}a {}ok {}err · {}/{} ms{}",
                    snapshot.attempts,
                    snapshot.successes,
                    snapshot.failures,
                    format_millis(snapshot.last_duration_us),
                    format_millis(snapshot.max_duration_us),
                    format_age(server_age)
                ),
            ),
        ]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TimedServerRuntimeMetrics {
    pub sample: ServerRuntimeMetrics,
    received_at: Instant,
}

impl TimedServerRuntimeMetrics {
    pub(crate) fn received(sample: ServerRuntimeMetrics) -> Self {
        Self {
            sample,
            received_at: Instant::now(),
        }
    }

    pub(crate) fn cached(&self) -> CachedServerRuntimeMetrics {
        let age = self.received_at.elapsed();
        CachedServerRuntimeMetrics {
            sample: self.sample.clone(),
            age_ms: duration_millis(age),
            stale: age >= SERVER_METRICS_STALE_AFTER,
        }
    }
}

pub(crate) fn unix_time_millis() -> u64 {
    duration_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

pub(crate) fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn format_queue(queue: QueueMetrics, age: Option<(u64, bool)>) -> String {
    format!(
        "{} · {}q{}",
        format_bytes(queue.bytes),
        queue.queued_items,
        format_age(age)
    )
}

fn format_bytes(bytes: ByteBufferMetrics) -> String {
    format!(
        "{}/{}/{}",
        compact_bytes(bytes.current_bytes),
        compact_bytes(bytes.high_water_bytes),
        compact_bytes(bytes.capacity_bytes)
    )
}

fn compact_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    if bytes >= MIB {
        format!("{:.1}M", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1}K", bytes as f64 / KIB as f64)
    } else {
        bytes.to_string()
    }
}

fn format_millis(micros: u64) -> String {
    format!("{:.2}", micros as f64 / 1000.0)
}

fn format_age(age: Option<(u64, bool)>) -> String {
    age.map(|(millis, stale)| format!(" · {}ms{}", millis, if stale { " stale" } else { "" }))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_metrics_json_has_stable_resource_shape() {
        let metrics = RuntimeMetrics {
            sampled_at_unix_ms: 100,
            client_inbound: Some(QueueMetrics {
                bytes: ByteBufferMetrics {
                    current_bytes: 1,
                    high_water_bytes: 2,
                    capacity_bytes: 3,
                },
                queued_items: 4,
            }),
            client_outbound: None,
            piped_remote: None,
            orphan_output: OrphanOutputMetrics::default(),
            server: Some(CachedServerRuntimeMetrics {
                sample: ServerRuntimeMetrics {
                    sampled_at_unix_ms: 90,
                    ..ServerRuntimeMetrics::default()
                },
                age_ms: 10,
                stale: false,
            }),
        };

        assert_eq!(
            serde_json::to_value(metrics).unwrap(),
            serde_json::json!({
                "sampled_at_unix_ms": 100,
                "client_inbound": {
                    "current_bytes": 1,
                    "high_water_bytes": 2,
                    "capacity_bytes": 3,
                    "queued_items": 4
                },
                "client_outbound": null,
                "piped_remote": null,
                "orphan_output": {
                    "current_bytes": 0,
                    "high_water_bytes": 0,
                    "capacity_bytes": 0,
                    "keys": 0,
                    "capacity_keys": 0
                },
                "server": {
                    "sampled_at_unix_ms": 90,
                    "pty_ingress": {
                        "current_bytes": 0,
                        "high_water_bytes": 0,
                        "capacity_bytes": 0,
                        "queued_items": 0
                    },
                    "client_outboxes": {
                        "current_bytes": 0,
                        "high_water_bytes": 0,
                        "capacity_bytes": 0,
                        "clients": 0
                    },
                    "resurrection": {
                        "attempts": 0,
                        "successes": 0,
                        "failures": 0,
                        "last_duration_us": 0,
                        "max_duration_us": 0,
                        "last_blocking_us": 0,
                        "max_blocking_us": 0
                    },
                    "age_ms": 10,
                    "stale": false
                }
            })
        );
    }

    #[test]
    fn cached_server_sample_reports_age_and_staleness() {
        let fresh = TimedServerRuntimeMetrics {
            sample: ServerRuntimeMetrics::default(),
            received_at: Instant::now(),
        }
        .cached();
        assert!(!fresh.stale);

        let stale = TimedServerRuntimeMetrics {
            sample: ServerRuntimeMetrics::default(),
            received_at: Instant::now() - SERVER_METRICS_STALE_AFTER - Duration::from_millis(1),
        }
        .cached();
        assert!(stale.stale);
        assert!(stale.age_ms >= SERVER_METRICS_STALE_AFTER.as_millis() as u64);
    }
}
