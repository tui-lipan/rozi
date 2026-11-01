//! Session action calls: one process per call, run and judged off the session loop.
//!
//! The server owns everything about a call that a caller could otherwise lie about: which extension
//! and generation it belongs to, how deep it is nested, and when it must end. A process learns only
//! a credential; a nested call finds its parent's depth and deadline through that credential.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::headless::ReplyTo;
use super::{ClientId, SessionServer};
use crate::config::ExtensionAction;
use crate::control::{
    ACTION_PAYLOAD_BYTES, ActionConcurrency, ActionInfo, ControlCommand, ControlErrorCode,
    ControlResponse, InvocationInfo, InvocationResult,
};
use crate::extension_runtime::protocol::{Capture, Launch, SpawnCwd};
use crate::extension_runtime::supervisor::{ProcessSpec, SupervisedProcess};

/// Calls running at once across the whole server. A call past this fails rather than waits: a
/// nested call waiting on a slot its own ancestors hold would never get one.
const MAX_RUNNING: usize = 8;
/// Longest chain of actions calling actions, which also stops an action calling itself.
const MAX_DEPTH: u8 = 8;
const POLL: Duration = Duration::from_millis(10);
/// How long shutdown waits for running calls to kill their processes.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

type CallResult = Result<serde_json::Value, (ControlErrorCode, String)>;

/// Who hears a call's answer. `None` once that caller has gone.
pub(super) type CallCaller = Option<(ClientId, ReplyTo)>;

struct Finished {
    id: String,
    result: CallResult,
}

struct Running {
    info: InvocationInfo,
    extension: String,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    credential: String,
    caller: CallCaller,
}

/// What a call needs from the extension that declares it.
pub(super) struct CallTarget {
    pub extension: String,
    pub generation: u64,
    pub action: ExtensionAction,
    pub spec: ProcessSpec,
}

/// A request to call an action, as the server received it.
pub(super) struct CallRequest {
    pub client: ClientId,
    pub reply: ReplyTo,
    pub args: serde_json::Value,
    pub timeout_ms: Option<u64>,
    pub caller_credential: Option<String>,
}

pub(super) struct Invocations {
    running: HashMap<String, Running>,
    sender: mpsc::Sender<Finished>,
    finished: mpsc::Receiver<Finished>,
}

fn refusal(code: ControlErrorCode, message: impl Into<String>) -> ControlResponse {
    ControlResponse::error_with(code, message)
}

fn random_hex(bytes: usize) -> std::io::Result<String> {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).map_err(std::io::Error::other)?;
    Ok(crate::extension_runtime::bundle::hex_digest(&random))
}

impl Invocations {
    pub fn new() -> Self {
        let (sender, finished) = mpsc::channel();
        Self {
            running: HashMap::new(),
            sender,
            finished,
        }
    }

