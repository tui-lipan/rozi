//! Running placed extension contributions.
//!
//! A contribution placed on a session host runs through that host's extension runtime: this
//! machine's in-process one, or one this client starts over SSH. This module decides which
//! instances of placed services should exist for the sessions attached right now, starts and
//! supervises them, runs placed commands and sidebar listings, and turns everything a runtime
//! reports into what the user sees.
//!
//! It never falls back. A contribution placed on a host that cannot run it is reported as
//! unavailable, with the reason, and does not run on this client instead: something meant for a
//! server must not quietly run on a laptop.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{
    ExtensionProvenance, LaunchTemplate, NamedCommand, PlacedService, Placement, ServiceRestart,
    SharedPlacements,
};
use crate::extension_runtime::client::{RuntimeConnection, RuntimeEvent};
use crate::extension_runtime::protocol::{Capture, Message, SpawnCwd, SpawnFailure};
use crate::ops::services::{INITIAL_BACKOFF, MAX_FAILURES, UPTIME_RESET_SECS, next_backoff};
use crate::state::{
    HostKey, HostRuntime, HostRuntimeStatus, InstanceKey, InstanceStatus, PendingLaunch,
    PlacedInstance, State, Unavailable, WorkerBinding, WorkerId, WorkerKind, WorkerProcess,
    WorkerProcessState, WorkerPurpose, WorkerRuntime,
};

/// Delay before reconnecting a runtime whose connection failed or dropped, doubling per failure.
fn reconnect_delay(failures: u32) -> Duration {
    Duration::from_millis((500_u64 << failures.min(6)).min(30_000))
}

/// What one launch needs, wherever it runs.
struct LaunchSpec {
    placements: SharedPlacements,
    placement: Placement,
    binding: WorkerBinding,
    kind: WorkerKind,
    purpose: WorkerPurpose,
    launch: LaunchTemplate,
    cwd: SpawnCwd,
    env: Vec<(String, String)>,
    capture: Option<Capture>,
}

/// The hosts with a session attached right now, foreground or background, and each attached
/// session with its host.
fn attached(state: &State) -> (Vec<HostKey>, Vec<WorkerBinding>) {
    let mut hosts = Vec::new();
    let mut sessions = Vec::new();
    for attachment in std::iter::once(state.current()).chain(state.background.values()) {
        // A session whose link is reconnecting is still this client's: tearing down its host's
        // runtime over a blip would restart every placed process there. Same rule the host
        // monitor uses for a held host.
        let held = attachment.session_attached
            || attachment.pending_session_attach.is_some() && attachment.session_instance.is_some()
            || matches!(
                attachment.connection,
                crate::state::ConnectionState::Reconnecting
                    | crate::state::ConnectionState::AuthRequired
            ) && attachment.session_instance.is_some();
        if !held {
            continue;
        }
        let host = HostKey::of(attachment.remote_target.as_ref());
        if !hosts.contains(&host) {
            hosts.push(host.clone());
        }
        sessions.push(WorkerBinding {
            host,
            session: attachment.session_instance.clone(),
        });
    }
    (hosts, sessions)
}

/// The binding of the session on screen, if one is attached.
fn active_binding(state: &State) -> Option<WorkerBinding> {
    crate::ops::extension_workers::current_scope(state)
        .map(|(host, session)| WorkerBinding { host, session })
}

/// Every placed service instance that should exist now.
fn desired(state: &State) -> Vec<(InstanceKey, SharedPlacements, PlacedService)> {
    let (hosts, sessions) = attached(state);
    let active = active_binding(state);
    let mut out = Vec::new();
    for placements in state.config.extension_placements.values() {
        for service in &placements.services {
            let bindings: Vec<WorkerBinding> = match service.placement {
                Placement::Client => Vec::new(),
                Placement::ActiveSession => active.iter().cloned().collect(),
                Placement::EachHost => hosts
                    .iter()
                    .map(|host| WorkerBinding {
                        host: host.clone(),
                        session: None,
                    })
                    .collect(),
                Placement::EachSession => sessions.clone(),
            };
            for binding in bindings {
                out.push((
                    InstanceKey {
                        service: service.name.clone(),
                        binding,
                    },
                    placements.clone(),
                    service.clone(),
                ));
            }
        }
    }
    out
}

fn signature(state: &State) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut generations: Vec<_> = state.extension_generations.iter().collect();
    generations.sort();
    generations.hash(&mut hasher);
    for (id, placements) in &state.config.extension_placements {
        id.hash(&mut hasher);
        placements.bundle_digest().hash(&mut hasher);
    }
    let (_, sessions) = attached(state);
    sessions.hash(&mut hasher);
    active_binding(state).hash(&mut hasher);
    hasher.finish()
}

/// Reconcile after every message. Cheap when nothing changed: one hash of the attachments and
/// generations, and a clock comparison for a due restart.
pub(crate) fn sync(ctx: &mut Context<AppRoot>) {
    // Runtimes report through the command link; until the app has one there is nothing to start
    // them with, and the first sync after it arrives does the work.
    if ctx.state.command_link.is_none() {
        return;
    }
    let now = Instant::now();
    let due = ctx
        .state
        .extension_runtime
        .wake_at
        .is_some_and(|at| at <= now);
    let signature = signature(&ctx.state);
    if !due && ctx.state.extension_runtime.signature == Some(signature) {
        return;
    }
    let changed = ctx.state.extension_runtime.signature != Some(signature);
    ctx.state.extension_runtime.signature = Some(signature);
    if due {
        ctx.state.extension_runtime.wake_at = None;
    }
    reconcile(ctx, changed);
}

pub(crate) fn tick(ctx: &mut Context<AppRoot>) -> Update {
    sync(ctx);
    Update::none()
}

