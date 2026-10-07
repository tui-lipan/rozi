//! Placed extension processes this client launched, and the credentials that identify them.
//!
//! A process an extension contribution starts on a session host is a *worker*. The UI trusts it
//! only as what it launched it as: one extension, one generation, bound to one host (and, for some
//! placements, one session). That identity is never read from anything the process says about
//! itself. It is looked up from an unguessable credential the process was handed at launch, and
//! for a worker on another machine only when the credential arrives over the very runtime
//! connection the worker was started through.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::config::ExtensionProvenance;
use crate::config::Placement;
use crate::session::protocol::SessionInstanceId;
use crate::session::remote::RemoteTarget;

/// The environment variable a worker's credential arrives in. The `rozi` CLI forwards it with
/// every control request; nothing else should read it.
pub const CREDENTIAL_ENV: &str = "ROZI_EXTENSION_CREDENTIAL";

/// A machine a worker can run on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostKey {
    /// This client's own machine.
    Local,
    Remote(RemoteTarget),
}

impl HostKey {
    pub fn of(target: Option<&RemoteTarget>) -> Self {
        target.map_or(Self::Local, |target| Self::Remote(target.clone()))
    }

    pub fn remote(&self) -> Option<&RemoteTarget> {
        match self {
            Self::Local => None,
            Self::Remote(target) => Some(target),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Local => "this machine".to_string(),
            Self::Remote(target) => target.display_label(),
        }
    }
}

/// What a worker may act on beyond the UI chrome: the sessions of one host, or one session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WorkerBinding {
    pub host: HostKey,
    /// Set for a worker placed for one particular session: `active-session` and `each-session`.
    pub session: Option<SessionInstanceId>,
}

impl WorkerBinding {
    /// Whether a session on `host` with `instance` lies within this binding.
    pub fn admits(&self, host: &HostKey, instance: Option<&SessionInstanceId>) -> bool {
        if &self.host != host {
            return false;
        }
        match &self.session {
            None => true,
            Some(bound) => instance == Some(bound),
        }
    }
}

