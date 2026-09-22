//! Worktree RPCs run on the attached session's host and keep read-only clients read-only.

use std::path::Path;
use std::process::{Command, Stdio};

use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::origin::{SessionOrigin, WorktreeOrigin};
use rozi::session::protocol::{
    ClientMessage, Frame, ServerMessage, WirePalette, WorktreeRequest, WorktreeResult,
};
use tui_lipan::prelude::TerminalColorPalette;

use crate::common::{
    ServerGuard, TestConnection, attach_message, connect_when_ready, private_temp_dir, read_until,
    subprocess_endpoint, unique_session_name,
};

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn worktree(
    client: &mut TestConnection,
    request_id: u64,
    request: WorktreeRequest,
) -> WorktreeResult {
    client.write_control(&ClientMessage::Worktree {
        request_id,
        request,
    });
    let mut reply = None;
    read_until(client, |frame| {
        if let Frame::Control(ServerMessage::WorktreeResult {
            request_id: returned,
            result,
        }) = frame
            && *returned == request_id
        {
            reply = Some(result.clone());
            true
        } else {
            false
        }
    });
    reply.expect("matching worktree reply")
}

#[test]
fn worktree_rpc_lists_creates_and_removes_on_the_session_host() {
    if !rozi::platform::command::program_exists("git") {
        return;
    }
    let root = private_temp_dir();
    let repo = root.join("source repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let cwd = repo.to_string_lossy().into_owned();
    let checkout = root.join("linked checkout");
    let checkout_path = checkout.to_string_lossy().into_owned();

    let session = unique_session_name();
    let runtime_base = root.join("runtime");
    let config_path = root.join("config.toml");
    std::fs::write(&config_path, "[session]\nresurrect = false\n").unwrap();
    let endpoint = subprocess_endpoint(&runtime_base, &session);
    let child = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(["--server", &session])
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", &runtime_base)
        .env("APPDATA", root.join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", root.join("AppData").join("Local"))
        .env("ROZI_CONFIG", config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch isolated session server");
    let mut guard = ServerGuard::new(child, root.clone());
    let mut writable = connect_when_ready(&endpoint, guard.child_mut());
    writable.write_control(&attach_message(&session, "writer"));
    read_until(&mut writable, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    let mut read_only = TestConnection::connect(&endpoint);
    let mut attach = attach_message(&session, "reader");
    if let ClientMessage::Attach { read_only, .. } = &mut attach {
        *read_only = true;
    }
    read_only.write_control(&attach);
    read_until(&mut read_only, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    let mut follower = TestConnection::connect(&endpoint);
    follower.write_control(&attach_message(&session, "writable follower"));
    let mut follower_has_lease = None;
    read_until(&mut follower, |frame| {
        if let Frame::Control(ServerMessage::Attached {
            client_id,
            controller,
            ..
        }) = frame
        {
            follower_has_lease = Some(controller == &Some(*client_id));
            true
        } else {
            false
        }
    });
    assert_eq!(follower_has_lease, Some(false));

    match worktree(
        &mut read_only,
        10,
        WorktreeRequest::List { cwd: cwd.clone() },
    ) {
        WorktreeResult::Listed { worktrees } => {
            // Git on Windows may spell the same directory differently (`/`, long names).
            let resolve = |path: &str| std::path::Path::new(path).canonicalize().unwrap();
            assert_eq!(resolve(&worktrees[0].path), resolve(&cwd));
            assert!(!worktrees[0].linked);
        }
        other => panic!("unexpected list reply: {other:?}"),
    }
    let WorktreeResult::Previewed { path: preview } = worktree(
        &mut read_only,
        17,
        WorktreeRequest::Preview {
            cwd: cwd.clone(),
            branch: "feat/preview".into(),
        },
    ) else {
        panic!("preview failed");
    };
    // The preview does not exist yet, so compare its existing grandparent and name its tail.
    let preview = std::path::Path::new(&preview);
    assert!(preview.ends_with(std::path::Path::new("source repo-worktrees").join("feat-preview")));
    assert_eq!(
        preview
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap()
            .canonicalize()
            .unwrap(),
        root.canonicalize().unwrap()
    );
    assert!(matches!(
        worktree(&mut read_only, 11, WorktreeRequest::Create {
            cwd: cwd.clone(),
            branch: "feat/rejected".into(),
            base: "HEAD".into(),
            path: Some(checkout_path.clone()),
        }),
        WorktreeResult::Failed { message } if message.contains("writable")
    ));
    assert!(!checkout.exists());

    let created = worktree(
        &mut writable,
        12,
        WorktreeRequest::Create {
            cwd: cwd.clone(),
            branch: "feat/accepted".into(),
            base: "HEAD".into(),
            path: Some(checkout_path.clone()),
        },
    );
    assert!(
        matches!(&created, WorktreeResult::Created { worktree } if worktree.linked
            && std::path::Path::new(&worktree.path).canonicalize().unwrap()
                == checkout.canonicalize().unwrap()),
        "{created:?}"
    );
    assert!(checkout.exists());

    let removable = root.join("removable checkout");
    let removable_path = removable.to_string_lossy().into_owned();
    assert!(matches!(
        worktree(
            &mut follower,
            14,
            WorktreeRequest::Create {
                cwd: cwd.clone(),
                branch: "feat/removable".into(),
                base: "HEAD".into(),
                path: Some(removable_path.clone()),
            }
        ),
        WorktreeResult::Created { .. }
    ));
    assert!(matches!(
        worktree(&mut read_only, 15, WorktreeRequest::Remove {
            cwd: cwd.clone(),
            path: removable_path.clone(),
            force: false,
        }),
        WorktreeResult::Failed { message } if message.contains("writable")
    ));
    let mut host_env = rozi::platform::paths::PlatformEnv::from_process();
    host_env.home = Some(root.clone());
    host_env.xdg_state_home = Some(root.join("state"));
    host_env.local_appdata = Some(root.join("AppData").join("Local"));
    let snapshot = rozi::platform::paths::state_dir(&host_env)
        .join("sessions")
        .join("saved-worktree");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::write(
        snapshot.join("meta.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 3,
            "session": "saved-worktree",
            "saved_at": 1,
            "layout_rev": 0,
            "origin": {"worktree": {"path": removable_path}},
            "panes": []
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        worktree(
            &mut writable,
            18,
            WorktreeRequest::Remove {
                cwd: cwd.clone(),
                path: removable_path.clone(),
                force: true,
            }
        ),
        WorktreeResult::Failed { message } if message.contains("saved-worktree")
    ));
    assert!(removable.exists());
    std::fs::remove_dir_all(snapshot).unwrap();
    assert!(matches!(
        worktree(
            &mut follower,
            16,
            WorktreeRequest::Remove {
                cwd: cwd.clone(),
                path: removable_path,
                force: false,
            }
        ),
        WorktreeResult::Removed { .. }
    ));
    assert!(!removable.exists());
    git(
        &repo,
        &["show-ref", "--verify", "refs/heads/feat/removable"],
    );

    // A session may claim the checkout only after its first pane exists. Once claimed, Git removal
    // remains blocked even when `force` is requested.
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    writable.write_control(&ClientMessage::SpawnPane {
        pane_id: 41,
        local: false,
        generation: 1,
        launch: None,
        cwd: Some(checkout_path.clone()),
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: None,
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    read_until(&mut writable, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::SpawnResult {
                pane_id: 41,
                ok: true,
                ..
            })
        )
    });
    writable.write_control(&ClientMessage::SetSessionOrigin {
        origin: SessionOrigin {
            profile: None,
            worktree: Some(WorktreeOrigin {
                path: checkout_path.clone(),
            }),
        },
    });
    read_until(&mut writable, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::SessionOriginSet { .. })
        )
    });
    assert!(matches!(
        worktree(&mut writable, 13, WorktreeRequest::Remove {
            cwd: cwd.clone(),
            path: checkout_path.clone(),
            force: true,
        }),
        WorktreeResult::Failed { message } if message.contains(&session)
    ));
    assert!(checkout.exists());

    writable.write_control(&ClientMessage::Shutdown);
    guard.wait_for_exit();
    drop(read_only);
    drop(follower);
    drop(writable);
    // The checkout is intentionally still registered: the test proved removal protection. Remove
    // it directly only after the server has stopped, then discard the isolated repository.
    git(
        &repo,
        &["worktree", "remove", "--force", "--", &checkout_path],
    );
    drop(guard);
}