/// `changed` is set when the extensions or the attached sessions changed, rather than only a timer
/// coming due: that is when an instance a host could not run is worth trying again - the user may
/// have installed what was missing and reloaded.
fn reconcile(ctx: &mut Context<AppRoot>, changed: bool) {
    let wanted = desired(&ctx.state);
    let wanted_keys: HashSet<&InstanceKey> = wanted.iter().map(|(key, _, _)| key).collect();

    // Instances nobody wants any more, or of a retired generation, stop.
    let generations = ctx.state.extension_generations.clone();
    let mut stale = Vec::new();
    ctx.state.extension_runtime.instances.retain(|instance| {
        let keep = wanted_keys.contains(&instance.key)
            && generations.get(&instance.extension) == Some(&instance.generation);
        if !keep {
            stale.push(instance.status.clone());
        }
        keep
    });
    for status in stale {
        if let InstanceStatus::Running { worker } = status {
            kill_worker(&mut ctx.state, worker);
        }
    }

    let now = Instant::now();
    let mut next_wake: Option<Instant> = None;
    for (key, placements, service) in wanted {
        let existing = ctx
            .state
            .extension_runtime
            .instances
            .iter()
            .position(|instance| instance.key == key);
        let start = match existing.map(|index| &ctx.state.extension_runtime.instances[index]) {
            None => true,
            Some(instance) => match &instance.status {
                InstanceStatus::Running { .. } | InstanceStatus::Stopped { .. } => false,
                InstanceStatus::Restarting { at } => {
                    if *at <= now {
                        true
                    } else {
                        next_wake = Some(next_wake.map_or(*at, |wake| wake.min(*at)));
                        false
                    }
                }
                // A host that could not run it may have become able to: a runtime that came back,
                // a reload after installing a missing program. Anything this host's platform rules
                // out stays out until the extension changes.
                InstanceStatus::Unavailable(reason) => match reason {
                    Unavailable::UnsupportedPlatform { .. } => false,
                    Unavailable::RuntimeUnreachable { .. } => {
                        host_retry_due(&ctx.state, &key.binding.host, now, &mut next_wake)
                    }
                    _ => changed,
                },
            },
        };
        if start {
            start_instance(ctx, key, placements, &service);
        }
    }

    // A runtime belongs to this client's attachment to its host. Once no session there is
    // attached, or nothing placed is loaded at all, it closes, and with it everything it runs.
    let (hosts, _) = attached(&ctx.state);
    let nothing_placed = ctx.state.config.extension_placements.is_empty();
    let idle: Vec<HostKey> = ctx
        .state
        .extension_runtime
        .hosts
        .keys()
        .filter(|host| nothing_placed || !hosts.contains(host))
        .cloned()
        .collect();
    for host in idle {
        close_runtime(&mut ctx.state, &host);
    }

    for host in ctx.state.extension_runtime.hosts.values() {
        if let Some(at) = host.retry_at {
            next_wake = Some(next_wake.map_or(at, |wake| wake.min(at)));
        }
    }
    arm_wake(&mut ctx.state, next_wake);
}

fn host_retry_due(
    state: &State,
    host: &HostKey,
    now: Instant,
    next_wake: &mut Option<Instant>,
) -> bool {
    match state.extension_runtime.hosts.get(host) {
        None => true,
        Some(runtime) => match (&runtime.status, runtime.retry_at) {
            // A launch on a runtime still connecting waits for it to be ready. Every instance the
            // host lost is relaunched, not only the first, whose launch started the reconnect.
            (HostRuntimeStatus::Ready { .. } | HostRuntimeStatus::Connecting, _) => true,
            (HostRuntimeStatus::Unavailable(_), Some(at)) if at <= now => true,
            (HostRuntimeStatus::Unavailable(_), Some(at)) => {
                *next_wake = Some(next_wake.map_or(at, |wake| wake.min(at)));
                false
            }
            _ => false,
        },
    }
}

fn arm_wake(state: &mut State, at: Option<Instant>) {
    let Some(at) = at else {
        return;
    };
    if state
        .extension_runtime
        .wake_at
        .is_some_and(|armed| armed <= at)
    {
        return;
    }
    state.extension_runtime.wake_at = Some(at);
    // A real timer, because the deadlines it serves are wall-clock `Instant`s: restart backoff and
    // reconnect delay are measured against the processes and connections they wait on.
    if let Some(link) = state.command_link.clone() {
        let delay = at.saturating_duration_since(Instant::now());
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            link.send(crate::Msg::PlacementTick);
        });
    }
}

fn start_instance(
    ctx: &mut Context<AppRoot>,
    key: InstanceKey,
    placements: SharedPlacements,
    service: &PlacedService,
) {
    let generation = ctx
        .state
        .extension_generations
        .get(&placements.id)
        .cloned()
        .unwrap_or_default();
    let mut env: Vec<(String, String)> = service
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    env.push((
        crate::config::GENERATION_ENV.to_string(),
        generation.clone(),
    ));
    env.push(("ROZI_SERVICE".to_string(), service.name.clone()));
    let spec = LaunchSpec {
        placements: placements.clone(),
        placement: service.placement,
        binding: key.binding.clone(),
        kind: WorkerKind::Service {
            name: service.name.clone(),
        },
        purpose: WorkerPurpose::Service { key: key.clone() },
        launch: service.launch.clone(),
        cwd: SpawnCwd::Bundle {
            path: service.cwd.clone(),
        },
        env,
        capture: None,
    };
    let status = match launch(&mut ctx.state, spec) {
        Ok(worker) => InstanceStatus::Running { worker },
        Err(reason) => InstanceStatus::Unavailable(reason),
    };
    let index = ctx
        .state
        .extension_runtime
        .instances
        .iter()
        .position(|instance| instance.key == key);
    match index {
        Some(index) => {
            let instance = &mut ctx.state.extension_runtime.instances[index];
            instance.status = status;
            instance.generation = generation;
            if matches!(instance.status, InstanceStatus::Running { .. }) {
                instance.started_at = Some(Instant::now());
            }
        }
        None => {
            ctx.state.extension_runtime.instances.push(PlacedInstance {
                key: key.clone(),
                extension: placements.id.clone(),
                generation,
                started_at: matches!(status, InstanceStatus::Running { .. }).then(Instant::now),
                status,
                failures: 0,
                backoff: INITIAL_BACKOFF,
                reported: None,
            });
        }
    }
    report_unavailable(ctx, &key);
}

/// Tell the user why an instance is not running, once per reason: a host that stays unreachable
/// across reconnect attempts is one problem, not one per attempt.
fn report_unavailable(ctx: &mut Context<AppRoot>, key: &InstanceKey) {
    let Some(instance) = ctx
        .state
        .extension_runtime
        .instances
        .iter_mut()
        .find(|instance| &instance.key == key)
    else {
        return;
    };
    let InstanceStatus::Unavailable(reason) = instance.status.clone() else {
        return;
    };
    if instance.reported == Some(reason.code()) {
        return;
    }
    instance.reported = Some(reason.code());
    crate::pane::pty_events::notify_error(
        ctx,
        "Extension service unavailable",
        format!(
            "`{}` on {}: {reason}",
            key.service,
            key.binding.host.label()
        ),
    );
}

/// Start a placed process through its host's runtime.
fn launch(state: &mut State, spec: LaunchSpec) -> std::result::Result<WorkerId, Unavailable> {
    let host = spec.binding.host.clone();
    if host == HostKey::Local
        && let Some(reason) = &spec.placements.client_unsupported
    {
        return Err(Unavailable::UnsupportedPlatform {
            os: format!("{} ({reason})", std::env::consts::OS),
        });
    }
    let epoch = ensure_runtime(state, &host)?;
    let generation = state
        .extension_generations
        .get(&spec.placements.id)
        .cloned()
        .ok_or_else(|| Unavailable::SpawnFailed {
            detail: "the extension is no longer active".to_string(),
        })?;
    let worker = state.extension_workers.issue(
        ExtensionProvenance {
            id: spec.placements.id.clone(),
            generation,
        },
        spec.placement,
        spec.binding,
        WorkerRuntime(epoch),
        spec.kind,
    );
    let (id, credential) = (worker.id, worker.credential().to_string());
    let digest = spec.placements.bundle_digest().to_string();
    let message = Message::Spawn {
        worker: id.0,
        digest: digest.clone(),
        launch: (&spec.launch).into(),
        cwd: spec.cwd,
        env: spec.env,
        credential,
        platforms: spec.placements.platforms.clone(),
        capture: spec.capture,
    };
    state.extension_runtime.processes.insert(
        id,
        WorkerProcess {
            host: host.clone(),
            purpose: spec.purpose,
            digest: digest.clone(),
            state: WorkerProcessState::Pending,
        },
    );
    state.extension_runtime.pending.push(PendingLaunch {
        worker: id,
        digest,
        message,
        restaged: false,
        bundle: spec.placements.bundle.clone(),
    });
    flush(state, &host);
    Ok(id)
}

