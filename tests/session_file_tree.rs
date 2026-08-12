//! The session server's filesystem browsing messages, which back the sidebar file tree when the
//! client is attached over `--remote` and the files live on the server's host.
//!
//! Driven through the real typed protocol and the platform IPC helpers, like every other session
//! test — never raw sockets or hand-rolled framing.

mod common;

use rozi::session::protocol::{
    ClientMessage, FILE_TREE_PROTOCOL, Frame, ServerMessage, WireChangeState,
};
use rozi::session::server::ServerSettings;

use common::{attach_client, private_temp_dir, spawn_listener};

/// Read the reply to one `ListDirectory`, ignoring unrelated traffic.
fn list_directory(
    client: &mut common::TestConnection,
    path: &str,
) -> (Vec<rozi::session::protocol::WireDirEntry>, Option<String>) {
    client.write_control(&ClientMessage::ListDirectory {
        path: path.to_string(),
        show_hidden: true,
    });
    let mut result = None;
    common::read_until(client, |frame| {
        if let Frame::Control(ServerMessage::DirectoryListing {
            path: replied,
            entries,
            error,
        }) = frame
            && replied == path
        {
            result = Some((entries.clone(), error.clone()));
            return true;
        }
        false
    });
    result.expect("directory listing reply")
}

#[test]
fn server_lists_its_own_filesystem_for_the_file_tree() {
    let root = private_temp_dir();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join("README.md"), b"# hi").unwrap();
    std::fs::write(root.join(".hidden"), b"x").unwrap();
    // Symlink creation needs privileges on Windows, so that assertion stays Unix-only.
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("README.md"), root.join("link.md")).unwrap();

    let guard = spawn_listener(ServerSettings::default());
    let (mut client, attached) = attach_client(guard.endpoint(), guard.session(), "tree");
    let ServerMessage::Attached {
        effective_protocol, ..
    } = attached
    else {
        unreachable!()
    };
    assert!(
        effective_protocol >= FILE_TREE_PROTOCOL,
        "same-build attach must negotiate a file-tree capable version"
    );

    let root_path = root.to_string_lossy().into_owned();
    let (entries, error) = list_directory(&mut client, &root_path);
    assert!(error.is_none(), "listing failed: {error:?}");

    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names.first(),
        Some(&"src"),
        "directories sort before files: {names:?}"
    );
    assert!(names.contains(&"README.md"));
    assert!(
        names.contains(&".hidden"),
        "show_hidden must include dotfiles: {names:?}"
    );

    let src = entries.iter().find(|entry| entry.name == "src").unwrap();
    assert!(src.is_dir);
    #[cfg(unix)]
    {
        let link = entries
            .iter()
            .find(|entry| entry.name == "link.md")
            .unwrap();
        assert!(link.is_symlink, "symlinks must be reported as such");
        assert!(!link.is_dir);
    }

    // Descending works the same way, which is what the widget does when a directory expands.
    let (nested, error) = list_directory(&mut client, &root.join("src").to_string_lossy());
    assert!(error.is_none());
    assert_eq!(
        nested.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["lib.rs"]
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unreadable_directory_reports_an_error_instead_of_dropping_the_connection() {
    let guard = spawn_listener(ServerSettings::default());
    let (mut client, _) = attach_client(guard.endpoint(), guard.session(), "tree");

    let (entries, error) = list_directory(&mut client, "/definitely/not/here/rozi");
    assert!(entries.is_empty());
    assert!(error.is_some(), "missing directory must report an error");

    // The connection must still be usable — a bad path is normal browsing, not a protocol fault.
    let (_, error) = list_directory(&mut client, &std::env::temp_dir().to_string_lossy());
    assert!(
        error.is_none(),
        "connection unusable after an error: {error:?}"
    );
}

#[test]
fn change_scan_reports_repository_state_from_the_server_host() {
    let root = private_temp_dir();
    if !rozi::platform::command::program_exists("git") {
        eprintln!("skipping: git not available");
        return;
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("run git")
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(root.join("tracked.txt"), b"one\n").unwrap();
    git(&["add", "tracked.txt"]);
    git(&["commit", "-qm", "init"]);
    std::fs::write(root.join("tracked.txt"), b"two\n").unwrap();
    std::fs::write(root.join("fresh.txt"), b"new\n").unwrap();

    let guard = spawn_listener(ServerSettings::default());
    let (mut client, _) = attach_client(guard.endpoint(), guard.session(), "tree");

    let root_path = root.to_string_lossy().into_owned();
    client.write_control(&ClientMessage::ListChanges {
        root: root_path.clone(),
    });
    let mut changes = None;
    common::read_until(&mut client, |frame| {
        if let Frame::Control(ServerMessage::ChangeListing {
            root: replied,
            changes: listed,
            ..
        }) = frame
            && replied == &root_path
        {
            changes = Some(listed.clone());
            return true;
        }
        false
    });
    let changes = changes.expect("change listing reply");

    let modified = changes
        .iter()
        .find(|change| change.path == "tracked.txt")
        .expect("modified file must appear");
    assert_eq!(modified.state, WireChangeState::Modified);
    let untracked = changes
        .iter()
        .find(|change| change.path == "fresh.txt")
        .expect("untracked file must appear");
    assert_eq!(untracked.state, WireChangeState::Untracked);

    let _ = std::fs::remove_dir_all(&root);
}
