//! Probe and optionally install hyprmux on a remote host before attach.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{HyprmuxRemoteConfig, RemoteInstallPolicy};
use crate::platform::command::program_exists;
use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};

use super::{RemoteTarget, ResolvedRemote};

const INSTALL_DIR: &str = ".local/bin";
const INSTALL_NAME: &str = "hyprmux";
const RELEASE_REPO: &str = "Razuer/hyprmux";

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
    Use {
        path: String,
    },
    /// Install without asking (`install = "always"` on a TTY).
    Install,
    /// Ask on stdin before installing (`install = "prompt"` on a TTY).
    Ask,
    Fail {
        message: String,
    },
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
      # Flatten version output to a single line for the report, keep protocol_* keys separate.
      printf 'version_line=%s\n' "$(printf '%s' "$out" | tr '\n' ' ')"
      printf '%s\n' "$out" | while IFS= read -r line; do
        case "$line" in
          protocol_min=*) printf '%s\n' "$line" ;;
          protocol_max=*) printf '%s\n' "$line" ;;
        esac
      done
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
    pub protocol_min: Option<u32>,
    pub protocol_max: Option<u32>,
}

/// Parse fixed-key probe stdout into a report (pure; safe for unit tests).
pub fn parse_probe_output(stdout: &str) -> ProbeReport {
    let mut report = ProbeReport::default();
    let mut pending_path: Option<String> = None;
    let mut pending_version = String::new();
    let mut pending_remote = false;
    let mut pending_min: Option<u32> = None;
    let mut pending_max: Option<u32> = None;
    let flush = |report: &mut ProbeReport,
                 path: &mut Option<String>,
                 version: &mut String,
                 remote: &mut bool,
                 min: &mut Option<u32>,
                 max: &mut Option<u32>| {
        if let Some(path) = path.take() {
            report.candidates.push(ProbeCandidate {
                path,
                speaks_remote: *remote,
                version_line: std::mem::take(version),
                protocol_min: min.take(),
                protocol_max: max.take(),
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
                &mut pending_min,
                &mut pending_max,
            );
            pending_path = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("version_line=") {
            pending_version = value.to_string();
        } else if let Some(value) = line.strip_prefix("protocol_min=") {
            pending_min = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("protocol_max=") {
            pending_max = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("speaks_remote=") {
            pending_remote = value.trim() == "1";
        }
    }
    flush(
        &mut report,
        &mut pending_path,
        &mut pending_version,
        &mut pending_remote,
        &mut pending_min,
        &mut pending_max,
    );
    report
}

/// Choose the best compatible candidate. Requires an advertised protocol range that overlaps ours.
pub fn select_compatible(report: &ProbeReport) -> ProbeResult {
    let mut saw_remote = false;
    let mut saw_without_range = false;
    for candidate in &report.candidates {
        if !candidate.speaks_remote {
            continue;
        }
        saw_remote = true;
        let (Some(protocol_max), Some(protocol_min)) =
            (candidate.protocol_max, candidate.protocol_min)
        else {
            saw_without_range = true;
            continue;
        };
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
        } else if saw_without_range && !saw_remote {
            "remote hyprmux binaries are too old for --remote (need a build that speaks --remote-serve)"
                .to_string()
        } else if saw_without_range {
            "remote hyprmux found but does not advertise a protocol range (upgrade it, or set binary_path / install)"
                .to_string()
        } else if saw_remote {
            "remote hyprmux protocol range does not overlap this client".to_string()
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
            RemoteInstallPolicy::Prompt if interactive => InstallDecision::Ask,
            RemoteInstallPolicy::Always | RemoteInstallPolicy::Prompt => InstallDecision::Fail {
                message: format!(
                    "{detail}; non-interactive --remote will not install on the remote host (run interactively or set binary_path / install = \"always\" on a TTY)"
                ),
            },
        },
    }
}

/// Prompt on stdin for install confirmation. Returns true only for an explicit yes.
pub fn prompt_install_confirmation(host: &str) -> io::Result<bool> {
    let mut stderr = io::stderr().lock();
    write!(
        stderr,
        "hyprmux: no compatible binary on {host}. Install to ~/.local/bin/hyprmux? [y/N] "
    )?;
    stderr.flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Run the probe over a short-lived ssh command. `binary_path` short-circuits with our protocol
/// range (caller-configured path is trusted).
pub fn probe_remote_report(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
) -> Result<ProbeReport, String> {
    let resolved = ResolvedRemote::resolve(target, config);
    if let Some(path) = &resolved.binary_path {
        return Ok(ProbeReport {
            platform: local_uname_platform(),
            machine: local_uname_machine(),
            candidates: vec![ProbeCandidate {
                path: path.clone(),
                speaks_remote: true,
                version_line: String::new(),
                protocol_min: Some(MIN_SUPPORTED_PROTOCOL),
                protocol_max: Some(PROTOCOL_VERSION),
            }],
        });
    }
    if !program_exists("ssh") {
        return Err("ssh was not found on PATH (required for --remote)".to_string());
    }
    let mut command = ssh_base_command(&resolved, config);
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
    Ok(parse_probe_output(&stdout))
}

#[allow(dead_code)] // CLI / test helper alongside probe_remote_report
pub fn probe_remote(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
) -> Result<ProbeResult, String> {
    Ok(select_compatible(&probe_remote_report(target, config)?))
}

/// Install policy entry point used before connect. Returns the remote binary path to invoke.
///
/// Call this on the main thread before the TUI takes over the terminal when `install = "prompt"`,
/// so the yes/no prompt can read stdin.
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
        return install_bytes(target, config, local, "HYPRMUX_REMOTE_BINARY override");
    }

    let report = probe_remote_report(target, config)?;
    let probe = select_compatible(&report);
    let host = ResolvedRemote::resolve(target, config).host;
    match decide_install(&probe, config.install, interactive) {
        InstallDecision::Use { path } => Ok(path),
        InstallDecision::Fail { message } => Err(message),
        InstallDecision::Install => install_for_platforms(target, config, &report),
        InstallDecision::Ask => {
            let accepted = prompt_install_confirmation(&host).map_err(|err| err.to_string())?;
            if !accepted {
                return Err(format!(
                    "install declined for {host}; set binary_path, install a compatible hyprmux, or pass install = \"always\""
                ));
            }
            install_for_platforms(target, config, &report)
        }
    }
}

fn install_for_platforms(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
    report: &ProbeReport,
) -> Result<String, String> {
    let local_os = normalize_os(std::env::consts::OS);
    let local_arch = normalize_arch(std::env::consts::ARCH);
    let remote_os = normalize_os(&report.platform);
    let remote_arch = normalize_arch(&report.machine);

    if remote_os == "unknown" || remote_arch == "unknown" {
        return Err(
            "remote probe did not report platform/machine; cannot choose an install artifact"
                .to_string(),
        );
    }

    if local_os == remote_os && local_arch == remote_arch {
        let local = std::env::current_exe()
            .map_err(|err| format!("cannot locate local hyprmux for install: {err}"))?;
        return install_bytes(target, config, &local, "same-platform current_exe");
    }

    let triple = rustc_target(&remote_os, &remote_arch).ok_or_else(|| {
        format!(
            "no release artifact mapping for remote platform {remote_os}/{remote_arch}; set binary_path or HYPRMUX_REMOTE_BINARY"
        )
    })?;
    let version = env!("CARGO_PKG_VERSION");
    let local_artifact = download_release_binary(triple, version)?;
    install_bytes(
        target,
        config,
        &local_artifact,
        &format!("release asset {triple}"),
    )
}

fn install_bytes(
    target: &RemoteTarget,
    config: &HyprmuxRemoteConfig,
    local: &Path,
    _source: &str,
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
    // Atomic install with quoted paths; refuse to overwrite a non-regular destination.
    let script = format!(
        r#"set -e
dir="$HOME/{INSTALL_DIR}"
final="$dir/{INSTALL_NAME}"
tmp="$dir/.hyprmux.install.$$"
mkdir -p "$dir"
if [ -e "$final" ] && [ ! -f "$final" ]; then
  printf 'refuse_non_regular=%s\n' "$final" >&2
  exit 1
fi
cat > "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$final"
printf 'installed=%s\n' "$final"
"#
    );

    let mut command = ssh_base_command(&resolved, config);
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
    Err("remote install succeeded but did not report installed= path".to_string())
}

fn download_release_binary(triple: &str, version: &str) -> Result<PathBuf, String> {
    let base = std::env::var("HYPRMUX_RELEASE_BASE_URL").unwrap_or_else(|_| {
        format!("https://github.com/{RELEASE_REPO}/releases/download/v{version}")
    });
    let archive_name = if triple.contains("windows") {
        format!("hyprmux-{version}-{triple}.zip")
    } else {
        format!("hyprmux-{version}-{triple}.tar.gz")
    };
    let archive_url = format!("{base}/{archive_name}");
    let sha_url = format!("{archive_url}.sha256");

    let tmp = std::env::temp_dir().join(format!(
        "hyprmux-remote-install-{}-{}",
        std::process::id(),
        triple
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|err| format!("temp dir: {err}"))?;
    let archive_path = tmp.join(&archive_name);
    let sha_path = tmp.join(format!("{archive_name}.sha256"));

    download_url(&archive_url, &archive_path)?;
    download_url(&sha_url, &sha_path)?;
    verify_sha256(&archive_path, &sha_path)?;

    let bin_name = if triple.contains("windows") {
        "hyprmux.exe"
    } else {
        "hyprmux"
    };
    let out_bin = tmp.join(bin_name);
    if archive_name.ends_with(".zip") {
        let status = Command::new("unzip")
            .args(["-o", "-j"])
            .arg(&archive_path)
            .arg(bin_name)
            .arg("-d")
            .arg(&tmp)
            .status()
            .map_err(|err| format!("unzip not available: {err}"))?;
        if !status.success() {
            return Err(format!("failed to unzip {archive_name}"));
        }
    } else {
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(&archive_path)
            .arg("-C")
            .arg(&tmp)
            .arg(bin_name)
            .status()
            .map_err(|err| format!("tar not available: {err}"))?;
        if !status.success() {
            // Some archives nest the binary; extract all then locate.
            let status = Command::new("tar")
                .args(["-xzf"])
                .arg(&archive_path)
                .arg("-C")
                .arg(&tmp)
                .status()
                .map_err(|err| format!("tar extract failed: {err}"))?;
            if !status.success() {
                return Err(format!("failed to extract {archive_name}"));
            }
        }
    }
    if !out_bin.is_file() {
        // Search one level for the binary name.
        for entry in std::fs::read_dir(&tmp).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            if entry.file_name() == *bin_name && entry.path().is_file() {
                return Ok(entry.path());
            }
        }
        return Err(format!(
            "release archive {archive_name} did not contain {bin_name}"
        ));
    }
    Ok(out_bin)
}

fn download_url(url: &str, dest: &Path) -> Result<(), String> {
    if !program_exists("curl") {
        return Err(
            "curl was not found on PATH (required to download a cross-platform remote binary)"
                .to_string(),
        );
    }
    let status = Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--tlsv1.2", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|err| format!("curl failed: {err}"))?;
    if !status.success() {
        return Err(format!("failed to download {url}"));
    }
    Ok(())
}

fn verify_sha256(archive: &Path, sha_file: &Path) -> Result<(), String> {
    let expected = std::fs::read_to_string(sha_file)
        .map_err(|err| format!("read checksum: {err}"))?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid sha256 file {}", sha_file.display()));
    }
    let output = if program_exists("sha256sum") {
        Command::new("sha256sum")
            .arg(archive)
            .output()
            .map_err(|err| format!("sha256sum: {err}"))?
    } else if program_exists("shasum") {
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(archive)
            .output()
            .map_err(|err| format!("shasum: {err}"))?
    } else {
        return Err("neither sha256sum nor shasum found for checksum verification".to_string());
    };
    if !output.status.success() {
        return Err("checksum command failed".to_string());
    }
    let actual = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if actual != expected {
        return Err(format!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            archive.display()
        ));
    }
    Ok(())
}

