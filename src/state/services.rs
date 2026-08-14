use crate::config::ServiceConfig;
use crate::platform::command::CommandGroup;
use std::collections::BTreeMap;
use std::process::Child;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RunningService {
    pub config: ServiceConfig,
    pub child: Child,
    pub group: CommandGroup,
    pub started_at: Instant,
    pub backoff_delay: Duration,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone)]
pub struct PendingRestart {
    pub config: ServiceConfig,
    pub restart_at: Instant,
    pub backoff_delay: Duration,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormantReason {
    ExhaustedBackoff,
    NeverRestart,
    NormalExit,
}

#[derive(Debug, Clone)]
pub struct DormantService {
    pub config: ServiceConfig,
    pub reason: DormantReason,
}

#[derive(Debug, Default)]
pub struct ServicesState {
    pub running: BTreeMap<String, RunningService>,
    pub pending: BTreeMap<String, PendingRestart>,
    pub dormant: BTreeMap<String, DormantService>,
    pub epoch: u64,
}

impl ServicesState {
    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.pending.is_empty() && self.dormant.is_empty()
    }

    pub fn bump_epoch(&mut self) -> u64 {
        self.epoch = self.epoch.wrapping_add(1);
        self.epoch
    }
}
