//! Probe and optionally install rozi on a remote host before attach.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;

use crate::config::{RemoteConfig, RemoteInstallPolicy};
use crate::platform::command::program_exists;
use crate::release_app::ROZI;
use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
use relswap::Downloader;
use url::Url;

use super::{
    RemoteTarget, ResolvedRemote, validate_remote_executable_token, validate_remote_target,
};

const INSTALL_NAME: &str = "rozi";
const RELEASE_REPO: &str = "tui-lipan/rozi";

/// Result of probing a remote host for a usable rozi binary.
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
///
/// Used for POSIX remotes. Windows sshd uses the PowerShell counterpart below.
const PROBE_SCRIPT: &str = r#"
set -e
printf 'platform=%s\n' "$(uname -s 2>/dev/null || echo unknown)"
printf 'machine=%s\n' "$(uname -m 2>/dev/null || echo unknown)"
try_bin() {
  bin="$1"
  reported="${2:-}"
  if [ -x "$bin" ] || command -v "$bin" >/dev/null 2>&1; then
    resolved=$(command -v "$bin" 2>/dev/null || echo "$bin")
    if [ -x "$resolved" ]; then
      out=$("$resolved" --version 2>/dev/null || true)
      printf 'candidate=%s\n' "${reported:-$resolved}"
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
if [ -n "${ROZI_PROBE_BIN:-}" ]; then
  try_bin "$ROZI_PROBE_BIN"
fi
try_bin rozi
try_bin "$HOME/.local/bin/rozi"
try_bin "$HOME/.cargo/bin/rozi"
try_bin /opt/homebrew/bin/rozi
try_bin /usr/local/bin/rozi
try_bin /usr/bin/rozi
try_bin "$HOME/bin/rozi"
try_bin "$HOME/.nix-profile/bin/rozi"
if [ -n "${XDG_DATA_HOME:-}" ] && [ "${XDG_DATA_HOME#/}" != "$XDG_DATA_HOME" ]; then
  data_home="$XDG_DATA_HOME"
else
  data_home="$HOME/.local/share"
fi
managed_root="$data_home/rozi/remote"
for managed in "$managed_root/"*/rozi; do
  [ -e "$managed" ] || continue
  try_bin "$managed"
done
printf 'probe_done=1\n'
"#;

const WINDOWS_FAMILY_PROBE_SCRIPT: &str = "if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) { Write-Output 'rozi_family=windows' }";

/// PowerShell counterpart of [`PROBE_SCRIPT`] for a Windows remote host. Emits the same fixed keys
/// the POSIX probe does; [`parse_probe_output`] handles both. Never treats binary output as code.
const WINDOWS_PROBE_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
Add-Type -TypeDefinition @'
using System.Text;
using System.Runtime.InteropServices;
public static class RoziLongPathName {
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern uint GetLongPathName(string shortPath, StringBuilder longPath, uint bufferLength);
}
'@ -ErrorAction SilentlyContinue
function Get-RoziLongPath($path) {
  $buffer = New-Object System.Text.StringBuilder 32768
  $length = [RoziLongPathName]::GetLongPathName($path, $buffer, [uint32]$buffer.Capacity)
  if ($length -gt 0 -and $length -lt $buffer.Capacity) { return $buffer.ToString() }
  return $path
}
Write-Output "platform=windows"
$arch = $env:PROCESSOR_ARCHITECTURE
if (-not $arch) { $arch = 'unknown' }
Write-Output "machine=$arch"
function Try-Bin($bin, $reported = $null) {
  $resolved = $null
  if (Test-Path -LiteralPath $bin -PathType Leaf) {
    $resolved = (Resolve-Path -LiteralPath $bin).Path
  } else {
    $cmd = Get-Command $bin -ErrorAction SilentlyContinue
    if ($cmd) { $resolved = $cmd.Source }
  }
  if (-not $resolved) { return }
  $resolved = Get-RoziLongPath $resolved
  $out = & $resolved --version 2>$null
  if (-not $reported) { $reported = $resolved }
  Write-Output "candidate=$reported"
  $flat = ($out -join ' ')
  Write-Output "version_line=$flat"
  foreach ($line in $out) {
    if ($line -match '^protocol_min=') { Write-Output $line }
    if ($line -match '^protocol_max=') { Write-Output $line }
  }
  $help = & $resolved --help 2>$null
  if ($help -match '--remote') { Write-Output 'speaks_remote=1' } else { Write-Output 'speaks_remote=0' }
}
if ($env:ROZI_PROBE_BIN) { Try-Bin $env:ROZI_PROBE_BIN }
Try-Bin 'rozi.exe'
Try-Bin (Join-Path $env:USERPROFILE '.local\bin\rozi.exe')
Try-Bin (Join-Path $env:USERPROFILE '.cargo\bin\rozi.exe')
$dataHome = $env:LOCALAPPDATA
if (-not $dataHome) { $dataHome = Join-Path $env:USERPROFILE '.local\share' }
$managedRoot = Join-Path $dataHome 'rozi\remote'
Get-ChildItem -LiteralPath $managedRoot -Directory | ForEach-Object {
  Try-Bin (Join-Path $_.FullName 'rozi.exe')
}
Write-Output "probe_done=1"
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
            "no rozi binary found on the remote host".to_string()
        } else if saw_without_range {
            // `saw_without_range` is only ever set inside a `speaks_remote` candidate, so it always
            // implies `saw_remote` — a `saw_without_range && !saw_remote` arm here would be dead.
            "remote rozi found but does not advertise a protocol range (upgrade it, or set binary_path / install)"
                .to_string()
        } else if saw_remote {
            "remote rozi protocol range does not overlap this client".to_string()
        } else {
            "remote rozi binaries are too old for --remote (need a build that speaks --remote-serve)"
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
                    "{detail}; set [remote] install = \"prompt\" or install rozi on the remote host"
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

/// Run the probe over a short-lived ssh command. A shell-safe `binary_path` short-circuits with our
/// protocol range; unsafe configured tokens are rejected before any remote command is spawned.
pub fn probe_remote_report(
    target: &RemoteTarget,
    config: &RemoteConfig,
) -> Result<ProbeReport, String> {
    probe_remote_report_with_connect_timeout(target, config, config.connection_timeout_secs)
}

fn probe_remote_report_with_connect_timeout(
    target: &RemoteTarget,
    config: &RemoteConfig,
    connect_timeout_secs: u64,
) -> Result<ProbeReport, String> {
    validate_remote_target(target)?;
    let resolved = ResolvedRemote::resolve(target, config);
    if let Some(path) = &resolved.binary_path {
        validate_remote_executable_token(path)?;
        if !program_exists("ssh") {
            return Err("ssh was not found on PATH (required for --remote)".to_string());
        }
        let family = detect_remote_family(&resolved, config, connect_timeout_secs)?;
        return Ok(ProbeReport {
            platform: if family == RemoteFamily::Windows {
                "windows".to_string()
            } else {
                local_uname_platform()
            },
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
    // The remote sshd default shell is not always POSIX (Windows defaults to `cmd.exe`). Detect the
    // family with one fixed, shell-agnostic probe, then feed the matching script to the matching
    // interpreter. Probe output is still parsed with fixed keys and never treated as argv.
    let stdout = match detect_remote_family(&resolved, config, connect_timeout_secs)? {
        // PowerShell's `-Command -` truncates a multi-line script read from stdin (only the first
        // statements run) over OpenSSH-for-Windows; pass the script as a base64 `-EncodedCommand`
        // instead, which runs the whole thing and needs no stdin.
        RemoteFamily::Windows => run_probe_command(
            &resolved,
            config,
            connect_timeout_secs,
            &[
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encode_powershell_command(WINDOWS_PROBE_SCRIPT),
            ],
        )?,
        RemoteFamily::Posix => run_probe_script(
            &resolved,
            config,
            connect_timeout_secs,
            &["sh", "-s"],
            PROBE_SCRIPT,
        )?,
    };
    Ok(parse_probe_output(&stdout))
}

/// Remote sshd default-shell family, chosen up front so the probe/install scripts target the right
/// interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFamily {
    Posix,
    Windows,
}

/// Detect the remote shell family by explicitly invoking each platform interpreter. This does not
/// depend on whether OpenSSH-for-Windows was configured with cmd.exe or PowerShell as its default
/// shell.
fn detect_remote_family(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    connect_timeout_secs: u64,
) -> Result<RemoteFamily, String> {
    let powershell_probe = encode_powershell_command(WINDOWS_FAMILY_PROBE_SCRIPT);
    if let Ok(stdout) = run_family_probe(
        resolved,
        config,
        connect_timeout_secs,
        &[
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &powershell_probe,
        ],
    ) && stdout
        .lines()
        .any(|line| line.trim() == "rozi_family=windows")
    {
        return Ok(RemoteFamily::Windows);
    }

    let stdout = run_family_probe(
        resolved,
        config,
        connect_timeout_secs,
        &["sh", "-c", "'echo rozi_family=posix'"],
    )?;
    if stdout
        .lines()
        .any(|line| line.trim() == "rozi_family=posix")
    {
        return Ok(RemoteFamily::Posix);
    }
    Err(format!(
        "remote shell probe of {} found neither PowerShell nor a POSIX shell",
        resolved.host
    ))
}

fn run_family_probe(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    connect_timeout_secs: u64,
    argv: &[&str],
) -> Result<String, String> {
    let mut command = ssh_base_command_with_connect_timeout(resolved, config, connect_timeout_secs);
    append_ssh_destination(&mut command, resolved);
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().map_err(|err| {
        format!(
            "failed to probe remote shell family for {}: {err}",
            resolved.host
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "remote shell probe of {} failed: {}",
            resolved.host,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Pipe `script` to a remote `interpreter` over ssh stdin and return its stdout (POSIX probe).
fn run_probe_script(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    connect_timeout_secs: u64,
    interpreter: &[&str],
    script: &str,
) -> Result<String, String> {
    let mut command = ssh_base_command_with_connect_timeout(resolved, config, connect_timeout_secs);
    append_ssh_destination(&mut command, resolved);
    for arg in interpreter {
        command.arg(arg);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to probe {}: {err}", resolved.host))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|err| format!("failed to write probe script: {err}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("probe ssh failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "probe of {} failed: {}",
            resolved.host,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a self-contained remote `argv` over ssh with no stdin and return its stdout (Windows probe,
/// whose script is carried in the argv as an `-EncodedCommand` rather than piped on stdin).
fn run_probe_command(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    connect_timeout_secs: u64,
    argv: &[&str],
) -> Result<String, String> {
    let mut command = ssh_base_command_with_connect_timeout(resolved, config, connect_timeout_secs);
    append_ssh_destination(&mut command, resolved);
    for arg in argv {
        command.arg(arg);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|err| format!("failed to probe {}: {err}", resolved.host))?;
    if !output.status.success() {
        return Err(format!(
            "probe of {} failed: {}",
            resolved.host,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[allow(dead_code)] // CLI / test helper alongside probe_remote_report
pub fn probe_remote(target: &RemoteTarget, config: &RemoteConfig) -> Result<ProbeResult, String> {
    Ok(select_compatible(&probe_remote_report(target, config)?))
}

/// Shell startup entry point. Non-interactive invocations never install implicitly.
pub fn ensure_remote_binary(
    target: &RemoteTarget,
    config: &RemoteConfig,
    interactive: bool,
) -> Result<String, String> {
    ensure_with_confirmation(target, config, interactive, |report, ask| {
        if !ask {
            return Ok(true);
        }
        let host = ResolvedRemote::resolve(target, config).ssh_destination();
        let destination = install_destination(report);
        let mut stderr = io::stderr().lock();
        write!(
            stderr,
            "rozi: install compatible Rozi on {host} at {destination}? [y/N] "
        )
        .map_err(|error| error.to_string())?;
        stderr.flush().map_err(|error| error.to_string())?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    })
}

/// Explicit TUI connections may install; background discovery uses the read-only resolver.
pub(crate) fn ensure_remote_binary_in_ui(
    target: &RemoteTarget,
    config: &RemoteConfig,
    probe_epoch: Option<u64>,
) -> Result<String, String> {
    ensure_with_confirmation(target, config, true, |report, ask| {
        if !ask {
            return Ok(true);
        }
        let host = ResolvedRemote::resolve(target, config).ssh_destination();
        super::askpass::confirm_install(
            format!(
                "Host: {host}\nDestination: {}\nVersion: {}",
                install_destination(report),
                env!("CARGO_PKG_VERSION")
            ),
            probe_epoch,
        )
    })
}

fn install_destination(report: &ProbeReport) -> String {
    if normalize_os(&report.platform) == "windows" {
        format!(
            r"%LOCALAPPDATA%\rozi\remote\{}\rozi.exe",
            env!("CARGO_PKG_VERSION")
        )
    } else {
        format!(
            "${{XDG_DATA_HOME:-$HOME/.local/share}}/rozi/remote/{}/{INSTALL_NAME}",
            env!("CARGO_PKG_VERSION")
        )
    }
}

fn ensure_with_confirmation(
    target: &RemoteTarget,
    config: &RemoteConfig,
    interactive: bool,
    confirm: impl FnOnce(&ProbeReport, bool) -> Result<bool, String>,
) -> Result<String, String> {
    if let Ok(path) = std::env::var("ROZI_REMOTE_BINARY") {
        let local = Path::new(&path);
        if !local.is_file() {
            return Err(format!("ROZI_REMOTE_BINARY={path} is not a regular file"));
        }
        let report = probe_remote_report(target, config)?;
        verify_override_targets_remote(local, &report)?;
        let family = family_from_os(&normalize_os(&report.platform));
        let path = install_bytes(target, config, local, "ROZI_REMOTE_BINARY override", family)?;
        return verify_installed(target, config, path);
    }
    if let Some(path) = super::binary::cached(target, config) {
        return Ok(path.path);
    }
    let report = probe_remote_report(target, config)?;
    let decision = decide_install(&select_compatible(&report), config.install, interactive);
    let ask = match decision {
        InstallDecision::Use { path } => {
            let family = family_from_os(&normalize_os(&report.platform));
            return super::binary::remember(target, config, path, family).map(|binary| binary.path);
        }
        InstallDecision::Fail { message } => return Err(message),
        InstallDecision::Install => false,
        InstallDecision::Ask => true,
    };
    if !confirm(&report, ask)? {
        return Err(format!(
            "remote installation cancelled for {}",
            target.display_label()
        ));
    }
    let path = install_for_platforms(target, config, &report)?;
    verify_installed(target, config, path)
}

fn verify_installed(
    target: &RemoteTarget,
    config: &RemoteConfig,
    path: String,
) -> Result<String, String> {
    super::binary::invalidate(target, config);
    let report = probe_remote_report(target, config)?;
    let family = family_from_os(&normalize_os(&report.platform));
    let candidates = report
        .candidates
        .iter()
        .map(|candidate| candidate.path.clone())
        .collect::<Vec<_>>();
    let installed = ProbeReport {
        candidates: report
            .candidates
            .into_iter()
            .filter(|candidate| same_remote_path(&candidate.path, &path, family))
            .collect(),
        ..report
    };
    match select_compatible(&installed) {
        ProbeResult::Found { path, .. } => {
            super::binary::remember(target, config, path, family).map(|binary| binary.path)
        }
        ProbeResult::Missing { detail } => Err(format!(
            "installed Rozi at {path:?} could not be verified on the remote host: {detail}; probe candidates: {candidates:?}"
        )),
    }
}

fn same_remote_path(left: &str, right: &str, family: RemoteFamily) -> bool {
    match family {
        RemoteFamily::Posix => left == right,
        RemoteFamily::Windows => left
            .replace('/', "\\")
            .eq_ignore_ascii_case(&right.replace('/', "\\")),
    }
}

fn install_for_platforms(
    target: &RemoteTarget,
    config: &RemoteConfig,
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

    let family = family_from_os(&remote_os);
    if local_os == remote_os && local_arch == remote_arch {
        let local = std::env::current_exe()
            .map_err(|err| format!("cannot locate local rozi for install: {err}"))?;
        return install_bytes(target, config, &local, "same-platform current_exe", family);
    }

    let triple = rustc_target(&remote_os, &remote_arch).ok_or_else(|| {
        format!(
            "no release artifact mapping for remote platform {remote_os}/{remote_arch}; set binary_path or ROZI_REMOTE_BINARY"
        )
    })?;
    let version = env!("CARGO_PKG_VERSION");
    let (_download_dir, local_artifact) = download_release_binary(triple, version)?;
    install_bytes(
        target,
        config,
        &local_artifact,
        &format!("release asset {triple}"),
        family,
    )
}

pub(crate) fn family_from_os(os: &str) -> RemoteFamily {
    if os == "windows" {
        RemoteFamily::Windows
    } else {
        RemoteFamily::Posix
    }
}

/// Stream `local` onto the remote and return the installed path.
///
/// The payload is staged and executed before it is moved into Rozi's private, versioned runtime
/// directory. The returned path is the resolved path reported by the remote host.
fn install_bytes(
    target: &RemoteTarget,
    config: &RemoteConfig,
    local: &Path,
    _source: &str,
    family: RemoteFamily,
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
    match family {
        RemoteFamily::Posix => install_bytes_posix(&resolved, config, local),
        RemoteFamily::Windows => install_bytes_windows(&resolved, config, local),
    }
}

/// Stream the binary onto a POSIX remote, validate it in place, then atomically activate it.
fn install_bytes_posix(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    local: &Path,
) -> Result<String, String> {
    let version = env!("CARGO_PKG_VERSION");
    let script = format!(
        r#"set -e
if [ -n "${{XDG_DATA_HOME:-}}" ] && [ "${{XDG_DATA_HOME#/}}" != "$XDG_DATA_HOME" ]; then
  data_home="$XDG_DATA_HOME"
else
  data_home="$HOME/.local/share"
fi
dir="$data_home/rozi/remote/{version}"
final="$dir/{INSTALL_NAME}"
mkdir -p "$dir"
if [ -L "$final" ] || {{ [ -e "$final" ] && [ ! -f "$final" ]; }}; then
  printf 'refuse_non_regular=%s\n' "$final" >&2
  exit 1
fi
tmp=$(mktemp "$dir/.rozi.install.XXXXXX")
trap 'rm -f "$tmp"' EXIT HUP INT TERM
cat > "$tmp"
chmod 755 "$tmp"
out=$("$tmp" --version 2>/dev/null) || {{ printf 'staged_binary_failed=%s\n' "$tmp" >&2; exit 1; }}
protocol_min=$(printf '%s\n' "$out" | sed -n 's/^protocol_min=//p' | sed -n '1p')
protocol_max=$(printf '%s\n' "$out" | sed -n 's/^protocol_max=//p' | sed -n '1p')
case "$protocol_min:$protocol_max" in *[!0-9:]*|:|*:|:*)
  printf 'staged_binary_has_no_protocol_range=%s\n' "$tmp" >&2
  exit 1
esac
if [ "$protocol_max" -lt {MIN_SUPPORTED_PROTOCOL} ] || [ "$protocol_min" -gt {PROTOCOL_VERSION} ]; then
  printf 'staged_binary_protocol_mismatch=%s:%s\n' "$protocol_min" "$protocol_max" >&2
  exit 1
fi
if ! "$tmp" --help 2>/dev/null | grep -q -- '--remote'; then
  printf 'staged_binary_has_no_remote_support=%s\n' "$tmp" >&2
  exit 1
fi
mv -f "$tmp" "$final"
printf 'installed=%s\n' "$final"
"#
    );
    let mut command = ssh_base_command(resolved, config);
    append_ssh_destination(&mut command, resolved);
    // OpenSSH joins argv with spaces before the remote shell parses it.
    command
        .arg("sh")
        .arg("-c")
        .arg(format!("'{}'", script.replace('\'', "'\"'\"'")));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut file = std::fs::File::open(local)
        .map_err(|err| format!("cannot read {}: {err}", local.display()))?;
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to start remote install: {err}"))?;
    let transfer = {
        let mut stdin = child.stdin.take().expect("piped install stdin");
        io::copy(&mut file, &mut stdin)
    };
    let output = child
        .wait_with_output()
        .map_err(|err| format!("remote install failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "remote install failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    transfer.map_err(|err| format!("upload binary: {err}"))?;
    parse_installed_path(&String::from_utf8_lossy(&output.stdout))
}

/// Install onto a Windows remote in two steps: `scp` the binary to a temp file (the sftp subsystem
/// has real flow control), then a small no-stdin `powershell -EncodedCommand` that moves it into
/// Rozi's private managed-runtime directory.
///
/// Streaming the binary through a command's stdin — as the POSIX path does — deadlocks on
/// OpenSSH-for-Windows once the data exceeds the channel's stdin buffer (a real ~11 MB binary hangs
/// hard). `scp` sidesteps that entirely.
fn install_bytes_windows(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    local: &Path,
) -> Result<String, String> {
    if !program_exists("scp") {
        return Err("scp was not found on PATH (required to install onto a Windows remote)".into());
    }
    // PowerShell only executes a PE payload directly when the staged path retains an executable
    // extension. Keep the random component while ending in `.exe`.
    let temp_name = windows_temp_name();

    let mut scp = scp_base_command(resolved, config);
    scp.arg(local);
    scp.arg(format!("{}:{temp_name}", resolved.ssh_destination()));
    scp.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let scp_out = scp
        .output()
        .map_err(|err| format!("failed to run scp to {}: {err}", resolved.host))?;
    if !scp_out.status.success() {
        return Err(format!(
            "scp upload to {} failed: {}",
            resolved.host,
            String::from_utf8_lossy(&scp_out.stderr).trim()
        ));
    }

    let version = env!("CARGO_PKG_VERSION");
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System.Text;
using System.Runtime.InteropServices;
public static class RoziLongPathName {{
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern uint GetLongPathName(string shortPath, StringBuilder longPath, uint bufferLength);
}}
'@ -ErrorAction SilentlyContinue
function Get-RoziLongPath($path) {{
  $buffer = New-Object System.Text.StringBuilder 32768
  $length = [RoziLongPathName]::GetLongPathName($path, $buffer, [uint32]$buffer.Capacity)
  if ($length -gt 0 -and $length -lt $buffer.Capacity) {{ return $buffer.ToString() }}
  return $path
}}
$dataHome = $env:LOCALAPPDATA
if (-not $dataHome) {{ $dataHome = Join-Path $env:USERPROFILE '.local\share' }}
$dir = Join-Path $dataHome 'rozi\remote\{version}'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$final = Join-Path $dir 'rozi.exe'
$src = Join-Path $env:USERPROFILE '{temp_name}'
try {{
  if (Test-Path -LiteralPath $final) {{
    $item = Get-Item -Force -LiteralPath $final
    if (-not ($item -is [System.IO.FileInfo]) -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {{
      throw "refuse_non_regular=$final"
    }}
  }}
  $out = & $src --version 2>$null
  if ($LASTEXITCODE -ne 0) {{ throw "staged binary failed: $src" }}
  $protocolMin = $null
  $protocolMax = $null
  foreach ($line in $out) {{
    if ($line -match '^protocol_min=([0-9]+)$') {{ $protocolMin = [uint32]$Matches[1] }}
    if ($line -match '^protocol_max=([0-9]+)$') {{ $protocolMax = [uint32]$Matches[1] }}
  }}
  if ($null -eq $protocolMin -or $null -eq $protocolMax) {{ throw "staged binary has no protocol range: $src" }}
  if ($protocolMax -lt {MIN_SUPPORTED_PROTOCOL} -or $protocolMin -gt {PROTOCOL_VERSION}) {{
    throw "staged binary protocol mismatch: $protocolMin..$protocolMax"
  }}
  $help = & $src --help 2>$null
  if ($LASTEXITCODE -ne 0 -or -not ($help -match '--remote')) {{ throw "staged binary has no remote support: $src" }}
  Move-Item -Force -LiteralPath $src -Destination $final
  $installed = Get-RoziLongPath (Resolve-Path -LiteralPath $final).Path
  Write-Output "installed=$installed"
}} finally {{
  if (Test-Path -LiteralPath $src) {{ Remove-Item -Force -LiteralPath $src -ErrorAction SilentlyContinue }}
}}"#
    );
    let mut command = ssh_base_command(resolved, config);
    append_ssh_destination(&mut command, resolved);
    command
        .arg("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-EncodedCommand")
        .arg(encode_powershell_command(&script));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|err| format!("remote install finalize failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "remote install finalize failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_installed_path(&String::from_utf8_lossy(&output.stdout))
}

fn parse_installed_path(stdout: &str) -> Result<String, String> {
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("installed=") {
            return Ok(path.trim().to_string());
        }
    }
    Err("remote install succeeded but did not report installed= path".to_string())
}

fn random_hex_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    token
}

fn windows_temp_name() -> String {
    format!("rozi.install.{}.tmp.exe", random_hex_token())
}

/// `scp` argv mirroring [`ssh_base_command`]'s connection options (scp uses `-P` for the port, not
/// `-p`). `ssh_args` are passed through — they are `-o key=value` pairs scp also accepts.
fn scp_base_command(resolved: &ResolvedRemote, config: &RemoteConfig) -> Command {
    let mut command = Command::new("scp");
    super::askpass::configure(&mut command);
    // Same control socket as ssh: an upload rides the connection the probe already authenticated.
    apply_multiplexing(&mut command);
    if config.batch_mode {
        command.arg("-o").arg("BatchMode=yes");
    }
    if config.connection_timeout_secs > 0 {
        command
            .arg("-o")
            .arg(format!("ConnectTimeout={}", config.connection_timeout_secs));
    }
    if let Some(port) = resolved.port {
        command.arg("-P").arg(port.to_string());
    }
    if let Some(identity) = &resolved.identity_file {
        command.arg("-i").arg(crate::config::expand_path(identity));
    }
    for arg in &resolved.ssh_args {
        command.arg(arg);
    }
    command
}

/// Encode a PowerShell script for `powershell -EncodedCommand`: UTF-16LE bytes, then standard
/// base64. This is quoting-proof, which matters when the outer transport is cmd.exe over ssh.
fn encode_powershell_command(script: &str) -> String {
    let mut utf16 = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    base64_standard(&utf16)
}

/// Minimal standard-alphabet base64 (with `=` padding). Kept in-crate rather than adding a direct
/// dependency for one fixed PowerShell command.
fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn download_release_binary(
    triple: &str,
    version: &str,
) -> Result<(tempfile::TempDir, PathBuf), String> {
    download_release_binary_with(&relswap::UreqDownloader::new(), triple, version)
}

fn download_release_binary_with(
    downloader: &impl Downloader,
    triple: &str,
    version: &str,
) -> Result<(tempfile::TempDir, PathBuf), String> {
    let base = std::env::var("ROZI_RELEASE_BASE_URL").unwrap_or_else(|_| {
        format!("https://github.com/{RELEASE_REPO}/releases/download/v{version}")
    });
    download_release_binary_from_base_with(downloader, triple, version, &base)
}

fn download_release_binary_from_base_with(
    downloader: &impl Downloader,
    triple: &str,
    version: &str,
    base: &str,
) -> Result<(tempfile::TempDir, PathBuf), String> {
    let mut release_base =
        Url::parse(base).map_err(|error| format!("invalid ROZI_RELEASE_BASE_URL: {error}"))?;
    if release_base.scheme() != "https" {
        return Err(format!(
            "release downloads require HTTPS, got {}",
            release_base.scheme()
        ));
    }
    if release_base.query().is_some() || release_base.fragment().is_some() {
        return Err("release base URL must not contain a query or fragment".to_string());
    }
    if !release_base.path().ends_with('/') {
        release_base.set_path(&format!("{}/", release_base.path()));
    }
    let version = semver::Version::parse(version)
        .map_err(|error| format!("invalid client release version: {error}"))?;
    let target = relswap::Target::from_str(triple).map_err(|error| error.to_string())?;

    let manifest_url = release_base
        .join(&ROZI.metadata_filename())
        .map_err(|error| format!("invalid release manifest URL: {error}"))?;
    let signature_url = release_base
        .join(&ROZI.signature_filename())
        .map_err(|error| format!("invalid release signature URL: {error}"))?;
    let manifest = downloader
        .fetch(&manifest_url, relswap::MAX_METADATA_SIZE)
        .map_err(|error| format!("download signed release manifest: {error}"))?;
    let signature = downloader
        .fetch(&signature_url, relswap::MAX_METADATA_SIZE)
        .map_err(|error| format!("download release signatures: {error}"))?;
    relswap::verify_manifest(&ROZI, &manifest.bytes, &signature.bytes)
        .map_err(|error| format!("authenticate release manifest: {error}"))?;
    let manifest = relswap::ReleaseManifest::from_bytes(&ROZI, &manifest.bytes)
        .map_err(|error| format!("read signed release manifest: {error}"))?;
    manifest
        .ensure_not_expired(chrono::Utc::now())
        .map_err(|error| format!("accept signed release manifest: {error}"))?;
    if manifest.version != version {
        return Err(format!(
            "signed release manifest describes {}, expected {version}",
            manifest.version
        ));
    }
    let asset = manifest
        .asset_for(&ROZI, target)
        .map_err(|error| format!("select release target {triple}: {error}"))?;
    let archive_url = release_base
        .join(asset.archive())
        .map_err(|error| format!("invalid release archive URL: {error}"))?;
    let archive = downloader
        .fetch(&archive_url, relswap::MAX_ARCHIVE_SIZE as usize)
        .map_err(|error| format!("download release archive: {error}"))?;
    let extracted = relswap::inspect_archive(&ROZI, &archive.bytes, asset.asset)
        .map_err(|error| format!("authenticate release archive: {error}"))?;

    let download_dir = tempfile::Builder::new()
        .prefix("rozi-remote-install-")
        .tempdir()
        .map_err(|error| format!("temp dir: {error}"))?;
    let tmp = download_dir.path();
    let bin_name = if target.is_windows() {
        "rozi.exe"
    } else {
        "rozi"
    };
    let binary = tmp.join(bin_name);
    std::fs::write(&binary, extracted.payload.data)
        .map_err(|error| format!("write authenticated release payload: {error}"))?;
    Ok((download_dir, binary))
}

/// Common ssh argv for every remote invocation: no tty, timeouts, and the per-host options.
///
/// `BatchMode` comes from `[remote] batch_mode` (default on). It is the single place that decides
/// whether ssh may prompt, so probe, install, attach, list, and kill all agree — a mix would mean
/// a host that lists fine but hangs on attach.
///
/// Being that single place is also why the askpass redirect is installed here: a prompt that
/// escaped even one of those invocations would land on the terminal the TUI is drawing on. See
/// [`super::askpass`].
pub(crate) fn ssh_base_command(resolved: &ResolvedRemote, config: &RemoteConfig) -> Command {
    ssh_base_command_with_connect_timeout(resolved, config, config.connection_timeout_secs)
}

/// Like [`ssh_base_command`], but with an explicit `ConnectTimeout` so a reconnect attempt can
/// spend only the remaining deadline rather than the configured default.
pub(crate) fn ssh_base_command_with_connect_timeout(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    connection_timeout_secs: u64,
) -> Command {
    let mut command = Command::new("ssh");
    command.arg("-T");
    super::askpass::configure(&mut command);
    apply_multiplexing(&mut command);
    // On every invocation, not just the attach: with multiplexing the *first* connection to a host
    // becomes the master that all the others ride on, and a client's keepalive settings are ignored
    // in favour of the master's. Setting them only on the attach would leave a session whose
    // liveness depends on a master the probe opened without any.
    command
        .arg("-o")
        .arg(format!(
            "ServerAliveInterval={}",
            config.server_alive_interval_secs
        ))
        .arg("-o")
        .arg(format!(
            "ServerAliveCountMax={}",
            config.server_alive_count_max
        ));
    if config.batch_mode {
        command.arg("-o").arg("BatchMode=yes");
    }
    if connection_timeout_secs > 0 {
        command
            .arg("-o")
            .arg(format!("ConnectTimeout={connection_timeout_secs}"));
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

/// How long a shared connection outlives the command that opened it. Long enough to carry a probe
/// straight into the attach that follows it, short enough that quitting rozi leaves nothing behind
/// worth noticing.
///
/// Unix only, like the multiplexing it configures: OpenSSH for Windows has none.
#[cfg(unix)]
const CONTROL_PERSIST_SECS: u32 = 60;

/// Share one authenticated connection across every ssh a single remote operation runs.
///
/// Opening a host is not one ssh. It is a shell-family probe, a capability probe, a re-probe before
/// the attach, and the attach — each its own connection, each its own authentication. On a host
/// with a key or an agent that is invisible; on one that asks for a password it means typing the
/// password four times to open a session, and it is the difference between a remote host being
/// usable and being a chore.
///
/// `ControlMaster=auto` makes the first connection the master and every later one a client riding
/// on it, so the password is asked once. It also removes several seconds of handshake from every
/// subsequent invocation.
///
/// Unix only: OpenSSH for Windows has no connection multiplexing, so a Windows client authenticates
/// per invocation exactly as before. The socket lives in the runtime directory, which is already
/// private to this user; without one - or without room in it for the socket - multiplexing is
/// simply skipped.
#[cfg(not(unix))]
fn apply_multiplexing(_command: &mut Command) {}

#[cfg(unix)]
fn apply_multiplexing(command: &mut Command) {
    let Ok(dir) = crate::control::runtime_dir() else {
        return;
    };
    let Some(control_path) = multiplexing_control_path(&dir) else {
        return;
    };
    command
        .arg("-o")
        .arg("ControlMaster=auto")
        .arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-o")
        .arg(format!("ControlPersist={CONTROL_PERSIST_SECS}"));
}

/// The `ControlPath` to multiplex through in `dir`, or `None` when `dir` cannot hold the socket.
///
/// `%C` is a hash of user/host/port/proxy, so one master per distinct destination, and a path short
/// enough for the socket-name limit however long the destination is. What ssh binds is still longer
/// than what it is handed: `%C` expands to 40 hex characters, and the socket is created under a
/// temporary `.<16 random>` suffix first.
///
/// A path that does not fit is not a multiplexing problem. ssh exits, and it is the *attach* that
/// fails - a convenience feature taking the whole connection down with it, which is what a 48-byte
/// macOS `TMPDIR` used to do. Returning `None` degrades to one authentication per invocation, the
/// same as a Windows client and the same as having no runtime directory at all.
#[cfg(unix)]
fn multiplexing_control_path(dir: &Path) -> Option<String> {
    /// `%C`, as OpenSSH expands it: SHA-1 of the destination tuple, in hex.
    const EXPANDED_HASH_LEN: usize = 40;
    /// The `.XXXXXXXXXXXXXXXX` OpenSSH binds under before renaming the socket into place.
    const TEMPORARY_SUFFIX_LEN: usize = 17;

    let control_path = dir.join("ssh-%C").to_str()?.to_string();
    let bound_len = control_path.len() - "%C".len() + EXPANDED_HASH_LEN + TEMPORARY_SUFFIX_LEN;
    (bound_len <= crate::platform::ipc::MAX_ENDPOINT_PATH_LEN).then_some(control_path)
}

/// Append the OpenSSH end-of-options marker and destination. OpenSSH expects the destination before
/// the remote command; putting `--` after the destination makes it part of that command instead.
pub(crate) fn append_ssh_destination(command: &mut Command, resolved: &ResolvedRemote) {
    command.arg("--").arg(resolved.ssh_destination());
}

/// Append one Rozi invocation using quoting for the detected remote shell family. Windows
/// invocation goes through encoded PowerShell so it works with either cmd.exe or PowerShell as
/// sshd's configured default shell.
pub(crate) fn append_remote_rozi_command(
    command: &mut Command,
    binary: &str,
    args: &[&str],
    family: RemoteFamily,
) {
    match family {
        RemoteFamily::Posix => {
            command.arg(shell_quote_posix(binary));
            for arg in args {
                command.arg(shell_quote_posix(arg));
            }
        }
        RemoteFamily::Windows => {
            let mut words = vec![binary];
            words.extend_from_slice(args);
            let script = format!(
                "$words = @({}); $exe = $words[0]; $remoteArgs = @($words | Select-Object -Skip 1); & $exe @remoteArgs; exit $LASTEXITCODE",
                words
                    .iter()
                    .map(|word| powershell_literal(word))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            command
                .arg("powershell")
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-EncodedCommand")
                .arg(encode_powershell_command(&script));
        }
    }
}

fn shell_quote_posix(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\"'\"'"))
}

fn powershell_literal(word: &str) -> String {
    format!("'{}'", word.replace('\'', "''"))
}

/// Fail if the `ROZI_REMOTE_BINARY` override is a binary built for a different OS/arch than the
/// remote host. Best-effort: an unrecognized executable format or an unknown remote platform is not
/// treated as a mismatch, so this only blocks a confirmed wrong-target upload.
fn verify_override_targets_remote(local: &Path, report: &ProbeReport) -> Result<(), String> {
    let remote_os = normalize_os(&report.platform);
    let remote_arch = normalize_arch(&report.machine);
    if remote_os == "unknown" || remote_arch == "unknown" {
        return Ok(());
    }
    let Some((bin_os, bin_arch)) = detect_binary_target(local) else {
        return Ok(());
    };
    if bin_os != remote_os || bin_arch != remote_arch {
        return Err(format!(
            "ROZI_REMOTE_BINARY={} targets {bin_os}/{bin_arch}, but the remote host is {remote_os}/{remote_arch}; provide a binary built for the remote platform",
            local.display()
        ));
    }
    Ok(())
}

/// Sniff an executable's target `(os, arch)` from its header, normalized to the same vocabulary as
/// [`normalize_os`]/[`normalize_arch`]. Returns `None` for a format we do not recognize.
fn detect_binary_target(path: &Path) -> Option<(String, String)> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 64];
    let read = file.read(&mut head).ok()?;
    let head = &head[..read];
    binary_target_from_header(head, path)
}

fn binary_target_from_header(head: &[u8], path: &Path) -> Option<(String, String)> {
    // ELF: 0x7f 'E' 'L' 'F', e_machine at offset 18 (little-endian when EI_DATA == 1).
    if head.len() >= 20 && head[..4] == [0x7f, b'E', b'L', b'F'] {
        let machine = u16::from_le_bytes([head[18], head[19]]);
        let arch = match machine {
            0x3e => "x86_64",
            0xb7 => "aarch64",
            _ => return None,
        };
        return Some((elf_os(head, path)?, arch.to_string()));
    }
    // Mach-O (macOS): 64-bit magic FEEDFACF (either endianness), cputype in the next 4 bytes.
    if head.len() >= 8
        && (head[..4] == [0xcf, 0xfa, 0xed, 0xfe] || head[..4] == [0xfe, 0xed, 0xfa, 0xcf])
    {
        let cputype = if head[..4] == [0xcf, 0xfa, 0xed, 0xfe] {
            u32::from_le_bytes([head[4], head[5], head[6], head[7]])
        } else {
            u32::from_be_bytes([head[4], head[5], head[6], head[7]])
        };
        let arch = match cputype {
            0x0100_0007 => "x86_64",
            0x0100_000c => "aarch64",
            _ => return None,
        };
        return Some(("macos".to_string(), arch.to_string()));
    }
    // PE (Windows): "MZ", a 4-byte PE-header offset at 0x3c, then "PE\0\0" + a 2-byte machine field.
    if head.len() >= 2 && head[..2] == *b"MZ" {
        return pe_target_from_file(path);
    }
    None
}

/// The operating system an ELF executable targets, or `None` when the file does not say.
///
/// ELF has a byte for this, `e_ident[EI_OSABI]`, and it is very nearly useless: Linux, NetBSD and
/// FreeBSD toolchains all routinely leave it `ELFOSABI_NONE`. Reading every ELF as Linux because
/// of that is what refused a NetBSD-built rozi as a wrong-target upload (issue #3), the same
/// "Linux, or else" shape as the `__errno` bug that opened it.
///
/// So take the byte when it is set, and otherwise ask the vendor note each system stamps into a
/// `PT_NOTE` segment. A file that answers neither way is `None`, which leaves the upload alone:
/// this check exists to block a *confirmed* mismatch, and an unreadable file confirms nothing.
fn elf_os(head: &[u8], path: &Path) -> Option<String> {
    const ELFOSABI_NONE: u8 = 0;
    const ELFOSABI_NETBSD: u8 = 2;
    const ELFOSABI_GNU: u8 = 3;
    const ELFOSABI_SOLARIS: u8 = 6;
    const ELFOSABI_FREEBSD: u8 = 9;
    const ELFOSABI_OPENBSD: u8 = 12;

    let os = match *head.get(7)? {
        ELFOSABI_NETBSD => "netbsd",
        ELFOSABI_GNU => "linux",
        ELFOSABI_SOLARIS => "solaris",
        ELFOSABI_FREEBSD => "freebsd",
        ELFOSABI_OPENBSD => "openbsd",
        ELFOSABI_NONE => return elf_os_from_notes(head, path),
        _ => return None,
    };
    Some(os.to_string())
}

/// Walk the `PT_NOTE` segments of `path` and return the OS the first recognised vendor note names.
fn elf_os_from_notes(head: &[u8], path: &Path) -> Option<String> {
    /// One note segment big enough to hold the identifying notes and small enough that a corrupt
    /// or hostile `p_filesz` cannot ask us to buffer the machine.
    const MAX_NOTE_BYTES: u64 = 64 * 1024;
    const PT_NOTE: u32 = 4;

    let sixty_four = match *head.get(4)? {
        1 => false,
        2 => true,
        _ => return None,
    };
    let little = match *head.get(5)? {
        1 => true,
        2 => false,
        _ => return None,
    };

    let read_u16 = |b: &[u8]| -> u16 {
        let raw = [b[0], b[1]];
        if little {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        }
    };

    let (phoff_at, phentsize_at, phnum_at) = if sixty_four {
        (32, 54, 56)
    } else {
        (28, 42, 44)
    };
    let phoff = read_elf_addr(head.get(phoff_at..)?, sixty_four, little)?;
    let phentsize = read_u16(head.get(phentsize_at..phentsize_at + 2)?) as u64;
    let phnum = read_u16(head.get(phnum_at..phnum_at + 2)?) as u64;
    if phentsize == 0 {
        return None;
    }

    let mut file = std::fs::File::open(path).ok()?;
    for index in 0..phnum {
        let mut entry = vec![0u8; phentsize as usize];
        seek_read(
            &mut file,
            phoff.checked_add(index.checked_mul(phentsize)?)?,
            &mut entry,
        )
        .ok()?;
        let p_type = read_elf_u32(entry.get(0..4)?, little);
        if p_type != PT_NOTE {
            continue;
        }
        let (offset_at, filesz_at) = if sixty_four { (8, 32) } else { (4, 16) };
        let offset = read_elf_addr(entry.get(offset_at..)?, sixty_four, little)?;
        let size = read_elf_addr(entry.get(filesz_at..)?, sixty_four, little)?;
        if size == 0 || size > MAX_NOTE_BYTES {
            continue;
        }
        let mut notes = vec![0u8; size as usize];
        if seek_read(&mut file, offset, &mut notes).is_err() {
            continue;
        }
        if let Some(os) = os_from_note_segment(&notes, little) {
            return Some(os.to_string());
        }
    }
    None
}

/// The OS named by the first recognised note in one `PT_NOTE` segment.
///
/// Each note is `namesz`, `descsz`, `type`, then the name and descriptor, both padded to four
/// bytes. Only the name is needed here: every system stamps its own.
fn os_from_note_segment(notes: &[u8], little: bool) -> Option<&'static str> {
    let align = |n: usize| n.div_ceil(4) * 4;
    let mut at = 0usize;
    while at + 12 <= notes.len() {
        let namesz = read_elf_u32(notes.get(at..at + 4)?, little) as usize;
        let descsz = read_elf_u32(notes.get(at + 4..at + 8)?, little) as usize;
        let name_at = at + 12;
        let name = notes.get(name_at..name_at.checked_add(namesz)?)?;
        let name = std::str::from_utf8(name).ok()?.trim_end_matches('\0');
        if let Some(os) = os_from_note_name(name) {
            return Some(os);
        }
        at = name_at
            .checked_add(align(namesz))?
            .checked_add(align(descsz))?;
    }
    None
}

/// The OS a note vendor name identifies. `GNU` is Linux in practice: the note is the GNU ABI tag,
/// and the toolchains that emit it on another kernel also set `EI_OSABI`, which is read first.
fn os_from_note_name(name: &str) -> Option<&'static str> {
    match name {
        "NetBSD" => Some("netbsd"),
        "OpenBSD" => Some("openbsd"),
        "FreeBSD" => Some("freebsd"),
        "DragonFly" => Some("dragonfly"),
        "Android" | "GNU" => Some("linux"),
        _ => None,
    }
}

fn read_elf_u32(bytes: &[u8], little: bool) -> u32 {
    let raw = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if little {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    }
}

/// A `u32` or `u64` offset, depending on the ELF class, widened to `u64`.
fn read_elf_addr(bytes: &[u8], sixty_four: bool, little: bool) -> Option<u64> {
    if sixty_four {
        let raw: [u8; 8] = bytes.get(0..8)?.try_into().ok()?;
        Some(if little {
            u64::from_le_bytes(raw)
        } else {
            u64::from_be_bytes(raw)
        })
    } else {
        Some(read_elf_u32(bytes.get(0..4)?, little) as u64)
    }
}

fn seek_read(file: &mut std::fs::File, at: u64, into: &mut [u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(into)
}

fn pe_target_from_file(path: &Path) -> Option<(String, String)> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut at_3c = [0u8; 4];
    file.seek(SeekFrom::Start(0x3c)).ok()?;
    file.read_exact(&mut at_3c).ok()?;
    let pe_offset = u32::from_le_bytes(at_3c) as u64;
    let mut sig_and_machine = [0u8; 6];
    file.seek(SeekFrom::Start(pe_offset)).ok()?;
    file.read_exact(&mut sig_and_machine).ok()?;
    if sig_and_machine[..4] != [b'P', b'E', 0, 0] {
        return None;
    }
    let machine = u16::from_le_bytes([sig_and_machine[4], sig_and_machine[5]]);
    let arch = match machine {
        0x8664 => "x86_64",
        0xaa64 => "aarch64",
        _ => return None,
    };
    Some(("windows".to_string(), arch.to_string()))
}

pub(crate) fn normalize_os(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    // MSYS/MinGW/Cygwin `uname -s` carries a version suffix (`MINGW64_NT-10.0-22631`,
    // `MSYS_NT-…`, `CYGWIN_NT-…`), and the PowerShell probe reports `windows` directly, so match on
    // the family prefix rather than an exact string.
    if lower.starts_with("mingw")
        || lower.starts_with("msys")
        || lower.starts_with("cygwin")
        || lower.starts_with("windows")
    {
        return "windows".into();
    }
    match lower.as_str() {
        "linux" => "linux".into(),
        "darwin" | "macos" => "macos".into(),
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
    fn installed_path_comparison_follows_remote_path_rules() {
        assert!(same_remote_path(
            r"C:\Users\Runner\AppData\Rozi.exe",
            r"c:/users/runner/appdata/rozi.exe",
            RemoteFamily::Windows
        ));
        assert!(!same_remote_path(
            r"C:\Users\Runner\rozi.exe",
            r"C:\Users\Other\rozi.exe",
            RemoteFamily::Windows
        ));
        assert!(!same_remote_path(
            "/home/u/rozi",
            "/home/U/rozi",
            RemoteFamily::Posix
        ));
    }

    #[test]
    fn remote_invocation_uses_the_known_shell_family_not_path_syntax() {
        let mut windows = Command::new("ssh");
        append_remote_rozi_command(
            &mut windows,
            "rozi.exe",
            &["--remote-serve", "dev"],
            RemoteFamily::Windows,
        );
        let windows_args: Vec<_> = windows
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        let encoded = windows_args.last().unwrap();
        let script = decode_powershell_command(encoded);
        assert!(windows_args[0].eq_ignore_ascii_case("powershell"));
        assert!(script.contains("'rozi.exe'"));

        let mut windows_spaced = Command::new("ssh");
        append_remote_rozi_command(
            &mut windows_spaced,
            r"C:\Program Files\Rozi\rozi.exe",
            &["--remote-serve", "dev"],
            RemoteFamily::Windows,
        );
        let args: Vec<_> = windows_spaced
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(
            decode_powershell_command(args.last().unwrap())
                .contains(r"'C:\Program Files\Rozi\rozi.exe'")
        );

        let mut windows_unicode = Command::new("ssh");
        append_remote_rozi_command(
            &mut windows_unicode,
            "C:\\Users\\Łukasz\\Adam's Rozi\\rozi.exe",
            &["--remote-serve", "dev"],
            RemoteFamily::Windows,
        );
        let args: Vec<_> = windows_unicode
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(decode_powershell_command(args.last().unwrap()).contains("Adam''s Rozi"));

        let mut posix = Command::new("ssh");
        append_remote_rozi_command(
            &mut posix,
            "/some path/rozi",
            &["--remote-serve", "dev"],
            RemoteFamily::Posix,
        );
        let args: Vec<_> = posix.get_args().map(|arg| arg.to_string_lossy()).collect();
        assert_eq!(args, ["'/some path/rozi'", "'--remote-serve'", "'dev'"]);
        assert_eq!(
            shell_quote_posix("rozi'; touch $HOME"),
            "'rozi'\"'\"'; touch $HOME'"
        );
    }

    #[test]
    fn parse_probe_collects_candidates_and_protocol_range() {
        let report = parse_probe_output(
            "\
platform=Linux
machine=x86_64
candidate=/home/u/.local/bin/rozi
version_line=rozi 0.1.0 protocol_min=12 protocol_max=12
protocol_min=12
protocol_max=12
speaks_remote=1
candidate=/usr/bin/rozi
version_line=rozi 0.0.1
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
        let report = parse_probe_output(&format!(
            "\
candidate=/old
speaks_remote=1
candidate=/new
speaks_remote=1
protocol_min={MIN_SUPPORTED_PROTOCOL}
protocol_max={PROTOCOL_VERSION}
"
        ));
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
        // One past our ceiling: disjoint whatever this build's version happens to be.
        let beyond = crate::session::protocol::PROTOCOL_VERSION + 1;
        let report = parse_probe_output(&format!(
            "\
candidate=/skew
speaks_remote=1
protocol_min={beyond}
protocol_max={beyond}
"
        ));
        assert!(matches!(
            select_compatible(&report),
            ProbeResult::Missing { .. }
        ));
    }

    #[test]
    fn prompt_policy_asks_when_interactive_never_auto_installs() {
        let missing = ProbeResult::Missing {
            detail: "no rozi".into(),
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
    fn detects_executable_target_from_headers() {
        // ELF x86_64: magic, EI_OSABI at offset 7, then e_machine 0x3e at offset 18.
        let mut elf = vec![0u8; 20];
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[5] = 1; // EI_DATA = little-endian
        elf[7] = 3; // ELFOSABI_GNU
        elf[18] = 0x3e;
        elf[19] = 0x00;
        assert_eq!(
            binary_target_from_header(&elf, Path::new("/x")),
            Some(("linux".into(), "x86_64".into()))
        );

        // ELF aarch64: e_machine 0xb7.
        let mut arm = elf.clone();
        arm[18] = 0xb7;
        assert_eq!(
            binary_target_from_header(&arm, Path::new("/x")),
            Some(("linux".into(), "aarch64".into()))
        );

        // The same header with NetBSD's EI_OSABI is NetBSD, not Linux. Reading this one as Linux
        // is what refused a NetBSD-built rozi as a wrong-target upload (issue #3).
        let mut netbsd = elf.clone();
        netbsd[7] = 2; // ELFOSABI_NETBSD
        assert_eq!(
            binary_target_from_header(&netbsd, Path::new("/x")),
            Some(("netbsd".into(), "x86_64".into()))
        );

        // ELFOSABI_NONE with no notes to read names no OS at all, rather than guessing one.
        let mut bare = elf.clone();
        bare[7] = 0;
        assert_eq!(binary_target_from_header(&bare, Path::new("/x")), None);

        // Mach-O 64-bit little-endian, cputype x86_64 (0x01000007).
        let mut macho = vec![0u8; 8];
        macho[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        macho[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes());
        assert_eq!(
            binary_target_from_header(&macho, Path::new("/x")),
            Some(("macos".into(), "x86_64".into()))
        );

        // Unrecognized formats are `None` (best-effort: never block on what we cannot read).
        assert_eq!(
            binary_target_from_header(b"not an exe", Path::new("/x")),
            None
        );
    }

    /// Build a minimal little-endian ELF64 carrying one `PT_NOTE` segment with `vendor`'s note.
    fn elf_with_note(vendor: &str) -> Vec<u8> {
        const EHDR: usize = 64;
        const PHDR: usize = 56;

        let mut name: Vec<u8> = vendor.as_bytes().to_vec();
        name.push(0);
        let namesz = name.len();
        while !name.len().is_multiple_of(4) {
            name.push(0);
        }
        let mut notes = Vec::new();
        notes.extend_from_slice(&(namesz as u32).to_le_bytes());
        notes.extend_from_slice(&4u32.to_le_bytes()); // descsz
        notes.extend_from_slice(&1u32.to_le_bytes()); // type
        notes.extend_from_slice(&name);
        notes.extend_from_slice(&0u32.to_le_bytes()); // descriptor

        let mut elf = vec![0u8; EHDR + PHDR];
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[4] = 2; // ELFCLASS64
        elf[5] = 1; // little-endian
        elf[7] = 0; // ELFOSABI_NONE, so only the note identifies the system
        elf[18] = 0x3e; // x86_64
        elf[32..40].copy_from_slice(&(EHDR as u64).to_le_bytes()); // e_phoff
        elf[54..56].copy_from_slice(&(PHDR as u16).to_le_bytes()); // e_phentsize
        elf[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum

        let ph = EHDR;
        elf[ph..ph + 4].copy_from_slice(&4u32.to_le_bytes()); // PT_NOTE
        elf[ph + 8..ph + 16].copy_from_slice(&((EHDR + PHDR) as u64).to_le_bytes()); // p_offset
        elf[ph + 32..ph + 40].copy_from_slice(&(notes.len() as u64).to_le_bytes()); // p_filesz
        elf.extend_from_slice(&notes);
        elf
    }

    /// Every Unix leaves `EI_OSABI` as `ELFOSABI_NONE`, so the vendor note is what actually
    /// separates them. This is the case a real NetBSD binary hits.
    #[test]
    fn a_vendor_note_names_the_system_when_the_osabi_byte_does_not() {
        let dir = std::env::temp_dir().join(format!("rozi-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for (vendor, expected) in [
            ("NetBSD", "netbsd"),
            ("GNU", "linux"),
            ("FreeBSD", "freebsd"),
        ] {
            let path = dir.join(vendor);
            std::fs::write(&path, elf_with_note(vendor)).unwrap();
            let head = std::fs::read(&path).unwrap();
            assert_eq!(
                binary_target_from_header(&head[..64], &path),
                Some((expected.to_string(), "x86_64".to_string())),
                "{vendor}"
            );
        }

        // A note segment naming nobody we know leaves the OS unidentified.
        let unknown = dir.join("unknown");
        std::fs::write(&unknown, elf_with_note("Weird")).unwrap();
        let head = std::fs::read(&unknown).unwrap();
        assert_eq!(binary_target_from_header(&head[..64], &unknown), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A NetBSD host must accept a NetBSD binary. This is the pkgsrc case from issue #3.
    #[test]
    fn a_netbsd_binary_is_not_refused_by_a_netbsd_host() {
        let dir = std::env::temp_dir().join(format!("rozi-netbsd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let bin = dir.join("rozi");
        std::fs::write(&bin, elf_with_note("NetBSD")).unwrap();
        let netbsd = ProbeReport {
            platform: "NetBSD".into(),
            machine: "x86_64".into(),
            candidates: Vec::new(),
        };
        verify_override_targets_remote(&bin, &netbsd).expect("a NetBSD binary installs on NetBSD");

        let linux = ProbeReport {
            platform: "Linux".into(),
            machine: "x86_64".into(),
            candidates: Vec::new(),
        };
        assert!(verify_override_targets_remote(&bin, &linux).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_check_blocks_only_a_confirmed_mismatch() {
        let dir = std::env::temp_dir().join(format!("rozi-override-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A minimal ELF x86_64 header on disk.
        let bin = dir.join("fake-rozi");
        let mut elf = vec![0u8; 20];
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[5] = 1;
        elf[7] = 3; // ELFOSABI_GNU, so the fixture says Linux rather than merely being an ELF.
        elf[18] = 0x3e;
        std::fs::write(&bin, &elf).unwrap();

        let linux = ProbeReport {
            platform: "Linux".into(),
            machine: "x86_64".into(),
            candidates: Vec::new(),
        };
        verify_override_targets_remote(&bin, &linux).expect("matching target installs");

        let windows = ProbeReport {
            platform: "windows".into(),
            machine: "x86_64".into(),
            candidates: Vec::new(),
        };
        assert!(verify_override_targets_remote(&bin, &windows).is_err());

        // Unknown remote platform never blocks.
        let unknown = ProbeReport {
            platform: "unknown".into(),
            machine: "unknown".into(),
            candidates: Vec::new(),
        };
        verify_override_targets_remote(&bin, &unknown).expect("unknown platform does not block");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors — the padding boundaries are what a hand-rolled encoder gets wrong.
        assert_eq!(base64_standard(b""), "");
        assert_eq!(base64_standard(b"f"), "Zg==");
        assert_eq!(base64_standard(b"fo"), "Zm8=");
        assert_eq!(base64_standard(b"foo"), "Zm9v");
        assert_eq!(base64_standard(b"foob"), "Zm9vYg==");
        assert_eq!(base64_standard(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_standard(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encoded_powershell_command_round_trips() {
        // UTF-16LE + base64, decodable back to the original script (what -EncodedCommand expects).
        let encoded = encode_powershell_command("Write-Output 'hi'");
        assert_eq!(decode_powershell_command(&encoded), "Write-Output 'hi'");
    }

    #[test]
    fn powershell_family_probe_checks_the_runtime_os() {
        let encoded = encode_powershell_command(WINDOWS_FAMILY_PROBE_SCRIPT);
        assert_eq!(
            decode_powershell_command(&encoded),
            "if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) { Write-Output 'rozi_family=windows' }"
        );
    }

    fn decode_powershell_command(encoded: &str) -> String {
        // Manually decode base64 -> UTF-16LE -> String.
        let decoded_bytes = {
            let table = |c: u8| -> Option<u32> {
                match c {
                    b'A'..=b'Z' => Some((c - b'A') as u32),
                    b'a'..=b'z' => Some((c - b'a' + 26) as u32),
                    b'0'..=b'9' => Some((c - b'0' + 52) as u32),
                    b'+' => Some(62),
                    b'/' => Some(63),
                    _ => None,
                }
            };
            let mut out = Vec::new();
            let clean: Vec<u8> = encoded.bytes().filter(|&c| c != b'=').collect();
            let mut buf = 0u32;
            let mut bits = 0u32;
            for c in clean {
                buf = (buf << 6) | table(c).unwrap();
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push(((buf >> bits) & 0xff) as u8);
                }
            }
            out
        };
        let units: Vec<u16> = decoded_bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn normalize_os_matches_the_windows_uname_families() {
        // MSYS/MinGW/Cygwin `uname -s` all carry a version suffix; the PowerShell probe says
        // `windows` outright. All fold to the same artifact family.
        assert_eq!(normalize_os("MINGW64_NT-10.0-22631"), "windows");
        assert_eq!(normalize_os("MSYS_NT-10.0"), "windows");
        assert_eq!(normalize_os("CYGWIN_NT-10.0-19045"), "windows");
        assert_eq!(normalize_os("windows"), "windows");
        assert_eq!(normalize_os("Linux"), "linux");
        assert_eq!(normalize_os("Darwin"), "macos");
        assert_eq!(normalize_os("plan9"), "plan9");
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

    #[test]
    fn cross_platform_download_rejects_an_unsigned_manifest() {
        struct UnsignedDownloader;

        impl Downloader for UnsignedDownloader {
            fn fetch(
                &self,
                url: &Url,
                _max_bytes: usize,
            ) -> relswap::ReleaseResult<relswap::DownloadResponse> {
                Ok(relswap::DownloadResponse::new(
                    url.clone(),
                    url.clone(),
                    Vec::new(),
                    b"{}".to_vec(),
                ))
            }
        }

        let error = download_release_binary_from_base_with(
            &UnsignedDownloader,
            "x86_64-unknown-linux-gnu",
            env!("CARGO_PKG_VERSION"),
            "https://mirror.example/releases/v0/",
        )
        .unwrap_err();
        assert!(error.contains("authenticate release manifest"), "{error}");
    }

    #[test]
    fn windows_upload_names_are_random_shell_safe_executables() {
        let first = windows_temp_name();
        let second = windows_temp_name();
        assert_eq!(first.len(), "rozi.install..tmp.exe".len() + 32);
        assert!(first.starts_with("rozi.install."));
        assert!(first.ends_with(".tmp.exe"));
        assert!(
            first["rozi.install.".len()..][..32]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert_ne!(first, second);
    }

    /// Opening a host is four ssh invocations — two probes, a re-probe, the attach — and without a
    /// shared connection that is four authentications, which on a password host means typing the
    /// password four times. The options that collapse them have to be on *every* invocation: the
    /// one that arrives first becomes the master, and it is not always the same one.
    /// Multiplexing is a convenience. Handing ssh a `ControlPath` it cannot bind is not a lost
    /// convenience but a failed *attach*, so a runtime directory with no room for the socket drops
    /// the option instead - which is what a 48-byte macOS `TMPDIR` used to turn into a dead remote.
    #[cfg(unix)]
    #[test]
    fn a_runtime_directory_too_small_for_the_socket_drops_multiplexing() {
        assert_eq!(
            multiplexing_control_path(Path::new("/run/user/1000/rozi")),
            Some("/run/user/1000/rozi/ssh-%C".to_string())
        );
        assert_eq!(
            multiplexing_control_path(Path::new(
                "/var/folders/df/djsxfhc17x95674wsm_g8s980000gn/T/rozi-501"
            )),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn every_invocation_offers_to_share_one_authenticated_connection() {
        let resolved = ResolvedRemote {
            alias: Some("workbox".into()),
            host: "workbox".into(),
            user: None,
            port: None,
            identity_file: None,
            ssh_args: Vec::new(),
            binary_path: None,
        };
        let config = RemoteConfig::default();
        let args: Vec<String> = ssh_base_command(&resolved, &config)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.iter().any(|arg| arg == "ControlMaster=auto"));
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("ControlPath=") && arg.contains("ssh-%C")),
            "one master per destination, named by ssh's own destination hash: {args:?}"
        );
        assert!(args.iter().any(|arg| arg.starts_with("ControlPersist=")));
        // Keepalive belongs here rather than on the attach alone: a client riding a master defers
        // to the master's settings, and the master is whichever invocation connected first.
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("ServerAliveInterval=")),
            "the master carries the keepalive: {args:?}"
        );
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("ServerAliveCountMax="))
        );
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
        let args = |config: &RemoteConfig| -> Vec<String> {
            ssh_base_command(&resolved, config)
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };

        let mut config = RemoteConfig::default();
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

    #[test]
    fn ssh_remote_command_argv_places_destination_before_remote_command() {
        let resolved = ResolvedRemote {
            alias: Some("workbox".into()),
            host: "workbox".into(),
            user: Some("me".into()),
            port: None,
            identity_file: None,
            ssh_args: Vec::new(),
            binary_path: None,
        };
        let mut command = ssh_base_command(&resolved, &RemoteConfig::default());
        append_ssh_destination(&mut command, &resolved);
        command.args(["/usr/local/bin/rozi", "--remote-serve", "dev"]);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let marker = args.iter().position(|arg| arg == "--").expect("-- marker");
        assert_eq!(args[marker + 1], "me@workbox");
        assert_eq!(args[marker + 2], "/usr/local/bin/rozi");
        assert_eq!(args[marker + 3], "--remote-serve");
        assert_eq!(args[marker + 4], "dev");
    }

    /// The scp used by the Windows install must carry the same connection options as ssh, but with
    /// scp's uppercase `-P` for the port (a lowercase `-p` would be silently misread).
    #[test]
    fn scp_base_command_mirrors_ssh_options_with_uppercase_port() {
        let resolved = ResolvedRemote {
            alias: Some("winbox".into()),
            host: "winbox".into(),
            user: Some("me".into()),
            port: Some(2222),
            identity_file: Some("/keys/id".into()),
            ssh_args: vec!["-o".into(), "UserKnownHostsFile=/tmp/kh".into()],
            binary_path: None,
        };
        let config = RemoteConfig::default();
        let args: Vec<String> = scp_base_command(&resolved, &config)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
        // scp's port flag is uppercase; the lowercase ssh form must not appear.
        let port_pos = args.iter().position(|arg| arg == "-P").expect("-P present");
        assert_eq!(args[port_pos + 1], "2222");
        assert!(!args.iter().any(|arg| arg == "-p"));
        assert!(args.iter().any(|arg| arg == "-i"));
        assert!(args.iter().any(|arg| arg == "UserKnownHostsFile=/tmp/kh"));
    }

    #[test]
    fn reconnect_budget_can_tighten_ssh_connect_timeout() {
        let resolved = ResolvedRemote {
            alias: Some("workbox".into()),
            host: "workbox".into(),
            user: None,
            port: None,
            identity_file: None,
            ssh_args: Vec::new(),
            binary_path: None,
        };
        let config = RemoteConfig::default();
        let args = |secs: u64| -> Vec<String> {
            ssh_base_command_with_connect_timeout(&resolved, &config, secs)
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert!(
            args(5).iter().any(|arg| arg == "ConnectTimeout=5"),
            "remaining reconnect budget must reach ssh argv"
        );
        assert!(
            args(config.connection_timeout_secs)
                .iter()
                .any(|arg| arg == "ConnectTimeout=15")
        );
    }
}
