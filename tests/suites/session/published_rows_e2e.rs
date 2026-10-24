//! End-to-end coverage for published activity rows.
//!
//! Talks to the real session server through the production typed protocol, so this exercises the
//! server's own run-clock bookkeeping rather than a reimplementation of it.

use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    ClientMessage, Frame, PaneRuntimeState, PublishedRow, PublisherKind, ServerMessage, WirePalette,
};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;

use crate::common::{attach_client, read_until, spawn_listener};

const PANE_ID: u32 = 52;
const PANE_GENERATION: u64 = 1;

fn row(id: &str, status: &str, active: bool) -> PublishedRow {
    PublishedRow {
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        live_session: None,
        reported: false,
        id: id.to_string(),
        title: format!("tab {id}"),
        status: status.to_string(),
        reason: None,
        active,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: None,
    }
}

fn spawn_pane(controller: &mut crate::common::TestConnection) {
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    controller.write_control(&ClientMessage::SpawnPane {
        placement: Some(crate::common::spawn_placement(
            0,
            &[(PANE_ID, PANE_GENERATION)],
        )),
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: None,
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: Some("published rows e2e".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    read_until(controller, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::SpawnResult {
                pane_id: PANE_ID,
                generation: PANE_GENERATION,
                ok: true,
                ..
            })
        )
    });
}

/// Wait for the next runtime broadcast whose rows satisfy `check`, and return that state.
fn read_rows(
    client: &mut crate::common::TestConnection,
    check: impl Fn(&[PublishedRow]) -> bool,
) -> PaneRuntimeState {
    let mut captured = None;
    read_until(client, |frame| match frame {
        Frame::Control(ServerMessage::PaneRuntimeChanged {
            pane_id: PANE_ID,
            state,
            ..
        }) if check(&state.rows) => {
            captured = Some(state.clone());
            true
        }
        _ => false,
    });
    captured.expect("read_until matched without capturing")
}

#[test]
fn published_rows_broadcast_and_keep_a_run_clock_per_row() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    let (mut follower, _) = attach_client(server.endpoint(), server.session(), "follower");
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        rows: vec![row("a", "working", true), row("b", "idle", false)],
    });

    // Both clients see the rows; the server stamps a run start for the working one only.
    let mut started = None;
    for client in [&mut controller, &mut follower] {
        let state = read_rows(client, |rows| rows.len() == 2);
        assert_eq!(state.rows[0].id, "a");
        assert_eq!(state.rows[1].id, "b");
        let a = state.rows[0].work_started_at.expect("a is working");
        assert_eq!(
            state.rows[1].work_started_at, None,
            "an idle row has no run"
        );
        started = Some(a);
    }
    let started = started.expect("at least one client read the rows");

    // Reordering and retitling must not restart a run: identity is the publisher's id.
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        rows: vec![
            row("b", "working", false),
            PublishedRow {
                title: "renamed".to_string(),
                ..row("a", "working", true)
            },
        ],
    });
    let state = read_rows(&mut controller, |rows| {
        rows.first().is_some_and(|row| row.id == "b")
    });
    let a = state
        .rows
        .iter()
        .find(|row| row.id == "a")
        .expect("row a survived the reorder");
    assert_eq!(
        a.work_started_at,
        Some(started),
        "reordering and retitling must not restart a run"
    );
    assert_eq!(a.title, "renamed");
    assert!(
        state
            .rows
            .iter()
            .any(|row| row.id == "b" && row.work_started_at.is_some_and(|at| at >= started)),
        "b started its own run when it began working"
    );

    // A blocked row keeps its run: blocking and resuming are one run, as for a whole pane.
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        rows: vec![row("a", "blocked", true)],
    });
    let state = read_rows(&mut controller, |rows| rows.len() == 1);
    assert_eq!(state.rows[0].work_started_at, Some(started));
    assert!(
        !state.rows.iter().any(|row| row.id == "b"),
        "a row the publisher dropped is gone"
    );

    // Withdrawing hands the pane back to screen detection.
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        rows: Vec::new(),
    });
    let state = read_rows(&mut controller, |rows| rows.is_empty());
    assert!(state.rows.is_empty());

    follower.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}

