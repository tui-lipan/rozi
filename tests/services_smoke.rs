use rozi::AppRoot;
use rozi::config::{ServiceConfig, ServiceLaunch, ServiceRestart};
use std::collections::BTreeMap;
use std::time::Duration;
use tui_lipan::TestBackend;

#[test]
fn services_spawn_on_ready_and_terminate_on_exit() {
    rozi::test_support::isolate_user_dirs();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().config.services = vec![ServiceConfig {
                name: "test-service".to_string(),
                launch: ServiceLaunch::Shell(
                    if cfg!(windows) {
                        "ping 127.0.0.1 -n 10"
                    } else {
                        "sleep 10"
                    }
                    .to_string(),
                ),
                cwd: None,
                restart: ServiceRestart::Never,
                env: BTreeMap::new(),
            }];

            for _ in 0..10 {
                backend.render();
                let _ = backend.pump();
                std::thread::sleep(Duration::from_millis(10));
            }

            assert_eq!(backend.state().services.running.len(), 1);
            assert!(
                backend
                    .state()
                    .services
                    .running
                    .contains_key("test-service")
            );

            backend
                .dispatch(rozi::Msg::ServicesTick {
                    epoch: backend.state().services.epoch,
                })
                .expect("dispatch tick");
            assert_eq!(backend.state().services.running.len(), 1);

            backend
                .dispatch(rozi::Msg::Hangup)
                .expect("dispatch hangup");
            assert_eq!(backend.state().services.running.len(), 0);
        })
        .expect("spawn thread")
        .join()
        .expect("join thread");
}

#[test]
fn service_with_never_restart_goes_dormant_on_exit() {
    rozi::test_support::isolate_user_dirs();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().config.services = vec![ServiceConfig {
                name: "fast-exit".to_string(),
                launch: ServiceLaunch::Shell(
                    if cfg!(windows) {
                        "cmd /c exit 0"
                    } else {
                        "true"
                    }
                    .to_string(),
                ),
                cwd: None,
                restart: ServiceRestart::Never,
                env: BTreeMap::new(),
            }];

            for _ in 0..10 {
                backend.render();
                let _ = backend.pump();
                std::thread::sleep(Duration::from_millis(10));
            }

            std::thread::sleep(Duration::from_millis(50));

            backend
                .dispatch(rozi::Msg::ServicesTick {
                    epoch: backend.state().services.epoch,
                })
                .expect("dispatch tick");

            assert_eq!(backend.state().services.running.len(), 0);
            assert_eq!(backend.state().services.dormant.len(), 1);
            assert!(backend.state().services.dormant.contains_key("fast-exit"));
        })
        .expect("spawn thread")
        .join()
        .expect("join thread");
}

#[test]
fn service_with_on_failure_restarts_on_error() {
    rozi::test_support::isolate_user_dirs();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().config.services = vec![ServiceConfig {
                name: "failing-service".to_string(),
                launch: ServiceLaunch::Shell(
                    if cfg!(windows) {
                        "cmd /c exit 1"
                    } else {
                        "false"
                    }
                    .to_string(),
                ),
                cwd: None,
                restart: ServiceRestart::OnFailure,
                env: BTreeMap::new(),
            }];

            for _ in 0..10 {
                backend.render();
                let _ = backend.pump();
                std::thread::sleep(Duration::from_millis(10));
            }

            std::thread::sleep(Duration::from_millis(50));

            backend
                .dispatch(rozi::Msg::ServicesTick {
                    epoch: backend.state().services.epoch,
                })
                .expect("dispatch tick");

            assert_eq!(backend.state().services.running.len(), 0);
            assert_eq!(backend.state().services.pending.len(), 1);
            assert!(
                backend
                    .state()
                    .services
                    .pending
                    .contains_key("failing-service")
            );
            assert_eq!(
                backend.state().services.pending["failing-service"].consecutive_failures,
                1
            );
        })
        .expect("spawn thread")
        .join()
        .expect("join thread");
}
