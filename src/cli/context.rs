//! Verified caller context, independent of the session a UI happens to display.
use std::io::{BufReader, Write};

use tui_lipan::Result;

use super::args::control_request;
use super::control::{discover_socket, read_socket_line};
use crate::control;
use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};

/// Query identity without interpreting discovery hints as proof. Failure to find the hinted
/// server is a stale context report, while a live server verifies the pane and fences its instance.
pub(super) fn caller_context(request: &control::ControlRequest) -> Result<control::CallerContext> {
    let mut context = verified_caller_context(request)?;
    // UI availability is independent of caller identity. A dead inherited socket must not hide
    // a live detached caller session. Discovery is optional and never launches a UI.
    if let Some(metadata) = probe_ui_context() {
        context.ui = caller_ui(metadata, context.session.as_ref());
    }
    Ok(context)
}

fn caller_ui(
    metadata: control::UiContextMetadata,
    session: Option<&control::CallerSession>,
) -> control::CallerUi {
    control::CallerUi {
        available: true,
        displays_session: session
            .map(|session| metadata.displayed_session.as_ref() == Some(&session.instance)),
        holds_session: session.map(|session| metadata.held_sessions.contains(&session.instance)),
    }
}

pub(super) fn verified_caller_context(
    request: &control::ControlRequest,
) -> Result<control::CallerContext> {
    let context = control::CallerContext {
        inside_rozi: false,
        status: "outside".into(),
        session: None,
        pane: None,
        agent: None,
        ui: control::CallerUi::default(),
    };
    verify_session_context(request, context)
}

fn verify_session_context(
    request: &control::ControlRequest,
    mut context: control::CallerContext,
) -> Result<control::CallerContext> {
    use crate::session::discovery::InstanceLookup;
    let Some(instance) = &request.source_session else {
        if request.source_pane.is_some() || std::env::var_os("ROZI").is_some() {
            context.status = "incomplete".into();
        }
        return Ok(context);
    };
    context.status = "stale-session".into();
    let InstanceLookup::Found(session) =
        crate::session::discovery::session_with_instance(instance)?
    else {
        return Ok(context);
    };
    let mut probe = request.clone();
    probe.command = control::ControlCommand::Context;
    let response = match crate::session::headless::run_session_control(&session, probe) {
        Ok(response) => response,
        Err(crate::session::headless::SessionControlError::NoSuchSession(_)) => return Ok(context),
        Err(crate::session::headless::SessionControlError::Refused { code, .. })
            if code == "instance-mismatch" =>
        {
            return Ok(context);
        }
        Err(error) => return Err(std::io::Error::other(error).into()),
    };
    if !response.ok {
        return Err(std::io::Error::other(
            response
                .error
                .unwrap_or_else(|| "context verification failed".into()),
        )
        .into());
    }
    serde_json::from_value(response.data.unwrap_or_default())
        .map_err(|error| std::io::Error::other(error).into())
}

fn probe_ui_context() -> Option<control::UiContextMetadata> {
    probe_ui_at(&discover_socket(None).ok()?)
}

/// Prefer the inherited UI when it still displays the caller's session. Otherwise the verified
/// instance disambiguates discovery, even after a UI replacement or with several UIs running.
pub(super) fn authority_ui(
    request: &control::ControlRequest,
    caller: &crate::session::protocol::PaneRef,
) -> Option<std::path::PathBuf> {
    if !request.command.uses_ui_authority() {
        return None;
    }
    let inherited = std::env::var_os("ROZI_SOCKET")
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    if let Some(path) = &inherited
        && ui_displays(path, &caller.session_instance)
    {
        return Some(path.clone());
    }
    let directory = control::runtime_dir().ok()?;
    let endpoints = EndpointRegistry::list_live_control_endpoints(&directory).ok()?;
    let mut matching = endpoints.into_iter().filter(|endpoint| {
        inherited.as_deref() != Some(endpoint.path())
            && ui_displays(endpoint.path(), &caller.session_instance)
    });
    let endpoint = matching.next()?;
    matching
        .next()
        .is_none()
        .then(|| endpoint.path().to_path_buf())
}

fn ui_displays(
    path: &std::path::Path,
    session: &crate::session::protocol::SessionInstanceId,
) -> bool {
    probe_ui_at(path).is_some_and(|metadata| metadata.displayed_session.as_ref() == Some(session))
}

fn probe_ui_at(path: &std::path::Path) -> Option<control::UiContextMetadata> {
    let mut stream = IpcEndpoint::at_path(path).connect().ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok()?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(2)))
        .ok()?;
    let request = control_request(control::ControlCommand::Context);
    writeln!(stream, "{}", serde_json::to_string(&request).ok()?).ok()?;
    let line = read_socket_line(&mut BufReader::new(stream)).ok()?;
    let response: control::ControlResponse = serde_json::from_str(&line).ok()?;
    if !response.ok {
        return None;
    }
    serde_json::from_value(response.data?).ok()
}

/// Omitted pane targets on `--here` mean the verified caller, even in a many-pane session.
pub(super) fn pin_here_target(command: &mut control::ControlCommand, pane: crate::state::PaneId) {
    use control::ControlCommand::*;
    let target = match command {
        CapturePane { target, .. }
        | SendText { target, .. }
        | SendKeys { target, .. }
        | PaneLogging { target, .. }
        | SetStatus { target, .. }
        | RecordStart { target, .. }
        | AgentReport { target, .. }
        | AgentRelease { target, .. } => target,
        RecordStop { id: None, target }
        | RecordMark {
            id: None, target, ..
        } => target,
        _ => return,
    };
    target.get_or_insert(pane);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::SessionInstanceId;
    #[test]
    fn a_caller_can_belong_to_a_background_attachment() {
        let caller = SessionInstanceId::for_test("caller");
        let ui = caller_ui(
            control::UiContextMetadata {
                displayed_session: Some(SessionInstanceId::for_test("on-screen")),
                held_sessions: vec![caller.clone()],
            },
            Some(&control::CallerSession {
                name: "dev".into(),
                instance: caller,
            }),
        );
        assert!(ui.available);
        assert_eq!(ui.displays_session, Some(false));
        assert_eq!(ui.holds_session, Some(true));
    }
}