/// Rows ride the attach snapshot, so a client that joins mid-run sees them without a broadcast.
#[test]
fn published_rows_survive_detach_and_reattach() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        rows: vec![row("only", "working", true)],
    });
    read_rows(&mut controller, |rows| rows.len() == 1);

    let (mut reattached, attached) =
        attach_client(server.endpoint(), server.session(), "reattached");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!("expected attached response");
    };
    let pane = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("reattach response omitted the live pane");
    assert_eq!(pane.runtime.rows.len(), 1);
    assert_eq!(pane.runtime.rows[0].id, "only");

    reattached.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}

/// A row that names its own directory comes back with that directory's project, resolved by the
/// server: a linked worktree's own root and branch, and the repository it belongs to. Whatever
/// project the publisher claimed is discarded.
#[test]
fn a_row_cwd_resolves_to_its_worktree_on_the_server() {
    let scratch = std::env::temp_dir().join(format!("rozi-e2e-row-cwd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repo = scratch.join("repo");
    let gitdir = repo.join(".git").join("worktrees").join("fix");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(repo.join(".git").join("HEAD"), b"ref: refs/heads/master\n").unwrap();
    std::fs::write(gitdir.join("HEAD"), b"ref: refs/heads/fix-login\n").unwrap();
    std::fs::write(gitdir.join("commondir"), b"../..\n").unwrap();
    let worktree = repo.join(".claude").join("worktrees").join("fix");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()).as_bytes(),
    )
    .unwrap();

    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: "sessions".into(),
        kind: PublisherKind::Agent,
        rows: vec![PublishedRow {
            cwd: Some(worktree.to_string_lossy().into_owned()),
            project: Some(rozi::session::protocol::RowProject {
                root: "/forged".to_string(),
                repository: None,
                branch: Some("forged".to_string()),
            }),
            ..row("fix", "working", false)
        }],
    });
    let state = read_rows(&mut controller, |rows| rows.len() == 1);
    let project = state.rows[0]
        .project
        .as_ref()
        .expect("server resolved a project");
    assert_eq!(project.root, worktree.to_string_lossy());
    assert_eq!(project.branch.as_deref(), Some("fix-login"));
    assert_eq!(
        project.repository.as_deref(),
        Some(repo.to_string_lossy().as_ref())
    );

    controller.write_control(&ClientMessage::Detach);
    let _ = std::fs::remove_dir_all(&scratch);
}

fn client_id(attached: &ServerMessage) -> u64 {
    match attached {
        ServerMessage::Attached { client_id, .. } => *client_id,
        other => panic!("expected attached response, got {other:?}"),
    }
}

fn report(
    client: &mut crate::common::TestConnection,
    publisher: &str,
    kind: PublisherKind,
    rows: Vec<PublishedRow>,
) {
    client.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        publisher: publisher.into(),
        kind,
        rows,
    });
}

fn owners(state: &PaneRuntimeState) -> Vec<(String, String, PublisherKind, bool)> {
    state
        .rows
        .iter()
        .map(|row| (row.publisher.clone(), row.id.clone(), row.kind, row.active))
        .collect()
}

