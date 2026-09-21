//! Control-socket subcommands: `publish`, `subscribe`, `pick`, and the one-shot `rozi <control
//! command>` forms.
//!
//! Each opens the endpoint discovered by [`discover_socket`] and speaks the line-delimited wire
//! protocol, so a caller needs no IPC code of its own.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use tui_lipan::Result;

use super::args::{
    ControlCli, ControlEndpoint, ListFormat, PickCli, PublishCli, SubscribeCli, control_request,
};
use super::output::{OutputStyles, format_control_text};
use crate::control;
use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};

fn discover_socket(explicit: Option<PathBuf>) -> std::result::Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("ROZI_SOCKET").map(PathBuf::from) {
        return Ok(path);
    }
    let dir =
        control::runtime_dir().map_err(|err| format!("could not inspect runtime dir: {err}"))?;
    let live: Vec<PathBuf> = EndpointRegistry::list_live_control_endpoints(&dir)
        .map_err(|err| format!("could not read {}: {err}", dir.display()))?
        .into_iter()
        .map(|endpoint| endpoint.path().to_path_buf())
        .collect();
    match live.as_slice() {
        [path] => Ok(path.clone()),
        [] => {
            Err("no live rozi control socket found (set ROZI_SOCKET or pass --socket)".to_string())
        }
        _ => Err("multiple live rozi sockets found; pass --socket PATH".to_string()),
    }
}

/// Bridge stdin/stdout to a `publish` control stream for the calling pane.
///
/// Runs until either side closes: rozi withdraws the pane's rows on EOF, so a publisher that
/// exits or crashes cleans up by construction and never has to say so.
pub(crate) fn run_publish_cli(command: PublishCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let source_pane = std::env::var("ROZI_PANE")
        .ok()
        .and_then(|value| value.parse::<crate::state::PaneId>().ok());
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    let mut request = control_request(control::ControlCommand::Publish);
    request.source_pane = source_pane;
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap())?;

    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    let reply = read_socket_line(&mut reader)?;
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }

    // Activations arrive whenever the user clicks; forward them as they come rather than pairing
    // them with anything this process writes.
    std::thread::spawn(move || {
        while let Ok(Some(line)) = crate::control::read_control_reply_line(&mut reader) {
            let mut stdout = std::io::stdout().lock();
            // A publisher that stopped reading its activations has gone away; end the thread
            // rather than spinning on a broken pipe.
            if writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return;
            }
        }
    });

    let mut stdin = BufReader::new(std::io::stdin().lock());
    while let Some(line) = crate::control::read_control_line(&mut stdin)? {
        writeln!(stream, "{line}")?;
    }
    Ok(())
}

/// Print matching application events as newline-delimited JSON until the connection closes.
pub(crate) fn run_subscribe_cli(command: SubscribeCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    let request = control_request(control::ControlCommand::Subscribe {
        events: command.events,
    });
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap())?;

    let mut reader = BufReader::new(stream);
    let response = read_socket_line(&mut reader)?;
    let value: serde_json::Value = serde_json::from_str(&response).unwrap_or_default();
    if value.get("ok").and_then(|value| value.as_bool()) != Some(true) {
        if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }

    let mut stdout = std::io::stdout().lock();
    while let Some(line) = crate::control::read_control_reply_line(&mut reader)? {
        writeln!(stdout, "{line}")?;
        stdout.flush()?;
    }
    Ok(())
}

pub(crate) fn run_pick_cli(command: PickCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    // In `--json` mode the first stdin line *is* the picker request, which is the only way to
    // declare `width` and `actions` - they have no flag spelling, and a mini-language inside one
    // would be worse than the object the caller is already writing. Its `rows`, if present, become
    // the initial set. Plain mode is a dumb list and needs none of it.
    let mut opening_rows = None;
    let (title, placeholder, empty, width, actions) = if command.json {
        let first_line = read_socket_line(&mut BufReader::new(std::io::stdin().lock()))?;
        let spec: serde_json::Value =
            serde_json::from_str(first_line.trim()).unwrap_or(serde_json::Value::Null);
        if spec.get("rows").is_some() {
            opening_rows = Some(serde_json::json!({ "rows": spec["rows"].clone() }));
        }
        (
            spec.get("title")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(command.title),
            spec.get("placeholder")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(command.placeholder),
            spec.get("empty")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|text| !text.is_empty()),
            spec.get("width").and_then(|v| v.as_u64()).map(|v| v as u16),
            spec.get("actions")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
        )
    } else {
        (command.title, command.placeholder, None, None, Vec::new())
    };

    let request = control_request(control::ControlCommand::Pick {
        title,
        placeholder,
        empty,
        width,
        actions,
    });
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap())?;

    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    let reply = read_socket_line(&mut reader)?;
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }

    let json = command.json;
    let reader_thread = std::thread::spawn(move || {
        while let Ok(Some(line)) = crate::control::read_control_reply_line(&mut reader) {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match classify_pick_stream_event(&value) {
                PickStreamEvent::Action => {
                    if json {
                        println!("{line}");
                        let _ = std::io::stdout().flush();
                    }
                }
                PickStreamEvent::Selected(selected) => {
                    // Plain mode prints the id alone, so `rozi pick | xargs $EDITOR` needs no `jq`.
                    println!("{}", if json { &line } else { selected });
                    let _ = std::io::stdout().flush();
                    std::process::exit(0);
                }
                PickStreamEvent::Cancelled => {
                    if json {
                        println!("{line}");
                        let _ = std::io::stdout().flush();
                    }
                    std::process::exit(1);
                }
                PickStreamEvent::Ignore => {}
            }
        }
        std::process::exit(2);
    });

    if command.json {
        if let Some(rows) = opening_rows {
            let _ = writeln!(stream, "{rows}");
        }
        let mut stdin = BufReader::new(std::io::stdin().lock());
        while let Ok(Some(line)) = crate::control::read_control_line(&mut stdin) {
            if writeln!(stream, "{line}").is_err() {
                break;
            }
        }
    } else {
        // Plain mode batches at EOF rather than streaming: it exists for `ls | rozi pick`, where
        // stdin closes immediately, and one send beats a redraw per line on a long pipeline. A
        // caller that wants to grow the list while the palette is open uses `--json` and controls
        // its own batching.
        let mut stdin = BufReader::new(std::io::stdin().lock());
        let mut rows = Vec::new();
        while let Ok(Some(line)) = crate::control::read_control_line(&mut stdin) {
            if line.trim().is_empty() {
                continue;
            }
            rows.push(serde_json::json!({ "id": line, "label": line }));
            if rows.len() >= crate::control::MAX_PICK_ROWS {
                break;
            }
        }
        let _ = writeln!(stream, "{}", serde_json::json!({ "rows": rows }));
    }

    let _ = reader_thread.join();
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickStreamEvent<'a> {
    Action,
    Selected(&'a str),
    Cancelled,
    Ignore,
}

