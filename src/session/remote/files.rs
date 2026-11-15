//! Reading one file from a remote host, to open on this machine.
//!
//! Like [`super::worktrees`], the request crosses the connection as JSON on stdin, never argv, and
//! the path stays opaque here: the far side resolves `~` and a relative path with its own rules.

use std::io::Read;
use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::config::RemoteConfig;

use super::json_transport::{self, TransportError};
use super::target::RemoteTarget;

/// The hidden flag the far side runs. It takes no arguments; the request arrives on stdin.
pub(crate) const RUNNER_FLAG: &str = "--remote-read-file";

/// The largest file the far side sends. The whole file is held in memory on both machines.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The path as the pane showed it.
    pub path: String,
    /// The pane's working directory on the host, which a relative path starts at.
    pub cwd: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    File {
        name: String,
        /// The file's bytes, base64-encoded.
        data: String,
    },
    Failed {
        message: String,
    },
}

/// A file read from a remote host.
pub(crate) struct RemoteFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Read the file `request` names on `target`.
pub(crate) fn fetch(
    target: &RemoteTarget,
    request: &Request,
    config: &RemoteConfig,
) -> Result<RemoteFile, String> {
    let host = target.display_label();
    let reply: Reply =
        json_transport::forward(target, &config.unattended(), &[RUNNER_FLAG], request).map_err(
            |error| match error {
                TransportError::Preflight(message) => message,
                TransportError::Encode(error) => format!("could not encode the request: {error}"),
                TransportError::Ssh(error) => format!("ssh failed: {error}"),
                TransportError::Send(error) => format!("could not send the request: {error}"),
                TransportError::Failed(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stderr = stderr.trim();
                    if stderr.contains(&format!("unknown flag `{RUNNER_FLAG}`")) {
                        format!("the rozi on `{host}` is too old to send files (update rozi there)")
                    } else if stderr.is_empty() {
                        format!("could not read the file on `{host}`")
                    } else {
                        stderr.to_string()
                    }
                }
                TransportError::Answer(_) => format!("could not read the answer from `{host}`"),
            },
        )?;
    match reply {
        Reply::File { name, data } => base64::engine::general_purpose::STANDARD
            .decode(data)
            .map(|bytes| RemoteFile { name, bytes })
            .map_err(|_| format!("could not read the answer from `{host}`")),
        Reply::Failed { message } => Err(message),
    }
}

/// The far side of [`fetch`]: one JSON [`Request`] on stdin, one JSON [`Reply`] line out.
pub fn run_remote_read_file() -> Result<Reply, String> {
    let mut payload = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut payload)
        .map_err(|err| format!("could not read the file request: {err}"))?;
    let request: Request = serde_json::from_str(payload.trim())
        .map_err(|err| format!("could not parse the file request: {err}"))?;
    let home = crate::platform::paths::home_directory();
    Ok(
        match read(&request, home.as_deref().map(Path::new), MAX_BYTES) {
            Ok((name, bytes)) => Reply::File {
                name,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            Err(message) => Reply::Failed { message },
        },
    )
}

/// The name and bytes of the regular file `request` names, when it is at most `max` bytes.
fn read(request: &Request, home: Option<&Path>, max: u64) -> Result<(String, Vec<u8>), String> {
    let shown = &request.path;
    let path = crate::platform::paths::absolute_shown_path(
        shown,
        request.cwd.as_deref().map(Path::new),
        home,
    )
    .ok_or_else(|| format!("{shown} is relative, and the pane's directory is unknown"))?;
    let metadata =
        std::fs::metadata(&path).map_err(|_| format!("No such file or directory: {shown}"))?;
    if metadata.is_dir() {
        return Err(format!("{shown} is a directory"));
    }
    if !metadata.is_file() {
        return Err(format!("{shown} is not a regular file"));
    }
    if metadata.len() > max {
        return Err(format!("{shown} is larger than {} MB", max / (1024 * 1024)));
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Read through the limit as well: the file can grow after it was measured.
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(max + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("could not read {shown}: {error}"))?;
    if bytes.len() as u64 > max {
        return Err(format!("{shown} is larger than {} MB", max / (1024 * 1024)));
    }
    Ok((name, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shown_path_is_read_from_the_panes_directory_within_the_size_limit() {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::create_dir(scratch.path().join("out")).unwrap();
        std::fs::write(scratch.path().join("out/plot.png"), b"12345").unwrap();
        let read_shown = |path: &str, cwd: bool, max| {
            let request = Request {
                path: path.into(),
                cwd: cwd.then(|| scratch.path().display().to_string()),
            };
            read(&request, None, max)
        };

        assert_eq!(
            read_shown("./out/plot.png", true, 5),
            Ok(("plot.png".into(), b"12345".to_vec()))
        );
        assert!(read_shown("out/plot.png", true, 4).is_err());
        assert!(read_shown("out/plot.png", false, 5).is_err());
        assert!(read_shown("out", true, 5).is_err());
    }
}