/// The current epoch of `host`'s runtime, starting one if there is none. A runtime that failed is
/// retried only once its backoff has passed; until then its reason is the answer.
fn ensure_runtime(state: &mut State, host: &HostKey) -> std::result::Result<u64, Unavailable> {
    let now = Instant::now();
    if let Some(runtime) = state.extension_runtime.hosts.get(host) {
        match (&runtime.status, runtime.retry_at) {
            (HostRuntimeStatus::Unavailable(_), Some(at)) if at <= now => {}
            (HostRuntimeStatus::Unavailable(reason), _) => return Err(reason.clone()),
            _ => return Ok(runtime.epoch),
        }
    }
    let Some(link) = state.command_link.clone() else {
        return Err(Unavailable::RuntimeUnreachable {
            detail: "the client is still starting".to_string(),
        });
    };
    state.extension_runtime.next_epoch += 1;
    let epoch = state.extension_runtime.next_epoch;
    let failures = state
        .extension_runtime
        .hosts
        .get(host)
        .map_or(0, |runtime| runtime.failures);
    let connection = RuntimeConnection::start(
        host.clone(),
        epoch,
        state.config.remote.clone(),
        link,
        state.event_hub.clone(),
    );
    state.extension_runtime.hosts.insert(
        host.clone(),
        HostRuntime {
            host: host.clone(),
            epoch,
            status: HostRuntimeStatus::Connecting,
            connection: Some(connection),
            staged: HashSet::new(),
            staging: HashSet::new(),
            failures,
            retry_at: None,
        },
    );
    Ok(epoch)
}

/// Send whatever is waiting on `host` that can go now: bundles to stage, then spawns whose bundle
/// is staged.
fn flush(state: &mut State, host: &HostKey) {
    let Some(runtime) = state.extension_runtime.hosts.get_mut(host) else {
        return;
    };
    if !matches!(runtime.status, HostRuntimeStatus::Ready { .. }) {
        return;
    }
    let Some(connection) = runtime.connection.clone() else {
        return;
    };
    let processes = &mut state.extension_runtime.processes;
    let sent = &mut state.extension_runtime.sent;
    // A write that fails means the channel is gone; the connection's own report of that takes
    // every process on this host down with it, so nothing is failed here twice.
    state.extension_runtime.pending.retain(|pending| {
        let Some(process) = processes.get_mut(&pending.worker) else {
            return false;
        };
        if &process.host != host {
            return true;
        }
        if runtime.staged.contains(&pending.digest) {
            let _ = connection.send(&pending.message);
            process.state = WorkerProcessState::Starting;
            sent.insert(pending.worker, pending.message.clone());
            return false;
        }
        if runtime.staging.insert(pending.digest.clone()) {
            let _ = connection.stage(&pending.bundle);
        }
        true
    });
}

/// Stop a worker and revoke its credential. Whatever it sends after this is refused, even before
/// the runtime confirms it is gone.
fn kill_worker(state: &mut State, worker: WorkerId) {
    state.extension_workers.revoke(worker);
    state
        .extension_runtime
        .pending
        .retain(|pending| pending.worker != worker);
    let Some(process) = state.extension_runtime.processes.remove(&worker) else {
        return;
    };
    if process.state == WorkerProcessState::Pending {
        return;
    }
    if let Some(connection) = state
        .extension_runtime
        .hosts
        .get(&process.host)
        .and_then(|runtime| runtime.connection.clone())
    {
        let _ = connection.send(&Message::Kill { worker: worker.0 });
    }
}

fn close_runtime(state: &mut State, host: &HostKey) {
    let Some(runtime) = state.extension_runtime.hosts.remove(host) else {
        return;
    };
    let epoch = runtime.epoch;
    state
        .extension_workers
        .revoke_where(|worker| worker.runtime == WorkerRuntime(epoch));
    if let Some(connection) = runtime.connection {
        connection.close();
    }
    let gone: Vec<WorkerId> = state
        .extension_runtime
        .processes
        .iter()
        .filter(|(_, process)| &process.host == host)
        .map(|(worker, _)| *worker)
        .collect();
    for worker in gone {
        state.extension_runtime.processes.remove(&worker);
    }
    let processes = &state.extension_runtime.processes;
    state
        .extension_runtime
        .pending
        .retain(|pending| processes.contains_key(&pending.worker));
}

/// A generation retired - reload with a changed definition, disable, or removal. Every worker of
/// those extensions is stopped and its credential revoked; the next reconcile starts replacements
/// for whatever is still wanted, under the new generation and its own bundle.
pub(crate) fn retire(ctx: &mut Context<AppRoot>, extensions: &HashSet<String>) {
    if extensions.is_empty() {
        return;
    }
    let workers: Vec<WorkerId> = ctx
        .state
        .extension_workers
        .iter()
        .filter(|worker| extensions.contains(&worker.extension.id))
        .map(|worker| worker.id)
        .collect();
    for worker in workers {
        kill_worker(&mut ctx.state, worker);
    }
    ctx.state
        .extension_runtime
        .instances
        .retain(|instance| !extensions.contains(&instance.extension));
    ctx.state.extension_runtime.signature = None;
}

/// Close every runtime: the client is leaving. Each runtime stops what it started when its
/// channel closes.
pub(crate) fn shutdown_all(state: &mut State) {
    let hosts: Vec<HostKey> = state.extension_runtime.hosts.keys().cloned().collect();
    for host in hosts {
        close_runtime(state, &host);
    }
    state.extension_runtime.instances.clear();
}

pub(crate) fn runtime_event(
    ctx: &mut Context<AppRoot>,
    host: HostKey,
    epoch: u64,
    event: RuntimeEvent,
) -> Update {
    if ctx
        .state
        .extension_runtime
        .hosts
        .get(&host)
        .is_none_or(|runtime| runtime.epoch != epoch)
    {
        return Update::none();
    }
    let mut update = Update::none();
    match event {
        RuntimeEvent::Ready(hello) => {
            let runtime = ctx
                .state
                .extension_runtime
                .hosts
                .get_mut(&host)
                .expect("checked above");
            runtime.status = HostRuntimeStatus::Ready {
                os: hello.os.unwrap_or_default(),
                version: hello.version,
            };
            runtime.failures = 0;
            runtime.retry_at = None;
            flush(&mut ctx.state, &host);
        }
        RuntimeEvent::Failed { reason, retry } => runtime_down(ctx, &host, reason, retry),
        RuntimeEvent::Lost(detail) => {
            runtime_down(ctx, &host, Unavailable::RuntimeUnreachable { detail }, true)
        }
        RuntimeEvent::Message(message) => {
            if let Some(connection) = ctx
                .state
                .extension_runtime
                .hosts
                .get(&host)
                .and_then(|runtime| runtime.connection.as_ref())
            {
                connection.delivered();
            }
            match ownership(&ctx.state, &host, epoch, &message) {
                Ownership::Ours => update = runtime_message(ctx, &host, message),
                Ownership::Gone => {}
                Ownership::Violation(detail) => {
                    // A runtime that speaks for a process it was never given, or sends what only
                    // a client may, is not trusted with anything more. Its connection closes and
                    // is not retried until the host is attached again.
                    runtime_down(
                        ctx,
                        &host,
                        Unavailable::RuntimeUnreachable {
                            detail: format!(
                                "the runtime broke protocol and was disconnected: {detail}"
                            ),
                        },
                        false,
                    );
                }
            }
        }
    }
    // Readiness and failures change what can start; reconcile now rather than on the next message.
    // Not as a change of sessions or extensions, though: an instance the host just refused must
    // not be retried because the refusal itself arrived.
    if ctx.state.command_link.is_some() {
        reconcile(ctx, false);
    }
    update
}

