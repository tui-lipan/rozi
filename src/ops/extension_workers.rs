//! What a placed extension process may ask of this UI.
//!
//! A placed worker - a process an extension contribution started on a session host, this machine
//! or another one - is trusted only as what this client launched it as. Its credential names one
//! extension, one generation, and a binding: the host it was placed on, and for some placements one
//! session there. Every request it makes is checked here, at the moment it is handled, against all
//! three:
//!
//! - The generation must still be active and the credential unrevoked.
//! - The command must be one a placed process may send at all. Nothing that would start a process
//!   on this client, directly or by running an action that does, is on that list, and neither is
//!   anything that reads the client as a whole (UI captures, recordings, metrics).
//! - Anything that acts on panes acts on the session on screen, and is honored only while that
//!   session lies within the worker's binding. Panes local to this client - the scratchpad and the
//!   popup - never do, whatever session is on screen.
//!
//! The same rules hold for a placed worker running on this machine, so an extension behaves the
//! same wherever Rozi places it, and one tested locally does not start failing once it runs on a
//! server.

use crate::config::ExtensionProvenance;
use crate::control::{
    AgentTarget, ControlCommand, ControlErrorCode, ControlRequest, ControlResponse, ExtensionGrant,
    RequestOrigin,
};
use crate::state::{HostKey, PaneId, State, Worker, WorkerBinding, WorkerId, WorkerRuntime};

/// Establish who sent a control request. See [`crate::control::RequestOrigin`].
pub(crate) fn authorize(
    state: &State,
    provenance: Option<ExtensionProvenance>,
    credential: Option<String>,
    origin: RequestOrigin,
) -> Result<Option<ExtensionGrant>, ControlResponse> {
    let runtime = match origin {
        RequestOrigin::Local => None,
        RequestOrigin::Bridged { runtime } => Some(WorkerRuntime(runtime)),
    };
    let Some(credential) = credential else {
        if runtime.is_some() {
            return Err(ControlResponse::error_with(
                ControlErrorCode::NotPermitted,
                "a request over the extension bridge must carry its worker credential",
            ));
        }
        return match provenance {
            None => Ok(None),
            Some(provenance)
                if crate::config::provenance_is_active(
                    &state.extension_generations,
                    &provenance,
                ) =>
            {
                Ok(Some(ExtensionGrant {
                    provenance,
                    worker: None,
                }))
            }
            Some(_) => Err(inactive()),
        };
    };
    // Identity comes from the credential alone. Whatever extension and generation the request
    // names are discarded: a process on another machine may write anything there. A credential is
    // only ever valid over the bridge of the runtime it was issued for.
    let Some(worker) =
        runtime.and_then(|runtime| state.extension_workers.authenticate(&credential, runtime))
    else {
        return Err(ControlResponse::error_with(
            ControlErrorCode::ExtensionInactive,
            "worker credential is not valid: it was revoked, or never issued for this connection",
        ));
    };
    if !crate::config::provenance_is_active(&state.extension_generations, &worker.extension) {
        return Err(inactive());
    }
    Ok(Some(ExtensionGrant {
        provenance: worker.extension.clone(),
        worker: Some(worker.id),
    }))
}

fn inactive() -> ControlResponse {
    ControlResponse::error_with(
        ControlErrorCode::ExtensionInactive,
        "extension generation is not active",
    )
}

/// The live worker behind an already-authorized request, re-checked now. A worker revoked, or
/// whose generation retired, since its request was authorized is refused here.
pub(crate) fn live_worker(state: &State, id: WorkerId) -> Result<&Worker, ControlResponse> {
    let worker = state.extension_workers.get(id).ok_or_else(|| {
        ControlResponse::error_with(
            ControlErrorCode::ExtensionInactive,
            "worker credential was revoked",
        )
    })?;
    if !crate::config::provenance_is_active(&state.extension_generations, &worker.extension) {
        return Err(inactive());
    }
    Ok(worker)
}

/// How a command relates to a placed worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Touches only this UI's own chrome - a picker, a toast, the palette - and runs nothing.
    Ui,
    /// Acts on, or reads, the session on screen.
    Session,
    /// Runs an action: allowed only for the worker's own extension's commands placed on the active
    /// session, which then run on that session's host, never here.
    RunAction,
    /// Never allowed for a placed worker.
    Denied(&'static str),
}

