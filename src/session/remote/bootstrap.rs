//! Probe and optionally install hyprmux on a remote host before attach.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::{HyprmuxRemoteConfig, RemoteInstallPolicy};
use crate::platform::command::program_exists;
use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};

use super::{RemoteTarget, ResolvedRemote};

const INSTALL_DIR: &str = ".local/bin";
const INSTALL_NAME: &str = "hyprmux";

/// Result of probing a remote host for a usable hyprmux binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeResult {
    /// Compatible binary found at this remote path.
    Found {
        path: String,
        protocol_max: u32,
        protocol_min: u32,
    },
    /// No binary, or none whose protocol range overlaps ours.
    Missing { detail: String },
}

/// Decision after applying install policy to a probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallDecision {
    Use { path: String },
    Install,
    Fail { message: String },
}

/// Fixed probe script run over ssh. Emits machine-readable lines; never trusts output as argv.
const PROBE_SCRIPT: &str = r#"
set -e
printf 'platform=%s\n' "$(uname -s 2>/dev/null || echo unknown)"
printf 'machine=%s\n' "$(uname -m 2>/dev/null || echo unknown)"
try_bin() {
  bin="$1"
  if [ -x "$bin" ] || command -v "$bin" >/dev/null 2>&1; then
    resolved=$(command -v "$bin" 2>/dev/null || echo "$bin")
    if [ -x "$resolved" ]; then
      out=$("$resolved" --version 2>/dev/null || true)
      printf 'candidate=%s\n' "$resolved"
      printf 'version_line=%s\n' "$out"
      # Protocol range: prefer --remote-serve self-report when available in future; for now
      # assume current builds speak the packaged protocol when --help mentions remote-serve.
      if "$resolved" --help 2>/dev/null | grep -q -- '--remote'; then
        printf 'speaks_remote=1\n'
      else
        printf 'speaks_remote=0\n'
      fi
    fi
  fi
}
if [ -n "${HYPRMUX_PROBE_BIN:-}" ]; then
  try_bin "$HYPRMUX_PROBE_BIN"
fi
try_bin hyprmux
try_bin "$HOME/.local/bin/hyprmux"
try_bin "$HOME/.cargo/bin/hyprmux"
try_bin /opt/homebrew/bin/hyprmux
try_bin /usr/local/bin/hyprmux
printf 'probe_done=1\n'
"#;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeReport {
    pub platform: String,
    pub machine: String,
    pub candidates: Vec<ProbeCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeCandidate {
    pub path: String,
    pub speaks_remote: bool,
    pub version_line: String,
}

/// Parse fixed-key probe stdout into a report (pure; safe for unit tests).
pub fn parse_probe_output(stdout: &str) -> ProbeReport {
    let mut report = ProbeReport::default();
    let mut pending_path: Option<String> = None;
    let mut pending_version = String::new();
    let mut pending_remote = false;
    let flush = |report: &mut ProbeReport,
                 path: &mut Option<String>,
                 version: &mut String,
                 remote: &mut bool| {
        if let Some(path) = path.take() {
            report.candidates.push(ProbeCandidate {
                path,
                speaks_remote: *remote,
                version_line: std::mem::take(version),
            });
            *remote = false;
        }
    };
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("platform=") {
            report.platform = value.to_string();
        } else if let Some(value) = line.strip_prefix("machine=") {
            report.machine = value.to_string();
        } else if let Some(value) = line.strip_prefix("candidate=") {
            flush(
                &mut report,
                &mut pending_path,
                &mut pending_version,
                &mut pending_remote,
            );
            pending_path = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("version_line=") {
            pending_version = value.to_string();
        } else if let Some(value) = line.strip_prefix("speaks_remote=") {
            pending_remote = value.trim() == "1";
        }
    }
    flush(
        &mut report,
        &mut pending_path,
        &mut pending_version,
        &mut pending_remote,
    );
    report
}