fn runtime_down(ctx: &mut Context<AppRoot>, host: &HostKey, reason: Unavailable, retry: bool) {
    let Some(runtime) = ctx.state.extension_runtime.hosts.get_mut(host) else {
        return;
    };
    // Already down: a runtime that announced its end is then also lost when its channel closes,
    // and that is one failure, not two.
    if matches!(runtime.status, HostRuntimeStatus::Unavailable(_)) {
        return;
    }
    let epoch = runtime.epoch;
    runtime.connection = None;
    runtime.staged.clear();
    runtime.staging.clear();
    runtime.status = HostRuntimeStatus::Unavailable(reason.clone());
    if retry {
        runtime.failures = runtime.failures.saturating_add(1);
        runtime.retry_at = Some(Instant::now() + reconnect_delay(runtime.failures));
    } else {
        runtime.retry_at = None;
    }
    // Every credential issued over that connection dies with it.
    ctx.state
        .extension_workers
        .revoke_where(|worker| worker.runtime == WorkerRuntime(epoch));
    let workers: Vec<WorkerId> = ctx
        .state
        .extension_runtime
        .processes
        .iter()
        .filter(|(_, process)| &process.host == host)
        .map(|(worker, _)| *worker)
        .collect();
    for worker in workers {
        worker_failed(ctx, worker, reason.clone());
    }
}

/// Whether a runtime message is this runtime's to send.
enum Ownership {
    Ours,
    /// About a process that has since been stopped; a reply crossing a kill.
    Gone,
    Violation(String),
}

/// A runtime may only speak for the processes it was asked to start, over the connection they were
/// started through. Worker ids are client-wide, so without this a host could report, restage, or
/// fail another host's worker.
fn ownership(state: &State, host: &HostKey, epoch: u64, message: &Message) -> Ownership {
    let worker = match message {
        Message::Spawned { worker, .. }
        | Message::SpawnFailed { worker, .. }
        | Message::Exited { worker, .. } => WorkerId(*worker),
        Message::Staged { .. } | Message::StageFailed { .. } | Message::RuntimeFatal { .. } => {
            return Ownership::Ours;
        }
        Message::StageCommit { .. }
        | Message::Spawn { .. }
        | Message::Kill { .. }
        | Message::BridgeOpen { .. }
        | Message::BridgeClose { .. } => {
            return Ownership::Violation("sent a message only a client sends".to_string());
        }
    };
    let Some(process) = state.extension_runtime.processes.get(&worker) else {
        return Ownership::Gone;
    };
    let issued_here = state
        .extension_workers
        .get(worker)
        .is_none_or(|issued| issued.runtime == WorkerRuntime(epoch));
    if &process.host != host || !issued_here {
        return Ownership::Violation(format!("reported worker {} of another host", worker.0));
    }
    Ownership::Ours
}

fn runtime_message(ctx: &mut Context<AppRoot>, host: &HostKey, message: Message) -> Update {
    match message {
        // The runtime is ending and its processes with it. Taken down here, while they are still
        // registered, so every one of them is marked unreachable and relaunched once the reconnect
        // backoff passes - not left failed for a reason that was never theirs.
        Message::RuntimeFatal { detail } => {
            runtime_down(ctx, host, Unavailable::RuntimeUnreachable { detail }, true);
        }
        Message::Staged { digest } => {
            if let Some(runtime) = ctx.state.extension_runtime.hosts.get_mut(host) {
                runtime.staging.remove(&digest);
                runtime.staged.insert(digest);
            }
            flush(&mut ctx.state, host);
        }
        Message::StageFailed { digest, detail } => {
            if let Some(runtime) = ctx.state.extension_runtime.hosts.get_mut(host) {
                runtime.staging.remove(&digest);
            }
            // Only this host's launches: another host staging the same digest is unaffected.
            let processes = &ctx.state.extension_runtime.processes;
            let workers: Vec<WorkerId> = ctx
                .state
                .extension_runtime
                .pending
                .iter()
                .filter(|pending| {
                    pending.digest == digest
                        && processes
                            .get(&pending.worker)
                            .is_some_and(|process| &process.host == host)
                })
                .map(|pending| pending.worker)
                .collect();
            for worker in workers {
                worker_failed(
                    ctx,
                    worker,
                    Unavailable::BundleFailed {
                        detail: detail.clone(),
                    },
                );
            }
        }
        Message::Spawned { worker, pid } => {
            let worker = WorkerId(worker);
            if let Some(process) = ctx.state.extension_runtime.processes.get_mut(&worker) {
                process.state = WorkerProcessState::Running { pid };
            }
            // Running again: the next failure is news.
            if let Some(instance) = ctx
                .state
                .extension_runtime
                .instances
                .iter_mut()
                .find(|instance| instance.status == (InstanceStatus::Running { worker }))
            {
                instance.reported = None;
            }
        }
        Message::SpawnFailed { worker, failure } => {
            let worker = WorkerId(worker);
            // A bundle the runtime lost or found tampered with is staged again once, from the
            // copy this client loaded, before the failure is believed.
            if matches!(
                failure,
                SpawnFailure::BundleMissing | SpawnFailure::BundleCorrupt { .. }
            ) && restage(ctx, host, worker)
            {
                return Update::none();
            }
            worker_failed(ctx, worker, failure.into());
        }
        Message::Exited {
            worker,
            code,
            killed,
            timed_out,
            output,
        } => {
            return worker_exited(ctx, WorkerId(worker), code, killed, timed_out, output);
        }
        Message::StageCommit { .. }
        | Message::Spawn { .. }
        | Message::Kill { .. }
        | Message::BridgeOpen { .. }
        | Message::BridgeClose { .. } => {}
    }
    Update::none()
}