pub(crate) fn gate(command: &ControlCommand) -> Gate {
    match command {
        ControlCommand::Pick { .. }
        | ControlCommand::Notify { .. }
        | ControlCommand::CommandVisibility { .. }
        | ControlCommand::Subscribe { .. } => Gate::Ui,
        ControlCommand::ListPanes
        | ControlCommand::LayoutGet { .. }
        | ControlCommand::LayoutSet { .. }
        | ControlCommand::PaneSet { .. }
        | ControlCommand::PaneMove { .. }
        | ControlCommand::PaneSwap { .. }
        | ControlCommand::PaneReveal { .. }
        | ControlCommand::PaneClose { .. }
        | ControlCommand::AgentsList
        | ControlCommand::AgentGet { .. }
        | ControlCommand::AgentRead { .. }
        | ControlCommand::AgentWait { .. }
        | ControlCommand::AgentPrompt { .. }
        | ControlCommand::Focus { .. }
        | ControlCommand::SendText { .. }
        | ControlCommand::SendKeys { .. }
        | ControlCommand::NewPane { .. }
        | ControlCommand::CapturePane { .. }
        | ControlCommand::SwitchWorkspace { .. }
        | ControlCommand::MoveToWorkspace { .. }
        | ControlCommand::Publish
        | ControlCommand::SetStatus { .. } => Gate::Session,
        ControlCommand::RunAction { .. } => Gate::RunAction,
        ControlCommand::Popup { .. } => Gate::Denied(
            "a popup opens in this client's own popup pane, which runs on this machine",
        ),
        ControlCommand::PaneLogging { .. } => {
            Gate::Denied("pane logging writes files on this client")
        }
        ControlCommand::CaptureUi { .. } => {
            Gate::Denied("a UI capture shows this whole client, not one session")
        }
        ControlCommand::Metrics => Gate::Denied("metrics describe this whole client"),
        ControlCommand::ExtensionRuntimeStatus => {
            Gate::Denied("the runtime status describes every placed process of this client")
        }
        ControlCommand::AgentReport { .. } | ControlCommand::AgentRelease { .. } => {
            Gate::Denied("agent reports come from the pane an agent runs in")
        }
        ControlCommand::RecordStart { .. }
        | ControlCommand::RecordStop { .. }
        | ControlCommand::RecordList
        | ControlCommand::RecordMark { .. }
        | ControlCommand::RecordUiStart { .. }
        | ControlCommand::RecordUiStop
        | ControlCommand::RecordUiMark { .. } => {
            Gate::Denied("recordings write files and capture this client")
        }
    }
}

/// The host and session on screen, if a session is attached at all.
pub(crate) fn current_scope(
    state: &State,
) -> Option<(HostKey, Option<crate::session::protocol::SessionInstanceId>)> {
    let attachment = state.current();
    if !attachment.session_attached {
        return None;
    }
    Some((
        HostKey::of(attachment.remote_target.as_ref()),
        attachment.session_instance.clone(),
    ))
}

/// Whether the session on screen lies within `binding`.
pub(crate) fn in_scope(state: &State, binding: &WorkerBinding) -> bool {
    current_scope(state).is_some_and(|(host, instance)| binding.admits(&host, instance.as_ref()))
}

/// What to do with a placed worker's request that passed admission.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    Proceed,
    /// A listing asked while no session within the binding is on screen: the answer is that there
    /// is nothing the worker may see, not an error it has to handle on every poll.
    EmptyListing,
}

