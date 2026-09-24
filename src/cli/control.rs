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
    // declare `width`, `actions`, and `tabs` - they have no flag spelling, and a mini-language
    // inside one would be worse than the object the caller is already writing. Its `rows`, if
    // present, become the initial set of the tab the picker opens on. Plain mode is a dumb list
    // and needs none of it.
    //
    // One reader serves the whole session: a buffer dropped after the opening line would take any
    // snapshot the producer wrote straight after it along with it.
    let mut stdin = BufReader::new(std::io::stdin().lock());
    let mut opening_rows = None;
    let (title, placeholder, empty, width, actions, tabs, tab) = if command.json {
        let first_line = read_socket_line(&mut stdin)?;
        let spec: serde_json::Value =
            serde_json::from_str(first_line.trim()).unwrap_or(serde_json::Value::Null);
        let tabs: Vec<crate::state::PickTab> = spec
            .get("tabs")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let tab = spec.get("tab").and_then(|v| v.as_str()).map(str::to_string);
        if spec.get("rows").is_some() {
            let mut rows = serde_json::json!({ "rows": spec["rows"].clone() });
            if let Some(opening) = opening_tab(&tabs, tab.as_deref()) {
                rows["tab"] = serde_json::Value::String(opening.to_string());
            }
            opening_rows = Some(rows);
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
            tabs,
            tab,
        )
    } else {
        (
            command.title,
            command.placeholder,
            None,
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
    };

    let request = control_request(control::ControlCommand::Pick {
        title,
        placeholder,
        empty,
        width,
        actions,
        tabs,
        tab,
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
                PickStreamEvent::Action | PickStreamEvent::Tab => {
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

/// The tab the picker opens on, mirroring the UI: the requested one when it names a usable tab,
/// otherwise the first usable one.
fn opening_tab<'a>(tabs: &'a [crate::state::PickTab], requested: Option<&str>) -> Option<&'a str> {
    let usable = crate::state::usable_pick_tabs(tabs);
    requested
        .and_then(|requested| usable.iter().find(|tab| tab.id == requested))
        .or(usable.first())
        .map(|tab| tab.id.as_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickStreamEvent<'a> {
    Action,
    /// The user switched tabs; the producer may fill the new one lazily.
    Tab,
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
    } else if value.get("selected").is_none() && value.get("tab").is_some() {
        PickStreamEvent::Tab
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

/// Where `capture-pane` or `capture-ui` writes the capture itself, rather than a report about it.
#[derive(Debug, PartialEq)]
enum RawCapture {
    Stdout,
    File(PathBuf),
}

/// `--output` always writes the capture itself. Without it, only a PNG does, since there is no
/// text report of an image; `--format json` still asks for the JSON envelope.
fn raw_capture(command: &ControlCli) -> Option<RawCapture> {
    let render = match command.request.command {
        control::ControlCommand::CapturePane { render, .. }
        | control::ControlCommand::CaptureUi { render, .. }
        | control::ControlCommand::SendText {
            capture: Some(render),
            ..
        }
        | control::ControlCommand::SendKeys {
            capture: Some(render),
            ..
        } => render,
        _ => return None,
    };
    if let Some(path) = &command.output {
        return Some(RawCapture::File(path.clone()));
    }
    (render == control::CaptureRender::Png && command.output_format != Some(ListFormat::Json))
        .then_some(RawCapture::Stdout)
}

/// The bytes a capture reply carries: its text, or its decoded PNG.
fn capture_bytes(response: &serde_json::Value) -> std::result::Result<Vec<u8>, String> {
    use base64::Engine as _;

    // Pane and UI replies wrap the same tagged content in different fields.
    let data = response.get("data").cloned().unwrap_or_default();
    let content: control::CaptureContent =
        serde_json::from_value(data).map_err(|err| format!("unexpected capture reply: {err}"))?;
    match content {
        control::CaptureContent::Text { mut text } | control::CaptureContent::Ansi { mut text } => {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            Ok(text.into_bytes())
        }
        control::CaptureContent::Png { png_base64 } => base64::engine::general_purpose::STANDARD
            .decode(png_base64)
            .map_err(|err| format!("capture reply carried invalid base64: {err}")),
    }
}

fn write_raw_capture(destination: RawCapture, response: &serde_json::Value) {
    let written = capture_bytes(response).and_then(|bytes| match &destination {
        RawCapture::Stdout => {
            let mut stdout = std::io::stdout().lock();
            match stdout.write_all(&bytes).and_then(|()| stdout.flush()) {
                // A reader that stopped early, such as `head`, already has what it wanted.
                Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                result => result.map_err(|err| format!("cannot write the capture: {err}")),
            }
        }
        RawCapture::File(path) => std::fs::write(path, &bytes)
            .map_err(|err| format!("cannot write {}: {err}", path.display())),
    });
    if let Err(err) = written {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

/// Report a failed reply's error and exit 1.
fn exit_on_failure(response: &serde_json::Value) {
    if response.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        if let Some(error) = response.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }
}

pub(crate) fn run_control_cli(command: ControlCli) -> Result<()> {
    use std::io::IsTerminal;

    let raw_capture = raw_capture(&command);
    if raw_capture == Some(RawCapture::Stdout) && std::io::stdout().is_terminal() {
        eprintln!("PNG output is binary; pass --output FILE or redirect stdout");
        std::process::exit(2);
    }
    let value = match command.endpoint {
        ControlEndpoint::Ui(socket) => ask_ui_endpoint(socket, &command.request)?,
        ControlEndpoint::Session(session) => {
            ask_session_endpoint(&session, command.request.clone())?
        }
        ControlEndpoint::Remote { target, session } => {
            ask_remote_endpoint(&target, &session, command.request.clone())?
        }
    };
    // A wait that failed still answers with what the pane showed at the end, which is the first
    // thing anyone debugging it wants; it is delivered where a capture would go, then the failure.
    let failed_wait_capture = command.request.command.pane_wait().is_some()
        && value.get("ok").and_then(|ok| ok.as_bool()) == Some(false)
        && value.get("data").is_some_and(|data| !data.is_null());
    if let Some(destination) = raw_capture {
        if !failed_wait_capture {
            exit_on_failure(&value);
        }
        write_raw_capture(destination, &value);
        exit_on_failure(&value);
        return Ok(());
    }
    let line = serde_json::to_string(&value).unwrap_or_default();
    let human_output = match command.output_format {
        Some(ListFormat::Text) => true,
        Some(ListFormat::Json) => false,
        None => std::io::stdout().is_terminal(),
    };
    if !human_output {
        println!("{line}");
    } else if failed_wait_capture {
        print!(
            "{}",
            crate::cli::output::format_capture_text(value.get("data"))
        );
    }
    exit_on_failure(&value);
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

    fn capture_cli(
        render: control::CaptureRender,
        output_format: Option<ListFormat>,
        output: Option<&str>,
    ) -> ControlCli {
        ControlCli {
            endpoint: ControlEndpoint::Ui(None),
            request: control_request(control::ControlCommand::CapturePane {
                target: None,
                scrollback: None,
                render,
                scale: None,
                wait: None,
            }),
            output_format,
            output: output.map(PathBuf::from),
        }
    }

    #[test]
    fn only_a_png_or_an_output_file_writes_the_capture_itself() {
        use control::CaptureRender::{Ansi, Png, Text};

        // A PNG has no text report, so it is written raw unless the JSON envelope is asked for.
        assert_eq!(
            raw_capture(&capture_cli(Png, None, None)),
            Some(RawCapture::Stdout)
        );
        assert_eq!(
            raw_capture(&capture_cli(Png, Some(ListFormat::Text), None)),
            Some(RawCapture::Stdout)
        );
        assert_eq!(
            raw_capture(&capture_cli(Png, Some(ListFormat::Json), None)),
            None
        );
        // Text and ANSI keep the usual report: human text on a terminal, JSON in a pipe.
        assert_eq!(raw_capture(&capture_cli(Text, None, None)), None);
        assert_eq!(raw_capture(&capture_cli(Ansi, None, None)), None);
        for render in [Text, Ansi, Png] {
            assert_eq!(
                raw_capture(&capture_cli(render, None, Some("out"))),
                Some(RawCapture::File(PathBuf::from("out")))
            );
        }

        // `capture-ui` follows the same rules.
        let mut ui = capture_cli(Png, None, None);
        ui.request.command = control::ControlCommand::CaptureUi {
            render: Png,
            scale: None,
        };
        assert_eq!(raw_capture(&ui), Some(RawCapture::Stdout));
        ui.request.command = control::ControlCommand::CaptureUi {
            render: Text,
            scale: None,
        };
        assert_eq!(raw_capture(&ui), None);
    }

    #[test]
    fn capture_bytes_decode_what_the_reply_carries() {
        use base64::Engine as _;

        let reply = |content: serde_json::Value| {
            let mut data = serde_json::json!({"id": 1, "title": null});
            data.as_object_mut()
                .unwrap()
                .extend(content.as_object().unwrap().clone());
            serde_json::json!({"ok": true, "data": data})
        };
        assert_eq!(
            capture_bytes(&reply(serde_json::json!({"render": "text", "text": "a"}))),
            Ok(b"a\n".to_vec())
        );
        assert_eq!(
            capture_bytes(&reply(
                serde_json::json!({"render": "ansi", "text": "\u{1b}[31ma\n"})
            )),
            Ok(b"\x1b[31ma\n".to_vec())
        );
        let png = b"\x89PNG\r\n\x1a\nbytes";
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        assert_eq!(
            capture_bytes(&reply(
                serde_json::json!({"render": "png", "png_base64": encoded})
            )),
            Ok(png.to_vec())
        );
        assert!(
            capture_bytes(&reply(
                serde_json::json!({"render": "png", "png_base64": "%%"})
            ))
            .is_err()
        );
    }

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

    #[test]
    fn a_tab_switch_is_non_terminal_and_a_tabbed_selection_still_ends_the_picker() {
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({ "tab": "tags" })),
            PickStreamEvent::Tab
        );
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({ "selected": "v1", "tab": "tags" })),
            PickStreamEvent::Selected("v1")
        );
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({
                "action": "delete",
                "selected": "v1",
                "tab": "tags"
            })),
            PickStreamEvent::Action
        );
    }

    /// Opening rows go to the tab the UI will actually show, which is the requested one only when
    /// it was declared.
    #[test]
    fn opening_rows_fill_the_tab_the_picker_opens_on() {
        let tabs = [
            crate::state::PickTab {
                id: String::new(),
                label: None,
            },
            crate::state::PickTab {
                id: "branches".into(),
                label: None,
            },
            crate::state::PickTab {
                id: "tags".into(),
                label: None,
            },
        ];
        assert_eq!(opening_tab(&tabs, Some("tags")), Some("tags"));
        assert_eq!(opening_tab(&tabs, Some("missing")), Some("branches"));
        assert_eq!(opening_tab(&tabs, None), Some("branches"));
        assert_eq!(opening_tab(&[], Some("tags")), None);

        // A tab past the cap does not exist to the UI, so its opening rows go to the first tab.
        let many: Vec<_> = (0..=crate::state::MAX_PICK_TABS)
            .map(|index| crate::state::PickTab {
                id: format!("t{index}"),
                label: None,
            })
            .collect();
        let last = format!("t{}", crate::state::MAX_PICK_TABS);
        assert_eq!(opening_tab(&many, Some(&last)), Some("t0"));
    }
}
