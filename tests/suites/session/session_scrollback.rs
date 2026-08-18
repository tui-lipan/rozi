//! Server and client scrollback limits applied independently to one pane's history.
//!
//! Unix-only, at file scope, for the same reason as `session_resurrect_e2e`: the pane is driven by
//! a POSIX shell program - a `while` loop feeding `printf` - and on Windows the launch shell is cmd
//! or PowerShell, which runs none of it, so the marker never arrives and the read waits out its
//! deadline. Filling scrollback portably would mean giving up the one-line generator this needs.
//!
//! Windows scrollback behaviour wants its own test with a shell it can actually run.
#![cfg(unix)]

use rozi::pane::TerminalPane;
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{ClientMessage, Frame, ServerMessage, WirePalette};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;

use crate::common::{attach_client, read_until, spawn_listener};

const PANE_ID: u32 = 71;
const GENERATION: u64 = 1;
const COLS: u16 = 40;
const ROWS: u16 = 4;
const FINAL_MARKER: &str = "scrollback-final-marker";

#[test]
fn server_and_clients_enforce_independent_scrollback_limits() {
    let low_server = spawn_listener(ServerSettings {
        scrollback: 3,
        ..ServerSettings::default()
    });
    populate_history(low_server.endpoint(), low_server.session());
    let server_trimmed = attach_replay(low_server.endpoint(), low_server.session(), "large", 100);
    assert!(server_trimmed.total_scrollback_rows() <= 3);
    assert!(
        !server_trimmed
            .capture_scrollback_text(None)
            .contains("line-000")
    );
    drop(low_server);

    let high_server = spawn_listener(ServerSettings {
        scrollback: 100,
        ..ServerSettings::default()
    });
    populate_history(high_server.endpoint(), high_server.session());
    let locally_trimmed = attach_replay(high_server.endpoint(), high_server.session(), "small", 3);
    let fully_retained = attach_replay(high_server.endpoint(), high_server.session(), "large", 100);
    assert!(locally_trimmed.total_scrollback_rows() <= 3);
    assert!(fully_retained.total_scrollback_rows() > 3);
    assert!(
        !locally_trimmed
            .capture_scrollback_text(None)
            .contains("line-000")
    );
    assert!(
        fully_retained
            .capture_scrollback_text(None)
            .contains("line-000")
    );
}

fn populate_history(endpoint: &rozi::platform::ipc::IpcEndpoint, session: &str) {
    let (mut client, _) = attach_client(endpoint, session, "producer");
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    client.write_control(&ClientMessage::SpawnPane {
        local: false,
        pane_id: PANE_ID,
        generation: GENERATION,
        launch: Some(rozi::pane_launch::PaneLaunch::shell(format!(
            "i=0; while [ $i -lt 30 ]; do printf 'line-%03d\\n' $i; i=$((i+1)); done; printf '{FINAL_MARKER}\\n'"
        ))),
        cwd: None,
        cols: COLS,
        rows: ROWS,
        keep_open: true,
        env: Vec::new(),
        title: None,
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    let mut output = Vec::new();
    read_until(&mut client, |frame| {
        if let Frame::PaneBytes { bytes, .. } = frame {
            output.extend_from_slice(bytes);
        }
        String::from_utf8_lossy(&output).contains(FINAL_MARKER)
    });
    client.write_control(&ClientMessage::Detach);
}

fn attach_replay(
    endpoint: &rozi::platform::ipc::IpcEndpoint,
    session: &str,
    label: &str,
    client_limit: usize,
) -> TerminalPane {
    let (mut client, attached) = attach_client(endpoint, session, label);
    let ServerMessage::Attached { panes, .. } = attached else {
        unreachable!()
    };
    let meta = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("pane metadata in attach replay");
    let mut terminal = TerminalPane::new(client_limit);
    terminal.apply_server_resize(meta.cols, meta.rows);
    terminal.bind_server_backend(PANE_ID, meta.generation);
    read_until(&mut client, |frame| {
        if let Frame::PaneBytes {
            pane_id: PANE_ID,
            local: false,
            generation: GENERATION,
            bytes,
        } = frame
        {
            terminal.process_server_output(bytes);
        }
        terminal
            .capture_scrollback_text(None)
            .contains(FINAL_MARKER)
    });
    client.write_control(&ClientMessage::Detach);
    terminal
}