/// Admit or refuse a placed worker's request, right before it is handled.
pub(crate) fn admit(
    state: &State,
    worker: &Worker,
    request: &ControlRequest,
) -> Result<Admission, ControlResponse> {
    match gate(&request.command) {
        Gate::Denied(reason) => Err(not_permitted(reason)),
        Gate::Ui => Ok(Admission::Proceed),
        Gate::RunAction => {
            let ControlCommand::RunAction { action } = &request.command else {
                unreachable!("gated as RunAction");
            };
            let placed = state
                .config
                .extension_placements
                .get(&worker.extension.id)
                .and_then(|placements| placements.commands.get(action))
                .is_some_and(|command| {
                    command.placement == crate::config::Placement::ActiveSession
                });
            if !placed {
                return Err(not_permitted(
                    "a placed process may only run its own extension's commands placed on the \
                     active session; anything else could start a process on this client",
                ));
            }
            if !in_scope(state, &worker.binding) {
                return Err(out_of_scope(worker));
            }
            Ok(Admission::Proceed)
        }
        Gate::Session => {
            if !in_scope(state, &worker.binding) {
                return if matches!(
                    request.command,
                    ControlCommand::ListPanes | ControlCommand::AgentsList
                ) {
                    Ok(Admission::EmptyListing)
                } else {
                    Err(out_of_scope(worker))
                };
            }
            if let Some(source) = request.source_pane {
                shared_pane(state, worker, source)?;
            }
            for target in pane_targets(&request.command)? {
                match target {
                    Some(id) => shared_pane(state, worker, id)?,
                    // No explicit target and no source pane: the command acts on the focused pane,
                    // which may be the scratchpad's or the popup's.
                    None if request.source_pane.is_none() => {
                        let focused = state.focused_pane().ok_or_else(|| {
                            ControlResponse::error_with(
                                ControlErrorCode::TargetRequired,
                                "no pane is focused",
                            )
                        })?;
                        shared_pane(state, worker, focused)?;
                    }
                    None => {}
                }
            }
            Ok(Admission::Proceed)
        }
    }
}

/// The panes a command acts on: `Some` for an explicit id, `None` for "the source or focused pane".
fn pane_targets(command: &ControlCommand) -> Result<Vec<Option<PaneId>>, ControlResponse> {
    let agent = |target: &AgentTarget| match target {
        AgentTarget::Pane(id) => Ok(Some(*id)),
        AgentTarget::Ref(_) => Err(not_permitted(
            "a placed process names an agent by its pane; a reference can reach a session other \
             than the one on screen",
        )),
    };
    Ok(match command {
        ControlCommand::PaneSet { target, .. }
        | ControlCommand::PaneMove { target, .. }
        | ControlCommand::PaneReveal { target }
        | ControlCommand::PaneClose { target, .. }
        | ControlCommand::Focus { target } => vec![Some(*target)],
        ControlCommand::PaneSwap { target, with, .. } => vec![Some(*target), Some(*with)],
        ControlCommand::AgentGet { target }
        | ControlCommand::AgentRead { target, .. }
        | ControlCommand::AgentWait { target, .. }
        | ControlCommand::AgentPrompt { target, .. } => vec![agent(target)?],
        ControlCommand::SendText { target, .. }
        | ControlCommand::SendKeys { target, .. }
        | ControlCommand::CapturePane { target, .. }
        | ControlCommand::SetStatus { target, .. } => vec![*target],
        ControlCommand::Publish => vec![None],
        _ => Vec::new(),
    })
}

/// Refuse a pane that is not one of the on-screen session's own. Pane ids are only unique within a
/// namespace, and the client-local one (scratchpad, popup) is searched first, so a shared pane's id
/// that a local pane happens to share is refused too rather than resolved to the local pane.
fn shared_pane(state: &State, worker: &Worker, id: PaneId) -> Result<(), ControlResponse> {
    let shared = !crate::pane::lifecycle::pane_is_local(state, id)
        && state
            .current()
            .workspaces
            .iter()
            .any(|workspace| workspace.panes.iter().any(|pane| pane.id == id));
    if shared {
        Ok(())
    } else {
        Err(ControlResponse::error_with(
            ControlErrorCode::OutOfScope,
            format!(
                "pane {id} is not in a session on {}",
                worker.binding.host.label()
            ),
        ))
    }
}

fn not_permitted(reason: &str) -> ControlResponse {
    ControlResponse::error_with(
        ControlErrorCode::NotPermitted,
        format!("not permitted for a placed extension process: {reason}"),
    )
}

fn out_of_scope(worker: &Worker) -> ControlResponse {
    let what = if worker.binding.session.is_some() {
        format!(
            "the session this process was placed for, on {}",
            worker.binding.host.label()
        )
    } else {
        format!("a session on {}", worker.binding.host.label())
    };
    ControlResponse::error_with(
        ControlErrorCode::OutOfScope,
        format!("the session on screen is not {what}"),
    )
}

