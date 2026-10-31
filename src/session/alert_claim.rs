//! Local-machine coordination for alerts observed by several clients.

use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use super::protocol::PaneRef;

const LIFETIME: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub(crate) enum Edge {
    Blocked,
    Finished,
    Failed,
}

impl Edge {
    fn name(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

/// Win an event once across clients. Filesystem errors leave the alert enabled.
pub(crate) fn claim(pane: &PaneRef, edge: Edge) -> bool {
    let result = crate::control::runtime_dir()
        .and_then(|runtime| claim_in(&runtime.join("alert-claims"), pane, edge));
    result.unwrap_or(true)
}

fn filename(pane: &PaneRef, edge: Edge) -> String {
    // Instance IDs arrive from peers as arbitrary strings. Only a digest reaches the filesystem.
    let instance: String = Sha256::digest(pane.session_instance.as_str().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{instance}-{}-{}-{}",
        pane.pane_id,
        pane.generation,
        edge.name()
    )
}

fn expired(path: &Path, now: SystemTime) -> io::Result<bool> {
    let modified = std::fs::symlink_metadata(path)?.modified()?;
    Ok(now.duration_since(modified).unwrap_or_default() >= LIFETIME)
}

fn claim_in(directory: &Path, pane: &PaneRef, edge: Edge) -> io::Result<bool> {
    crate::platform::fs_security::ensure_private_dir(directory)?;
    // Keep this file: unlinking a held lock would let another process lock a different inode.
    // Serializing the short file transaction also prevents two clients deleting the same stale
    // claim after one of them has already replaced it with a fresh claim.
    let lock = crate::platform::persist::open_private_lock(&directory.join(".lock"))?;
    lock.lock()?;
    claim_locked(directory, pane, edge)
}

fn claim_locked(directory: &Path, pane: &PaneRef, edge: Edge) -> io::Result<bool> {
    let path = directory.join(filename(pane, edge));
    let now = SystemTime::now();
    match crate::platform::persist::create_private_file(&path, false) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !expired(&path, now)? {
                return Ok(false);
            }
            std::fs::remove_file(&path)?;
            crate::platform::persist::create_private_file(&path, false)?;
        }
        Err(error) => return Err(error),
    }
    prune(directory, now);
    Ok(true)
}

fn prune(directory: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != ".lock" && expired(&entry.path(), now).unwrap_or(false) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::SessionInstanceId;

    fn pane() -> PaneRef {
        PaneRef {
            session_instance: SessionInstanceId::generate(),
            pane_id: 7,
            generation: 2,
        }
    }

    fn age(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::now() - LIFETIME - Duration::from_secs(1)),
        )
        .unwrap();
    }

    #[test]
    fn first_claim_wins_then_expires_and_prunes_other_stale_claims() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("claims");
        crate::platform::fs_security::ensure_private_dir(&root).unwrap();
        let pane = pane();
        assert!(claim_in(root.as_path(), &pane, Edge::Blocked).unwrap());
        assert!(!claim_in(root.as_path(), &pane, Edge::Blocked).unwrap());
        assert!(claim_in(root.as_path(), &pane, Edge::Finished).unwrap());
        let blocked = root.as_path().join(filename(&pane, Edge::Blocked));
        let finished = root.as_path().join(filename(&pane, Edge::Finished));
        age(&blocked);
        age(&finished);
        assert!(claim_in(root.as_path(), &pane, Edge::Blocked).unwrap());
        assert!(!claim_in(root.as_path(), &pane, Edge::Blocked).unwrap());
        assert!(!finished.exists());
        assert!(root.as_path().join(".lock").exists());
    }

    #[test]
    fn concurrent_claims_have_one_winner_including_expiry() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("claims");
        crate::platform::fs_security::ensure_private_dir(&root).unwrap();
        let pane = pane();
        for round in 0..2 {
            let winners = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..8)
                    .map(|_| scope.spawn(|| claim_in(root.as_path(), &pane, Edge::Failed).unwrap()))
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| usize::from(handle.join().unwrap()))
                    .sum::<usize>()
            });
            assert_eq!(winners, 1, "round {round}");
            age(&root.as_path().join(filename(&pane, Edge::Failed)));
        }
    }

    #[test]
    fn unusable_directory_keeps_the_alert_and_hostile_instance_stays_in_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("claims");
        crate::platform::fs_security::ensure_private_dir(&root).unwrap();
        let path = root.as_path().join("file");
        std::fs::write(&path, b"not a directory").unwrap();
        assert!(claim_in(&path, &pane(), Edge::Blocked).unwrap_or(true));
        let pane = PaneRef {
            session_instance: SessionInstanceId::for_test("../../escape/\\0host"),
            ..pane()
        };
        assert!(claim_in(root.as_path(), &pane, Edge::Blocked).unwrap());
        assert_eq!(std::fs::read_dir(root.as_path()).unwrap().count(), 3);
        assert_eq!(
            Path::new(&filename(&pane, Edge::Blocked))
                .components()
                .count(),
            1
        );
    }
}
