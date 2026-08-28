//! Probe and optionally install rozi on a remote host before attach.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{RemoteConfig, RemoteInstallPolicy};
use crate::platform::command::program_exists;
use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};

use super::{
    RemoteTarget, ResolvedRemote, validate_remote_executable_token, validate_remote_target,
};

const INSTALL_DIR: &str = ".local/bin";
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
/// Assumes a POSIX shell on the remote, which is why a Windows host cannot be the remote end of
/// `--remote` (its sshd defaults to `cmd.exe`). Supporting one needs a `cmd`/PowerShell probe
/// variant here, `uname`-equivalent platform detection, and the `.exe`-aware install path noted on
/// [`install_bytes`]. See the platform matrix in `docs/remote.md`.
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
if [ -n "${ROZI_PROBE_BIN:-}" ]; then
  try_bin "$ROZI_PROBE_BIN"
fi
try_bin rozi
try_bin "$HOME/.local/bin/rozi"
try_bin "$HOME/.cargo/bin/rozi"
try_bin /opt/homebrew/bin/rozi
try_bin /usr/local/bin/rozi
printf 'probe_done=1\n'
"#;

/// PowerShell counterpart of [`PROBE_SCRIPT`] for a Windows remote host (default sshd shell is
/// `cmd.exe`, so this is fed to `powershell -Command -`). Emits the same fixed keys the POSIX probe
/// does; [`parse_probe_output`] handles both. Never treats binary output as code.
const WINDOWS_PROBE_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
Write-Output "platform=windows"
$arch = $env:PROCESSOR_ARCHITECTURE
if (-not $arch) { $arch = 'unknown' }
Write-Output "machine=$arch"
function Try-Bin($bin) {
  $resolved = $null
  if (Test-Path -LiteralPath $bin -PathType Leaf) {
    $resolved = (Resolve-Path -LiteralPath $bin).Path
  } else {
    $cmd = Get-Command $bin -ErrorAction SilentlyContinue
    if ($cmd) { $resolved = $cmd.Source }
  }
  if (-not $resolved) { return }
  $out = & $resolved --version 2>$null
  Write-Output "candidate=$resolved"
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

/// Prompt on stdin for install confirmation. Returns true only for an explicit yes.
pub fn prompt_install_confirmation(host: &str) -> io::Result<bool> {
    let mut stderr = io::stderr().lock();
    write!(
        stderr,
        "rozi: no compatible binary on {host}. Install to ~/.local/bin/rozi? [y/N] "
    )?;
    stderr.flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Run the probe over a short-lived ssh command. A shell-safe `binary_path` short-circuits with our
/// protocol range; unsafe configured tokens are rejected before any remote command is spawned.
pub fn probe_remote_report(
    target: &RemoteTarget,
    config: &RemoteConfig,
) -> Result<ProbeReport, String> {
    validate_remote_target(target)?;
    let resolved = ResolvedRemote::resolve(target, config);
    if let Some(path) = &resolved.binary_path {
        validate_remote_executable_token(path)?;
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
    // The remote sshd default shell is not always POSIX (Windows defaults to `cmd.exe`). Detect the
    // family with one fixed, shell-agnostic probe, then feed the matching script to the matching
    // interpreter. Probe output is still parsed with fixed keys and never treated as argv.
    let stdout = match detect_remote_family(&resolved, config)? {
        // PowerShell's `-Command -` truncates a multi-line script read from stdin (only the first
        // statements run) over OpenSSH-for-Windows; pass the script as a base64 `-EncodedCommand`
        // instead, which runs the whole thing and needs no stdin.
        RemoteFamily::Windows => run_probe_command(
            &resolved,
            config,
            &[
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encode_powershell_command(WINDOWS_PROBE_SCRIPT),
            ],
        )?,
        RemoteFamily::Posix => run_probe_script(&resolved, config, &["sh", "-s"], PROBE_SCRIPT)?,
    };
    Ok(parse_probe_output(&stdout))
}

/// Remote sshd default-shell family, chosen up front so the probe/install scripts target the right
/// interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteFamily {
    Posix,
    Windows,
}

/// Detect the remote shell family with a single fixed command. `cmd.exe` expands `%OS%` to
/// `Windows_NT`; a POSIX shell echoes the literal `%OS%`. Neither treats the marker as code.
fn detect_remote_family(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
) -> Result<RemoteFamily, String> {
    let mut command = ssh_base_command(resolved, config);
    append_ssh_destination(&mut command, resolved);
    command.arg("echo").arg("rozi_family=%OS%");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().map_err(|err| {
        format!(
            "failed to probe remote shell family for {}: {err}",
            resolved.host
        )
    })?;
    // A cmd.exe host still exits 0 here; a POSIX host does too. A hard ssh/auth failure is caught by
    // a non-zero status, which we surface rather than silently defaulting to POSIX.
    if !output.status.success() {
        return Err(format!(
            "remote shell probe of {} failed: {}",
            resolved.host,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("rozi_family=Windows_NT") {
        Ok(RemoteFamily::Windows)
    } else {
        Ok(RemoteFamily::Posix)
    }
}

/// Pipe `script` to a remote `interpreter` over ssh stdin and return its stdout (POSIX probe).
fn run_probe_script(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    interpreter: &[&str],
    script: &str,
) -> Result<String, String> {
    let mut command = ssh_base_command(resolved, config);
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
    argv: &[&str],
) -> Result<String, String> {
    let mut command = ssh_base_command(resolved, config);
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

/// Install policy entry point used before connect. Returns the remote binary path to invoke.
///
/// Call this before the TUI takes over the terminal when `install = "prompt"`, so the yes/no
/// prompt still has stdin to read.
pub fn ensure_remote_binary(
    target: &RemoteTarget,
    config: &RemoteConfig,
    interactive: bool,
) -> Result<String, String> {
    if let Ok(path) = std::env::var("ROZI_REMOTE_BINARY") {
        let local = Path::new(&path);
        if !local.is_file() {
            return Err(format!("ROZI_REMOTE_BINARY={path} is not a regular file"));
        }
        // The override used to upload blindly, leaving a wrong-arch binary to fail as an opaque
        // exec-format error on the remote. Now that the probe reports platform/machine, verify the
        // override's own binary format against it — but only hard-fail on a *confirmed* mismatch,
        // so an unrecognized format or an unknown remote platform still installs as before.
        let report = probe_remote_report(target, config)?;
        verify_override_targets_remote(local, &report)?;
        let family = family_from_os(&normalize_os(&report.platform));
        return install_bytes(target, config, local, "ROZI_REMOTE_BINARY override", family);
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
                    "install declined for {host}; set binary_path, install a compatible rozi, or pass install = \"always\""
                ));
            }
            install_for_platforms(target, config, &report)
        }
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
    let local_artifact = download_release_binary(triple, version)?;
    install_bytes(
        target,
        config,
        &local_artifact,
        &format!("release asset {triple}"),
        family,
    )
}

fn family_from_os(os: &str) -> RemoteFamily {
    if os == "windows" {
        RemoteFamily::Windows
    } else {
        RemoteFamily::Posix
    }
}

/// Stream `local` onto the remote and return the installed path.
///
/// The POSIX path installs `$HOME/.local/bin/rozi` (`chmod 755`, atomic `mv`) by streaming the
/// binary over ssh stdin. The Windows path installs `%USERPROFILE%\.local\bin\rozi.exe` via `scp`
/// then a finalize step, because OpenSSH-on-Windows deadlocks a large command stdin. Either way the
/// installed path is echoed back — `connect.rs` invokes `--remote-serve` with it verbatim, so the
/// `.exe` suffix propagates.
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

/// Stream the binary onto a POSIX remote over ssh stdin (`cat > tmp`, `chmod 755`, atomic `mv`).
fn install_bytes_posix(
    resolved: &ResolvedRemote,
    config: &RemoteConfig,
    local: &Path,
) -> Result<String, String> {
    // Atomic install with quoted paths; refuse to overwrite a non-regular destination. The binary
    // arrives on stdin (`cat > tmp`), the script as an argument.
    let script = format!(
        r#"set -e
dir="$HOME/{INSTALL_DIR}"
final="$dir/{INSTALL_NAME}"
tmp="$dir/.rozi.install.$$"
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
    let mut command = ssh_base_command(resolved, config);
    append_ssh_destination(&mut command, resolved);
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
    parse_installed_path(&String::from_utf8_lossy(&output.stdout))
}

/// Install onto a Windows remote in two steps: `scp` the binary to a temp file (the sftp subsystem
/// has real flow control), then a small no-stdin `powershell -EncodedCommand` that moves it into
/// `%USERPROFILE%\.local\bin\rozi.exe`.
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
    // A relative scp destination lands in the remote user's home (%USERPROFILE%). Keep it unique per
    // local process so concurrent installs to one host cannot clobber each other mid-upload.
    let temp_name = format!("rozi.install.{}.tmp", std::process::id());

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

    // Finalize with a no-stdin PowerShell step: move the uploaded temp file into place under a
    // `.exe` name. `-EncodedCommand` is quoting-proof through cmd.exe.
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$dir = Join-Path $env:USERPROFILE '.local\bin'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$final = Join-Path $dir 'rozi.exe'
if (Test-Path -LiteralPath $final -PathType Container) {{
  [Console]::Error.WriteLine("refuse_non_regular=$final")
  exit 1
}}
$src = Join-Path $env:USERPROFILE '{temp_name}'
Move-Item -Force -LiteralPath $src -Destination $final
Write-Output "installed=$final""#
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

/// `scp` argv mirroring [`ssh_base_command`]'s connection options (scp uses `-P` for the port, not
/// `-p`). `ssh_args` are passed through — they are `-o key=value` pairs scp also accepts.
fn scp_base_command(resolved: &ResolvedRemote, config: &RemoteConfig) -> Command {
    let mut command = Command::new("scp");
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

/// Minimal standard-alphabet base64 (with `=` padding). Kept in-crate rather than pulling a direct
/// dependency, mirroring the hand-rolled `sha256` module used for the same install path.
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

fn download_release_binary(triple: &str, version: &str) -> Result<PathBuf, String> {
    let base = std::env::var("ROZI_RELEASE_BASE_URL").unwrap_or_else(|_| {
        format!("https://github.com/{RELEASE_REPO}/releases/download/v{version}")
    });
    let archive_name = if triple.contains("windows") {
        format!("rozi-{version}-{triple}.zip")
    } else {
        format!("rozi-{version}-{triple}.tar.gz")
    };
    let archive_url = format!("{base}/{archive_name}");
    let sha_url = format!("{archive_url}.sha256");

    let tmp = std::env::temp_dir().join(format!(
        "rozi-remote-install-{}-{}",
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
        "rozi.exe"
    } else {
        "rozi"
    };
    extract_release_binary(&archive_path, &archive_name, &tmp, bin_name)
}

/// Extract `archive_path` into `tmp` and return the path to the contained binary.
///
/// The release archives (see `.github/workflows/release.yml`) wrap the binary in a versioned
/// directory: `rozi-<version>-<triple>/rozi`. Extract everything, then locate the binary by
/// name — extracting a bare top-level member would always miss it.
fn extract_release_binary(
    archive_path: &Path,
    archive_name: &str,
    tmp: &Path,
    bin_name: &str,
) -> Result<PathBuf, String> {
    if archive_name.ends_with(".zip") {
        let status = Command::new("unzip")
            .args(["-o"])
            .arg(archive_path)
            .arg("-d")
            .arg(tmp)
            .status()
            .map_err(|err| format!("unzip not available: {err}"))?;
        if !status.success() {
            return Err(format!("failed to unzip {archive_name}"));
        }
    } else {
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(archive_path)
            .arg("-C")
            .arg(tmp)
            .status()
            .map_err(|err| format!("tar not available: {err}"))?;
        if !status.success() {
            return Err(format!("failed to extract {archive_name}"));
        }
    }
    find_file_named(tmp, bin_name, 4)
        .ok_or_else(|| format!("release archive {archive_name} did not contain {bin_name}"))
}

/// Recursively search `dir` (bounded to `max_depth` levels) for a regular file named `name`.
fn find_file_named(dir: &Path, name: &str, max_depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = entry.file_type().ok()?;
        if file_type.is_file() && entry.file_name() == *name {
            return Some(path);
        }
        if file_type.is_dir() {
            subdirs.push(path);
        }
    }
    if max_depth == 0 {
        return None;
    }
    for subdir in subdirs {
        if let Some(found) = find_file_named(&subdir, name, max_depth - 1) {
            return Some(found);
        }
    }
    None
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
    // Hashed in-process rather than through `sha256sum`/`shasum`: neither exists on Windows, and
    // verification is the security-relevant step of a cross-platform install — it must never be
    // skipped, or silently degraded, for want of a tool on the client.
    let actual = relswap::sha256_file(archive)
        .map_err(|err| format!("hash {}: {err}", archive.display()))?;
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
pub(crate) fn ssh_base_command(resolved: &ResolvedRemote, config: &RemoteConfig) -> Command {
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

/// Append the OpenSSH end-of-options marker and destination. OpenSSH expects the destination before
/// the remote command; putting `--` after the destination makes it part of that command instead.
pub(crate) fn append_ssh_destination(command: &mut Command, resolved: &ResolvedRemote) {
    command.arg("--").arg(resolved.ssh_destination());
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
    // ELF (Linux): 0x7f 'E' 'L' 'F', e_machine at offset 18 (little-endian when EI_DATA == 1).
    if head.len() >= 20 && head[..4] == [0x7f, b'E', b'L', b'F'] {
        let machine = u16::from_le_bytes([head[18], head[19]]);
        let arch = match machine {
            0x3e => "x86_64",
            0xb7 => "aarch64",
            _ => return None,
        };
        return Some(("linux".to_string(), arch.to_string()));
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

fn normalize_os(raw: &str) -> String {
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
        // ELF x86_64: magic, then e_machine 0x3e at offset 18.
        let mut elf = vec![0u8; 20];
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[5] = 1; // EI_DATA = little-endian
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
        assert_eq!(String::from_utf16(&units).unwrap(), "Write-Output 'hi'");
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

    /// The release archives nest the binary in a versioned directory
    /// (`rozi-<version>-<triple>/rozi`, per `.github/workflows/release.yml`). Build a fixture
    /// with exactly that layout and prove the extract-then-locate path finds it — the earlier
    /// single-member extraction could never reach into the directory, so `--remote` install always
    /// failed with "release archive did not contain rozi".
    #[test]
    fn extract_locates_binary_nested_in_versioned_directory() {
        let root = std::env::temp_dir().join(format!(
            "rozi-extract-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        // Mirror release.yml: dist/<name>/{rozi,README...} tarred as `<name>`.
        let name = "rozi-9.9.9-x86_64-unknown-linux-gnu";
        let staging = root.join("dist");
        let pkg = staging.join(name);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("rozi"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(pkg.join("README.md"), b"readme").unwrap();

        let archive_name = format!("{name}.tar.gz");
        let archive_path = staging.join(&archive_name);
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&staging)
            .arg(name)
            .status()
            .expect("tar available");
        assert!(status.success(), "fixture tar failed");

        let out = root.join("extract");
        std::fs::create_dir_all(&out).unwrap();
        let located = extract_release_binary(&archive_path, &archive_name, &out, "rozi")
            .expect("binary located in nested archive");
        assert!(located.is_file());
        assert_eq!(located.file_name().unwrap(), "rozi");

        let _ = std::fs::remove_dir_all(&root);
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
}