/// Common ssh argv for every remote invocation: no tty, timeouts, and the per-host options.
///
/// `BatchMode` comes from `[remote] batch_mode` (default on). It is the single place that decides
/// whether ssh may prompt, so probe, install, attach, list, and kill all agree — a mix would mean
/// a host that lists fine but hangs on attach.
pub(crate) fn ssh_base_command(resolved: &ResolvedRemote, config: &HyprmuxRemoteConfig) -> Command {
    let mut command = Command::new("ssh");
    command.arg("-T");
    if config.batch_mode {
        command.arg("-o").arg("BatchMode=yes");
    }
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
    command
}

fn normalize_os(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "linux" => "linux".into(),
        "darwin" | "macos" => "macos".into(),
        "windows" | "windows_nt" | "mingw" | "msys" => "windows".into(),
        other => other.to_string(),
    }
}

fn normalize_arch(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x86_64".into(),
        "aarch64" | "arm64" => "aarch64".into(),
        other => other.to_string(),
    }
}

fn rustc_target(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn local_uname_platform() -> String {
    normalize_os(std::env::consts::OS)
}

fn local_uname_machine() -> String {
    normalize_arch(std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_probe_collects_candidates_and_protocol_range() {
        let report = parse_probe_output(
            "\
platform=Linux
machine=x86_64
candidate=/home/u/.local/bin/hyprmux
version_line=hyprmux 0.1.0 protocol_min=12 protocol_max=12
protocol_min=12
protocol_max=12
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
        assert_eq!(report.candidates[0].protocol_min, Some(12));
        assert_eq!(report.candidates[0].protocol_max, Some(12));
        assert!(!report.candidates[1].speaks_remote);
    }

    #[test]
    fn select_requires_overlapping_protocol_range() {
        let report = parse_probe_output(
            "\
candidate=/old
speaks_remote=1
candidate=/new
speaks_remote=1
protocol_min=12
protocol_max=12
",
        );
        match select_compatible(&report) {
            ProbeResult::Found { path, .. } => assert_eq!(path, "/new"),
            other => panic!("expected found, got {other:?}"),
        }
    }

    #[test]
    fn select_rejects_speaks_remote_without_protocol_range() {
        let report = parse_probe_output(
            "\
candidate=/new
speaks_remote=1
",
        );
        assert!(matches!(
            select_compatible(&report),
            ProbeResult::Missing { .. }
        ));
    }

    #[test]
    fn select_rejects_disjoint_protocol_range() {
        let report = parse_probe_output(
            "\
candidate=/skew
speaks_remote=1
protocol_min=1
protocol_max=1
",
        );
        assert!(matches!(
            select_compatible(&report),
            ProbeResult::Missing { .. }
        ));
    }

    #[test]
    fn prompt_policy_asks_when_interactive_never_auto_installs() {
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
            InstallDecision::Ask
        ));
        assert!(matches!(
            decide_install(&missing, RemoteInstallPolicy::Always, true),
            InstallDecision::Install
        ));
    }

    #[test]
    fn rustc_target_mapping_covers_release_matrix() {
        assert_eq!(
            rustc_target("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            rustc_target("macos", "aarch64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            rustc_target("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
        assert!(rustc_target("plan9", "x86_64").is_none());
    }

    /// `[remote] batch_mode` is the one switch deciding whether ssh may prompt, so it has to reach
    /// the argv — and reach it identically for every remote invocation.
    #[test]
    fn batch_mode_config_drives_the_ssh_argv() {
        let resolved = ResolvedRemote {
            alias: Some("workbox".into()),
            host: "workbox".into(),
            user: None,
            port: None,
            identity_file: None,
            ssh_args: Vec::new(),
            binary_path: None,
        };
        let args = |config: &HyprmuxRemoteConfig| -> Vec<String> {
            ssh_base_command(&resolved, config)
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };

        let mut config = HyprmuxRemoteConfig::default();
        assert!(config.batch_mode, "batch mode must default on");
        assert!(
            args(&config).iter().any(|arg| arg == "BatchMode=yes"),
            "default config must refuse interactive ssh prompts"
        );

        config.batch_mode = false;
        assert!(
            !args(&config).iter().any(|arg| arg == "BatchMode=yes"),
            "batch_mode = false must let ssh prompt"
        );
        // Everything else is unaffected by the switch.
        assert!(args(&config).iter().any(|arg| arg == "-T"));
        assert!(
            args(&config)
                .iter()
                .any(|arg| arg.starts_with("ConnectTimeout="))
        );
    }
}