/// Requeue a worker whose bundle the runtime did not have, staging it again. `false` when it was
/// already restaged once, or is gone.
fn restage(ctx: &mut Context<AppRoot>, host: &HostKey, worker: WorkerId) -> bool {
    let Some(process) = ctx.state.extension_runtime.processes.get(&worker) else {
        return false;
    };
    if process.state == WorkerProcessState::Pending || &process.host != host {
        return false;
    }
    let Some(placements) = ctx
        .state
        .config
        .extension_placements
        .values()
        .find(|placements| placements.bundle_digest() == process.digest)
        .cloned()
    else {
        return false;
    };
    if ctx.state.extension_workers.get(worker).is_none() {
        return false;
    }
    if ctx.state.extension_runtime.restaged.contains(&worker) {
        return false;
    }
    ctx.state.extension_runtime.restaged.insert(worker);
    let Some(message) = respawn_message(&ctx.state, worker) else {
        return false;
    };
    if let Some(process) = ctx.state.extension_runtime.processes.get_mut(&worker) {
        process.state = WorkerProcessState::Pending;
    }
    if let Some(runtime) = ctx.state.extension_runtime.hosts.get_mut(host) {
        runtime.staged.remove(placements.bundle_digest());
    }
    ctx.state.extension_runtime.pending.push(PendingLaunch {
        worker,
        digest: placements.bundle_digest().to_string(),
        message,
        restaged: true,
        bundle: placements.bundle.clone(),
    });
    flush(&mut ctx.state, host);
    true
}

/// The spawn message last sent for `worker`, kept so a restage can resend it unchanged.
fn respawn_message(state: &State, worker: WorkerId) -> Option<Message> {
    state.extension_runtime.sent.get(&worker).cloned()
}

fn worker_failed(ctx: &mut Context<AppRoot>, worker: WorkerId, reason: Unavailable) {
    ctx.state.extension_workers.revoke(worker);
    ctx.state
        .extension_runtime
        .pending
        .retain(|pending| pending.worker != worker);
    ctx.state.extension_runtime.sent.remove(&worker);
    let Some(process) = ctx.state.extension_runtime.processes.remove(&worker) else {
        return;
    };
    match process.purpose {
        WorkerPurpose::Service { key } => {
            let Some(instance) = ctx
                .state
                .extension_runtime
                .instances
                .iter_mut()
                .find(|instance| instance.key == key)
            else {
                return;
            };
            if instance.status != (InstanceStatus::Running { worker }) {
                return;
            }
            instance.status = InstanceStatus::Unavailable(reason.clone());
            report_unavailable(ctx, &key);
        }
        WorkerPurpose::Command { label } | WorkerPurpose::TabClick { label } => {
            crate::pane::pty_events::notify_error(
                ctx,
                "Command unavailable",
                format!("`{label}` on {}: {reason}", process.host.label()),
            );
        }
        WorkerPurpose::TabListing {
            tab,
            epoch,
            group_prefix: _,
        } => {
            let rows = vec![crate::update::sidebar::polling::error_row(&format!(
                "unavailable on {}: {reason}",
                process.host.label()
            ))];
            let _ = crate::update::sidebar::polling::command_output(ctx, epoch, tab, rows);
        }
    }
}

fn worker_exited(
    ctx: &mut Context<AppRoot>,
    worker: WorkerId,
    code: Option<i32>,
    killed: bool,
    timed_out: bool,
    output: Option<String>,
) -> Update {
    ctx.state.extension_workers.revoke(worker);
    ctx.state.extension_runtime.sent.remove(&worker);
    let Some(process) = ctx.state.extension_runtime.processes.remove(&worker) else {
        return Update::none();
    };
    match process.purpose {
        WorkerPurpose::Service { key } => {
            service_exited(ctx, &key, worker, code, killed);
            Update::full()
        }
        WorkerPurpose::Command { label } | WorkerPurpose::TabClick { label } => {
            if !killed && code != Some(0) {
                let status = code.map_or_else(
                    || "was terminated".to_string(),
                    |code| format!("exited with status {code}"),
                );
                crate::pane::pty_events::notify_error(
                    ctx,
                    "Command failed",
                    format!("`{label}` {status} on {}", process.host.label()),
                );
            }
            Update::full()
        }
        WorkerPurpose::TabListing {
            tab,
            epoch,
            group_prefix,
        } => {
            let rows = crate::update::sidebar::polling::command_rows(
                Ok(crate::platform::command::CommandOutput {
                    stdout: output.unwrap_or_default().into_bytes(),
                    stderr: Vec::new(),
                    status: code,
                    timed_out,
                }),
                group_prefix.as_deref(),
            );
            crate::update::sidebar::polling::command_output(ctx, epoch, tab, rows)
        }
    }
}

fn service_exited(
    ctx: &mut Context<AppRoot>,
    key: &InstanceKey,
    worker: WorkerId,
    code: Option<i32>,
    killed: bool,
) {
    let restart = ctx
        .state
        .config
        .extension_placements
        .values()
        .flat_map(|placements| placements.services.iter())
        .find(|service| service.name == key.service)
        .map(|service| service.restart);
    let Some(instance) = ctx
        .state
        .extension_runtime
        .instances
        .iter_mut()
        .find(|instance| &instance.key == key)
    else {
        return;
    };
    if instance.status != (InstanceStatus::Running { worker }) || killed {
        return;
    }
    if instance
        .started_at
        .is_some_and(|at| at.elapsed() >= Duration::from_secs(UPTIME_RESET_SECS))
    {
        instance.failures = 0;
        instance.backoff = INITIAL_BACKOFF;
    }
    let clean = code == Some(0);
    let described = code.map_or_else(
        || "was terminated".to_string(),
        |code| format!("exited with status {code}"),
    );
    let restart = restart.unwrap_or(ServiceRestart::OnFailure);
    if restart == ServiceRestart::Never || (restart == ServiceRestart::OnFailure && clean) {
        instance.status = InstanceStatus::Stopped { detail: described };
        return;
    }
    instance.failures += 1;
    if instance.failures >= MAX_FAILURES {
        instance.status = InstanceStatus::Stopped {
            detail: format!("{described}; gave up after {MAX_FAILURES} attempts"),
        };
        let message = format!(
            "`{}` on {} {described} {MAX_FAILURES} times in a row and was not restarted",
            key.service,
            key.binding.host.label()
        );
        crate::pane::pty_events::notify_error(ctx, "Extension service failed", message);
        return;
    }
    let at = Instant::now() + instance.backoff;
    instance.backoff = next_backoff(instance.backoff);
    instance.status = InstanceStatus::Restarting { at };
    arm_wake(&mut ctx.state, Some(at));
}

/// The placed definition of a named command, if it has one.
pub(crate) fn placed_command(
    state: &State,
    id: &str,
) -> Option<(SharedPlacements, crate::config::PlacedCommand)> {
    state
        .config
        .extension_placements
        .values()
        .find_map(|placements| {
            placements
                .commands
                .get(id)
                .map(|placed| (placements.clone(), placed.clone()))
        })
}

/// The directory a process on the session on screen's host starts in: the focused pane's, as that
/// host sees it.
fn session_cwd(state: &State) -> SpawnCwd {
    let path = if state.current().remote_host.is_some() {
        crate::pane::lifecycle::focused_spawn_cwd(state)
    } else {
        crate::pane::lifecycle::focused_local_cwd(state)
    };
    path.map_or(SpawnCwd::Inherit, |path| SpawnCwd::Host { path })
}

