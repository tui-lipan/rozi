use super::*;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const SNAPSHOT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SnapshotMeta {
    version: u32,
    session: String,
    saved_at: u64,
    layout_rev: u64,
    panes: Vec<SnapshotPane>,
}

#[derive(Serialize, Deserialize)]
struct SnapshotPane {
    pane_id: PaneId,
    generation: u64,
    command: Option<String>,
    cwd: Option<String>,
    keep_open: bool,
    title: Option<String>,
    palette: WirePalette,
    cols: u16,
    rows: u16,
}

impl SessionServer {
    pub(super) fn snapshot_path(&self) -> io::Result<PathBuf> {
        if !crate::session::discovery::valid_attach_target(&self.session_name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session name",
            ));
        }
        let root = self
            .settings
            .snapshot_dir
            .clone()
            .or_else(default_snapshot_dir)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "state directory unavailable")
            })?;
        Ok(root.join(&self.session_name))
    }

    pub(super) fn maybe_snapshot(&mut self) -> io::Result<()> {
        if !self.settings.resurrect
            || !self.dirty
            || crate::state::is_ephemeral_session_name(&self.session_name)
        {
            return Ok(());
        }
        let attached = self.attached_count();
        let last_detached = self.last_attached_count > 0 && attached == 0;
        self.last_attached_count = attached;
        if !last_detached && self.last_snapshot.elapsed() < self.settings.snapshot_interval {
            return Ok(());
        }
        // Advance the deadline before doing synchronous I/O. A transient filesystem error keeps
        // the snapshot dirty for a later retry, but must not turn the server loop into a 1 ms
        // export/write/sync retry storm.
        self.last_snapshot = Instant::now();
        self.write_snapshot()?;
        self.dirty = false;
        Ok(())
    }

    fn write_snapshot(&mut self) -> io::Result<()> {
        let final_path = self.snapshot_path()?;
        let parent = final_path.parent().unwrap();
        crate::platform::fs_security::ensure_private_dir(parent)?;
        let suffix = format!(
            "{}.{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let temp = parent.join(format!(".{}.tmp-{suffix}", self.session_name));
        let backup = parent.join(format!(".{}.old-{suffix}", self.session_name));
        crate::platform::fs_security::ensure_private_dir(&temp)?;
        let panes_dir = temp.join("panes");
        crate::platform::fs_security::ensure_private_dir(&panes_dir)?;

        let mut panes = Vec::new();
        for (&pane_id, pane) in &mut self.panes {
            // The popup slot is a transient client-local overlay; resurrecting it would
            // revive an invisible orphan pane no client adopts.
            if pane.exited.is_some() || pane_id == crate::state::POPUP_PANE_ID {
                continue;
            }
            panes.push(SnapshotPane {
                pane_id,
                generation: pane.generation,
                command: pane.command.clone(),
                cwd: pane.spawnable_cwd(),
                keep_open: pane.keep_open,
                title: pane.effective_title(),
                palette: pane.palette,
                cols: pane.cols,
                rows: pane.rows,
            });
            write_secure(
                &panes_dir.join(format!("{pane_id}.replay")),
                &pane.screen.export_replay_bytes(),
            )?;
        }
        panes.sort_by_key(|pane| pane.pane_id);
        let meta = SnapshotMeta {
            version: SNAPSHOT_VERSION,
            session: self.session_name.clone(),
            saved_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            layout_rev: self.layout_rev,
            panes,
        };
        write_secure(
            &temp.join("meta.json"),
            &serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?,
        )?;
        if let Some(layout) = &self.layout {
            let mut layout = layout.clone();
            for workspace in &mut layout.workspaces {
                workspace.panes.retain(|saved| {
                    self.panes
                        .get(&saved.pane_id)
                        .is_some_and(|pane| pane.exited.is_none())
                });
            }
            write_secure(
                &temp.join("layout.json"),
                &serde_json::to_vec_pretty(&layout).map_err(io::Error::other)?,
            )?;
        }
        sync_directory(&temp)?;
        if final_path.exists() {
            fs::rename(&final_path, &backup)?;
        }
        if let Err(err) = fs::rename(&temp, &final_path) {
            if backup.exists() {
                let _ = fs::rename(&backup, &final_path);
            }
            let _ = fs::remove_dir_all(&temp);
            return Err(err);
        }
        sync_directory(parent)?;
        if backup.exists() {
            fs::remove_dir_all(backup)?;
        }
        Ok(())
    }

    pub(super) fn restore(&mut self) -> io::Result<usize> {
        let path = self.snapshot_path()?;
        let meta: SnapshotMeta =
            serde_json::from_slice(&fs::read(path.join("meta.json"))?).map_err(io::Error::other)?;
        if meta.version != SNAPSHOT_VERSION || meta.session != self.session_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported or mismatched session snapshot",
            ));
        }
        let mut layout: Option<SharedLayout> = fs::read(path.join("layout.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let mut restored = 0;
        for saved in meta.panes {
            let generation = self.next_generation;
            self.next_generation += 1;
            let replay = fs::read(path.join("panes").join(format!("{}.replay", saved.pane_id)))
                .unwrap_or_default();
            let result = self.spawn_pane_inner(
                SpawnRequest {
                    pane_id: saved.pane_id,
                    generation,
                    command: saved.command,
                    cwd: saved.cwd,
                    title: saved.title,
                    cols: saved.cols,
                    rows: saved.rows,
                    keep_open: saved.keep_open,
                    env: Vec::new(),
                    palette: saved.palette,
                    shell: self.settings.shell.clone(),
                    command_shell: self.settings.command_shell.clone(),
                },
                Some(&replay),
                true,
            );
            if matches!(result, ServerMessage::SpawnResult { ok: true, .. }) {
                restored += 1;
            }
            if let Some(layout) = &mut layout {
                for workspace in &mut layout.workspaces {
                    for pane in &mut workspace.panes {
                        if pane.pane_id == saved.pane_id {
                            pane.generation = generation;
                        }
                    }
                }
            }
        }
        self.layout = layout;
        self.layout_rev = u64::from(self.layout.is_some());
        self.dirty = false;
        self.last_snapshot = Instant::now();
        Ok(restored)
    }

    pub(super) fn delete_snapshot(&self) -> io::Result<()> {
        let path = self.snapshot_path()?;
        match fs::remove_dir_all(path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

pub fn delete_snapshot_for(name: &str) -> io::Result<()> {
    let server = SessionServer::new_named_with_settings(
        name,
        ServerSettings {
            resurrect: true,
            ..ServerSettings::default()
        },
    );
    server.delete_snapshot()
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    // Opening a directory for FlushFileBuffers requires Windows-only handle flags and the flush is
    // not supported consistently across filesystems. Snapshot files themselves are synced before
    // the atomic rename; do not fail an otherwise valid snapshot on this durability enhancement.
    Ok(())
}

fn write_secure(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn default_snapshot_dir() -> Option<PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    if env.home.is_none() && env.xdg_state_home.is_none() {
        return None;
    }
    Some(crate::platform::paths::state_dir(&env).join("sessions"))
}
