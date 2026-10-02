//! Session-owned policy over resolved semantic agent runtimes.
use super::*;
use crate::platform::sleep_inhibit::SleepInhibitor;
use crate::session::protocol::AgentState;

const RELEASE_GRACE: Duration = Duration::from_secs(5);

pub(super) trait SleepGuard {
    fn check(&mut self) -> io::Result<()>;
}

impl SleepGuard for SleepInhibitor {
    fn check(&mut self) -> io::Result<()> {
        self.check()
    }
}

pub(super) struct AgentSleepPolicy<G = SleepInhibitor> {
    inhibitor: Option<G>,
    release_at: Option<Instant>,
    acquire_failed_for_epoch: bool,
}

impl<G> Default for AgentSleepPolicy<G> {
    fn default() -> Self {
        Self {
            inhibitor: None,
            release_at: None,
            acquire_failed_for_epoch: false,
        }
    }
}

impl<G: SleepGuard> AgentSleepPolicy<G> {
    /// An error is returned once per working epoch. Failure always allows sleep.
    fn reconcile(
        &mut self,
        enabled: bool,
        any_agent_working: bool,
        now: Instant,
        acquire: impl FnOnce() -> io::Result<G>,
    ) -> io::Result<()> {
        if !enabled {
            self.inhibitor = None;
            self.release_at = None;
            self.acquire_failed_for_epoch = false;
            return Ok(());
        }
        if !any_agent_working {
            self.acquire_failed_for_epoch = false;
            if self.inhibitor.is_some() {
                let deadline = *self.release_at.get_or_insert(now + RELEASE_GRACE);
                if now >= deadline {
                    self.inhibitor = None;
                    self.release_at = None;
                }
            }
            return Ok(());
        }
        self.release_at = None;
        if let Some(guard) = self.inhibitor.as_mut() {
            if let Err(err) = guard.check() {
                self.inhibitor = None;
                self.acquire_failed_for_epoch = true;
                return Err(err);
            }
        } else if !self.acquire_failed_for_epoch {
            match acquire() {
                Ok(guard) => self.inhibitor = Some(guard),
                Err(err) => {
                    self.acquire_failed_for_epoch = true;
                    return Err(err);
                }
            }
        }
        Ok(())
    }
}

impl SessionServer {
    pub(super) fn any_agent_working(&self) -> bool {
        self.panes
            .iter()
            .map(|(&id, pane)| (id, pane))
            .chain(self.local_panes.iter().map(|(&(_, id), pane)| (id, pane)))
            .any(|(pane_id, pane)| {
                if pane.exited.is_some() {
                    return false;
                }
                let reference = protocol::PaneRef {
                    session_instance: self.instance_id.clone(),
                    pane_id,
                    generation: pane.generation,
                };
                protocol::effective_agent_runtimes(&pane.runtime, &pane.agent.references(reference))
                    .iter()
                    .any(|runtime| runtime.state == AgentState::Working)
            })
    }

    pub(super) fn reconcile_sleep_policy(&mut self) {
        let working = self.settings.keep_awake_while_agents_work && self.any_agent_working();
        if let Err(err) = self.sleep_policy.reconcile(
            self.settings.keep_awake_while_agents_work,
            working,
            Instant::now(),
            || SleepInhibitor::acquire("rozi agents are working"),
        ) {
            eprintln!("rozi: could not keep the system awake while agents work: {err}");
        }
    }

    pub(super) fn reload_sleep_policy(&mut self) {
        let loaded = crate::config::load_config();
        crate::config::log_config_warnings(&loaded.warnings);
        if !loaded.rejected {
            self.settings.keep_awake_while_agents_work =
                loaded.config.session.keep_awake_while_agents_work;
            self.reconcile_sleep_policy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeGuard(Rc<Cell<usize>>);
    impl SleepGuard for FakeGuard {
        fn check(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn working_acquires_once_grace_cancels_and_expiration_releases() {
        let drops = Rc::new(Cell::new(0));
        let acquisitions = Cell::new(0);
        let mut policy = AgentSleepPolicy::default();
        let start = Instant::now();
        let acquire = || {
            acquisitions.set(acquisitions.get() + 1);
            Ok(FakeGuard(drops.clone()))
        };
        for seconds in [0, 1, 2] {
            policy
                .reconcile(true, true, start + Duration::from_secs(seconds), acquire)
                .unwrap();
        }
        assert_eq!(acquisitions.get(), 1);
        policy
            .reconcile(true, false, start + Duration::from_secs(3), acquire)
            .unwrap();
        policy
            .reconcile(true, true, start + Duration::from_secs(7), acquire)
            .unwrap();
        assert_eq!(drops.get(), 0);
        assert!(policy.release_at.is_none());
        policy
            .reconcile(true, false, start + Duration::from_secs(8), acquire)
            .unwrap();
        policy
            .reconcile(true, false, start + Duration::from_secs(12), acquire)
            .unwrap();
        assert_eq!(drops.get(), 0);
        policy
            .reconcile(true, false, start + Duration::from_secs(13), acquire)
            .unwrap();
        assert_eq!(drops.get(), 1);
        policy
            .reconcile(true, true, start + Duration::from_secs(14), acquire)
            .unwrap();
        assert_eq!(acquisitions.get(), 2);
        policy
            .reconcile(false, true, start + Duration::from_secs(14), acquire)
            .unwrap();
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn unexpected_helper_exit_releases_and_does_not_retry_this_epoch() {
        struct FailedGuard(Rc<Cell<usize>>);
        impl SleepGuard for FailedGuard {
            fn check(&mut self) -> io::Result<()> {
                Err(io::Error::other("helper exited"))
            }
        }
        impl Drop for FailedGuard {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let calls = Cell::new(0);
        let acquire = || {
            calls.set(calls.get() + 1);
            Ok(FailedGuard(drops.clone()))
        };
        let mut policy = AgentSleepPolicy::default();
        let now = Instant::now();
        policy.reconcile(true, true, now, acquire).unwrap();
        assert!(policy.reconcile(true, true, now, acquire).is_err());
        assert_eq!(drops.get(), 1);
        policy.reconcile(true, true, now, acquire).unwrap();
        assert_eq!(calls.get(), 1);
        policy.reconcile(true, false, now, acquire).unwrap();
        policy.reconcile(true, true, now, acquire).unwrap();
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn failure_retries_only_in_next_epoch_or_after_reenable() {
        let mut policy: AgentSleepPolicy<FakeGuard> = AgentSleepPolicy::default();
        let calls = Cell::new(0);
        let acquire = || {
            calls.set(calls.get() + 1);
            Err(io::Error::other("unavailable"))
        };
        let now = Instant::now();
        assert!(policy.reconcile(true, true, now, acquire).is_err());
        policy
            .reconcile(true, true, now + RELEASE_GRACE, acquire)
            .unwrap();
        assert_eq!(calls.get(), 1);
        policy
            .reconcile(true, false, now + RELEASE_GRACE, acquire)
            .unwrap();
        assert!(
            policy
                .reconcile(true, true, now + RELEASE_GRACE, acquire)
                .is_err()
        );
        assert_eq!(calls.get(), 2);
        policy.reconcile(false, true, now, acquire).unwrap();
        assert!(policy.reconcile(true, true, now, acquire).is_err());
        assert_eq!(calls.get(), 3);
    }
}