fn classify_pick_stream_event(value: &serde_json::Value) -> PickStreamEvent<'_> {
    if value
        .get("action")
        .and_then(serde_json::Value::as_str)
        .is_some()
    {
        PickStreamEvent::Action
    } else if let Some(selected) = value.get("selected").and_then(serde_json::Value::as_str) {
        PickStreamEvent::Selected(selected)
    } else if value.get("cancelled").is_some() {
        PickStreamEvent::Cancelled
    } else {
        PickStreamEvent::Ignore
    }
}

/// Ask the UI endpoint, returning its raw reply line.
fn ask_ui_endpoint(
    socket: Option<PathBuf>,
    request: &control::ControlRequest,
) -> Result<serde_json::Value> {
    let path = match discover_socket(socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    writeln!(stream, "{}", serde_json::to_string(request).unwrap())?;
    let line = read_socket_line(&mut BufReader::new(stream))?;
    if line.trim().is_empty() {
        eprintln!("empty response from rozi");
        std::process::exit(2);
    }
    match serde_json::from_str(&line) {
        Ok(value) => Ok(value),
        Err(err) => {
            eprintln!("invalid JSON response: {err}");
            std::process::exit(2);
        }
    }
}

/// Ask a named session server directly, with no UI in the picture.
///
/// A failure to *reach* the session exits 2 like an unreachable UI endpoint; a command the server
/// answered with `ok: false` flows on and exits 1 through the shared path below, so a script sees
/// the same two outcomes whichever endpoint served it.
fn ask_session_endpoint(
    session: &str,
    mut request: control::ControlRequest,
) -> Result<serde_json::Value> {
    // `ROZI_PANE` is a bare pane id with no session attached to it, and `--session` names a
    // different namespace than the one the caller is sitting in. Sending it would let a script in
    // pane 3 of one session address pane 3 of another, so the field is dropped rather than left
    // for the server to ignore. A pane addressing its own session passes `--target "$ROZI_PANE"`.
    request.source_pane = None;
    match crate::session::headless::run_session_control(session, request) {
        Ok(response) => Ok(serde_json::to_value(response).unwrap_or_default()),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

/// Run one control command against a session on another host, over the SSH transport `--remote`
/// attach already uses.
///
/// The response is rendered here rather than on the far side, so `rozi --remote box --session dev
/// list-panes` prints what `rozi --session dev list-panes` prints, and exits the same way.
fn ask_remote_endpoint(
    target: &str,
    session: &str,
    mut request: control::ControlRequest,
) -> Result<serde_json::Value> {
    // Same reason as the local session endpoint: `ROZI_PANE` names a pane in the namespace the
    // caller is sitting in, which is not the one being addressed - and here it is not even the
    // same machine.
    request.source_pane = None;
    let parsed = match crate::session::remote::parse_remote_target(target) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let config = crate::config::load_config().config.remote;
    match crate::session::remote::control::forward_control(&parsed, session, &request, &config) {
        Ok(response) => Ok(serde_json::to_value(response).unwrap_or_default()),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

pub(crate) fn run_control_cli(command: ControlCli) -> Result<()> {
    use std::io::IsTerminal;

    let value = match command.endpoint {
        ControlEndpoint::Ui(socket) => ask_ui_endpoint(socket, &command.request)?,
        ControlEndpoint::Session(session) => {
            ask_session_endpoint(&session, command.request.clone())?
        }
        ControlEndpoint::Remote { target, session } => {
            ask_remote_endpoint(&target, &session, command.request.clone())?
        }
    };
    let line = serde_json::to_string(&value).unwrap_or_default();
    let human_output = match command.output_format {
        Some(ListFormat::Text) => true,
        Some(ListFormat::Json) => false,
        None => std::io::stdout().is_terminal(),
    };
    if !human_output {
        println!("{line}");
    }
    if value.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }
    if human_output {
        print!(
            "{}",
            format_control_text(&command.request.command, &value, OutputStyles::detect())
        );
    }
    Ok(())
}

fn read_socket_line(reader: &mut impl BufRead) -> std::io::Result<String> {
    match crate::control::read_control_reply_line(reader)? {
        Some(line) => Ok(line),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "control connection closed",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_actions_are_non_terminal_even_when_they_carry_a_selection() {
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({
                "action": "delete",
                "selected": "feature"
            })),
            PickStreamEvent::Action
        );
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({
                "action": "refresh",
                "selected": null
            })),
            PickStreamEvent::Action
        );
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({ "selected": "feature" })),
            PickStreamEvent::Selected("feature")
        );
    }
}
