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
use crate::control::{
    ACTION_PAYLOAD_BYTES, ActionConcurrency, ActionInfo, ControlCommand, ControlErrorCode,
    ControlResponse, InvocationInfo, InvocationResult,
};
use crate::extension_runtime::protocol::{Capture, Launch, SpawnCwd, SpawnFailure};
use crate::extension_runtime::supervisor::{ExitOutcome, ProcessSpec, SupervisedProcess};

/// Calls running at once across the whole server. A call past this fails rather than waits: a
/// nested call waiting on a slot its own ancestors hold would never get one.
const MAX_RUNNING: usize = 8;
/// Longest chain of actions calling actions, which also stops an action calling itself.
const MAX_DEPTH: u8 = 8;
const POLL: Duration = Duration::from_millis(10);
/// How long shutdown waits for running calls to kill their processes.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

type CallError = (ControlErrorCode, String);
type CallResult = Result<serde_json::Value, CallError>;

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

impl Running {
    fn called_by(&self, client: ClientId) -> bool {
        self.caller
            .as_ref()
            .is_some_and(|(owner, _)| *owner == client)
    }

    fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a call needs from the extension that declares it.
pub(super) struct CallTarget {
    pub extension: String,
    pub generation: u64,
    pub action: ActionInfo,
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

/// A call that has ended, with who hears about it and the credential that ended with it.
pub(super) struct EndedCall {
    pub caller: CallCaller,
    pub credential: String,
    pub response: ControlResponse,
}

pub(super) struct Invocations {
    running: HashMap<String, Running>,
    sender: mpsc::Sender<Finished>,
    finished: mpsc::Receiver<Finished>,
}

fn random_hex(bytes: usize) -> std::io::Result<String> {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).map_err(std::io::Error::other)?;
    Ok(crate::extension_runtime::bundle::hex_digest(&random))
}

/// How deep a call runs, given the call that made it.
fn nesting_depth(parent: Option<&Running>) -> Result<u8, ControlResponse> {
    let depth = parent.map_or(1, |parent| parent.info.depth.saturating_add(1));
    if depth > MAX_DEPTH {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvocationDepth,
            format!("actions may nest at most {MAX_DEPTH} calls deep"),
        ));
    }
    Ok(depth)
}

/// The shortest of what the action declares, what the caller asked for, and what its parent has left.
fn time_allowed(
    action: &ActionInfo,
    timeout_ms: Option<u64>,
    parent: Option<&Running>,
    now: Instant,
) -> Result<Duration, ControlResponse> {
    let declared = Duration::from_millis(action.deadline_ms);
    let allowed = timeout_ms.map_or(declared, |ms| declared.min(Duration::from_millis(ms)));
    let Some(parent) = parent else {
        return Ok(allowed);
    };
    let remaining = parent.deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvocationTimeout,
            "the calling action's deadline has passed",
        ));
    }
    Ok(allowed.min(remaining))
}

fn encode_args(args: &serde_json::Value) -> Result<Vec<u8>, ControlResponse> {
    let encoded = serde_json::to_vec(args).map_err(|error| {
        ControlResponse::error_with(ControlErrorCode::InvalidArguments, error.to_string())
    })?;
    if encoded.len() > ACTION_PAYLOAD_BYTES {
        return Err(ControlResponse::error_with(
            ControlErrorCode::InvalidArguments,
            format!("action arguments may be at most {ACTION_PAYLOAD_BYTES} bytes"),
        ));
    }
    Ok(encoded)
}