    /// Start a call, or say why it cannot start. `None` means the answer comes later.
    pub fn start(
        &mut self,
        target: CallTarget,
        request: CallRequest,
        instance: &str,
    ) -> Option<ControlResponse> {
        let CallTarget {
            extension,
            generation,
            action,
            mut spec,
        } = target;
        let parent = request
            .caller_credential
            .as_deref()
            .and_then(|credential| self.by_credential(credential));
        let depth = parent.map_or(1, |parent| parent.info.depth.saturating_add(1));
        if depth > MAX_DEPTH {
            return Some(refusal(
                ControlErrorCode::InvocationDepth,
                format!("actions may nest at most {MAX_DEPTH} calls deep"),
            ));
        }
        let now = Instant::now();
        let mut allowed = Duration::from_millis(action.info.deadline_ms);
        if let Some(timeout_ms) = request.timeout_ms {
            allowed = allowed.min(Duration::from_millis(timeout_ms));
        }
        if let Some(parent) = parent {
            let remaining = parent.deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                return Some(refusal(
                    ControlErrorCode::InvocationTimeout,
                    "the calling action's deadline has passed",
                ));
            }
            allowed = allowed.min(remaining);
        }
        if self.running.len() >= MAX_RUNNING {
            return Some(refusal(
                ControlErrorCode::InvocationLimit,
                format!("this session already runs {MAX_RUNNING} action calls"),
            ));
        }
        if action.info.concurrency == ActionConcurrency::Singleton
            && self
                .running
                .values()
                .any(|running| running.info.action == action.info.id)
        {
            return Some(refusal(
                ControlErrorCode::ActionBusy,
                format!("`{}` is already running", action.info.id),
            ));
        }
        let args = match serde_json::to_vec(&request.args) {
            Ok(args) if args.len() <= ACTION_PAYLOAD_BYTES => args,
            Ok(_) => {
                return Some(refusal(
                    ControlErrorCode::InvalidArguments,
                    format!("action arguments may be at most {ACTION_PAYLOAD_BYTES} bytes"),
                ));
            }
            Err(error) => {
                return Some(refusal(
                    ControlErrorCode::InvalidArguments,
                    error.to_string(),
                ));
            }
        };
        let (id, secret) = match (random_hex(16), random_hex(32)) {
            (Ok(id), Ok(secret)) => (id, secret),
            (Err(error), _) | (_, Err(error)) => {
                return Some(ControlResponse::error(format!(
                    "cannot issue an invocation credential: {error}"
                )));
            }
        };
        let credential = format!(
            "{}{instance}:{secret}",
            crate::extension_runtime::SESSION_CREDENTIAL_PREFIX
        );
        spec.credential = credential.clone();
        spec.env
            .push((crate::config::ACTION_ENV.into(), action.info.id.clone()));
        spec.env
            .push((crate::config::INVOCATION_ENV.into(), id.clone()));
        spec.capture = Some(Capture {
            // One past the limit tells a result that is too long from one that just fits.
            max_bytes: ACTION_PAYLOAD_BYTES + 1,
            timeout_ms: u64::try_from(allowed.as_millis()).unwrap_or(u64::MAX),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let info = InvocationInfo {
            invocation_id: id.clone(),
            action: action.info.id.clone(),
            generation,
            depth,
            deadline_unix_ms: unix_ms_after(allowed),
        };
        let sender = self.sender.clone();
        let thread_cancel = cancel.clone();
        let thread_id = id.clone();
        let schemas = action.info.clone();
        let spawned = std::thread::Builder::new()
            .name("rozi-action".into())
            .spawn(move || {
                let result = run_call(spec, &schemas, &request.args, args, &thread_cancel);
                let _ = sender.send(Finished {
                    id: thread_id,
                    result,
                });
            });
        if let Err(error) = spawned {
            return Some(ControlResponse::error(format!(
                "cannot start the action: {error}"
            )));
        }
        self.running.insert(
            id,
            Running {
                info,
                extension,
                deadline: now + allowed,
                cancel,
                credential,
                caller: Some((request.client, request.reply)),
            },
        );
        None
    }

    /// The live call holding `credential`. A cancelled call's credential is already dead, however
    /// long the call takes to finish ending.
    fn by_credential(&self, credential: &str) -> Option<&Running> {
        self.running.values().find(|running| {
            running.credential == credential && !running.cancel.load(Ordering::Relaxed)
        })
    }

    pub fn knows_credential(&self, credential: &str) -> bool {
        self.by_credential(credential).is_some()
    }

    pub fn list(&self) -> Vec<InvocationInfo> {
        let mut calls: Vec<_> = self
            .running
            .values()
            .map(|running| running.info.clone())
            .collect();
        calls.sort_by(|a, b| a.invocation_id.cmp(&b.invocation_id));
        calls
    }

    pub fn cancel(&self, invocation_id: &str) -> Option<InvocationInfo> {
        let running = self.running.get(invocation_id)?;
        running.cancel.store(true, Ordering::Relaxed);
        Some(running.info.clone())
    }

    /// Cancel the call a client started as attached request `request_id`, if it is still running.
    pub fn cancel_attached(&self, client: ClientId, request_id: u64) {
        let started_it = |running: &&Running| {
            running.caller.as_ref().is_some_and(|(owner, reply)| {
                *owner == client
                    && matches!(reply, ReplyTo::Attached { request_id: id } if *id == request_id)
            })
        };
        if let Some(running) = self.running.values().find(started_it) {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Cancel a departed caller's calls. Nobody is left to answer.
    pub fn abandon_client(&mut self, client: ClientId) {
        for running in self.running.values_mut() {
            if running
                .caller
                .as_ref()
                .is_some_and(|(owner, _)| *owner == client)
            {
                running.caller = None;
                running.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Cancel an extension's calls as its generation retires, returning the credentials to revoke.
    pub fn retire_extension(&mut self, extension: &str, generation: u64) -> Vec<String> {
        self.running
            .values()
            .filter(|running| {
                running.extension == extension && running.info.generation == generation
            })
            .map(|running| {
                running.cancel.store(true, Ordering::Relaxed);
                running.credential.clone()
            })
            .collect()
    }

    pub fn holds_reply(&self, client: ClientId) -> bool {
        self.running.values().any(|running| {
            running
                .caller
                .as_ref()
                .is_some_and(|(owner, _)| *owner == client)
        })
    }

    /// Calls that have ended, each with its caller and the credential that ended with it.
    pub fn drain(&mut self) -> Vec<(CallCaller, String, ControlResponse)> {
        let finished: Vec<_> = self.finished.try_iter().collect();
        finished
            .into_iter()
            .filter_map(|finished| {
                let running = self.running.remove(&finished.id)?;
                let response = match finished.result {
                    Ok(result) => ControlResponse::ok(InvocationResult {
                        invocation_id: finished.id,
                        result,
                    }),
                    Err((code, message)) => ControlResponse::error_with(code, message),
                };
                Some((running.caller, running.credential, response))
            })
            .collect()
    }

    /// Stop every call, as [`Self::shut_down`] does, and return who is waiting to hear about them.
    pub fn stop_all(&mut self) -> Vec<(ClientId, ReplyTo)> {
        let callers = self
            .running
            .values_mut()
            .filter_map(|running| running.caller.take())
            .collect();
        self.shut_down();
        callers
    }

    /// Stop every call and give them a moment to kill their processes. A call stuck past that is
    /// left behind: shutting a session down must not depend on what an extension started.
    pub fn shut_down(&mut self) {
        for running in self.running.values() {
            running.cancel.store(true, Ordering::Relaxed);
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while !self.running.is_empty() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            match self.finished.recv_timeout(left) {
                Ok(finished) => {
                    self.running.remove(&finished.id);
                }
                Err(_) => return,
            }
        }
    }
}

fn unix_ms_after(duration: Duration) -> u64 {
    SystemTime::now()
        .checked_add(duration)
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .and_then(|at| u64::try_from(at.as_millis()).ok())
        .unwrap_or(u64::MAX)
}

fn check_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    code: ControlErrorCode,
    what: &str,
) -> Result<(), (ControlErrorCode, String)> {
    let validator = jsonschema::options()
        .offline()
        .build(schema)
        .map_err(|error| (ControlErrorCode::ActionFailed, error.to_string()))?;
    validator
        .validate(value)
        .map_err(|error| (code, format!("{what} do not match the schema: {error}")))
}

fn run_call(
    spec: ProcessSpec,
    action: &ActionInfo,
    args: &serde_json::Value,
    encoded_args: Vec<u8>,
    cancel: &AtomicBool,
) -> CallResult {
    check_schema(
        &action.args_schema,
        args,
        ControlErrorCode::InvalidArguments,
        "arguments",
    )?;
    let mut process = SupervisedProcess::spawn(spec)
        .map_err(|failure| (ControlErrorCode::ActionFailed, failure.to_string()))?;
    // On its own thread: a process that never reads must still meet its deadline.
    if let Some(mut stdin) = process.take_stdin() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&encoded_args);
        });
    }
    let outcome = loop {
        if cancel.load(Ordering::Relaxed) {
            process.kill("stopped: cancelled");
            return Err((
                ControlErrorCode::InvocationCancelled,
                "the call was cancelled".to_string(),
            ));
        }
        if let Some(outcome) = process.poll() {
            break outcome;
        }
        std::thread::sleep(POLL);
    };
    if outcome.timed_out {
        return Err((
            ControlErrorCode::InvocationTimeout,
            format!("`{}` passed its deadline", action.id),
        ));
    }
    if !outcome.is_success() {
        return Err((
            ControlErrorCode::ActionFailed,
            format!(
                "`{}` {}; its log has the details",
                action.id,
                outcome.describe()
            ),
        ));
    }
    let Some(output) = outcome.output_bytes else {
        return Err((
            ControlErrorCode::ActionFailed,
            format!("`{}` left a process holding its output open", action.id),
        ));
    };
    if output.len() > ACTION_PAYLOAD_BYTES {
        return Err((
            ControlErrorCode::InvalidResult,
            format!("an action result may be at most {ACTION_PAYLOAD_BYTES} bytes"),
        ));
    }
    let result: serde_json::Value = serde_json::from_slice(&output).map_err(|error| {
        (
            ControlErrorCode::InvalidResult,
            format!("`{}` did not print one JSON value: {error}", action.id),
        )
    })?;
    check_schema(
        &action.result_schema,
        &result,
        ControlErrorCode::InvalidResult,
        "results",
    )?;
    Ok(result)
}

impl SessionServer {
    pub(super) fn submit_invocation(
        &mut self,
        client: ClientId,
        reply: ReplyTo,
        command: ControlCommand,
        caller_credential: Option<String>,
    ) -> Option<ControlResponse> {
        let instance = self.instance_id.to_string();
        let session = self.session_name.clone();
        let automation = self.automation.as_mut();
        match command {
            ControlCommand::Invoke {
                action,
                args,
                timeout_ms,
            } => {
                let Some(automation) = automation else {
                    return Some(refusal(
                        ControlErrorCode::ActionNotFound,
                        format!("no deployed action `{action}`"),
                    ));
                };
                automation.invoke(
                    &action,
                    CallRequest {
                        client,
                        reply,
                        args,
                        timeout_ms,
                        caller_credential,
                    },
                    &instance,
                    &session,
                )
            }
            ControlCommand::InvocationList => Some(ControlResponse::ok(
                automation.map_or_else(Vec::new, |automation| automation.invocations().list()),
            )),
            ControlCommand::InvocationCancel { invocation_id } => Some(
                match automation
                    .and_then(|automation| automation.invocations().cancel(&invocation_id))
                {
                    Some(info) => ControlResponse::ok(info),
                    None => refusal(
                        ControlErrorCode::InvocationNotFound,
                        format!("no running invocation `{invocation_id}`"),
                    ),
                },
            ),
            _ => unreachable!("only invocation commands are submitted"),
        }
    }