/// Run a command placed on the active session: on the host of the session on screen, in its
/// focused pane's directory there.
pub(crate) fn run_command(
    ctx: &mut Context<AppRoot>,
    command: &NamedCommand,
    placements: SharedPlacements,
    placed: &crate::config::PlacedCommand,
    extra_env: Vec<(String, String)>,
) -> Update {
    let label = command.label.clone().unwrap_or_else(|| command.id.clone());
    let Some(binding) = active_binding(&ctx.state) else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Command unavailable",
            format!("`{label}`: {}", Unavailable::NoSession),
        );
        return Update::full();
    };
    let mut env = command.env.clone();
    env.extend(extra_env);
    let spec = LaunchSpec {
        placements,
        placement: placed.placement,
        binding,
        kind: WorkerKind::Command {
            id: command.id.clone(),
        },
        purpose: WorkerPurpose::Command {
            label: label.clone(),
        },
        launch: placed.launch.clone(),
        cwd: session_cwd(&ctx.state),
        env,
        capture: None,
    };
    if let Err(reason) = launch(&mut ctx.state, spec) {
        crate::pane::pty_events::notify_error(
            ctx,
            "Command unavailable",
            format!("`{label}`: {reason}"),
        );
    }
    Update::full()
}

/// The placed definition of a sidebar tab, if it has one.
pub(crate) fn placed_tab(
    state: &State,
    id: &crate::config::SidebarTabId,
) -> Option<(SharedPlacements, crate::config::PlacedTab)> {
    state
        .config
        .extension_placements
        .values()
        .find_map(|placements| {
            placements
                .tabs
                .get(id)
                .map(|placed| (placements.clone(), placed.clone()))
        })
}

/// List a placed sidebar tab on the active session's host. The rows arrive as the listing's exit.
pub(crate) fn poll_tab(
    ctx: &mut Context<AppRoot>,
    tab: crate::config::SidebarTabId,
    epoch: u64,
    placements: SharedPlacements,
    placed: &crate::config::PlacedTab,
    env: Vec<(String, String)>,
    group_prefix: Option<String>,
) -> Update {
    let Some(binding) = active_binding(&ctx.state) else {
        let rows = vec![crate::update::sidebar::polling::error_row(&format!(
            "unavailable: {}",
            Unavailable::NoSession
        ))];
        return crate::update::sidebar::polling::command_output(ctx, epoch, tab, rows);
    };
    let spec = LaunchSpec {
        placements,
        placement: placed.placement,
        binding,
        kind: WorkerKind::Tab {
            id: tab.as_str().to_string(),
        },
        purpose: WorkerPurpose::TabListing {
            tab: tab.clone(),
            epoch,
            group_prefix,
        },
        launch: LaunchTemplate::Shell(placed.command.clone()),
        cwd: session_cwd(&ctx.state),
        env,
        capture: Some(Capture {
            max_bytes: crate::update::sidebar::polling::COMMAND_CAPTURE_BYTES,
            timeout_ms: crate::update::sidebar::polling::COMMAND_TIMEOUT.as_millis() as u64,
        }),
    };
    match launch(&mut ctx.state, spec) {
        Ok(_) => Update::none(),
        Err(reason) => {
            let host = active_binding(&ctx.state)
                .map(|binding| binding.host.label())
                .unwrap_or_default();
            let rows = vec![crate::update::sidebar::polling::error_row(&format!(
                "unavailable on {host}: {reason}"
            ))];
            crate::update::sidebar::polling::command_output(ctx, epoch, tab, rows)
        }
    }
}

/// Run a placed tab's `on_click` `exec` for the clicked row, on the active session's host.
pub(crate) fn click_tab(
    ctx: &mut Context<AppRoot>,
    tab: &crate::config::SidebarTabId,
    placements: SharedPlacements,
    placed: &crate::config::PlacedTab,
    line: &str,
    mut env: Vec<(String, String)>,
) -> Update {
    let Some(command) = placed.on_click_exec.clone() else {
        return Update::none();
    };
    let Some(binding) = active_binding(&ctx.state) else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Command unavailable",
            format!("`{command}`: {}", Unavailable::NoSession),
        );
        return Update::full();
    };
    env.push(("ROZI_ROW".to_string(), line.to_string()));
    let spec = LaunchSpec {
        placements,
        placement: placed.placement,
        binding,
        kind: WorkerKind::Tab {
            id: tab.as_str().to_string(),
        },
        purpose: WorkerPurpose::TabClick {
            label: crate::config::truncate_for_label(&command),
        },
        launch: LaunchTemplate::Shell(command.clone()),
        cwd: session_cwd(&ctx.state),
        env,
        capture: None,
    };
    if let Err(reason) = launch(&mut ctx.state, spec) {
        crate::pane::pty_events::notify_error(
            ctx,
            "Command unavailable",
            format!("`{command}`: {reason}"),
        );
    }
    Update::full()
}

fn host_name(host: &HostKey) -> String {
    match host {
        HostKey::Local => "local".to_string(),
        HostKey::Remote(target) => target.display_label(),
    }
}

fn unavailable_info(reason: &Unavailable) -> crate::control::UnavailableInfo {
    crate::control::UnavailableInfo {
        code: reason.code().to_string(),
        message: reason.to_string(),
    }
}

