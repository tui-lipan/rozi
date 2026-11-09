//! Shared process, isolation, and release-binary setup for the compat suite.
//!
//! The released binary is installed with [`rozi::platform::install::from_platform_env`], which is
//! the updater's constructor: it verifies the signed manifest and archive through `relswap` and
//! the compiled trust anchor before it will execute the payload. The bytes land under the Cargo
//! target directory so a later run can reuse them and nothing is written into the developer's
//! own config, state, or runtime directories.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rozi::platform::paths::PlatformEnv;

/// First wire generation that carries the support window. A released binary older than this, paired
/// with a build that is not the same generation, is refused with `protocol-mismatch`. Two builds at
/// or above it run the full session flows. Decided from each binary's `rozi api describe`
/// `session_protocol`.
pub(crate) const SUPPORT_WINDOW_GENERATION: u32 = 52;

const DEFAULT_OLD_VERSION: &str = "0.0.43";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

static NEXT_LAB: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Both binaries advertise a generation inside the support window.
    Window,
    /// The generations differ and at least one predates the window, so attach must be refused.
    Mismatch,
    /// Both advertise the same generation, and it predates the window.
    Same,
}

pub(crate) fn mode(old_protocol: u32, new_protocol: u32) -> Mode {
    if old_protocol >= SUPPORT_WINDOW_GENERATION && new_protocol >= SUPPORT_WINDOW_GENERATION {
        Mode::Window
    } else if old_protocol != new_protocol {
        Mode::Mismatch
    } else {
        Mode::Same
    }
}

pub(crate) struct Pair {
    pub old_bin: PathBuf,
    pub new_bin: PathBuf,
    pub old_protocol: u32,
    pub new_protocol: u32,
}

impl Pair {
    pub(crate) fn mode(&self) -> Mode {
        mode(self.old_protocol, self.new_protocol)
    }
}

/// `true` when this process should download and drive real binaries.
pub(crate) fn enabled() -> bool {
    std::env::var("ROZI_COMPAT").as_deref() == Ok("1")
}

/// Print why an ignored test is returning, and report whether the caller should continue.
pub(crate) fn skip_unless_enabled() -> bool {
    if enabled() {
        return true;
    }
    eprintln!(
        "skipping: set ROZI_COMPAT=1 to run the cross-version suite (it downloads a signed release unless ROZI_COMPAT_OLD_BIN is set)"
    );
    false
}

pub(crate) fn pair() -> &'static Pair {
    static PAIR: OnceLock<Pair> = OnceLock::new();
    PAIR.get_or_init(|| {
        let old_bin = acquire_old_binary();
        let new_bin = PathBuf::from(env!("CARGO_BIN_EXE_rozi"));
        let old_protocol = session_protocol(&old_bin);
        let new_protocol = session_protocol(&new_bin);
        eprintln!(
            "compat binaries: released session_protocol {old_protocol} ({}), built session_protocol {new_protocol} ({})",
            old_bin.display(),
            new_bin.display()
        );
        Pair {
            old_bin,
            new_bin,
            old_protocol,
            new_protocol,
        }
    })
}

pub(crate) struct Ran {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub(crate) fn success(&self) -> bool {
        self.status.success()
    }

    pub(crate) fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Run `bin` with `args` and isolated user directories. Fails the test if it does not exit.
pub(crate) fn run(bin: &Path, env: &Isolated, args: &[&str]) -> Ran {
    run_with_home(bin, env, args, false)
}

#[cfg(unix)]
/// Like [`run`], but leave `HOME` as this process found it so `ssh` can read `~/.ssh`.
pub(crate) fn run_keeping_home(bin: &Path, env: &Isolated, args: &[&str]) -> Ran {
    run_with_home(bin, env, args, true)
}

fn run_with_home(bin: &Path, env: &Isolated, args: &[&str], keep_home: bool) -> Ran {
    let mut command = if keep_home {
        env.command_keeping_home(bin)
    } else {
        env.command(bin)
    };
    command.args(args);
    finish(command, COMMAND_TIMEOUT)
}

/// How a client reaches the session server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transport {
    /// The session socket in the lab's runtime directory.
    Local,
    /// `ssh localhost`, with the server binary pinned so nothing is installed on the host.
    #[cfg(unix)]
    Ssh,
}

