//! Session-owned process supervision and generation credentials.
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use super::deployments::PreparedDeployment;
use super::{ClientId, SessionServer};
use crate::control::{
    ControlCommand, ControlErrorCode, ControlRequest, ControlResponse, SessionServiceState,
    SessionServiceStatus,
};
use crate::extension_runtime::protocol::{Launch, SpawnCwd};
use crate::extension_runtime::supervisor::{ProcessSpec, ServiceSpec, Supervisor, SupervisorEvent};

struct ActiveExtension {
    generation: u64,
    credential: String,
    services: Vec<SessionServiceStatus>,
}

pub(super) struct Automation {
    supervisor: Supervisor,
    events: mpsc::Receiver<SupervisorEvent>,
    extensions: HashMap<String, ActiveExtension>,
    callers: HashMap<ClientId, String>,
}

impl Automation {
    pub fn new() -> Self {
        let (sender, events) = mpsc::channel();
        let supervisor = Supervisor::start(move |event| {
            let _ = sender.send(event);
        });
        Self {
            supervisor,
            events,
            extensions: HashMap::new(),
            callers: HashMap::new(),
        }
    }

    pub fn reconcile(
        &mut self,
        prepared: Vec<PreparedDeployment>,
        instance: &str,
        session: &str,
    ) -> Vec<ClientId> {
        let keep: HashSet<_> = prepared.iter().map(|p| (&p.id, p.generation)).collect();
        let retired: Vec<_> = self
            .extensions
            .iter()
            .filter(|(id, active)| !keep.contains(&(id, active.generation)))
            .map(|(id, _)| id.clone())
            .collect();
        let mut revoked = HashSet::new();
        for id in retired {
            let active = self.extensions.remove(&id).expect("active extension");
            revoked.insert(active.credential);
            for service in active.services {
                let _ = self.supervisor.stop_service(service.name);
            }
        }
        let clients = self
            .callers
            .iter()
            .filter(|(_, credential)| revoked.contains(*credential))
            .map(|(client, _)| *client)
            .collect();
        self.callers
            .retain(|_, credential| !revoked.contains(credential));
        for definition in prepared {
            if !self.extensions.contains_key(&definition.id) {
                self.start(definition, instance, session);
            }
        }
        clients
    }

    fn start(&mut self, definition: PreparedDeployment, instance: &str, session: &str) {
        let mut random = [0u8; 32];
        if let Err(error) = getrandom::fill(&mut random) {
            eprintln!("rozi: cannot issue session credential: {error}");
            return;
        }
        let credential = format!(
            "{}{instance}:{}",
            crate::extension_runtime::SESSION_CREDENTIAL_PREFIX,
            crate::extension_runtime::bundle::hex_digest(&random)
        );
        let mut services = Vec::new();
        let settings = match definition.settings {
            Ok(settings) => settings,
            Err(error) => {
                // Started with the wrong settings is worse than stopped with a reason. The record
                // still takes this generation, so a later pass does not retry until one is due.
                let reason = format!("the host's config cannot be read: {error}");
                self.extensions.insert(
                    definition.id,
                    ActiveExtension {
                        generation: definition.generation,
                        credential,
                        services: definition
                            .services
                            .into_iter()
                            .map(|service| SessionServiceStatus {
                                name: service.name,
                                state: SessionServiceState::Stopped,
                                pid: None,
                                reason: Some(reason.clone()),
                            })
                            .collect(),
                    },
                );
                return;
            }
        };
        for service in definition.services {
            let mut env = service.env;
            env.insert("ROZI_EXTENSION".into(), definition.id.clone());
            env.insert(
                crate::config::GENERATION_ENV.into(),
                definition.generation.to_string(),
            );
            env.insert(crate::config::SETTINGS_ENV.into(), settings.clone());
            env.insert(crate::config::SESSION_NAME_ENV.into(), session.into());
            env.insert(
                crate::session::protocol::SESSION_INSTANCE_ENV.into(),
                instance.into(),
            );
            env.insert("ROZI_SERVICE".into(), service.name.clone());
            let result = self.supervisor.start_service(
                service.name.clone(),
                ServiceSpec {
                    process: ProcessSpec {
                        bundle_dir: definition.directory.clone(),
                        socket_path: Default::default(),
                        launch: Launch::from(&service.launch),
                        cwd: SpawnCwd::Bundle { path: service.cwd },
                        env: env.into_iter().collect(),
                        credential: credential.clone(),
                        platforms: definition.platforms.clone(),
                        capture: None,
                        unit: Some(format!("service.{}", service.name)),
                    },
                    restart: service.restart,
                },
            );
            services.push(SessionServiceStatus {
                name: service.name,
                state: if result.is_ok() {
                    SessionServiceState::Starting
                } else {
                    SessionServiceState::Stopped
                },
                pid: None,
                reason: result.err().map(|error| error.to_string()),
            });
        }
        self.extensions.insert(
            definition.id,
            ActiveExtension {
                generation: definition.generation,
                credential,
                services,
            },
        );
    }

    pub fn poll(&mut self) {
        for event in self.events.try_iter() {
            let (name, state, pid, reason) = match event {
                SupervisorEvent::Spawned { name, pid } => {
                    (name, SessionServiceState::Running, Some(pid), None)
                }
                SupervisorEvent::RestartScheduled { name, detail, .. } => {
                    (name, SessionServiceState::Restarting, None, Some(detail))
                }
                SupervisorEvent::Stopped { name, detail, .. } => {
                    (name, SessionServiceState::Stopped, None, Some(detail))
                }
            };
            if let Some(service) = self
                .extensions
                .values_mut()
                .flat_map(|extension| &mut extension.services)
                .find(|service| service.name == name)
            {
                service.state = state;
                service.pid = pid;
                service.reason = reason;
            }
        }
    }

