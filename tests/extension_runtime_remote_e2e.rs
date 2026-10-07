//! Placed extension processes on a remote host, end to end.
//!
//! The "host" is this machine behind a stand-in `ssh` that runs the real `rozi extensions
//! runtime` with a home, cache, and runtime directory of its own, and marks everything it starts
//! with `ROZI_TEST_HOST=pc`. Every process a test sees report that marker ran on the host; anything
//! reporting an empty one ran on the client. The client itself is the real UI, in-process.
//!
//! Each case runs in a child process so its `PATH` can put the stand-in `ssh` first without
//! mutating this process's environment.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use rozi::platform::paths::{PlatformEnv, extensions_dir};
use rozi::session::remote::RemoteTarget;
use rozi::state::{
    Attachment, HostKey, HostRuntimeStatus, InstanceStatus, Unavailable, WorkerProcessState,
};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;

const CASE_ENV: &str = "ROZI_REMOTE_RUNTIME_CASE";
const EXTENSION_ID: &str = "remote-probe";

fn executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Everything one case shares between the parent, the child client, and the stand-in host.
struct World {
    root: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        // Socket paths live under here, so it has to stay short.
        let root = tempfile::Builder::new()
            .prefix("rozi-rrt")
            .tempdir_in("/tmp")
            .unwrap();
        let world = Self { root };
        for dir in ["bin", "host/home", "host/run", "host/cache", "out", "both"] {
            std::fs::create_dir_all(world.path(dir)).unwrap();
        }
        std::fs::set_permissions(
            world.path("host/run"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        executable(
            &world.path("bin/ssh"),
            r#"#!/bin/sh
while [ "$#" -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
shift
unset ROZI_ASKPASS_ENDPOINT ROZI_ASKPASS_TOKEN ROZI_ASKPASS_SESSION SSH_ASKPASS SSH_ASKPASS_REQUIRE
unset ROZI_SOCKET ROZI_PANE ROZI_BIN ROZI_SESSION_INSTANCE ROZI_CONFIG
export HOME="$FAKE_HOST/home" XDG_RUNTIME_DIR="$FAKE_HOST/run" XDG_CACHE_HOME="$FAKE_HOST/cache"
export XDG_DATA_HOME="$FAKE_HOST/data" XDG_STATE_HOME="$FAKE_HOST/state" XDG_CONFIG_HOME="$FAKE_HOST/config"
export ROZI_TEST_HOST=pc
case "$*" in
  *extensions*runtime*)
    if [ -n "$FAKE_HOST_OLD_ROZI" ]; then
      echo "rozi: unknown extensions command \`runtime\` (expected list, install, update, remove, new, or check)" >&2
      exit 2
    fi
    echo "$$" >> "$FAKE_HOST/runtimes"
    ;;
esac
cd "$HOME"
exec /bin/sh -c "$*"
"#,
        );
        world
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// Run `case` as a child process playing the client.
    fn run(&self, case: &str, extra: &[(&str, &str)]) {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "remote_runtime_case", "--nocapture"])
            .env(CASE_ENV, case)
            .env("FAKE_HOST", self.path("host"))
            .env("WORLD", self.root.path())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env_remove("ROZI_SOCKET")
            .env_remove("ROZI_PANE")
            .env_remove("ROZI_BIN")
            .env_remove("ROZI_SESSION_INSTANCE")
            .env_remove("ROZI_CONFIG");
        for (key, value) in extra {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "case {case} failed\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn world() -> PathBuf {
    PathBuf::from(std::env::var("WORLD").unwrap())
}

fn host_dir(relative: &str) -> PathBuf {
    world().join("host").join(relative)
}

fn out(relative: &str) -> PathBuf {
    world().join("out").join(relative)
}

/// The probe service. It runs on the host, so everything it learns about the client it learns
/// through Rozi's bridge, and everything it writes says where it ran.
const PROBE: &str = r#"#!/bin/sh
dir="$OUT/w$$"
mkdir -p "$dir"
printf '%s\n' "$ROZI_TEST_HOST" "$ROZI_EXTENSION_DIR" "$ROZI_SOCKET" "$ROZI_BIN" "$PWD" > "$dir/env"
"$ROZI_BIN" run-action remote-probe.local > /dev/null 2> "$dir/local.err"; echo $? > "$dir/local.code"
ROZI_EXTENSION_CREDENTIAL=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff \
  "$ROZI_BIN" list-panes > /dev/null 2> "$dir/forged.err"; echo $? > "$dir/forged.code"
( printf '{"rows":[{"id":"r1","title":"Row","status":"working"}]}\n'; exec sleep 300 ) \
  | ROZI_PANE=1 "$ROZI_BIN" publish > "$dir/activations" 2> "$dir/publish.err" &
"$ROZI_BIN" subscribe config-reloaded > "$dir/events" 2> "$dir/subscribe.err" &
touch "$dir/ready"
while :; do
  "$ROZI_BIN" list-panes --format json > "$dir/panes.tmp" 2> "$dir/panes.err" && mv "$dir/panes.tmp" "$dir/panes"
  sleep 0.2
done
"#;

fn write_extension(service: &str) {
    let root = extensions_dir(&PlatformEnv::from_process()).join(EXTENSION_ID);
    executable(&root.join("bin/probe"), PROBE);
    let out = out("").display().to_string();
    let where_ = format!(
        "pwd -P > '{out}/where.tmp'; printf '%s\\n' \\\"$ROZI_TEST_HOST\\\" >> '{out}/where.tmp'; \
         mv '{out}/where.tmp' '{out}/where'"
    );
    std::fs::write(
        root.join("extension.toml"),
        format!(
            "[extension]\nid = \"{EXTENSION_ID}\"\napi = 1\n{service}\n\
             [[commands]]\nid = \"where\"\nshell = \"{where_}\"\nplacement = \"active-session\"\n\
             [[commands]]\nid = \"local\"\nshell = \"touch '{out}/local-ran'\"\n"
        ),
    )
    .unwrap();
}

fn standard_service() -> String {
    format!(
        "[[services]]\nname = \"watch\"\nexec = [\"./bin/probe\"]\nplacement = \"each-host\"\n\
         restart = \"always\"\n[services.env]\nOUT = {:?}\n",
        out("").display().to_string()
    )
}

fn write_config() {
    std::fs::write(
        rozi::config::config_path(),
        format!(
            "[remote.hosts.pc]\nbinary_path = {:?}\n",
            env!("CARGO_BIN_EXE_rozi")
        ),
    )
    .unwrap();
}

fn pc() -> RemoteTarget {
    RemoteTarget::Alias("pc".to_string())
}

/// Put an attached session on `host` on screen.
fn show_session(backend: &mut TestBackend<AppRoot>, host: Option<RemoteTarget>) {
    let state = backend.state_mut();
    let attachment = state.current_mut();
    attachment.session_attached = true;
    attachment.remote_host = host.as_ref().map(RemoteTarget::display_label);
    attachment.remote_target = host;
    attachment.session_instance = Some(rozi::session::protocol::SessionInstanceId::generate());
    if let Some(focused) = attachment.workspaces[0].panes.first().map(|pane| pane.id) {
        attachment.focused_pane = Some(focused);
        attachment.workspaces[0].focused_pane = Some(focused);
    }
    backend.dispatch(Msg::SidebarTreeFocused).unwrap();
}

fn set_pane_cwd(backend: &mut TestBackend<AppRoot>, cwd: &str) {
    backend.state_mut().current_mut().workspaces[0].panes[0]
        .terminal
        .cwd = Some(cwd.to_string());
}

fn pump_until(
    backend: &mut TestBackend<AppRoot>,
    timeout: Duration,
    what: &str,
    mut predicate: impl FnMut(&TestBackend<AppRoot>) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while !predicate(backend) {
        backend.render();
        let _ = backend.pump();
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}\n{}", describe(backend));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn pump_for(backend: &mut TestBackend<AppRoot>, span: Duration) {
    let deadline = Instant::now() + span;
    while Instant::now() < deadline {
        backend.render();
        let _ = backend.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn describe(backend: &TestBackend<AppRoot>) -> String {
    let state = backend.state();
    let instances: Vec<_> = state
        .extension_runtime
        .instances
        .iter()
        .map(|instance| format!("{:?} {:?}", instance.key, instance.status))
        .collect();
    let hosts: Vec<_> = state
        .extension_runtime
        .hosts
        .values()
        .map(|host| format!("{:?} epoch {} {:?}", host.host, host.epoch, host.status))
        .collect();
    let processes: Vec<_> = state
        .extension_runtime
        .processes
        .iter()
        .map(|(worker, process)| format!("{worker:?} {:?} {:?}", process.purpose, process.state))
        .collect();
    let pending: Vec<_> = state
        .extension_runtime
        .pending
        .iter()
        .map(|pending| format!("{:?} {}", pending.worker, pending.digest))
        .collect();
    let staged: Vec<_> = state
        .extension_runtime
        .hosts
        .values()
        .map(|host| format!("staged {:?} staging {:?}", host.staged, host.staging))
        .collect();
    let listing: Vec<_> = std::fs::read_dir(out(""))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    let inner: Vec<_> = std::fs::read_dir(entry.path())
                        .map(|files| files.flatten().map(|file| file.file_name()).collect())
                        .unwrap_or_default();
                    format!("{:?} {inner:?}", entry.file_name())
                })
                .collect()
        })
        .unwrap_or_default();
    format!(
        "instances {instances:#?}\nruntimes {hosts:#?}\nprocesses {processes:#?}\npending {pending:#?}\n{staged:?}\nout {listing:?}"
    )
}