/// Arguments for a pane that stays up long enough to be snapshotted and restarted.
pub(crate) fn sleeper_argv() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["ping", "-n", "60", "127.0.0.1"]
    } else {
        vec!["sleep", "60"]
    }
}

fn finish(mut command: Command, timeout: Duration) -> Ran {
    let displayed = format!("{command:?}");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {displayed}: {error}"));
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = std::thread::spawn(move || read_pipe(stdout));
    let stderr_task = std::thread::spawn(move || read_pipe(stderr));
    let started = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait {displayed}: {error}"))
        {
            Some(status) => break status,
            None if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = stdout_task.join().unwrap_or_default();
                let stderr = stderr_task.join().unwrap_or_default();
                panic!(
                    "{displayed} did not exit within {}s\n{stdout}\n{stderr}",
                    timeout.as_secs()
                );
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    Ran {
        status,
        stdout: stdout_task.join().unwrap_or_default(),
        stderr: stderr_task.join().unwrap_or_default(),
    }
}

fn read_pipe(pipe: Option<impl Read>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut text = String::new();
    let _ = pipe.read_to_string(&mut text);
    text
}

/// A private scratch root whose rozi directories are not the developer's.
pub(crate) struct Isolated {
    pub root: PathBuf,
    pub runtime: PathBuf,
    pub config_path: PathBuf,
}

impl Isolated {
    pub(crate) fn new() -> Self {
        let n = NEXT_LAB.fetch_add(1, Ordering::Relaxed);
        let root = rozi::test_support::socket_safe_temp_dir().join(format!(
            "rzc{}-{}",
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&root);
        rozi::platform::fs_security::ensure_private_dir(&root).expect("private compat lab");
        for name in ["home", "config", "state", "data", "cache", "run", "work"] {
            fs::create_dir_all(root.join(name)).expect("compat lab directory");
        }
        let config_path = root.join("config").join("config.toml");
        fs::write(&config_path, "[session]\nresurrect = true\n").expect("compat config");
        Self {
            runtime: root.join("run"),
            config_path,
            root,
        }
    }

    pub(crate) fn command(&self, bin: &Path) -> Command {
        let mut command = Command::new(bin);
        command
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("APPDATA", self.root.join("AppData").join("Roaming"))
            .env("LOCALAPPDATA", self.root.join("AppData").join("Local"))
            .env("ROZI_CONFIG", &self.config_path)
            .env_remove("ROZI_PANE")
            .env_remove("ROZI_SESSION")
            .env_remove("ROZI_SESSION_INSTANCE")
            .env_remove("ROZI_SOCKET")
            .env_remove("ROZI_BIN")
            .env_remove("ROZI_EXTENSION")
            .env_remove("ROZI_EXTENSION_ID")
            .env_remove("ROZI_EXTENSION_GENERATION")
            .env_remove("ROZI_AGENT");
        command
    }

    /// Like [`Self::command`], but leave `HOME` alone so `ssh` can read the user's keys.
    pub(crate) fn command_keeping_home(&self, bin: &Path) -> Command {
        let home = std::env::var_os("HOME");
        let mut command = self.command(bin);
        match home {
            Some(home) => {
                command.env("HOME", home);
            }
            None => {
                command.env_remove("HOME");
            }
        }
        command
    }

    /// Point `[remote.hosts.localhost] binary_path` at a wrapper that execs `server` inside this lab.
    ///
    /// The wrapper is what ssh runs, so the remote server inherits these directories instead of the
    /// developer's. `binary_path` skips the install probe.
    #[cfg(unix)]
    pub(crate) fn pin_localhost_binary(&self, server: &Path) {
        let wrapper = self.root.join("wrap");
        let mut script = String::from("#!/bin/sh\n");
        for (key, value) in [
            ("HOME", self.root.join("home")),
            ("XDG_CONFIG_HOME", self.root.join("config")),
            ("XDG_STATE_HOME", self.root.join("state")),
            ("XDG_DATA_HOME", self.root.join("data")),
            ("XDG_CACHE_HOME", self.root.join("cache")),
            ("XDG_RUNTIME_DIR", self.runtime.clone()),
            ("APPDATA", self.root.join("AppData").join("Roaming")),
            ("LOCALAPPDATA", self.root.join("AppData").join("Local")),
            ("ROZI_CONFIG", self.config_path.clone()),
        ] {
            let text = value.to_str().expect("compat lab paths are utf-8");
            script.push_str(&format!("export {key}={}\n", posix_single(text)));
        }
        script.push_str(
            "unset ROZI_PANE ROZI_SESSION ROZI_SESSION_INSTANCE ROZI_SOCKET ROZI_BIN \
             ROZI_EXTENSION ROZI_EXTENSION_ID ROZI_EXTENSION_GENERATION ROZI_AGENT\n",
        );
        let bin = server.to_str().expect("server path is utf-8");
        script.push_str(&format!("exec {} \"$@\"\n", posix_single(bin)));
        fs::write(&wrapper, script).expect("remote wrapper");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).expect("wrapper mode");

        let path = toml_basic(wrapper.to_str().expect("wrapper path is utf-8"));
        let config = format!(
            "[session]\nresurrect = true\n\n[remote]\ninstall = \"never\"\n\n\
             [remote.hosts.localhost]\nbinary_path = \"{path}\"\n"
        );
        fs::write(&self.config_path, config).expect("remote config");
    }
}

#[cfg(unix)]
fn posix_single(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn toml_basic(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One detached session server, killed with `kill_with` when dropped.
pub(crate) struct Session {
    pub env: Isolated,
    pub name: String,
    pub kill_with: PathBuf,
}

impl Session {
    pub(crate) fn start(server: &Path) -> Self {
        let env = Isolated::new();
        let n = NEXT_LAB.fetch_add(1, Ordering::Relaxed);
        let name = format!("c{:x}{n}", std::process::id());
        let started = run(server, &env, &["sessions", "new", &name, "--detach"]);
        assert!(
            started.success(),
            "session server did not start:\n{}",
            started.combined()
        );
        Self {
            env,
            name,
            kill_with: server.to_path_buf(),
        }
    }

    pub(crate) fn client(&self, bin: &Path, args: &[&str]) -> Ran {
        let mut all = vec!["--session", self.name.as_str()];
        all.extend_from_slice(args);
        run(bin, &self.env, &all)
    }

    pub(crate) fn list_panes(&self, bin: &Path) -> Ran {
        self.call(bin, Transport::Local, &["list-panes", "--format", "json"])
    }

    pub(crate) fn call(&self, bin: &Path, transport: Transport, args: &[&str]) -> Ran {
        match transport {
            Transport::Local => self.client(bin, args),
            #[cfg(unix)]
            Transport::Ssh => {
                let mut all = vec!["--remote", "localhost", "--session", self.name.as_str()];
                all.extend_from_slice(args);
                run_keeping_home(bin, &self.env, &all)
            }
        }
    }

    pub(crate) fn stop(&self) {
        let stopped = run(
            &self.kill_with,
            &self.env,
            &["sessions", "kill", &self.name],
        );
        if !stopped.success() && !session_already_gone(&stopped) {
            eprintln!("session kill failed:\n{}", stopped.combined());
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn session_already_gone(ran: &Ran) -> bool {
    let text = ran.combined();
    text.contains("was not found") || text.contains("not running") || text.contains("No such")
}

pub(crate) fn pane_ids(ran: &Ran) -> Vec<u64> {
    assert!(ran.success(), "list-panes failed:\n{}", ran.combined());
    let value: serde_json::Value = serde_json::from_str(ran.stdout.trim())
        .unwrap_or_else(|error| panic!("list-panes was not JSON: {error}\n{}", ran.combined()));
    assert_eq!(
        value.get("ok").and_then(serde_json::Value::as_bool),
        Some(true),
        "list-panes refused:\n{}",
        ran.combined()
    );
    value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("list-panes has no data array:\n{}", ran.combined()))
        .iter()
        .filter_map(|pane| pane.get("id").and_then(serde_json::Value::as_u64))
        .collect()
}

/// The client exited with a refusal, not a crash, and named the protocol mismatch.
pub(crate) fn assert_clean_protocol_mismatch(ran: &Ran) {
    assert!(
        !ran.success(),
        "expected a protocol mismatch, but the client succeeded:\n{}",
        ran.combined()
    );
    assert!(
        ran.status.code().is_some(),
        "client crashed instead of refusing the mismatched peer:\n{}",
        ran.combined()
    );
    let text = ran.combined();
    assert!(
        text.contains("protocol-mismatch")
            || text.contains("incompatible rozi version")
            || text.contains("is incompatible with"),
        "expected a protocol-mismatch refusal, got:\n{text}"
    );
    let lower = text.to_ascii_lowercase();
    assert!(
        !lower.contains("stack backtrace") && !lower.contains("panicked at"),
        "protocol mismatch was not clean:\n{text}"
    );
}

fn session_protocol(bin: &Path) -> u32 {
    let mut command = Command::new(bin);
    command.arg("api").arg("describe");
    let ran = finish(command, Duration::from_secs(30));
    assert!(
        ran.success(),
        "`{} api describe` failed:\n{}",
        bin.display(),
        ran.combined()
    );
    let value: serde_json::Value =
        serde_json::from_str(ran.stdout.trim()).unwrap_or_else(|error| {
            panic!(
                "`{} api describe` was not JSON: {error}\n{}",
                bin.display(),
                ran.combined()
            )
        });
    value
        .get("session_protocol")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("api describe has no session_protocol:\n{}", ran.stdout))
        as u32
}

fn acquire_old_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("ROZI_COMPAT_OLD_BIN") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "ROZI_COMPAT_OLD_BIN is not a file: {}",
            path.display()
        );
        return path;
    }
    let version = std::env::var("ROZI_COMPAT_OLD_VERSION")
        .unwrap_or_else(|_| DEFAULT_OLD_VERSION.to_string());
    install_signed_release(&version)
}

fn install_signed_release(version: &str) -> PathBuf {
    let root = target_dir().join("compat-cache").join(version);
    if let Some(cached) = reusable_binary(&root, version) {
        eprintln!("compat cache hit: {}", cached.display());
        return cached;
    }
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("compat cache directory");
    eprintln!(
        "verifying and installing signed rozi {version} into {}",
        root.display()
    );
    let installation = rozi::platform::install::from_platform_env(&cache_env(&root));
    let parsed = semver::Version::parse(version).unwrap_or_else(|error| {
        panic!("ROZI_COMPAT_OLD_VERSION `{version}` is not a version: {error}")
    });
    installation
        .install_exact(parsed)
        .unwrap_or_else(|error| panic!("signed release {version} did not verify: {error}"));
    let path = installation.command_path().to_path_buf();
    assert!(
        path.is_file(),
        "verified release {version} did not produce {}",
        path.display()
    );
    let described = session_protocol(&path);
    eprintln!(
        "verified rozi {version} session_protocol {described} at {}",
        path.display()
    );
    path
}

fn reusable_binary(root: &Path, version: &str) -> Option<PathBuf> {
    let path = rozi::platform::install::from_platform_env(&cache_env(root))
        .command_path()
        .to_path_buf();
    if !path.is_file() {
        return None;
    }
    let mut command = Command::new(&path);
    command.arg("--version");
    let ran = finish(command, Duration::from_secs(30));
    if ran.success()
        && ran
            .stdout
            .lines()
            .next()
            .is_some_and(|line| line.contains(version))
    {
        Some(path)
    } else {
        None
    }
}

fn cache_env(root: &Path) -> PlatformEnv {
    PlatformEnv {
        home: Some(root.join("home")),
        xdg_config_home: Some(root.join("config")),
        xdg_state_home: Some(root.join("state")),
        xdg_cache_home: Some(root.join("cache")),
        xdg_data_home: Some(root.join("data")),
        xdg_runtime_dir: Some(root.join("run")),
        appdata: Some(root.join("AppData").join("Roaming")),
        local_appdata: Some(root.join("AppData").join("Local")),
    }
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"))
}

pub(crate) fn spawn_reader(bin: &Path, env: &Isolated, args: &[&str]) -> std::process::Child {
    let mut command = env.command(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn subscriber: {error}"))
}

#[test]
fn generation_mode_follows_the_support_window() {
    assert_eq!(mode(51, 51), Mode::Same);
    assert_eq!(mode(51, SUPPORT_WINDOW_GENERATION), Mode::Mismatch);
    assert_eq!(mode(SUPPORT_WINDOW_GENERATION, 51), Mode::Mismatch);
    assert_eq!(
        mode(SUPPORT_WINDOW_GENERATION, SUPPORT_WINDOW_GENERATION),
        Mode::Window
    );
    assert_eq!(
        mode(SUPPORT_WINDOW_GENERATION, SUPPORT_WINDOW_GENERATION + 1),
        Mode::Window
    );
}
