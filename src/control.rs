use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::Msg;
use crate::state::PaneId;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRequest {
    #[serde(flatten)]
    pub command: ControlCommand,
    #[serde(default)]
    pub source_pane: Option<PaneId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum ControlCommand {
    ListPanes,
    Focus {
        target: PaneId,
    },
    SendText {
        target: Option<PaneId>,
        text: String,
    },
    NewPane {
        command: Option<String>,
        cwd: Option<String>,
        title: Option<String>,
        #[serde(default)]
        keep_open: bool,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn ok(data: impl Serialize) -> Self {
        Self {
            ok: true,
            data: Some(serde_json::to_value(data).unwrap_or(serde_json::Value::Null)),
            error: None,
        }
    }
    pub fn empty() -> Self {
        Self {
            ok: true,
            data: None,
            error: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ControlEnvelope {
    pub request: ControlRequest,
    pub reply: mpsc::Sender<ControlResponse>,
}

#[derive(Debug)]
pub struct ControlSocketGuard {
    path: PathBuf,
}

impl ControlSocketGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for ControlSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn runtime_dir() -> std::io::Result<PathBuf> {
    runtime_dir_with_base(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

fn runtime_dir_with_base(base: Option<PathBuf>) -> std::io::Result<PathBuf> {
    let dir = match base {
        Some(base) => base.join("hyprmux"),
        None => fallback_runtime_dir_path(),
    };
    ensure_private_dir(&dir)?;
    Ok(dir)
}

fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

fn fallback_runtime_dir_path() -> PathBuf {
    std::env::temp_dir().join(format!("hyprmux-{}", current_uid()))
}

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) => validate_private_dir(dir, &metadata),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            validate_private_dir(dir, &fs::symlink_metadata(dir)?)
        }
        Err(err) => Err(err),
    }
}

fn validate_private_dir(dir: &Path, metadata: &fs::Metadata) -> std::io::Result<()> {
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a directory", dir.display()),
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not owned by the current user", dir.display()),
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{} permissions must not allow group/other access",
                dir.display()
            ),
        ));
    }
    Ok(())
}

pub fn socket_path_for_pid(pid: u32) -> std::io::Result<PathBuf> {
    Ok(runtime_dir()?.join(format!("control-{pid}.sock")))
}

pub fn bind_control_socket() -> std::io::Result<(UnixListener, ControlSocketGuard)> {
    let path = socket_path_for_pid(std::process::id())?;
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Ok((listener, ControlSocketGuard { path }))
}

pub fn run_listener(listener: UnixListener, link: CommandLink<Msg>) {
    for stream in listener.incoming().flatten() {
        let link = link.clone();
        std::thread::spawn(move || handle_connection(stream, link));
    }
}

fn handle_connection(mut stream: UnixStream, link: CommandLink<Msg>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut line = String::new();
    if BufReader::new(reader_stream).read_line(&mut line).is_err() {
        return;
    }
    let request = match serde_json::from_str::<ControlRequest>(&line) {
        Ok(request) => request,
        Err(err) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::error(format!("invalid request: {err}")))
                    .unwrap()
            );
            return;
        }
    };
    let (tx, rx) = mpsc::channel();
    link.send(Msg::ControlRequest(ControlEnvelope { request, reply: tx }));
    let response = rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| ControlResponse::error("control request timed out"));
    let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hyprmux-test-{name}-{}", std::process::id()))
    }

    #[test]
    fn runtime_dir_uses_per_user_temp_fallback_without_xdg() {
        let expected = std::env::temp_dir().join(format!("hyprmux-{}", current_uid()));
        assert_eq!(fallback_runtime_dir_path(), expected);
    }

    #[test]
    fn runtime_dir_rejects_unsafe_existing_directory_without_chmod() {
        let base = temp_base("unsafe");
        let dir = base.join("hyprmux");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();

        let err = runtime_dir_with_base(Some(base.clone())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let mode = fs::symlink_metadata(&dir).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o777);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn runtime_dir_rejects_symlink() {
        let base = temp_base("symlink");
        let dir = base.join("hyprmux");
        let target = temp_base("symlink-target");
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &dir).unwrap();

        let err = runtime_dir_with_base(Some(base.clone())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

        let _ = fs::remove_dir_all(base);
        let _ = fs::remove_dir_all(target);
    }
}
