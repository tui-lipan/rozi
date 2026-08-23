use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rozi::config::GENERATION_ENV;
use rozi::platform::paths::{PlatformEnv, extensions_dir};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;

const EXTENSION_ID: &str = "generation-e2e";

#[test]
fn extension_service_helper() {
    if std::env::var("ROZI_EXTENSION").as_deref() != Ok(EXTENSION_ID) {
        return;
    }
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn write_manifest(directory: &Path, revision: &str) {
    std::fs::create_dir_all(directory).unwrap();
    let executable = std::env::current_exe().unwrap();
    std::fs::write(
        directory.join("extension.toml"),
        format!(
            "[extension]\nid = \"{EXTENSION_ID}\"\napi = 1\n\
             [[services]]\nname = \"watch\"\n\
             exec = [{executable:?}, \"--exact\", \"extension_service_helper\", \"--nocapture\"]\n\
             restart = \"never\"\n[services.env]\nREVISION = \"{revision}\"\n"
        ),
    )
    .unwrap();
}

fn pump_until(
    backend: &mut TestBackend<AppRoot>,
    timeout: Duration,
    mut predicate: impl FnMut(&TestBackend<AppRoot>) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while !predicate(backend) {
        backend.render();
        let _ = backend.pump();
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_cli(socket: &Path, generation: &str, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(args)
        .env("ROZI_SOCKET", socket)
        .env("ROZI_EXTENSION", EXTENSION_ID)
        .env(GENERATION_ENV, generation)
        // A developer running the suite from inside rozi has ROZI_PANE set, and `publish` forwards
        // it as the request's source pane. That pane id belongs to the developer's own session, not
        // to the isolated app this test builds, so the server rejects the request with "pane N not
        // found" and the generation assertion fails for a reason that has nothing to do with
        // generations. CI has no ambient pane, which is why this only ever failed locally.
        .env_remove("ROZI_PANE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn expect_cli_exit(
    backend: &mut TestBackend<AppRoot>,
    socket: &Path,
    generation: &str,
    args: &[&str],
    success: bool,
) {
    let mut child = spawn_cli(socket, generation, args);
    pump_until(backend, Duration::from_secs(5), |_| {
        child.try_wait().unwrap().is_some()
    });
    assert_eq!(child.wait().unwrap().success(), success, "{args:?}");
}

fn expect_cli_stream_accepted(
    backend: &mut TestBackend<AppRoot>,
    socket: &Path,
    generation: &str,
    args: &[&str],
) {
    let mut child = open_cli_stream(backend, socket, generation, args);
    child.kill().unwrap();
    child.wait().unwrap();
}

fn open_cli_stream(
    backend: &mut TestBackend<AppRoot>,
    socket: &Path,
    generation: &str,
    args: &[&str],
) -> Child {
    let mut child = spawn_cli(socket, generation, args);
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        backend.render();
        let _ = backend.pump();
        assert!(
            child.try_wait().unwrap().is_none(),
            "{args:?} was rejected instead of opening a stream"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child
}

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

#[test]
fn retired_generation_is_fenced_across_all_extension_control_surfaces() {
    rozi::test_support::isolate_user_dirs();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let root = extensions_dir(&PlatformEnv::from_process()).join(EXTENSION_ID);
            write_manifest(&root, "a");

            let mut backend =
                CleanupBackend(TestBackend::new(rozi::test_support::configured_app()));
            if !backend
                .state()
                .config
                .active_extensions
                .contains(EXTENSION_ID)
            {
                let diagnostic = Command::new(env!("CARGO_BIN_EXE_rozi"))
                    .args(["check-extension", root.to_str().unwrap(), "--json"])
                    .output()
                    .unwrap();
                panic!(
                    "fixture extension was not loaded: {}\n{}",
                    String::from_utf8_lossy(&diagnostic.stdout),
                    String::from_utf8_lossy(&diagnostic.stderr)
                );
            }
            pump_until(&mut backend, Duration::from_secs(5), |backend| {
                backend.state().control_socket_path.is_some()
                    && backend
                        .state()
                        .services
                        .running
                        .contains_key("generation-e2e.watch")
            });
            let socket = backend.state().control_socket_path.clone().unwrap();
            let token_a = backend.state().extension_generations[EXTENSION_ID].clone();
            let pid_a = backend.state().services.running["generation-e2e.watch"]
                .child
                .id();

            backend
                .dispatch(Msg::RunAction(rozi::input::Action::ReloadConfig))
                .unwrap();
            pump_until(&mut backend, Duration::from_secs(5), |backend| {
                backend.state().services.running["generation-e2e.watch"]
                    .child
                    .id()
                    == pid_a
            });
            assert_eq!(
                backend.state().extension_generations[EXTENSION_ID],
                token_a,
                "unchanged reload rotated the fencing token"
            );

            write_manifest(&root, "b");
            backend
                .dispatch(Msg::RunAction(rozi::input::Action::ReloadConfig))
                .unwrap();
            pump_until(&mut backend, Duration::from_secs(5), |backend| {
                backend.state().extension_generations[EXTENSION_ID] != token_a
                    && backend.state().services.running["generation-e2e.watch"]
                        .child
                        .id()
                        != pid_a
            });
            let token_b = backend.state().extension_generations[EXTENSION_ID].clone();

            for args in [
                &["notify", "stale"][..],
                &["publish"][..],
                &["pick"][..],
                &["subscribe"][..],
            ] {
                expect_cli_exit(&mut backend, &socket, &token_a, args, false);
            }
            expect_cli_exit(
                &mut backend,
                &socket,
                &token_b,
                &["notify", "current"],
                true,
            );
            expect_cli_exit(&mut backend, &socket, &token_b, &["publish"], true);
            expect_cli_stream_accepted(&mut backend, &socket, &token_b, &["pick"]);
            let mut subscription = open_cli_stream(&mut backend, &socket, &token_b, &["subscribe"]);

            write_manifest(&root, "a");
            backend
                .dispatch(Msg::RunAction(rozi::input::Action::ReloadConfig))
                .unwrap();
            pump_until(&mut backend, Duration::from_secs(5), |backend| {
                let token = &backend.state().extension_generations[EXTENSION_ID];
                token != &token_a && token != &token_b
            });
            pump_until(&mut backend, Duration::from_secs(5), |_| {
                subscription.try_wait().unwrap().is_some()
            });
            assert!(
                subscription.wait().unwrap().success(),
                "retired subscription did not close cleanly"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