/// Running service instances' pids. A placed command's process is short-lived and not one of them.
fn running(backend: &TestBackend<AppRoot>) -> Vec<u32> {
    backend
        .state()
        .extension_runtime
        .processes
        .values()
        .filter(|process| matches!(process.purpose, rozi::state::WorkerPurpose::Service { .. }))
        .filter_map(|process| match process.state {
            WorkerProcessState::Running { pid } => Some(pid),
            _ => None,
        })
        .collect()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn kill(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn wait_file(backend: &mut TestBackend<AppRoot>, path: &Path) {
    pump_until(
        backend,
        Duration::from_secs(30),
        &path.display().to_string(),
        |_| path.exists(),
    );
}

fn wait_dead(backend: &mut TestBackend<AppRoot>, pid: u32) {
    pump_until(
        backend,
        Duration::from_secs(15),
        &format!("pid {pid} to exit"),
        |_| !alive(pid),
    );
}

/// The one worker running, once it has reported in.
fn worker(backend: &mut TestBackend<AppRoot>, not: &[u32]) -> (u32, PathBuf) {
    pump_until(
        backend,
        Duration::from_secs(30),
        "a running worker",
        |backend| running(backend).iter().any(|pid| !not.contains(pid)),
    );
    let pid = running(backend)
        .into_iter()
        .find(|pid| !not.contains(pid))
        .unwrap();
    let dir = out(&format!("w{pid}"));
    wait_file(backend, &dir.join("ready"));
    (pid, dir)
}

fn runtimes() -> Vec<u32> {
    read(&host_dir("runtimes"))
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

fn instance_statuses(backend: &TestBackend<AppRoot>) -> Vec<InstanceStatus> {
    backend
        .state()
        .extension_runtime
        .instances
        .iter()
        .map(|instance| instance.status.clone())
        .collect()
}

struct Quit(TestBackend<AppRoot>);

impl Drop for Quit {
    fn drop(&mut self) {
        let _ = self.0.dispatch(Msg::RunAction(rozi::input::Action::Quit));
    }
}

fn client(service: &str) -> Quit {
    rozi::test_support::isolate_user_dirs();
    write_config();
    write_extension(service);
    Quit(TestBackend::new(rozi::test_support::configured_app()))
}

/// Dispatches to each case when run as the child. A no-op in the parent run.
#[test]
fn remote_runtime_case() {
    let Ok(case) = std::env::var(CASE_ENV) else {
        return;
    };
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || match case.as_str() {
            "main" => main_case(),
            "old" => old_rozi_case(),
            "platform" => platform_case(),
            "missing" => missing_case(),
            "sessions" => sessions_case(),
            "client" => client_case(),
            other => panic!("unknown case {other}"),
        })
        .unwrap()
        .join()
        .unwrap();
}

fn main_case() {
    let mut app = client(&standard_service());
    let backend = &mut app.0;
    set_pane_cwd(backend, &world().join("both").display().to_string());
    show_session(backend, Some(pc()));
    let (pid, dir) = worker(backend, &[]);

    // It ran on the host, from the host's bundle cache, with the host's bridge and binary.
    let env = read(&dir.join("env"));
    let env: Vec<_> = env.lines().collect();
    assert_eq!(env[0], "pc", "the service ran on the host: {env:?}");
    let digest = backend.state().config.extension_placements[EXTENSION_ID]
        .bundle_digest()
        .to_string();
    assert!(
        env[1].starts_with(host_dir("cache").to_str().unwrap()),
        "{env:?}"
    );
    assert!(env[1].ends_with(&digest), "{env:?}");
    assert!(
        env[2].starts_with(host_dir("run").to_str().unwrap()),
        "{env:?}"
    );
    assert!(env[2].contains("extension-bridge-"), "{env:?}");
    assert_eq!(env[3], env!("CARGO_BIN_EXE_rozi"));
    assert_eq!(runtimes().len(), 1, "one runtime for the host");

    // Nothing it asks for may start a process on the client, directly or not.
    assert_ne!(read(&dir.join("local.code")).trim(), "0");
    assert!(read(&dir.join("local.err")).contains("not permitted"));
    assert!(
        !out("local-ran").exists(),
        "the client-run command never ran"
    );
    assert_ne!(read(&dir.join("forged.code")).trim(), "0");
    assert!(read(&dir.join("forged.err")).contains("credential"));

    // The session on screen is on its host: it sees its panes.
    wait_file(backend, &dir.join("panes"));
    let panes = |dir: &Path| -> usize {
        serde_json::from_str::<serde_json::Value>(&read(&dir.join("panes")))
            .ok()
            .and_then(|value| value["data"].as_array().map(Vec::len))
            .unwrap_or(usize::MAX)
    };
    assert_eq!(panes(&dir), 1);

    // Streaming: rows published over the bridge land on the pane, and an activation travels back.
    pump_until(
        backend,
        Duration::from_secs(15),
        "the publish stream",
        |backend| backend.state().publish_streams.contains_key(&1),
    );
    backend.state().publish_streams[&1]
        .sender
        .try_send("{\"activate\":\"r1\"}\n".to_string())
        .unwrap();
    pump_until(backend, Duration::from_secs(15), "the activation", |_| {
        read(&dir.join("activations")).contains("r1")
    });
    // A reload with nothing changed keeps the generation and is heard by the subscriber.
    let generation = backend.state().extension_generations[EXTENSION_ID].clone();
    backend
        .dispatch(Msg::RunAction(rozi::input::Action::ReloadExtensions))
        .unwrap();
    pump_until(backend, Duration::from_secs(15), "the event", |_| {
        read(&dir.join("events")).contains("config-reloaded")
    });
    assert_eq!(
        backend.state().extension_generations[EXTENSION_ID],
        generation
    );
    assert_eq!(
        running(backend),
        vec![pid],
        "an unchanged reload keeps the worker"
    );

    // A local session on screen, the PC's still attached behind it: the worker stays, but sees
    // and hears nothing of the local session.
    {
        let state = backend.state_mut();
        let remote = std::mem::replace(state.current_mut(), Attachment::new());
        state.background.insert(9_001, remote);
    }
    show_session(backend, None);
    pump_until(backend, Duration::from_secs(15), "an empty listing", |_| {
        panes(&dir) == 0
    });
    let events = read(&dir.join("events")).lines().count();
    backend
        .dispatch(Msg::RunAction(rozi::input::Action::ReloadExtensions))
        .unwrap();
    pump_for(backend, Duration::from_millis(600));
    assert_eq!(read(&dir.join("events")).lines().count(), events);
    assert!(alive(pid));
    assert_eq!(runtimes().len(), 1, "no new runtime for the same host");

    // Back on the PC. A command placed on the active session runs there, in the pane's directory -
    // a path that also exists on the client, so only the host marker tells them apart.
    {
        let state = backend.state_mut();
        let remote = state.background.remove(&9_001).unwrap();
        *state.current_mut() = remote;
    }
    backend.dispatch(Msg::SidebarTreeFocused).unwrap();
    let both = world().join("both");
    set_pane_cwd(backend, &both.display().to_string());
    run_named(backend, "remote-probe.where");
    wait_file(backend, &out("where"));
    let where_ = read(&out("where"));
    let mut lines = where_.lines();
    assert_eq!(
        lines.next(),
        Some(std::fs::canonicalize(&both).unwrap().to_str().unwrap())
    );
    assert_eq!(lines.next(), Some("pc"));
    // A directory the host does not have: the command still runs there, in the host's own one.
    std::fs::remove_file(out("where")).unwrap();
    set_pane_cwd(backend, "/nonexistent/rozi/remote/only");
    run_named(backend, "remote-probe.where");
    wait_file(backend, &out("where"));
    let where_ = read(&out("where"));
    assert_eq!(
        where_.lines().collect::<Vec<_>>(),
        [
            std::fs::canonicalize(host_dir("home"))
                .unwrap()
                .to_str()
                .unwrap(),
            "pc"
        ]
    );

    // Tampering with the staged files: the next start verifies them, discards them, and the client
    // stages its own copy again.
    let staged = host_dir("cache")
        .join("rozi/extension-bundles")
        .join(&digest);
    std::fs::set_permissions(staged.join("bin"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(
        staged.join("bin/probe"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(
        staged.join("bin/probe"),
        "#!/bin/sh\necho pwned > \"$OUT/pwned\"\n",
    )
    .unwrap();
    kill(pid);
    let (restarted, restarted_dir) = worker(backend, &[pid]);
    assert!(!out("pwned").exists(), "the tampered file never ran");
    assert_eq!(read(&staged.join("bin/probe")), PROBE);
    assert!(read(&restarted_dir.join("env")).starts_with("pc\n"));

    // Editing the extension on the client restages it under a new digest and generation.
    let root = extensions_dir(&PlatformEnv::from_process()).join(EXTENSION_ID);
    executable(&root.join("bin/probe"), &format!("{PROBE}# edited\n"));
    backend
        .dispatch(Msg::RunAction(rozi::input::Action::ReloadExtensions))
        .unwrap();
    let (edited, edited_dir) = worker(backend, &[pid, restarted]);
    wait_dead(backend, restarted);
    let new_digest = backend.state().config.extension_placements[EXTENSION_ID]
        .bundle_digest()
        .to_string();
    assert_ne!(new_digest, digest);
    assert!(
        read(&edited_dir.join("env"))
            .lines()
            .nth(1)
            .unwrap()
            .ends_with(&new_digest)
    );

    // The SSH channel dies. The client reconnects with a new runtime, the worker starts again
    // under it, and nothing from the old connection is honored.
    let old_epoch = backend.state().extension_runtime.hosts[&HostKey::Remote(pc())].epoch;
    let old_runtime = *runtimes().last().unwrap();
    kill(old_runtime);
    wait_dead(backend, edited);
    let (reconnected, _) = worker(backend, &[pid, restarted, edited]);
    let new_epoch = backend.state().extension_runtime.hosts[&HostKey::Remote(pc())].epoch;
    assert!(new_epoch > old_epoch);
    assert_eq!(runtimes().len(), 2);
    assert!(
        backend
            .state()
            .extension_workers
            .iter()
            .all(|worker| worker.runtime == rozi::state::WorkerRuntime(new_epoch))
    );

    // Leaving the client leaves nothing behind on the host.
    let runtime = *runtimes().last().unwrap();
    drop(app);
    let deadline = Instant::now() + Duration::from_secs(15);
    while alive(reconnected) || alive(runtime) {
        assert!(
            Instant::now() < deadline,
            "the host kept running after the client left"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_named(backend: &mut TestBackend<AppRoot>, id: &str) {
    let index = backend
        .state()
        .config
        .commands
        .iter()
        .position(|command| command.id == id)
        .unwrap();
    backend
        .dispatch(Msg::RunAction(rozi::input::Action::RunNamedCommand(index)))
        .unwrap();
}

fn unavailable(backend: &mut TestBackend<AppRoot>) -> Unavailable {
    pump_until(
        backend,
        Duration::from_secs(30),
        "an unavailable instance",
        |backend| {
            instance_statuses(backend)
                .iter()
                .any(|status| matches!(status, InstanceStatus::Unavailable(_)))
        },
    );
    instance_statuses(backend)
        .into_iter()
        .find_map(|status| match status {
            InstanceStatus::Unavailable(reason) => Some(reason),
            _ => None,
        })
        .unwrap()
}

fn old_rozi_case() {
    let mut app = client(&standard_service());
    let backend = &mut app.0;
    show_session(backend, Some(pc()));
    let reason = unavailable(backend);
    assert!(
        matches!(reason, Unavailable::RuntimeUnsupported { .. }),
        "{reason:?}"
    );
    // Not retried: asking an old Rozi again will not teach it the runtime.
    let host = &backend.state().extension_runtime.hosts[&HostKey::Remote(pc())];
    assert!(matches!(host.status, HostRuntimeStatus::Unavailable(_)));
    assert!(host.retry_at.is_none());
    assert!(!out("").read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('w')
    }));
}

fn platform_case() {
    let mut app = client(&standard_service());
    // Rewrite the manifest to exclude the host's platform. The client is the same machine here, so
    // its own placed contributions are excluded too; what matters is what the host says.
    let root = extensions_dir(&PlatformEnv::from_process()).join(EXTENSION_ID);
    let manifest = read(&root.join("extension.toml"))
        .replace("api = 1\n", "api = 1\nplatforms = [\"windows\"]\n");
    std::fs::write(root.join("extension.toml"), manifest).unwrap();
    let backend = &mut app.0;
    backend
        .dispatch(Msg::RunAction(rozi::input::Action::ReloadExtensions))
        .unwrap();
    show_session(backend, Some(pc()));
    let reason = unavailable(backend);
    assert_eq!(
        reason,
        Unavailable::UnsupportedPlatform {
            os: std::env::consts::OS.to_string()
        }
    );
}

fn missing_case() {
    let mut app = client(
        "[[services]]\nname = \"watch\"\nexec = [\"rozi-test-tool-that-is-not-installed\"]\n\
         placement = \"each-host\"\n",
    );
    let backend = &mut app.0;
    show_session(backend, Some(pc()));
    let reason = unavailable(backend);
    assert_eq!(
        reason,
        Unavailable::MissingExecutable {
            program: "rozi-test-tool-that-is-not-installed".to_string()
        }
    );
}

/// Several sessions on one host share its runtime; a per-session service runs once per session,
/// a per-host one once in all.
fn sessions_case() {
    let service = |name: &str, placement: &str| {
        format!(
            "[[services]]\nname = \"{name}\"\nexec = [\"./bin/probe\"]\nplacement = \"{placement}\"\n\
             [services.env]\nOUT = {:?}\n",
            out("").display().to_string()
        )
    };
    let mut app = client(&format!(
        "{}{}",
        service("host", "each-host"),
        service("session", "each-session")
    ));
    let backend = &mut app.0;
    show_session(backend, Some(pc()));
    {
        let state = backend.state_mut();
        let first = std::mem::replace(state.current_mut(), Attachment::new());
        state.background.insert(9_002, first);
    }
    show_session(backend, Some(pc()));
    pump_until(
        backend,
        Duration::from_secs(30),
        "three workers",
        |backend| running(backend).len() == 3,
    );
    let instances = &backend.state().extension_runtime.instances;
    let count = |name: &str| {
        instances
            .iter()
            .filter(|instance| instance.key.service == format!("{EXTENSION_ID}.{name}"))
            .count()
    };
    assert_eq!(count("host"), 1);
    assert_eq!(count("session"), 2);
    assert_eq!(backend.state().extension_runtime.hosts.len(), 1);
    assert_eq!(
        runtimes().len(),
        1,
        "one runtime serves every session on the host"
    );
}

/// One of two clients: runs until told to stop, recording what its worker and runtime are.
fn client_case() {
    let name = std::env::var("CLIENT_NAME").unwrap();
    let mut app = client(&standard_service());
    let backend = &mut app.0;
    show_session(backend, Some(pc()));
    let (pid, _) = worker(backend, &[]);
    std::fs::write(out(&format!("client-{name}")), pid.to_string()).unwrap();
    let stop = out(&format!("stop-{name}"));
    pump_until(backend, Duration::from_secs(60), "the stop signal", |_| {
        stop.exists()
    });
    drop(app);
    let deadline = Instant::now() + Duration::from_secs(15);
    while alive(pid) {
        assert!(Instant::now() < deadline, "worker outlived its client");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn placed_processes_run_on_the_host_and_reach_the_client_only_through_rozi() {
    World::new().run("main", &[]);
}

#[test]
fn an_old_remote_rozi_is_reported_rather_than_worked_around() {
    World::new().run("old", &[("FAKE_HOST_OLD_ROZI", "1")]);
}

#[test]
fn a_host_platform_the_extension_excludes_is_reported() {
    World::new().run("platform", &[]);
}

#[test]
fn a_program_missing_on_the_host_is_reported() {
    World::new().run("missing", &[]);
}

#[test]
fn sessions_on_one_host_share_its_runtime() {
    World::new().run("sessions", &[]);
}

/// Two clients on the same host each get their own runtime and their own workers, and one
/// leaving takes nothing of the other's with it.
#[test]
fn two_clients_on_one_host_are_independent() {
    let world = World::new();
    std::thread::scope(|scope| {
        let a = scope.spawn(|| world.run("client", &[("CLIENT_NAME", "a")]));
        let b = scope.spawn(|| world.run("client", &[("CLIENT_NAME", "b")]));
        let pid = |name: &str| -> u32 {
            let path = world.path(&format!("out/client-{name}"));
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                if let Ok(pid) = read(&path).trim().parse() {
                    return pid;
                }
                assert!(
                    Instant::now() < deadline,
                    "client {name} never started its worker"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let (pid_a, pid_b) = (pid("a"), pid("b"));
        assert_ne!(pid_a, pid_b);
        assert_eq!(
            read(&world.path("host/runtimes")).lines().count(),
            2,
            "each client has its own runtime"
        );
        std::fs::write(world.path("out/stop-a"), "").unwrap();
        a.join().unwrap();
        assert!(!alive(pid_a));
        assert!(alive(pid_b), "the other client's worker is untouched");
        std::fs::write(world.path("out/stop-b"), "").unwrap();
        b.join().unwrap();
        assert!(!alive(pid_b));
    });
}