/// Name the host on what a remote worker shows in this UI, so a prompt or a toast can never pass
/// for this client's own.
pub(crate) fn attribute(worker: &Worker, text: &str) -> String {
    match &worker.binding.host {
        HostKey::Local => text.to_string(),
        HostKey::Remote(target) => format!("{text} · {}", target.display_label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ControlEnvelope, ControlRequest};
    use crate::session::remote::RemoteTarget;
    use crate::state::{WorkerKind, WorkerRuntime};
    use tui_lipan::TestBackend;

    const EXTENSION: &str = "sessions";

    fn pc() -> HostKey {
        HostKey::Remote(RemoteTarget::Alias("pc".to_string()))
    }

    fn backend() -> TestBackend<crate::AppRoot> {
        let mut backend = TestBackend::new(crate::AppRoot::default());
        let state = backend.state_mut();
        state
            .extension_generations
            .insert(EXTENSION.to_string(), "g1".to_string());
        backend
    }

    fn issue(
        backend: &mut TestBackend<crate::AppRoot>,
        host: HostKey,
        runtime: WorkerRuntime,
    ) -> (WorkerId, String) {
        let worker = backend.state_mut().extension_workers.issue(
            ExtensionProvenance {
                id: EXTENSION.to_string(),
                generation: "g1".to_string(),
            },
            crate::config::Placement::EachHost,
            WorkerBinding {
                host,
                session: None,
            },
            runtime,
            WorkerKind::Service {
                name: "sessions.watch".to_string(),
            },
        );
        (worker.id, worker.credential().to_string())
    }

    fn show_session_on(backend: &mut TestBackend<crate::AppRoot>, host: &HostKey) {
        let attachment = backend.state_mut().current_mut();
        attachment.session_attached = true;
        attachment.remote_target = host.remote().cloned();
        attachment.remote_host = host.remote().map(RemoteTarget::display_label);
    }

    fn request(command: ControlCommand) -> ControlRequest {
        ControlRequest {
            command,
            source_pane: None,
            source_session: None,
            extension: None,
            credential: None,
        }
    }

    fn ask(
        backend: &mut TestBackend<crate::AppRoot>,
        worker: WorkerId,
        request: ControlRequest,
    ) -> ControlResponse {
        let (reply, response) = std::sync::mpsc::channel();
        backend
            .dispatch(crate::Msg::ControlRequest(ControlEnvelope {
                request,
                reply,
                worker: Some(worker),
            }))
            .expect("dispatch");
        response
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("answered")
    }

    fn on_test_thread(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    /// The credential is the identity. A request that names another extension, or a generation
    /// of its own choosing, is attributed to whatever the credential was issued for.
    #[test]
    fn a_credential_overrides_whatever_identity_the_request_claims() {
        on_test_thread(|| {
            let mut backend = backend();
            backend
                .state_mut()
                .extension_generations
                .insert("other".to_string(), "g9".to_string());
            let (worker, credential) = issue(&mut backend, pc(), WorkerRuntime(3));
            let grant = authorize(
                backend.state(),
                Some(ExtensionProvenance {
                    id: "other".to_string(),
                    generation: "g9".to_string(),
                }),
                Some(credential.clone()),
                RequestOrigin::Bridged { runtime: 3 },
            )
            .unwrap()
            .unwrap();
            assert_eq!(grant.worker, Some(worker));
            assert_eq!(grant.provenance.id, EXTENSION);

            // The same credential over another runtime, or locally, is nobody.
            for origin in [RequestOrigin::Bridged { runtime: 4 }, RequestOrigin::Local] {
                let refused =
                    authorize(backend.state(), None, Some(credential.clone()), origin).unwrap_err();
                assert_eq!(refused.code, Some(ControlErrorCode::ExtensionInactive));
            }
            // A guessed credential is nobody too.
            let refused = authorize(
                backend.state(),
                None,
                Some("0".repeat(64)),
                RequestOrigin::Bridged { runtime: 3 },
            )
            .unwrap_err();
            assert_eq!(refused.code, Some(ControlErrorCode::ExtensionInactive));
            // And the bridge carries nothing without one, even a claim to a live generation.
            let refused = authorize(
                backend.state(),
                Some(ExtensionProvenance {
                    id: EXTENSION.to_string(),
                    generation: "g1".to_string(),
                }),
                None,
                RequestOrigin::Bridged { runtime: 3 },
            )
            .unwrap_err();
            assert_eq!(refused.code, Some(ControlErrorCode::NotPermitted));
        });
    }

    #[test]
    fn a_revoked_or_retired_worker_is_refused_when_its_request_is_handled() {
        on_test_thread(|| {
            let mut backend = backend();
            show_session_on(&mut backend, &pc());
            let (worker, _) = issue(&mut backend, pc(), WorkerRuntime(1));
            assert!(ask(&mut backend, worker, request(ControlCommand::ListPanes)).ok);

            backend
                .state_mut()
                .extension_generations
                .insert(EXTENSION.to_string(), "g2".to_string());
            let retired = ask(&mut backend, worker, request(ControlCommand::ListPanes));
            assert_eq!(retired.code, Some(ControlErrorCode::ExtensionInactive));

            let (worker, _) = issue(&mut backend, pc(), WorkerRuntime(1));
            backend
                .state_mut()
                .extension_generations
                .insert(EXTENSION.to_string(), "g1".to_string());
            backend.state_mut().extension_workers.revoke(worker);
            let revoked = ask(&mut backend, worker, request(ControlCommand::ListPanes));
            assert_eq!(revoked.code, Some(ControlErrorCode::ExtensionInactive));
        });
    }

    /// A worker placed on the PC acts on the PC's sessions. While a local session is on screen it
    /// sees no panes and touches none: typing into one would run a command on this client.
    #[test]
    fn a_worker_reaches_panes_only_while_a_session_on_its_host_is_on_screen() {
        on_test_thread(|| {
            let mut backend = backend();
            show_session_on(&mut backend, &HostKey::Local);
            let (worker, _) = issue(&mut backend, pc(), WorkerRuntime(1));
            let pane = backend.state().current().workspaces[0].panes[0].id;
            let send = || {
                request(ControlCommand::SendText {
                    target: Some(pane),
                    text: "rm -rf ~\n".to_string(),
                    wait: None,
                    capture: None,
                    scale: None,
                })
            };

            let listed = ask(&mut backend, worker, request(ControlCommand::ListPanes));
            assert!(listed.ok);
            assert_eq!(listed.data, Some(serde_json::json!([])));
            let refused = ask(&mut backend, worker, send());
            assert_eq!(refused.code, Some(ControlErrorCode::OutOfScope));
            let focus = ask(
                &mut backend,
                worker,
                request(ControlCommand::Focus { target: pane }),
            );
            assert_eq!(focus.code, Some(ControlErrorCode::OutOfScope));

            show_session_on(&mut backend, &pc());
            let listed = ask(&mut backend, worker, request(ControlCommand::ListPanes));
            assert_eq!(listed.data.unwrap().as_array().unwrap().len(), 1);
            let sent = ask(&mut backend, worker, send());
            assert_ne!(sent.code, Some(ControlErrorCode::OutOfScope));
            assert_ne!(sent.code, Some(ControlErrorCode::NotPermitted));

            // Another remote host is no better than this machine.
            show_session_on(
                &mut backend,
                &HostKey::Remote(RemoteTarget::Alias("laptop".to_string())),
            );
            let refused = ask(&mut backend, worker, send());
            assert_eq!(refused.code, Some(ControlErrorCode::OutOfScope));
        });
    }

    /// The scratchpad runs on this client whatever session is on screen. Its panes are searched
    /// before the session's, so a shared pane id it happens to repeat must be refused, not resolved
    /// to the local pane.
    #[test]
    fn scratchpad_panes_stay_out_of_reach_even_when_their_ids_collide() {
        on_test_thread(|| {
            let mut backend = backend();
            show_session_on(&mut backend, &pc());
            let (worker, _) = issue(&mut backend, pc(), WorkerRuntime(1));
            let id = backend.state().current().workspaces[0].panes[0].id;
            let shadow = crate::state::Pane::new(
                id,
                100,
                tui_lipan::prelude::FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 80.0,
                    h: 24.0,
                },
            );
            backend.state_mut().scratch.panes.push(shadow);

            let send = request(ControlCommand::SendText {
                target: Some(id),
                text: "id\n".to_string(),
                wait: None,
                capture: None,
                scale: None,
            });
            let refused = ask(&mut backend, worker, send);
            assert_eq!(refused.code, Some(ControlErrorCode::OutOfScope));

            let mut sourced = request(ControlCommand::Publish);
            sourced.source_pane = Some(id);
            assert!(
                admit(
                    backend.state(),
                    backend.state().extension_workers.get(worker).unwrap(),
                    &sourced
                )
                .is_err()
            );

            // And they are not listed.
            let listed = ask(&mut backend, worker, request(ControlCommand::ListPanes));
            let local_listed = {
                let (reply, response) = std::sync::mpsc::channel();
                backend
                    .dispatch(crate::Msg::ControlRequest(ControlEnvelope::local(
                        request(ControlCommand::ListPanes),
                        reply,
                    )))
                    .unwrap();
                response.recv().unwrap()
            };
            assert_eq!(listed.data.unwrap().as_array().unwrap().len(), 1);
            assert_eq!(local_listed.data.unwrap().as_array().unwrap().len(), 2);
        });
    }

    /// Nothing a placed worker can send starts a process on this client - not directly, and not
    /// by running an action that would.
    #[test]
    fn commands_that_could_run_something_on_this_client_are_refused() {
        on_test_thread(|| {
            let mut backend = backend();
            show_session_on(&mut backend, &pc());
            let (worker, _) = issue(&mut backend, pc(), WorkerRuntime(1));
            backend
                .state_mut()
                .config
                .commands
                .push(crate::config::NamedCommand {
                    id: "sessions.local".to_string(),
                    label: None,
                    action: crate::config::UserCommandAction::Exec {
                        command: "touch /tmp/rozi-should-never-run".to_string(),
                    },
                    category: "Sessions".to_string(),
                    env: Vec::new(),
                    default_key: None,
                    hidden: false,
                });
            for command in [
                ControlCommand::RunAction {
                    action: "sessions.local".to_string(),
                },
                ControlCommand::RunAction {
                    action: "reload-extensions".to_string(),
                },
                ControlCommand::RunAction {
                    action: "new-pane".to_string(),
                },
                ControlCommand::Popup {
                    command: "id".to_string(),
                    cwd: None,
                    width: None,
                    height: None,
                    title: None,
                    keep_open: None,
                },
                ControlCommand::CaptureUi {
                    render: crate::control::CaptureRender::Text,
                    scale: None,
                    image_pixels: false,
                },
                ControlCommand::PaneLogging {
                    target: None,
                    enabled: Some(true),
                },
                ControlCommand::Metrics,
                ControlCommand::RecordUiStop,
            ] {
                let refused = ask(&mut backend, worker, request(command.clone()));
                assert_eq!(
                    refused.code,
                    Some(ControlErrorCode::NotPermitted),
                    "{command:?}"
                );
            }
        });
    }

    #[test]
    fn its_own_active_session_command_is_the_one_action_a_worker_may_run() {
        on_test_thread(|| {
            let mut backend = backend();
            let mut placements = crate::config::ExtensionPlacements {
                id: EXTENSION.to_string(),
                platforms: Vec::new(),
                local_dir: "/x".to_string(),
                bundle: std::sync::Arc::new(
                    crate::extension_runtime::bundle::Bundle::from_files(Vec::new()).unwrap(),
                ),
                commands: Default::default(),
                tabs: Default::default(),
                services: Vec::new(),
                client_unsupported: None,
            };
            placements.commands.insert(
                "sessions.open".to_string(),
                crate::config::PlacedCommand {
                    placement: crate::config::Placement::ActiveSession,
                    launch: crate::config::LaunchTemplate::Shell("true".to_string()),
                },
            );
            backend
                .state_mut()
                .config
                .extension_placements
                .insert(EXTENSION.to_string(), std::sync::Arc::new(placements));
            let (worker, _) = issue(&mut backend, pc(), WorkerRuntime(1));
            let run = request(ControlCommand::RunAction {
                action: "sessions.open".to_string(),
            });
            let worker_ref = backend
                .state()
                .extension_workers
                .get(worker)
                .unwrap()
                .clone();
            // On the wrong host it would run here.
            show_session_on(&mut backend, &HostKey::Local);
            assert_eq!(
                admit(backend.state(), &worker_ref, &run).unwrap_err().code,
                Some(ControlErrorCode::OutOfScope)
            );
            show_session_on(&mut backend, &pc());
            assert_eq!(
                admit(backend.state(), &worker_ref, &run).unwrap(),
                Admission::Proceed
            );
        });
    }

    #[test]
    fn a_remote_workers_toasts_and_pickers_name_its_host() {
        let mut registry = crate::state::WorkerRegistry::default();
        let worker = registry
            .issue(
                ExtensionProvenance {
                    id: EXTENSION.to_string(),
                    generation: "g1".to_string(),
                },
                crate::config::Placement::EachHost,
                WorkerBinding {
                    host: pc(),
                    session: None,
                },
                WorkerRuntime(1),
                WorkerKind::Service {
                    name: "s".to_string(),
                },
            )
            .clone();
        assert_eq!(attribute(&worker, "Password"), "Password · pc");
    }
}
