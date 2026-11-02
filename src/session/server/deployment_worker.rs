//! Deployment requests, run one at a time off the session server's loop.

use std::sync::mpsc;

use super::deployments::{DEPLOY_CHUNK_BYTES, DeployError, PreparedDeployment, SessionDeployments};
use super::headless::ReplyTo;
use super::{ClientId, SessionServer};
use crate::control::{
    ControlCommand, ControlErrorCode, ControlResponse, ExtensionDeploymentReport,
};

/// How many requests may wait for the worker. Each carries at most one chunk of a bundle.
const MAX_PENDING: usize = 8;

/// How a deployment failure reaches a control caller: an argument it can fix, a store it cannot
/// reach, or a bundle the sender has to look at again.
fn deployment_error(error: DeployError) -> ControlResponse {
    let code = match error {
        DeployError::Invalid(_) | DeployError::Mismatch(_) => ControlErrorCode::InvalidArgument,
        DeployError::Unavailable(_) => ControlErrorCode::Unavailable,
    };
    ControlResponse::error_with(code, error.to_string())
}

/// A finished job's answer. A status report stays typed because only the server can add what its
/// services are doing, and the worker cannot see them.
enum DeploymentAnswer {
    Response(ControlResponse),
    Status(ExtensionDeploymentReport),
}

fn run_deployment_job(
    store: &SessionDeployments,
    session: &str,
    command: ControlCommand,
) -> DeploymentAnswer {
    let response = match command {
        ControlCommand::ExtensionsDeploy {
            id,
            digest,
            size,
            offset,
            data,
            last,
        } => {
            use base64::Engine as _;
            let bytes = match base64::engine::general_purpose::STANDARD.decode(data) {
                Ok(bytes) if bytes.len() <= DEPLOY_CHUNK_BYTES => bytes,
                _ => {
                    return DeploymentAnswer::Response(ControlResponse::error_with(
                        ControlErrorCode::InvalidArgument,
                        "invalid or oversized deployment chunk",
                    ));
                }
            };
            match store.append(&id, &digest, size, offset, &bytes, last) {
                Ok(upload) => ControlResponse::ok(upload),
                Err(error) => deployment_error(error),
            }
        }
        ControlCommand::ExtensionsUndeploy { id } => match store.undeploy(&id) {
            Ok(removed) => ControlResponse::ok(crate::control::ExtensionUndeployReport {
                extension: id,
                removed,
            }),
            Err(error) => deployment_error(error),
        },
        ControlCommand::ExtensionsStatus => match store.status() {
            Ok(deployments) => {
                return DeploymentAnswer::Status(ExtensionDeploymentReport {
                    session: session.to_string(),
                    deployments,
                });
            }
            Err(error) => deployment_error(error),
        },
        _ => unreachable!("deployment worker accepts deployment commands only"),
    };
    DeploymentAnswer::Response(response)
}

struct DeploymentJob {
    client_id: ClientId,
    reply: ReplyTo,
    command: ControlCommand,
}

struct DeploymentResult {
    reply: Option<(ClientId, ReplyTo, DeploymentAnswer)>,
    /// What the session should now be running, when this job could have changed that.
    prepared: Option<Result<Vec<PreparedDeployment>, String>>,
}

/// Whether a job calls for checking what the session runs, which hashes every deployment. A chunk
/// of an upload changes nothing until the last one, and a large bundle has many. `status` does,
/// because it is how a tampered bundle comes to light.
fn job_can_change_automation(command: &ControlCommand) -> bool {
    !matches!(
        command,
        ControlCommand::ExtensionsDeploy { last: false, .. }
    )
}

/// Runs deployment requests one at a time off the server loop: they write, hash, and fsync.
pub(super) struct DeploymentWorker {
    sender: Option<mpsc::SyncSender<DeploymentJob>>,
    results: mpsc::Receiver<DeploymentResult>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub(super) pending: usize,
    pending_clients: std::collections::HashMap<ClientId, usize>,
}

