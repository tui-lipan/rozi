//! Placed extension processes, end to end, through this machine's in-process extension runtime:
//! the same runtime a remote host runs, minus SSH. See `extension_runtime_remote_e2e.rs` for the
//! remote half.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rozi::platform::paths::{PlatformEnv, extensions_dir};
use rozi::state::{InstanceStatus, WorkerProcessState};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;

struct CleanupBackend(TestBackend<AppRoot>);

impl std::ops::Deref for CleanupBackend {
    type Target = TestBackend<AppRoot>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for CleanupBackend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for CleanupBackend {
    fn drop(&mut self) {
        let _ = self.0.dispatch(Msg::RunAction(rozi::input::Action::Quit));
    }
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
            let state = backend.state();
            let instances: Vec<_> = state
                .extension_runtime
                .instances
                .iter()
                .map(|instance| format!("{} {:?}", instance.key.service, instance.status))
                .collect();
            let hosts: Vec<_> = state
                .extension_runtime
                .hosts
                .values()
                .map(|host| format!("{:?} {:?}", host.host, host.status))
                .collect();
            panic!("timed out waiting for {what}; instances {instances:?}; runtimes {hosts:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A probe that asks the UI for things through the bridge and records what it was told, then
/// stays alive. Uses `PROBE_ROZI` rather than `ROZI_BIN`: in a test the runtime runs inside the
/// test binary, whose path is not a `rozi`.
const PROBE: &str = r#"#!/bin/sh
out="$PROBE_OUT/$$"
mkdir -p "$out"
printf '%s\n' "$ROZI_EXTENSION_DIR" "$PWD" "$ROZI_SOCKET" > "$out/env"
"$PROBE_ROZI" list-panes --format json > "$out/panes" 2> "$out/panes.err"; echo $? > "$out/panes.code"
"$PROBE_ROZI" run-action reload-extensions > /dev/null 2> "$out/denied.err"; echo $? > "$out/denied.code"
ROZI_EXTENSION_CREDENTIAL=0000000000000000000000000000000000000000000000000000000000000000 \
  "$PROBE_ROZI" list-panes > /dev/null 2> "$out/forged.err"; echo $? > "$out/forged.code"
ROZI_EXTENSION_CREDENTIAL= "$PROBE_ROZI" notify unattributed > /dev/null 2> "$out/bare.err"; echo $? > "$out/bare.code"
touch "$out/done"
exec sleep 300
"#;

fn write_extension(root: &Path, id: &str, out: &Path) {
    executable(&root.join("bin/probe"), PROBE);
    std::fs::write(
        root.join("extension.toml"),
        format!(
            "[extension]\nid = \"{id}\"\napi = 1\n\
             [[services]]\nname = \"watch\"\nexec = [\"./bin/probe\"]\nplacement = \"each-host\"\n\
             restart = \"never\"\n\
             [services.env]\nPROBE_ROZI = {rozi:?}\nPROBE_OUT = {out:?}\n\
             [[commands]]\nid = \"where\"\nshell = \"pwd -P > \\\"$PROBE_OUT/where\\\"\"\n\
             placement = \"active-session\"\n",
            rozi = env!("CARGO_BIN_EXE_rozi"),
            out = out.display().to_string(),
        ),
    )
    .unwrap();
}

/// The running worker of `extension`. Tests in this binary share one extensions directory, so
/// another test's extension may be running beside this one's.
fn running_pid(backend: &TestBackend<AppRoot>, extension: &str) -> Option<u32> {
    let state = backend.state();
    state
        .extension_runtime
        .processes
        .iter()
        .filter(|(worker, _)| {
            state
                .extension_workers
                .get(**worker)
                .is_some_and(|worker| worker.extension.id == extension)
        })
        .find_map(|(_, process)| match process.state {
            WorkerProcessState::Running { pid } => Some(pid),
            _ => None,
        })
}

fn alive(pid: u32) -> bool {
    // Signal 0 probes without delivering anything; a zombie the runtime has not reaped yet still
    // answers, so callers wait for it to go.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_dead(backend: &mut TestBackend<AppRoot>, pid: u32) {
    pump_until(
        backend,
        Duration::from_secs(15),
        "the worker to exit",
        |_| !alive(pid),
    );
}

fn report_dir(out: &Path, pid: u32) -> PathBuf {
    out.join(pid.to_string())
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Mark the client attached to a local session. A real attach arrives as a message, and the
/// placement sync runs after messages, so one is sent the way the attach would have been.
fn attach_local_session(backend: &mut TestBackend<AppRoot>) {
    backend.state_mut().current_mut().session_attached = true;
    backend.dispatch(Msg::SidebarTreeFocused).unwrap();
}

#[test]
fn a_placed_service_runs_from_its_bundle_and_reaches_the_ui_only_as_itself() {
    rozi::test_support::isolate_user_dirs();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            const EXTENSION_ID: &str = "placed-e2e";
            // Every test here loads the shared config; hold it so none sees another's edits.
            let _config = rozi::test_support::lock_config_file();
            let out = tempfile::Builder::new()
                .prefix("rozi-placed")
                .tempdir_in("/tmp")
                .unwrap();
            let root = extensions_dir(&PlatformEnv::from_process()).join(EXTENSION_ID);
            write_extension(&root, EXTENSION_ID, out.path());

            let mut backend =
                CleanupBackend(TestBackend::new(rozi::test_support::configured_app()));
            assert!(
                backend
                    .state()
                    .config
                    .extension_placements
                    .contains_key(EXTENSION_ID),
                "fixture has placed contributions"
            );
            // Nothing runs until a session is attached: the service is placed on each host *with a
            // session*, and there is none yet.
            backend.render();
            let _ = backend.pump();
            assert!(backend.state().extension_runtime.processes.is_empty());

            attach_local_session(&mut backend);
            pump_until(
                &mut backend,
                Duration::from_secs(15),
                "the worker",
                |backend| running_pid(backend, EXTENSION_ID).is_some(),
            );
            let pid = running_pid(&backend, EXTENSION_ID).unwrap();
            let report = report_dir(out.path(), pid);
            pump_until(&mut backend, Duration::from_secs(30), "the probe", |_| {
                report.join("done").exists()
            });

            let env = read(&report.join("env"));
            let mut env = env.lines();
            let bundle_dir = env.next().unwrap().to_string();
            let digest = backend.state().config.extension_placements[EXTENSION_ID]
                .bundle_digest()
                .to_string();
            assert!(
                bundle_dir.ends_with(&digest),
                "runs from the bundle, not the installation: {bundle_dir}"
            );
            assert_ne!(Path::new(&bundle_dir), root.as_path());
            assert_eq!(
                std::fs::canonicalize(env.next().unwrap()).unwrap(),
                std::fs::canonicalize(&bundle_dir).unwrap()
            );
            assert!(env.next().unwrap().contains("extension-bridge-"));

            // Its own credential reaches the UI, through the bridge.
            assert_eq!(
                read(&report.join("panes.code")).trim(),
                "0",
                "{}",
                read(&report.join("panes.err"))
            );
            let panes: serde_json::Value =
                serde_json::from_str(&read(&report.join("panes"))).unwrap();
            assert_eq!(panes["data"].as_array().map(Vec::len), Some(1), "{panes}");
            // Running an action that would start something on the client is refused.
            assert_ne!(read(&report.join("denied.code")).trim(), "0");
            assert!(
                read(&report.join("denied.err")).contains("not permitted"),
                "{}",
                read(&report.join("denied.err"))
            );
            // A forged credential is nobody, and so is none at all over the bridge.
            assert_ne!(read(&report.join("forged.code")).trim(), "0");
            assert!(read(&report.join("forged.err")).contains("credential"));
            assert_ne!(read(&report.join("bare.code")).trim(), "0");

            // Editing the installed files is a new bundle and a new generation: the old worker
            // stops and a new one runs the new files.
            let generation = backend.state().extension_generations[EXTENSION_ID].clone();
            let probe = root.join("bin/probe");
            executable(&probe, &format!("{PROBE}# edited\n"));
            backend
                .dispatch(Msg::RunAction(rozi::input::Action::ReloadExtensions))
                .unwrap();
            pump_until(
                &mut backend,
                Duration::from_secs(15),
                "the new generation",
                |backend| backend.state().extension_generations[EXTENSION_ID] != generation,
            );
            wait_dead(&mut backend, pid);
            pump_until(
                &mut backend,
                Duration::from_secs(15),
                "the new worker",
                |backend| running_pid(backend, EXTENSION_ID).is_some_and(|new| new != pid),
            );
            let new_pid = running_pid(&backend, EXTENSION_ID).unwrap();
            let new_digest = backend.state().config.extension_placements[EXTENSION_ID]
                .bundle_digest()
                .to_string();
            assert_ne!(new_digest, digest);
            let new_report = report_dir(out.path(), new_pid);
            pump_until(
                &mut backend,
                Duration::from_secs(30),
                "the new probe",
                |_| new_report.join("done").exists(),
            );
            assert!(
                read(&new_report.join("env"))
                    .lines()
                    .next()
                    .unwrap()
                    .ends_with(&new_digest)
            );

            // Leaving the client leaves nothing running.
            drop(backend);
            let deadline = Instant::now() + Duration::from_secs(15);
            while alive(new_pid) {
                assert!(Instant::now() < deadline, "worker outlived its client");
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn a_command_placed_on_the_active_session_runs_in_its_focused_pane_directory() {
    rozi::test_support::isolate_user_dirs();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let _config = rozi::test_support::lock_config_file();
            let out = tempfile::Builder::new()
                .prefix("rozi-placed-cmd")
                .tempdir_in("/tmp")
                .unwrap();
            let root = extensions_dir(&PlatformEnv::from_process()).join("placed-cmd");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join("extension.toml"),
                format!(
                    "[extension]\nid = \"placed-cmd\"\napi = 1\n\
                     [[commands]]\nid = \"where\"\nshell = \"pwd -P > '{}/where'\"\n\
                     placement = \"active-session\"\n",
                    out.path().display()
                ),
            )
            .unwrap();
            let mut backend =
                CleanupBackend(TestBackend::new(rozi::test_support::configured_app()));
            let index = backend
                .state()
                .config
                .commands
                .iter()
                .position(|command| command.id == "placed-cmd.where")
                .expect("command loaded");

            // Without a session there is nowhere to run it, and it does not run here instead.
            backend
                .dispatch(Msg::RunAction(rozi::input::Action::RunNamedCommand(index)))
                .unwrap();
            assert!(backend.state().extension_workers.is_empty());

            let project = tempfile::tempdir().unwrap();
            attach_local_session(&mut backend);
            {
                let state = backend.state_mut();
                let focused = state.current().workspaces[0].panes[0].id;
                state.current_mut().focused_pane = Some(focused);
                state.current_mut().workspaces[0].focused_pane = Some(focused);
                state.current_mut().workspaces[0].panes[0].terminal.cwd =
                    Some(project.path().display().to_string());
            }
            backend
                .dispatch(Msg::RunAction(rozi::input::Action::RunNamedCommand(index)))
                .unwrap();
            let target = out.path().join("where");
            pump_until(&mut backend, Duration::from_secs(30), "the command", |_| {
                read(&target).ends_with('\n')
            });
            assert_eq!(
                read(&target).trim(),
                std::fs::canonicalize(project.path())
                    .unwrap()
                    .display()
                    .to_string()
            );
            pump_until(
                &mut backend,
                Duration::from_secs(10),
                "the command to exit",
                |backend| {
                    !backend
                        .state()
                        .extension_workers
                        .iter()
                        .any(|worker| worker.extension.id == "placed-cmd")
                },
            );
            assert!(
                !backend
                    .state()
                    .extension_workers
                    .iter()
                    .any(|worker| worker.extension.id == "placed-cmd"),
                "credential revoked on exit"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn disabling_a_placed_extension_stops_its_workers_and_revokes_their_credentials() {
    rozi::test_support::isolate_user_dirs();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            const EXTENSION_ID: &str = "placed-off";
            let _config = rozi::test_support::lock_config_file();
            let out = tempfile::Builder::new()
                .prefix("rozi-placed-off")
                .tempdir_in("/tmp")
                .unwrap();
            let root = extensions_dir(&PlatformEnv::from_process()).join(EXTENSION_ID);
            write_extension(&root, EXTENSION_ID, out.path());
            std::fs::write(rozi::config::config_path(), "").unwrap();
            let mut backend =
                CleanupBackend(TestBackend::new(rozi::test_support::configured_app()));
            attach_local_session(&mut backend);
            pump_until(
                &mut backend,
                Duration::from_secs(15),
                "the worker",
                |backend| running_pid(backend, EXTENSION_ID).is_some(),
            );
            let pid = running_pid(&backend, EXTENSION_ID).unwrap();
            let ours = |backend: &TestBackend<AppRoot>| {
                backend
                    .state()
                    .extension_runtime
                    .instances
                    .iter()
                    .filter(|instance| instance.extension == EXTENSION_ID)
                    .map(|instance| instance.status.clone())
                    .collect::<Vec<_>>()
            };
            assert!(matches!(
                ours(&backend)[..],
                [InstanceStatus::Running { .. }]
            ));

            std::fs::write(
                rozi::config::config_path(),
                format!("[extensions]\ndisabled = [\"{EXTENSION_ID}\"]\n"),
            )
            .unwrap();
            backend
                .dispatch(Msg::RunAction(rozi::input::Action::ReloadExtensions))
                .unwrap();
            wait_dead(&mut backend, pid);
            assert!(
                !backend
                    .state()
                    .extension_workers
                    .iter()
                    .any(|worker| worker.extension.id == EXTENSION_ID)
            );
            assert!(ours(&backend).is_empty());
            std::fs::write(rozi::config::config_path(), "").unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
}