/// Which runtime started a worker. A credential is honored only over the runtime it was issued
/// for, so one host's runtime can never present a worker of another host, or a local one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkerRuntime {
    Local,
    /// A remote runtime connection, by the epoch the client gave it. A reconnect is a new epoch,
    /// so every credential issued before it dies with the old connection.
    Remote(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerKind {
    Service { name: String },
    Command { id: String },
    Tab { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerId(pub u64);

#[derive(Clone, Debug)]
pub struct Worker {
    pub id: WorkerId,
    pub extension: ExtensionProvenance,
    pub placement: Placement,
    pub binding: WorkerBinding,
    pub runtime: WorkerRuntime,
    pub kind: WorkerKind,
    credential: String,
}

impl Worker {
    pub fn credential(&self) -> &str {
        &self.credential
    }
}

/// Every live worker credential this client has issued.
#[derive(Debug, Default)]
pub struct WorkerRegistry {
    workers: HashMap<WorkerId, Worker>,
    by_credential: HashMap<String, WorkerId>,
    next_id: u64,
}

impl WorkerRegistry {
    /// Issue a credential for a new worker. The credential is what the worker's environment
    /// carries; the registry is what gives it meaning.
    pub fn issue(
        &mut self,
        extension: ExtensionProvenance,
        placement: Placement,
        binding: WorkerBinding,
        runtime: WorkerRuntime,
        kind: WorkerKind,
    ) -> &Worker {
        self.next_id += 1;
        let id = WorkerId(self.next_id);
        let credential = fresh_credential();
        self.by_credential.insert(credential.clone(), id);
        self.workers.insert(
            id,
            Worker {
                id,
                extension,
                placement,
                binding,
                runtime,
                kind,
                credential,
            },
        );
        &self.workers[&id]
    }

    pub fn get(&self, id: WorkerId) -> Option<&Worker> {
        self.workers.get(&id)
    }

    /// The worker a credential was issued to, if it is still live and was issued for `runtime`.
    pub fn authenticate(&self, credential: &str, runtime: WorkerRuntime) -> Option<&Worker> {
        let id = self.by_credential.get(credential)?;
        self.workers
            .get(id)
            .filter(|worker| worker.runtime == runtime)
    }

    /// Revoke one worker's credential. Whatever it sends afterwards is refused.
    pub fn revoke(&mut self, id: WorkerId) -> Option<Worker> {
        let worker = self.workers.remove(&id)?;
        self.by_credential.remove(&worker.credential);
        Some(worker)
    }

    /// Revoke every worker matching `predicate`, returning them.
    pub fn revoke_where(&mut self, mut predicate: impl FnMut(&Worker) -> bool) -> Vec<Worker> {
        let ids: Vec<_> = self
            .workers
            .values()
            .filter(|worker| predicate(worker))
            .map(|worker| worker.id)
            .collect();
        ids.into_iter().filter_map(|id| self.revoke(id)).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Worker> {
        self.workers.values()
    }

    pub fn len(&self) -> usize {
        self.workers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }
}

fn fresh_credential() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> ExtensionProvenance {
        ExtensionProvenance {
            id: "sessions".to_string(),
            generation: "g1".to_string(),
        }
    }

    fn remote(name: &str) -> HostKey {
        HostKey::Remote(RemoteTarget::Alias(name.to_string()))
    }

    #[test]
    fn a_credential_only_authenticates_over_the_runtime_it_was_issued_for() {
        let mut registry = WorkerRegistry::default();
        let credential = registry
            .issue(
                provenance(),
                Placement::EachHost,
                WorkerBinding {
                    host: remote("pc"),
                    session: None,
                },
                WorkerRuntime::Remote(7),
                WorkerKind::Service {
                    name: "sessions.watch".to_string(),
                },
            )
            .credential()
            .to_string();
        assert!(
            registry
                .authenticate(&credential, WorkerRuntime::Remote(7))
                .is_some()
        );
        assert!(
            registry
                .authenticate(&credential, WorkerRuntime::Remote(8))
                .is_none()
        );
        assert!(
            registry
                .authenticate(&credential, WorkerRuntime::Local)
                .is_none()
        );
        assert!(
            registry
                .authenticate("not-a-credential", WorkerRuntime::Remote(7))
                .is_none()
        );
    }

    #[test]
    fn revoking_a_worker_retires_its_credential() {
        let mut registry = WorkerRegistry::default();
        let worker = registry.issue(
            provenance(),
            Placement::ActiveSession,
            WorkerBinding {
                host: HostKey::Local,
                session: None,
            },
            WorkerRuntime::Local,
            WorkerKind::Command {
                id: "sessions.open".to_string(),
            },
        );
        let (id, credential) = (worker.id, worker.credential().to_string());
        assert_eq!(credential.len(), 64);
        registry.revoke(id);
        assert!(
            registry
                .authenticate(&credential, WorkerRuntime::Local)
                .is_none()
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn a_session_binding_admits_only_its_session_and_a_host_binding_every_session_there() {
        let a = SessionInstanceId::for_test("a");
        let b = SessionInstanceId::for_test("b");
        let host = WorkerBinding {
            host: remote("pc"),
            session: None,
        };
        assert!(host.admits(&remote("pc"), Some(&a)));
        assert!(host.admits(&remote("pc"), Some(&b)));
        assert!(!host.admits(&remote("laptop"), Some(&a)));
        assert!(!host.admits(&HostKey::Local, Some(&a)));
        let session = WorkerBinding {
            host: remote("pc"),
            session: Some(a.clone()),
        };
        assert!(session.admits(&remote("pc"), Some(&a)));
        assert!(!session.admits(&remote("pc"), Some(&b)));
        assert!(!session.admits(&remote("pc"), None));
    }
}