/// Choose the best compatible candidate. Protocol overlap is required; when a candidate cannot
/// report a range yet, `speaks_remote` is used as a stand-in for "new enough for --remote".
pub fn select_compatible(report: &ProbeReport) -> ProbeResult {
    for candidate in &report.candidates {
        if !candidate.speaks_remote {
            continue;
        }
        // Until remotes advertise min/max explicitly, treat speaks_remote as overlapping our range.
        let protocol_max = PROTOCOL_VERSION;
        let protocol_min = MIN_SUPPORTED_PROTOCOL;
        if crate::session::protocol::negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            protocol_max,
            protocol_min,
        )
        .is_ok()
        {
            return ProbeResult::Found {
                path: candidate.path.clone(),
                protocol_max,
                protocol_min,
            };
        }
    }
    ProbeResult::Missing {
        detail: if report.candidates.is_empty() {
            "no hyprmux binary found on the remote host".to_string()
        } else {
            "remote hyprmux binaries are too old for --remote (need a build that speaks --remote-serve)"
                .to_string()
        },
    }
}

/// Apply install policy. `interactive` is false for non-TTY / CI — then we never mutate the host.
pub fn decide_install(
    probe: &ProbeResult,
    policy: RemoteInstallPolicy,
    interactive: bool,
) -> InstallDecision {
    match probe {
        ProbeResult::Found { path, .. } => InstallDecision::Use { path: path.clone() },
        ProbeResult::Missing { detail } => match policy {
            RemoteInstallPolicy::Never => InstallDecision::Fail {
                message: format!(
                    "{detail}; set [remote] install = \"prompt\" or install hyprmux on the remote host"
                ),
            },
            RemoteInstallPolicy::Always if interactive => InstallDecision::Install,
            RemoteInstallPolicy::Prompt if interactive => InstallDecision::Install,
            RemoteInstallPolicy::Always | RemoteInstallPolicy::Prompt => InstallDecision::Fail {
                message: format!(
                    "{detail}; non-interactive --remote will not install on the remote host (run interactively or set binary_path)"
                ),
            },
        },
    }
}