    pub(super) fn holds_invocation_reply(&self, client: ClientId) -> bool {
        self.automation
            .as_ref()
            .is_some_and(|automation| automation.invocations().holds_reply(client))
    }

    pub(super) fn action_infos(&self) -> Vec<ActionInfo> {
        self.automation
            .as_ref()
            .map_or_else(Vec::new, |automation| automation.action_infos())
    }

    pub(super) fn drain_invocation_results(&mut self) {
        let Some(automation) = &mut self.automation else {
            return;
        };
        let (answers, departed) = automation.drain_invocations();
        for (caller, response) in answers {
            if let Some((client, reply)) = caller {
                self.answer_held(client, &reply, response);
            }
        }
        for client in departed {
            self.remove_client(client);
        }
    }
}

/// How a process names itself to a call: `Launch` from the manifest, run from the bundle's root.
pub(super) fn call_spec(
    directory: std::path::PathBuf,
    platforms: Vec<String>,
    launch: &crate::config::LaunchTemplate,
    env: Vec<(String, String)>,
    unit: String,
) -> ProcessSpec {
    ProcessSpec {
        bundle_dir: directory,
        socket_path: Default::default(),
        launch: Launch::from(launch),
        cwd: SpawnCwd::Bundle { path: ".".into() },
        env,
        credential: String::new(),
        platforms,
        capture: None,
        unit: Some(unit),
    }
}