impl DeploymentWorker {
    fn start(store: SessionDeployments, session: String, restart: bool) -> std::io::Result<Self> {
        let (sender, requests) = mpsc::sync_channel::<DeploymentJob>(MAX_PENDING);
        let (results, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("session-deployments".into())
            .spawn(move || {
                let initial = (if restart {
                    store.restart_generations()
                } else {
                    Ok(())
                })
                .and_then(|()| store.prepare())
                .map_err(|error| error.to_string());
                if results
                    .send(DeploymentResult {
                        reply: None,
                        prepared: Some(initial),
                    })
                    .is_err()
                {
                    return;
                }
                while let Ok(job) = requests.recv() {
                    let refresh = job_can_change_automation(&job.command);
                    let answer = run_deployment_job(&store, &session, job.command);
                    let prepared =
                        refresh.then(|| store.prepare().map_err(|error| error.to_string()));
                    if results
                        .send(DeploymentResult {
                            reply: Some((job.client_id, job.reply, answer)),
                            prepared,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            results: receiver,
            thread: Some(thread),
            pending: 0,
            pending_clients: Default::default(),
        })
    }
}

impl Drop for DeploymentWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl SessionServer {
    pub(super) fn ensure_deployment_worker(&mut self, restart: bool) -> std::io::Result<()> {
        if self.deployment_worker.is_none() {
            self.deployment_worker = Some(DeploymentWorker::start(
                self.deployments.clone(),
                self.session_name.clone(),
                restart,
            )?);
        }
        Ok(())
    }

    pub(super) fn submit_deployment(
        &mut self,
        client_id: ClientId,
        reply: ReplyTo,
        command: ControlCommand,
    ) -> Option<ControlResponse> {
        if crate::state::is_ephemeral_session_name(&self.session_name) {
            return Some(ControlResponse::error_with(
                ControlErrorCode::NotPermitted,
                "temporary sessions cannot hold deployments",
            ));
        }
        if self.client_attached(client_id)
            && self.client_read_only(client_id)
            && !matches!(command, ControlCommand::ExtensionsStatus)
        {
            return Some(ControlResponse::error_with(
                ControlErrorCode::ReadOnly,
                "read-only clients cannot change deployments",
            ));
        }
        if let Err(error) = self.ensure_deployment_worker(false) {
            return Some(ControlResponse::error(format!(
                "cannot start deployment worker: {error}"
            )));
        }
        let worker = self.deployment_worker.as_mut().expect("worker started");
        if worker.pending >= MAX_PENDING {
            return Some(ControlResponse::error(
                "deployment worker is busy; retry later",
            ));
        }
        match worker
            .sender
            .as_ref()
            .expect("worker alive")
            .try_send(DeploymentJob {
                client_id,
                reply,
                command,
            }) {
            Ok(()) => {
                worker.pending += 1;
                *worker.pending_clients.entry(client_id).or_default() += 1;
                None
            }
            Err(_) => Some(ControlResponse::error(
                "deployment worker unavailable; retry later",
            )),
        }
    }

    pub(super) fn holds_deployment_reply(&self, client_id: ClientId) -> bool {
        self.deployment_worker
            .as_ref()
            .is_some_and(|worker| worker.pending_clients.contains_key(&client_id))
    }

    pub(super) fn drain_deployment_results(&mut self) {
        let responses: Vec<_> = self
            .deployment_worker
            .as_mut()
            .map_or_else(Vec::new, |worker| {
                let responses: Vec<_> = worker.results.try_iter().collect();
                for result in &responses {
                    if let Some((client_id, _, _)) = &result.reply {
                        worker.pending -= 1;
                        if let Some(count) = worker.pending_clients.get_mut(client_id) {
                            *count -= 1;
                            if *count == 0 {
                                worker.pending_clients.remove(client_id);
                            }
                        }
                    }
                }
                responses
            });
        for result in responses {
            match result.prepared {
                Some(Ok(prepared)) => self.activate_deployments(prepared),
                Some(Err(error)) => eprintln!("rozi: session automation unavailable: {error}"),
                None => {}
            }
            if let Some((client_id, reply, answer)) = result.reply {
                let response = self.deployment_response(answer);
                self.answer_held(client_id, &reply, response);
            }
        }
        if let Some(automation) = &mut self.automation {
            automation.poll();
        }
    }

    fn deployment_response(&mut self, answer: DeploymentAnswer) -> ControlResponse {
        let mut report = match answer {
            DeploymentAnswer::Response(response) => return response,
            DeploymentAnswer::Status(report) => report,
        };
        if let Some(automation) = &mut self.automation {
            automation.poll();
            for deployment in &mut report.deployments {
                deployment.services = automation.services(&deployment.extension);
            }
        }
        ControlResponse::ok(report)
    }
}