/// Run the probe over a short-lived ssh command. `binary_path` short-circuits.
pub fn probe_remote(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
) -> Result<ProbeResult, String> {
    let resolved = ResolvedRemote::resolve(target, config);
    if let Some(path) = &resolved.binary_path {
        return Ok(ProbeResult::Found {
            path: path.clone(),
            protocol_max: PROTOCOL_VERSION,
            protocol_min: MIN_SUPPORTED_PROTOCOL,
        });
    }
    if !program_exists("ssh") {
        return Err("ssh was not found on PATH (required for --remote)".to_string());
    }
    let mut command = Command::new("ssh");
    command.arg("-T").arg("-o").arg("BatchMode=yes");
    if config.connection_timeout_secs > 0 {
        command
            .arg("-o")
            .arg(format!("ConnectTimeout={}", config.connection_timeout_secs));
    }
    if let Some(port) = resolved.port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = &resolved.identity_file {
        command.arg("-i").arg(crate::config::expand_path(identity));
    }
    for arg in &resolved.ssh_args {
        command.arg(arg);
    }
    command.arg(resolved.ssh_destination());
    command.arg("--");
    command.arg("sh").arg("-s");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to probe {}: {err}", resolved.host))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(PROBE_SCRIPT.as_bytes())
            .map_err(|err| format!("failed to write probe script: {err}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("probe ssh failed: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "probe of {} failed: {}",
            resolved.host,
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(select_compatible(&parse_probe_output(&stdout)))
}

/// Install policy entry point used before connect. Returns the remote binary path to invoke.
pub fn ensure_remote_binary(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
    interactive: bool,
) -> Result<String, String> {
    if let Ok(path) = std::env::var("HYPRMUX_REMOTE_BINARY") {
        let local = Path::new(&path);
        if !local.is_file() {
            return Err(format!(
                "HYPRMUX_REMOTE_BINARY={path} is not a regular file"
            ));
        }
        return install_local_file(target, config, local);
    }

    let probe = probe_remote(target, config)?;
    match decide_install(&probe, config.install, interactive) {
        InstallDecision::Use { path } => Ok(path),
        InstallDecision::Fail { message } => Err(message),
        InstallDecision::Install => {
            let local = std::env::current_exe()
                .map_err(|err| format!("cannot locate local hyprmux for install: {err}"))?;
            install_local_file(target, config, &local)
        }
    }
}

fn install_local_file(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
    local: &Path,
) -> Result<String, String> {
    if !local.is_file() {
        return Err(format!(
            "refusing to install non-regular file {}",
            local.display()
        ));
    }
    let resolved = ResolvedRemote::resolve(target, config);
    if !program_exists("ssh") {
        return Err("ssh was not found on PATH (required for --remote install)".to_string());
    }
    // Atomic install: stream to a temp path then rename into ~/.local/bin/hyprmux.
    let remote_tmp = format!("$HOME/{INSTALL_DIR}/.hyprmux.install.$$$$");
    let remote_final = format!("$HOME/{INSTALL_DIR}/{INSTALL_NAME}");
    let script = format!(
        "set -e\nmkdir -p \"$HOME/{INSTALL_DIR}\"\ncat > {remote_tmp}\nchmod 755 {remote_tmp}\nmv -f {remote_tmp} {remote_final}\nprintf 'installed=%s\\n' {remote_final}\n"
    );

    let mut command = Command::new("ssh");
    command.arg("-T").arg("-o").arg("BatchMode=yes");
    if let Some(port) = resolved.port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = &resolved.identity_file {
        command.arg("-i").arg(crate::config::expand_path(identity));
    }
    for arg in &resolved.ssh_args {
        command.arg(arg);
    }
    command.arg(resolved.ssh_destination());
    command.arg("--");
    command.arg("sh").arg("-c").arg(script);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to start remote install: {err}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "remote install stdin missing".to_string())?;
        let mut file = std::fs::File::open(local)
            .map_err(|err| format!("cannot read {}: {err}", local.display()))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|err| format!("read local binary: {err}"))?;
            if n == 0 {
                break;
            }
            stdin
                .write_all(&buf[..n])
                .map_err(|err| format!("upload binary: {err}"))?;
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("remote install failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "remote install failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("installed=") {
            return Ok(path.to_string());
        }
    }
    Ok(format!("$HOME/{INSTALL_DIR}/{INSTALL_NAME}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_probe_collects_candidates() {
        let report = parse_probe_output(
            "\
platform=Linux
machine=x86_64
candidate=/home/u/.local/bin/hyprmux
version_line=hyprmux 0.1.0
speaks_remote=1
candidate=/usr/bin/hyprmux
version_line=hyprmux 0.0.1
speaks_remote=0
probe_done=1
",
        );
        assert_eq!(report.platform, "Linux");
        assert_eq!(report.candidates.len(), 2);
        assert!(report.candidates[0].speaks_remote);
        assert!(!report.candidates[1].speaks_remote);
    }

    #[test]
    fn select_prefers_speaking_remote_candidate() {
        let report = parse_probe_output(
            "\
candidate=/old
speaks_remote=0
candidate=/new
speaks_remote=1
",
        );
        match select_compatible(&report) {
            ProbeResult::Found { path, .. } => assert_eq!(path, "/new"),
            other => panic!("expected found, got {other:?}"),
        }
    }

    #[test]
    fn non_interactive_never_installs() {
        let missing = ProbeResult::Missing {
            detail: "no hyprmux".into(),
        };
        assert!(matches!(
            decide_install(&missing, RemoteInstallPolicy::Prompt, false),
            InstallDecision::Fail { .. }
        ));
        assert!(matches!(
            decide_install(&missing, RemoteInstallPolicy::Always, false),
            InstallDecision::Fail { .. }
        ));
        assert!(matches!(
            decide_install(&missing, RemoteInstallPolicy::Never, true),
            InstallDecision::Fail { .. }
        ));
        assert!(matches!(
            decide_install(&missing, RemoteInstallPolicy::Prompt, true),
            InstallDecision::Install
        ));
    }
}