    pub fn services(&self, id: &str) -> Vec<SessionServiceStatus> {
        self.extensions
            .get(id)
            .map_or_else(Vec::new, |extension| extension.services.clone())
    }

    fn authorize(
        &mut self,
        client_id: ClientId,
        request: &mut ControlRequest,
    ) -> Result<(), ControlResponse> {
        let Some(credential) = request.credential.as_ref() else {
            return Ok(());
        };
        if !credential.starts_with(crate::extension_runtime::SESSION_CREDENTIAL_PREFIX) {
            return Err(ControlResponse::error_with(
                ControlErrorCode::NotPermitted,
                "client-issued credentials are refused by session endpoints",
            ));
        }
        if !self
            .extensions
            .values()
            .any(|extension| &extension.credential == credential)
        {
            return Err(ControlResponse::error_with(
                ControlErrorCode::ExtensionInactive,
                "session credential is retired or unknown",
            ));
        }
        if !session_worker_permitted(&request.command) {
            return Err(ControlResponse::error_with(
                ControlErrorCode::NotPermitted,
                "session processes cannot run this command",
            ));
        }
        self.callers.insert(client_id, credential.clone());
        request.extension = None;
        Ok(())
    }

    pub fn forget_client(&mut self, client: ClientId) {
        self.callers.remove(&client);
    }
}

/// Every session-worker command also passes the session endpoint's exhaustive command gate.
fn session_worker_permitted(command: &ControlCommand) -> bool {
    match command {
        ControlCommand::Context
        | ControlCommand::ListPanes
        | ControlCommand::LayoutGet { .. }
        | ControlCommand::LayoutSet { .. }
        | ControlCommand::PaneSet { .. }
        | ControlCommand::PaneMove { .. }
        | ControlCommand::PaneSwap { .. }
        | ControlCommand::PaneClose { .. }
        | ControlCommand::AgentsList
        | ControlCommand::AgentGet { .. }
        | ControlCommand::AgentRead { .. }
        | ControlCommand::Metrics
        | ControlCommand::ExtensionsStatus
        | ControlCommand::SendText { .. }
        | ControlCommand::SendKeys { .. }
        | ControlCommand::NewPane { .. }
        | ControlCommand::CapturePane { .. }
        | ControlCommand::Subscribe { .. }
        | ControlCommand::PaneLogging { .. }
        | ControlCommand::Notify { .. }
        | ControlCommand::SetStatus { .. }
        | ControlCommand::AgentWait { .. }
        | ControlCommand::AgentPrompt { .. }
        | ControlCommand::AgentReport { .. }
        | ControlCommand::AgentRelease { .. }
        | ControlCommand::RecordStart { .. }
        | ControlCommand::RecordStop { .. }
        | ControlCommand::RecordList
        | ControlCommand::RecordMark { .. } => true,
        ControlCommand::ListAttachedPanes
        | ControlCommand::PaneReveal { .. }
        | ControlCommand::AgentsOverview
        | ControlCommand::AgentGo { .. }
        | ControlCommand::ExtensionRuntimeStatus
        | ControlCommand::ExtensionsDeploy { .. }
        | ControlCommand::ExtensionsUndeploy { .. }
        | ControlCommand::Focus { .. }
        | ControlCommand::RunAction { .. }
        | ControlCommand::CaptureUi { .. }
        | ControlCommand::SwitchWorkspace { .. }
        | ControlCommand::MoveToWorkspace { .. }
        | ControlCommand::Popup { .. }
        | ControlCommand::Publish { .. }
        | ControlCommand::SidebarPublish { .. }
        | ControlCommand::CommandVisibility { .. }
        | ControlCommand::RecordUiStart { .. }
        | ControlCommand::RecordUiStop
        | ControlCommand::RecordUiMark { .. }
        | ControlCommand::Pick { .. }
        | ControlCommand::Prompt { .. } => false,
    }
}

impl SessionServer {
    pub(super) fn authorize_session_worker(
        &mut self,
        client_id: ClientId,
        request: &mut ControlRequest,
    ) -> Result<(), ControlResponse> {
        if request.credential.is_none() {
            return Ok(());
        }
        self.automation
            .as_mut()
            .ok_or_else(|| {
                ControlResponse::error_with(
                    ControlErrorCode::ExtensionInactive,
                    "no session credential is active",
                )
            })?
            .authorize(client_id, request)
    }

    pub(super) fn activate_deployments(&mut self, prepared: Vec<PreparedDeployment>) {
        // The supervisor wakes twenty times a second, so a session with nothing to supervise goes
        // without one.
        if prepared.is_empty() && self.automation.is_none() {
            return;
        }
        let automation = self.automation.get_or_insert_with(Automation::new);
        let retired =
            automation.reconcile(prepared, &self.instance_id.to_string(), &self.session_name);
        for client in retired {
            self.remove_client(client);
        }
    }

    /// Stop every session process before returning, and close the connections they hold.
    pub(super) fn stop_automation(&mut self) {
        let Some(automation) = self.automation.take() else {
            return;
        };
        let clients: Vec<ClientId> = automation.callers.keys().copied().collect();
        drop(automation);
        for client in clients {
            self.remove_client(client);
        }
    }

    /// Bring deployments back up under a new generation, as a restart of the server does.
    pub(super) fn restart_deployments(&mut self) {
        if !crate::state::is_ephemeral_session_name(&self.session_name)
            && let Err(error) = self.ensure_deployment_worker(true)
        {
            eprintln!("rozi: session automation unavailable: {error}");
        }
    }
}