/// What `extension-runtime-status` reports: every runtime, service instance, and placed process.
pub(crate) fn status_report(state: &State) -> crate::control::ExtensionRuntimeReport {
    let runtime = &state.extension_runtime;
    let mut runtimes: Vec<_> = runtime
        .hosts
        .values()
        .map(|host| {
            let (status, os, version, reason) = match &host.status {
                HostRuntimeStatus::Connecting => ("connecting", None, None, None),
                HostRuntimeStatus::Ready { os, version } => {
                    ("ready", Some(os.clone()), version.clone(), None)
                }
                HostRuntimeStatus::Unavailable(reason) => {
                    ("unavailable", None, None, Some(unavailable_info(reason)))
                }
            };
            crate::control::ExtensionRuntimeInfo {
                host: host_name(&host.host),
                epoch: host.epoch,
                status: status.to_string(),
                os,
                version,
                reason,
            }
        })
        .collect();
    runtimes.sort_by(|a, b| a.host.cmp(&b.host));
    let bundle_of = |extension: &str| {
        state
            .config
            .extension_placements
            .get(extension)
            .map(|placements| placements.bundle_digest().to_string())
            .unwrap_or_default()
    };
    let placement_of = |service: &str| {
        state
            .config
            .extension_placements
            .values()
            .flat_map(|placements| placements.services.iter())
            .find(|placed| placed.name == service)
            .map_or("", |placed| placed.placement.as_str())
            .to_string()
    };
    let pid_of = |worker: WorkerId| match runtime.processes.get(&worker).map(|p| &p.state) {
        Some(WorkerProcessState::Running { pid }) => Some(*pid),
        _ => None,
    };
    let mut instances: Vec<_> = runtime
        .instances
        .iter()
        .map(|instance| {
            let (status, pid, detail, reason) = match &instance.status {
                InstanceStatus::Running { worker } => ("running", pid_of(*worker), None, None),
                InstanceStatus::Restarting { .. } => ("restarting", None, None, None),
                InstanceStatus::Stopped { detail } => ("stopped", None, Some(detail.clone()), None),
                InstanceStatus::Unavailable(reason) => {
                    ("unavailable", None, None, Some(unavailable_info(reason)))
                }
            };
            crate::control::PlacedInstanceInfo {
                service: instance.key.service.clone(),
                extension: instance.extension.clone(),
                placement: placement_of(&instance.key.service),
                host: host_name(&instance.key.binding.host),
                session: instance
                    .key
                    .binding
                    .session
                    .as_ref()
                    .map(|session| session.as_str().to_string()),
                generation: instance.generation.clone(),
                bundle: bundle_of(&instance.extension),
                status: status.to_string(),
                pid,
                detail,
                reason,
                failures: instance.failures,
            }
        })
        .collect();
    instances.sort_by(|a, b| (&a.service, &a.host).cmp(&(&b.service, &b.host)));
    let mut processes: Vec<_> = runtime
        .processes
        .iter()
        .filter_map(|(worker_id, process)| {
            let worker = state.extension_workers.get(*worker_id)?;
            let (kind, id) = match &worker.kind {
                WorkerKind::Service { name } => ("service", name.clone()),
                WorkerKind::Command { id } => ("command", id.clone()),
                WorkerKind::Tab { id } => ("tab", id.clone()),
            };
            let (state_name, pid) = match process.state {
                WorkerProcessState::Pending => ("pending", None),
                WorkerProcessState::Starting => ("starting", None),
                WorkerProcessState::Running { pid } => ("running", Some(pid)),
            };
            Some(crate::control::PlacedProcessInfo {
                extension: worker.extension.id.clone(),
                kind: kind.to_string(),
                id,
                host: host_name(&process.host),
                generation: worker.extension.generation.clone(),
                bundle: process.digest.clone(),
                state: state_name.to_string(),
                pid,
            })
        })
        .collect();
    processes.sort_by(|a, b| (&a.id, &a.host, a.pid).cmp(&(&b.id, &b.host, b.pid)));
    crate::control::ExtensionRuntimeReport {
        runtimes,
        instances,
        processes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_runtime::protocol::Launch;
    use crate::session::remote::RemoteTarget;
    use crate::state::{PendingLaunch, WorkerBinding};
    use tui_lipan::TestBackend;

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn host(name: &str) -> HostKey {
        HostKey::Remote(RemoteTarget::Alias(name.to_string()))
    }

    fn on_test_thread(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    /// Two hosts, each with a launch of the same bundle waiting to stage.
    fn two_hosts() -> (TestBackend<AppRoot>, WorkerId, WorkerId) {
        let mut backend = TestBackend::new(AppRoot::default());
        let state = backend.state_mut();
        state
            .extension_generations
            .insert("sessions".to_string(), "g1".to_string());
        // A session on each host, so both runtimes are wanted.
        state.current_mut().session_attached = true;
        state.current_mut().remote_target = host("pc").remote().cloned();
        let mut background = crate::state::Attachment::new();
        background.session_attached = true;
        background.remote_target = host("server").remote().cloned();
        state.background.insert(9_001, background);
        let bundle = std::sync::Arc::new(
            crate::extension_runtime::bundle::Bundle::from_files(Vec::new()).unwrap(),
        );
        state.config.extension_placements.insert(
            "sessions".to_string(),
            std::sync::Arc::new(crate::config::ExtensionPlacements {
                id: "sessions".to_string(),
                platforms: Vec::new(),
                local_dir: "/x".to_string(),
                bundle: bundle.clone(),
                commands: Default::default(),
                tabs: Default::default(),
                services: Vec::new(),
                client_unsupported: None,
            }),
        );
        let mut workers = Vec::new();
        for (name, epoch) in [("pc", 1), ("server", 2)] {
            state.extension_runtime.hosts.insert(
                host(name),
                HostRuntime {
                    host: host(name),
                    epoch,
                    status: HostRuntimeStatus::Ready {
                        os: "linux".to_string(),
                        version: None,
                    },
                    connection: None,
                    staged: HashSet::new(),
                    staging: [DIGEST.to_string()].into_iter().collect(),
                    failures: 0,
                    retry_at: None,
                },
            );
            let worker = state
                .extension_workers
                .issue(
                    ExtensionProvenance {
                        id: "sessions".to_string(),
                        generation: "g1".to_string(),
                    },
                    Placement::ActiveSession,
                    WorkerBinding {
                        host: host(name),
                        session: None,
                    },
                    WorkerRuntime(epoch),
                    WorkerKind::Command {
                        id: "sessions.run".to_string(),
                    },
                )
                .id;
            state.extension_runtime.processes.insert(
                worker,
                WorkerProcess {
                    host: host(name),
                    purpose: WorkerPurpose::Command {
                        label: "run".to_string(),
                    },
                    digest: DIGEST.to_string(),
                    state: WorkerProcessState::Pending,
                },
            );
            let message = Message::Spawn {
                worker: worker.0,
                digest: DIGEST.to_string(),
                launch: Launch::Shell {
                    line: "true".to_string(),
                },
                cwd: SpawnCwd::Inherit,
                env: Vec::new(),
                credential: "secret".to_string(),
                platforms: Vec::new(),
                capture: None,
            };
            state.extension_runtime.sent.insert(worker, message.clone());
            state.extension_runtime.pending.push(PendingLaunch {
                worker,
                digest: DIGEST.to_string(),
                message,
                restaged: false,
                bundle: bundle.clone(),
            });
            workers.push(worker);
        }
        (backend, workers[0], workers[1])
    }

    fn report(backend: &mut TestBackend<AppRoot>, name: &str, epoch: u64, message: Message) {
        backend
            .dispatch(crate::Msg::ExtensionRuntime {
                host: host(name),
                epoch,
                event: RuntimeEvent::Message(message),
            })
            .unwrap();
    }

    /// The same bundle staging on two hosts: one host failing it fails only its own launch.
    #[test]
    fn a_staging_failure_on_one_host_leaves_another_host_staging_the_same_bundle_alone() {
        on_test_thread(|| {
            let (mut backend, on_pc, on_server) = two_hosts();
            report(
                &mut backend,
                "pc",
                1,
                Message::StageFailed {
                    digest: DIGEST.to_string(),
                    detail: "disk full".to_string(),
                },
            );
            let state = backend.state();
            assert!(!state.extension_runtime.processes.contains_key(&on_pc));
            assert!(state.extension_runtime.processes.contains_key(&on_server));
            assert!(state.extension_workers.get(on_server).is_some());
            assert!(
                state
                    .extension_runtime
                    .pending
                    .iter()
                    .any(|pending| pending.worker == on_server)
            );
        });
    }

    /// A runtime naming another host's worker is disconnected, and nothing of that worker - its
    /// launch, its credential, its state - moves to it.
    #[test]
    fn a_runtime_naming_another_hosts_worker_is_disconnected_and_changes_nothing_there() {
        for message in [
            |worker: u64| Message::Spawned { worker, pid: 1 },
            |worker: u64| Message::Exited {
                worker,
                code: Some(0),
                killed: false,
                timed_out: false,
                output: None,
            },
            |worker: u64| Message::SpawnFailed {
                worker,
                failure: SpawnFailure::BundleMissing,
            },
        ] {
            on_test_thread(move || {
                let (mut backend, _, on_server) = two_hosts();
                // Pretend the server's launch went out, so a restage would have something to send.
                backend
                    .state_mut()
                    .extension_runtime
                    .processes
                    .get_mut(&on_server)
                    .unwrap()
                    .state = WorkerProcessState::Starting;
                report(&mut backend, "pc", 1, message(on_server.0));
                let state = backend.state();
                assert!(matches!(
                    state.extension_runtime.hosts[&host("pc")].status,
                    HostRuntimeStatus::Unavailable(_)
                ));
                assert!(
                    state.extension_runtime.hosts[&host("pc")]
                        .retry_at
                        .is_none()
                );
                let process = &state.extension_runtime.processes[&on_server];
                assert_eq!(process.host, host("server"));
                assert_eq!(process.state, WorkerProcessState::Starting);
                assert!(state.extension_workers.get(on_server).is_some());
                assert!(
                    !state
                        .extension_runtime
                        .pending
                        .iter()
                        .any(|pending| pending.worker == on_server && pending.restaged),
                    "the server's launch was never queued again, for any host"
                );
            });
        }
    }

    #[test]
    fn a_runtime_sending_what_only_a_client_sends_is_disconnected() {
        on_test_thread(|| {
            let (mut backend, _, _) = two_hosts();
            report(&mut backend, "pc", 1, Message::Kill { worker: 1 });
            assert!(matches!(
                backend.state().extension_runtime.hosts[&host("pc")].status,
                HostRuntimeStatus::Unavailable(_)
            ));
            assert!(matches!(
                backend.state().extension_runtime.hosts[&host("server")].status,
                HostRuntimeStatus::Ready { .. }
            ));
        });
    }

    /// A runtime whose cache lease failed announces its end. Every service it ran - not just the
    /// one whose launch hit the failure - is marked unreachable, the host reconnects once the
    /// backoff passes, and all of them are launched again under the new runtime.
    #[test]
    fn a_fatal_runtime_relaunches_every_service_it_ran_after_reconnecting() {
        on_test_thread(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let deadline = Instant::now() + Duration::from_secs(10);
            while backend.state().command_link.is_none() {
                assert!(Instant::now() < deadline, "no command link");
                backend.pump().unwrap();
            }
            let state = backend.state_mut();
            state
                .extension_generations
                .insert("sessions".to_string(), "g1".to_string());
            state.current_mut().session_attached = true;
            let service = |name: &str| crate::config::PlacedService {
                name: format!("sessions.{name}"),
                placement: Placement::EachHost,
                launch: LaunchTemplate::Shell("exit 0".to_string()),
                cwd: ".".to_string(),
                restart: ServiceRestart::Always,
                env: Default::default(),
            };
            state.config.extension_placements.insert(
                "sessions".to_string(),
                std::sync::Arc::new(crate::config::ExtensionPlacements {
                    id: "sessions".to_string(),
                    platforms: Vec::new(),
                    local_dir: "/x".to_string(),
                    bundle: std::sync::Arc::new(
                        crate::extension_runtime::bundle::Bundle::from_files(Vec::new()).unwrap(),
                    ),
                    commands: Default::default(),
                    tabs: Default::default(),
                    services: vec![service("one"), service("two")],
                    client_unsupported: None,
                }),
            );
            // Both services running on this machine's runtime, epoch 1.
            state.extension_runtime.next_epoch = 1;
            state.extension_runtime.hosts.insert(
                HostKey::Local,
                HostRuntime {
                    host: HostKey::Local,
                    epoch: 1,
                    status: HostRuntimeStatus::Ready {
                        os: std::env::consts::OS.to_string(),
                        version: None,
                    },
                    connection: None,
                    staged: HashSet::new(),
                    staging: HashSet::new(),
                    failures: 0,
                    retry_at: None,
                },
            );
            let binding = WorkerBinding {
                host: HostKey::Local,
                session: None,
            };
            for name in ["one", "two"] {
                let key = InstanceKey {
                    service: format!("sessions.{name}"),
                    binding: binding.clone(),
                };
                let worker = state
                    .extension_workers
                    .issue(
                        ExtensionProvenance {
                            id: "sessions".to_string(),
                            generation: "g1".to_string(),
                        },
                        Placement::EachHost,
                        binding.clone(),
                        WorkerRuntime(1),
                        WorkerKind::Service {
                            name: key.service.clone(),
                        },
                    )
                    .id;
                state.extension_runtime.processes.insert(
                    worker,
                    WorkerProcess {
                        host: HostKey::Local,
                        purpose: WorkerPurpose::Service { key: key.clone() },
                        digest: DIGEST.to_string(),
                        state: WorkerProcessState::Running { pid: 1 },
                    },
                );
                state.extension_runtime.instances.push(PlacedInstance {
                    key,
                    extension: "sessions".to_string(),
                    generation: "g1".to_string(),
                    status: InstanceStatus::Running { worker },
                    started_at: Some(Instant::now()),
                    failures: 0,
                    backoff: INITIAL_BACKOFF,
                    reported: None,
                });
            }
            let signature = signature(state);
            state.extension_runtime.signature = Some(signature);

            let event = |event| crate::Msg::ExtensionRuntime {
                host: HostKey::Local,
                epoch: 1,
                event,
            };
            backend
                .dispatch(event(RuntimeEvent::Message(Message::RuntimeFatal {
                    detail: "cannot lease".to_string(),
                })))
                .unwrap();
            // The channel closing right after is the same failure, not a second one.
            backend
                .dispatch(event(RuntimeEvent::Lost("closed".to_string())))
                .unwrap();
            let state = backend.state();
            let runtime = &state.extension_runtime.hosts[&HostKey::Local];
            assert!(matches!(runtime.status, HostRuntimeStatus::Unavailable(_)));
            assert_eq!(runtime.failures, 1);
            assert!(runtime.retry_at.is_some());
            assert!(
                state.extension_workers.is_empty(),
                "epoch 1 credentials revoked"
            );
            for instance in &state.extension_runtime.instances {
                assert!(
                    matches!(
                        instance.status,
                        InstanceStatus::Unavailable(Unavailable::RuntimeUnreachable { .. })
                    ),
                    "{:?}",
                    instance.status
                );
            }

            // The backoff passes.
            let past = Instant::now() - Duration::from_secs(1);
            let state = backend.state_mut();
            state
                .extension_runtime
                .hosts
                .get_mut(&HostKey::Local)
                .unwrap()
                .retry_at = Some(past);
            state.extension_runtime.wake_at = Some(past);
            // Inspect the tick itself before draining asynchronous runtime replies. Dispatch may
            // already consume Hello and service exits, making Connecting a timing-dependent state.
            backend.update_level(crate::Msg::PlacementTick).unwrap();

            let state = backend.state();
            let runtime = &state.extension_runtime.hosts[&HostKey::Local];
            assert_eq!(runtime.epoch, 2, "a new runtime");
            assert_eq!(runtime.status, HostRuntimeStatus::Connecting);
            assert_eq!(state.extension_runtime.instances.len(), 2);
            for instance in &state.extension_runtime.instances {
                let InstanceStatus::Running { worker } = instance.status else {
                    panic!(
                        "{} was not relaunched: {:?}",
                        instance.key.service, instance.status
                    );
                };
                assert_eq!(
                    state.extension_workers.get(worker).unwrap().runtime,
                    WorkerRuntime(2)
                );
            }
        });
    }
}