/// Two publishers in one pane own their rows independently: colliding row ids stay two rows with
/// two run clocks, and one publisher withdrawing leaves the other's rows exactly as they were.
#[test]
fn publishers_in_one_pane_keep_their_own_rows() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, attached) =
        attach_client(server.endpoint(), server.session(), "controller");
    let me = client_id(&attached);
    spawn_pane(&mut controller);

    report(
        &mut controller,
        "codex-rozi-sessions",
        PublisherKind::Agent,
        vec![row("1", "working", true)],
    );
    read_rows(&mut controller, |rows| rows.len() == 1);
    // A forged publisher or kind in the row itself is replaced by the stream's.
    report(
        &mut controller,
        "publish:prs",
        PublisherKind::Activity,
        vec![PublishedRow {
            publisher: "codex-rozi-sessions".into(),
            kind: PublisherKind::Agent,
            ..row("1", "idle", true)
        }],
    );
    let state = read_rows(&mut controller, |rows| rows.len() == 2);
    assert_eq!(
        owners(&state),
        [
            (
                format!("{me}/codex-rozi-sessions"),
                "1".into(),
                PublisherKind::Agent,
                true
            ),
            (
                format!("{me}/publish:prs"),
                "1".into(),
                PublisherKind::Activity,
                true
            ),
        ],
        "colliding ids stay distinct and each publisher keeps its own active row"
    );
    assert!(state.rows[0].work_started_at.is_some());
    assert_eq!(state.rows[1].work_started_at, None);

    // Withdrawing the activity publisher leaves the agent publisher's row and its clock alone.
    let started = state.rows[0].work_started_at;
    report(
        &mut controller,
        "publish:prs",
        PublisherKind::Activity,
        Vec::new(),
    );
    let state = read_rows(&mut controller, |rows| rows.len() == 1);
    assert_eq!(state.rows[0].publisher, format!("{me}/codex-rozi-sessions"));
    assert_eq!(state.rows[0].work_started_at, started);

    controller.write_control(&ClientMessage::Detach);
}

/// Two clients attached to one session, each running a publisher with the same identity in the
/// same pane: they are two publishers. Neither's reports, late updates, or close touch the other's
/// rows, and a client that leaves takes only its own rows with it.
#[test]
fn publishers_of_two_clients_never_touch_each_others_rows() {
    let server = spawn_listener(ServerSettings::default());
    let (mut a, attached_a) = attach_client(server.endpoint(), server.session(), "a");
    let id_a = client_id(&attached_a);
    spawn_pane(&mut a);
    let (mut b, attached_b) = attach_client(server.endpoint(), server.session(), "b");
    let id_b = client_id(&attached_b);
    assert_ne!(id_a, id_b);

    report(
        &mut a,
        "publish:tasks",
        PublisherKind::Activity,
        vec![row("a", "working", true)],
    );
    read_rows(&mut b, |rows| rows.len() == 1);
    report(
        &mut b,
        "publish:tasks",
        PublisherKind::Activity,
        vec![row("b", "working", true)],
    );
    let state = read_rows(&mut a, |rows| rows.len() == 2);
    let publishers: Vec<_> = state.rows.iter().map(|row| row.publisher.clone()).collect();
    assert_eq!(
        publishers,
        [
            format!("{id_a}/publish:tasks"),
            format!("{id_b}/publish:tasks")
        ],
        "the same identity in two clients is two publishers"
    );

    // A late update from A replaces only A's row.
    report(
        &mut a,
        "publish:tasks",
        PublisherKind::Activity,
        vec![row("a2", "idle", true)],
    );
    let state = read_rows(&mut b, |rows| rows.iter().any(|row| row.id == "a2"));
    assert!(state.rows.iter().any(|row| row.id == "b"));

    // A closing its stream withdraws A's rows, never B's.
    report(&mut a, "publish:tasks", PublisherKind::Activity, Vec::new());
    let state = read_rows(&mut b, |rows| rows.len() == 1);
    assert_eq!(state.rows[0].id, "b");

    // B restarts its publisher (same identity, new snapshot) and A publishes again; then B leaves
    // without closing anything: only B's rows go.
    report(
        &mut b,
        "publish:tasks",
        PublisherKind::Activity,
        vec![row("b2", "working", true)],
    );
    report(
        &mut a,
        "publish:tasks",
        PublisherKind::Activity,
        vec![row("a3", "idle", true)],
    );
    read_rows(&mut a, |rows| {
        rows.iter().any(|row| row.id == "b2") && rows.iter().any(|row| row.id == "a3")
    });
    b.write_control(&ClientMessage::Detach);
    drop(b);
    let state = read_rows(&mut a, |rows| rows.len() == 1);
    assert_eq!(state.rows[0].id, "a3");
    assert_eq!(state.rows[0].publisher, format!("{id_a}/publish:tasks"));

    a.write_control(&ClientMessage::Detach);
}