/// A new call's id, and the credential that proves a process belongs to it.
fn issue_identity(instance: &str) -> Result<(String, String), ControlResponse> {
    let issue = || -> std::io::Result<(String, String)> {
        let id = random_hex(16)?;
        let credential = format!(
            "{}{instance}:{}",
            crate::extension_runtime::SESSION_CREDENTIAL_PREFIX,
            random_hex(32)?
        );
        Ok((id, credential))
    };
    issue().map_err(|error| {
        ControlResponse::error(format!("cannot issue an invocation credential: {error}"))
    })
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
        self.try_start(target, request, instance).err()
    }

    fn try_start(
        &mut self,
        target: CallTarget,
        request: CallRequest,
        instance: &str,
    ) -> Result<(), ControlResponse> {
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
        let depth = nesting_depth(parent)?;
        let now = Instant::now();
        let allowed = time_allowed(&action, request.timeout_ms, parent, now)?;
        self.check_capacity(&action)?;
        let args = encode_args(&request.args)?;
        let (id, credential) = issue_identity(instance)?;
        spec.credential = credential.clone();
        spec.env
            .push((crate::config::ACTION_ENV.into(), action.id.clone()));
        spec.env
            .push((crate::config::INVOCATION_ENV.into(), id.clone()));
        let deadline = now + allowed;
        spec.capture = Some(Capture {
            // One past the limit tells a result that is too long from one that just fits.
            max_bytes: ACTION_PAYLOAD_BYTES + 1,
            // Not what ends the call: `run_call` holds it to `deadline`, which started earlier.
            timeout_ms: u64::try_from(allowed.as_millis()).unwrap_or(u64::MAX),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let info = InvocationInfo {
            invocation_id: id.clone(),
            action: action.id.clone(),
            generation,
            depth,
            deadline_unix_ms: unix_ms_after(allowed),
        };
        let sender = self.sender.clone();
        let thread_cancel = cancel.clone();
        let thread_id = id.clone();
        std::thread::Builder::new()
            .name("rozi-action".into())
            .spawn(move || {
                let result = run_call(spec, &action, &request.args, args, deadline, &thread_cancel);
                let _ = sender.send(Finished {
                    id: thread_id,
                    result,
                });
            })
            .map_err(|error| ControlResponse::error(format!("cannot start the action: {error}")))?;
        self.running.insert(
            id,
            Running {
                info,
                extension,
                deadline,
                cancel,
                credential,
                caller: Some((request.client, request.reply)),
            },
        );
        Ok(())
    }

    /// Refuse a call there is no room for. It fails rather than waits; see [`MAX_RUNNING`].
    fn check_capacity(&self, action: &ActionInfo) -> Result<(), ControlResponse> {
        if self.running.len() >= MAX_RUNNING {
            return Err(ControlResponse::error_with(
                ControlErrorCode::InvocationLimit,
                format!("this session already runs {MAX_RUNNING} action calls"),
            ));
        }
        let busy = action.concurrency == ActionConcurrency::Singleton
            && self
                .running
                .values()
                .any(|running| running.info.action == action.id);
        if busy {
            return Err(ControlResponse::error_with(
                ControlErrorCode::ActionBusy,
                format!("`{}` is already running", action.id),
            ));
        }
        Ok(())
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
        running.stop();
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
            running.stop();
        }
    }

    /// Cancel a departed caller's calls. Nobody is left to answer.
    pub fn abandon_client(&mut self, client: ClientId) {
        for running in self.running.values_mut() {
            if running.called_by(client) {
                running.caller = None;
                running.stop();
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
                running.stop();
                running.credential.clone()
            })
            .collect()
    }

    pub fn holds_reply(&self, client: ClientId) -> bool {
        self.running
            .values()
            .any(|running| running.called_by(client))
    }

    /// The calls that have ended since the last time this was asked.
    pub fn drain(&mut self) -> Vec<EndedCall> {
        self.finished
            .try_iter()
            .filter_map(|finished| {
                let running = self.running.remove(&finished.id)?;
                let response = match finished.result {
                    Ok(result) => ControlResponse::ok(InvocationResult {
                        invocation_id: finished.id,
                        result,
                    }),
                    Err((code, message)) => ControlResponse::error_with(code, message),
                };
                Some(EndedCall {
                    caller: running.caller,
                    credential: running.credential,
                    response,
                })
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
            running.stop();
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
) -> Result<(), CallError> {
    let validator = crate::config::action_schema_validator(schema)
        .map_err(|error| (ControlErrorCode::ActionFailed, error))?;
    validator
        .validate(value)
        .map_err(|error| (code, format!("{what} do not match the schema: {error}")))
}

fn cancelled() -> CallError {
    (
        ControlErrorCode::InvocationCancelled,
        "the call was cancelled".to_string(),
    )
}

/// Refuse to go on with a call that was cancelled or has passed its deadline.
fn check_live(
    action: &ActionInfo,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<(), CallError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    if Instant::now() >= deadline {
        return Err((
            ControlErrorCode::InvocationTimeout,
            format!("`{}` passed its deadline", action.id),
        ));
    }
    Ok(())
}

fn run_call(
    spec: ProcessSpec,
    action: &ActionInfo,
    args: &serde_json::Value,
    encoded_args: Vec<u8>,
    deadline: Instant,
    cancel: &AtomicBool,
) -> CallResult {
    check_schema(
        &action.args_schema,
        args,
        ControlErrorCode::InvalidArguments,
        "arguments",
    )?;
    let outcome = run_process(spec, action, encoded_args, deadline, cancel)?;
    let result = read_result(outcome, action)?;
    check_live(action, deadline, cancel)?;
    Ok(result)
}

/// Start the call's process and wait for it to end, or end it.
fn run_process(
    spec: ProcessSpec,
    action: &ActionInfo,
    encoded_args: Vec<u8>,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<ExitOutcome, CallError> {
    let spawn_failed =
        |failure: SpawnFailure| (ControlErrorCode::ActionFailed, failure.to_string());
    let prepared = SupervisedProcess::prepare(spec).map_err(spawn_failed)?;
    // After the bundle check, which is slow enough for a short deadline to pass during it.
    check_live(action, deadline, cancel)?;
    let mut process = prepared.launch().map_err(spawn_failed)?;
    // On its own thread: a process that never reads must still meet its deadline.
    if let Some(mut stdin) = process.take_stdin() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&encoded_args);
        });
    }
    let outcome = loop {
        if cancel.load(Ordering::Relaxed) {
            process.kill("stopped: cancelled");
            return Err(cancelled());
        }
        if let Some(outcome) = process.poll_until(deadline) {
            break outcome;
        }
        std::thread::sleep(POLL);
    };
    check_live(action, deadline, cancel)?;
    Ok(outcome)
}

/// The JSON result a finished process printed, checked against the action's schema.
fn read_result(outcome: ExitOutcome, action: &ActionInfo) -> CallResult {
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
    let Some(output) = outcome.output else {
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
                    return Some(ControlResponse::error_with(
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
                    None => ControlResponse::error_with(
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

/// How to start a process for a call: the manifest's command, from the bundle's root.
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
