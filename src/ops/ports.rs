//! Client-owned SSH port-forward lifecycle, URL policy, and delayed link opening.

use std::net::IpAddr;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{LinkAction, UserCommandAction};
use crate::pane::id::PaneId;
use crate::session::remote::RemoteTarget;
use crate::session::remote::forward::{
    ForwardEvent, ForwardHost, ForwardKey, ForwardSpec, LocalPortMode, exact_local_port,
    preferred_local_port, start_forward,
};
use crate::state::{
    BrowserScheme, ForwardState, OpenDestination, PendingLinkContext, PendingLinkOpen,
    PortFormField, PortFormState, PortForwardEntry, PortsOverlayState, ResolvedLinkDestination,
    State,
};

use super::links::{Machine, PanePlace};

/// A port-forward event or UI action routed through [`crate::Msg::Ports`].
#[derive(Clone)]
pub enum PortMsg {
    Event {
        key: ForwardKey,
        generation: u64,
        event: ForwardEvent,
    },
    CloseOverlay,
    QueryChanged(String),
    Select(ForwardKey),
    Activate(ForwardKey),
    OpenAddForm,
    CloseAddForm,
    FormChanged(PortFormField, InputEvent),
    SubmitAddForm,
    RetrySelected,
    CloseSelected,
}

/// Forward policy decision for a parsed terminal URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkForwardRequest {
    pub key: ForwardKey,
    pub scheme: BrowserScheme,
}

/// Decide whether activating `url` from `source` onto `destination` requires an SSH tunnel.
pub(crate) fn forward_request_for_link(
    state: &State,
    source: PaneId,
    url: &url::Url,
    destination: &OpenDestination,
) -> Option<LinkForwardRequest> {
    if !state.config.remote.ports.auto_forward_links {
        return None;
    }
    let scheme = BrowserScheme::from_scheme(url.scheme())?;
    let remote_port = url.port_or_known_default().filter(|&port| port != 0)?;
    let remote_host = loopback_forward_host(url.host()?)?;
    if !matches!(PanePlace::for_pane(state, source), PanePlace::SessionHost) {
        return None;
    }
    let runs_on = match destination {
        OpenDestination::System => Machine::Client,
        OpenDestination::Handler(action) => super::links::handler_machine(state, action),
    };
    if runs_on != Machine::Client {
        return None;
    }
    let target = state.current().remote_target.clone()?;
    Some(LinkForwardRequest {
        key: ForwardKey {
            target,
            remote_host,
            remote_port,
        },
        scheme,
    })
}

fn loopback_forward_host(host: url::Host<&str>) -> Option<ForwardHost> {
    match host {
        url::Host::Domain(domain) => domain
            .eq_ignore_ascii_case("localhost")
            .then_some(ForwardHost::Localhost),
        url::Host::Ipv4(ip) => ForwardHost::from_loopback_ip(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => ForwardHost::from_loopback_ip(IpAddr::V6(ip)),
    }
}

/// Rewrite a remote loopback/wildcard URL onto the local forwarded loopback port, preserving
/// path, query, fragment, and scheme.
pub(crate) fn rewrite_forwarded_url(original: &str, local_port: u16) -> Option<String> {
    let mut url = url::Url::parse(original).ok()?;
    let keep_host = match url.host() {
        Some(url::Host::Domain(domain)) if domain.eq_ignore_ascii_case("localhost") => true,
        Some(url::Host::Ipv4(ip)) if ip == std::net::Ipv4Addr::LOCALHOST => true,
        _ => false,
    };
    if !keep_host {
        url.set_host(Some("127.0.0.1")).ok()?;
    }
    url.set_port(Some(local_port)).ok()?;
    Some(url.into())
}

fn resolve_pending_open(
    state: &State,
    source: PaneId,
    target: RemoteTarget,
    original_url: &str,
    destination: OpenDestination,
) -> std::result::Result<PendingLinkOpen, String> {
    if original_url.contains('\0') {
        return Err("Link contains a NUL character".to_string());
    }
    let current = state.current();
    let context = PendingLinkContext {
        epoch: state.runtime_epoch,
        target,
        source_pane: source,
        source_session: current.session_instance.clone(),
        source_session_name: current.session_name.clone(),
        remote_host: current.remote_host.clone(),
    };
    let resolved = match destination {
        OpenDestination::System => ResolvedLinkDestination::System,
        OpenDestination::Handler(action) => {
            let (user_action, command_env) = match action {
                LinkAction::Inline(action) => (action, Vec::new()),
                LinkAction::Command(id) => {
                    if super::placement::placed_command(state, &id).is_some() {
                        return Err(format!("Link command `{id}` is placed on the session host"));
                    }
                    let command = super::links::resolve_link_command(state, &id)?;
                    (command.action, command.env)
                }
            };
            match user_action {
                UserCommandAction::Exec { command } => ResolvedLinkDestination::Exec {
                    command,
                    env: command_env,
                },
                UserCommandAction::ExecDirect { argv } => ResolvedLinkDestination::ExecDirect {
                    argv,
                    env: command_env,
                },
                UserCommandAction::Run { command, keep_open } if state.scratch_visible => {
                    ResolvedLinkDestination::ScratchRun {
                        command,
                        keep_open,
                        env: command_env,
                    }
                }
                _ => {
                    return Err("Link command does not launch a client-side process".to_string());
                }
            }
        }
    };
    Ok(PendingLinkOpen {
        original_url: original_url.to_string(),
        context,
        destination: resolved,
    })
}

fn context_is_live(state: &State, context: &PendingLinkContext) -> bool {
    state
        .attachment_for_epoch(context.epoch)
        .is_some_and(|attachment| {
            attachment.remote_target.as_ref() == Some(&context.target)
                && attachment.session_instance == context.source_session
        })
}

fn combine_client_link_env(
    mut command_env: Vec<(String, String)>,
    link_vars: Vec<(String, String)>,
    context: &PendingLinkContext,
) -> Vec<(String, String)> {
    if command_env.iter().any(|(key, _)| key == "ROZI_EXTENSION") {
        command_env.push((
            crate::config::SESSION_NAME_ENV.to_string(),
            context.source_session_name.clone().unwrap_or_default(),
        ));
    }
    command_env.extend(link_vars);
    command_env
}

fn execute_resolved_open(
    ctx: &mut Context<AppRoot>,
    open: PendingLinkOpen,
    local_port: u16,
) -> Update {
    if !context_is_live(&ctx.state, &open.context) {
        return Update::none();
    }
    let rewritten = rewrite_forwarded_url(&open.original_url, local_port)
        .unwrap_or_else(|| open.original_url.clone());
    let include_pane = matches!(
        open.destination,
        ResolvedLinkDestination::Exec { .. } | ResolvedLinkDestination::ExecDirect { .. }
    );
    let link_vars = super::links::link_env(
        open.context.source_pane,
        open.context.source_session.as_ref(),
        open.context.remote_host.as_deref(),
        &open.original_url,
        super::links::LinkTarget {
            uri: &rewritten,
            ..Default::default()
        },
        include_pane,
    );
    match open.destination {
        ResolvedLinkDestination::System => match tui_lipan::utils::open_url(&rewritten) {
            Ok(()) => Update::none(),
            Err(error) => super::links::failed(ctx, error.to_string()),
        },
        ResolvedLinkDestination::Exec { command, env } => {
            let full_env = combine_client_link_env(env, link_vars, &open.context);
            super::user_command::execute_client_exec(
                ctx,
                &UserCommandAction::Exec { command },
                full_env,
                None,
            )
        }
        ResolvedLinkDestination::ExecDirect { argv, env } => {
            let full_env = combine_client_link_env(env, link_vars, &open.context);
            super::user_command::execute_client_exec(
                ctx,
                &UserCommandAction::ExecDirect { argv },
                full_env,
                None,
            )
        }
        ResolvedLinkDestination::ScratchRun {
            command,
            keep_open,
            env,
        } => {
            let full_env = combine_client_link_env(env, link_vars, &open.context);
            if let Err(update) = super::user_command::prepare_client_command_env(ctx, &full_env) {
                return update;
            }
            let identity = crate::state::PaneIdentity {
                launch: Some(crate::pane::launch::PaneLaunch::shell(command)),
                keep_open,
                cwd: None,
                env: full_env,
                ..crate::state::PaneIdentity::default()
            };
            crate::pane::lifecycle::spawn_pane_in_scratch(
                ctx,
                ctx.state.scratch.focused_pane,
                identity,
            )
            .1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalPortRequest {
    Unpinned,
    Pinned(u16),
    Exact(u16),
}

impl LocalPortRequest {
    fn from_entry(entry: &PortForwardEntry) -> Self {
        match entry.local_port_mode {
            LocalPortMode::Exact if entry.local_port != 0 => Self::Exact(entry.local_port),
            LocalPortMode::Auto | LocalPortMode::Exact
                if entry.port_pinned && entry.local_port != 0 =>
            {
                Self::Pinned(entry.local_port)
            }
            LocalPortMode::Auto | LocalPortMode::Exact => Self::Unpinned,
        }
    }
}

/// Open a remote loopback URL by reusing a ready tunnel, queueing behind one starting up, or
/// launching a new independently owned `ssh -N` tunnel.
pub(crate) fn open_forwarded_link(
    ctx: &mut Context<AppRoot>,
    source: PaneId,
    original_url: &str,
    request: LinkForwardRequest,
    destination: OpenDestination,
) -> Update {
    let pending = match resolve_pending_open(
        &ctx.state,
        source,
        request.key.target.clone(),
        original_url,
        destination,
    ) {
        Ok(pending) => pending,
        Err(error) => return super::links::failed(ctx, error),
    };
    let port_request = if let Some(entry) = ctx.state.ports.get_mut(&request.key) {
        match entry.state {
            ForwardState::Ready => {
                entry.browser_scheme = Some(request.scheme);
                let local_port = entry.local_port;
                return execute_resolved_open(ctx, pending, local_port);
            }
            ForwardState::Starting | ForwardState::Reconnecting => {
                entry.browser_scheme = Some(request.scheme);
                entry.pending_opens.push(pending);
                return Update::none();
            }
            ForwardState::AuthRequired | ForwardState::Failed => {
                LocalPortRequest::from_entry(entry)
            }
        }
    } else {
        LocalPortRequest::Unpinned
    };

    start_forward_entry(
        ctx,
        request.key,
        port_request,
        Some(request.scheme),
        true,
        vec![pending],
    )
}

fn unpinned_preferred_local_port(prefer_same_port: bool, remote_port: u16) -> Option<u16> {
    prefer_same_port.then_some(remote_port)
}

fn try_build_started_entry(
    ctx: &mut Context<AppRoot>,
    key: ForwardKey,
    local_request: LocalPortRequest,
    browser_scheme: Option<BrowserScheme>,
    interactive: bool,
) -> std::result::Result<PortForwardEntry, String> {
    let Some(link) = ctx.state.command_link.clone() else {
        return Err("Client runtime is not ready".to_string());
    };
    let (local_port, pin_local_port, local_port_mode) = match local_request {
        LocalPortRequest::Unpinned => preferred_local_port(unpinned_preferred_local_port(
            ctx.state.config.remote.ports.prefer_same_port,
            key.remote_port,
        ))
        .map(|port| (port, false, LocalPortMode::Auto))
        .map_err(|error| error.to_string())?,
        LocalPortRequest::Pinned(port) => (port, true, LocalPortMode::Auto),
        LocalPortRequest::Exact(port) => exact_local_port(port)
            .map(|port| (port, true, LocalPortMode::Exact))
            .map_err(|error| format!("local port {port} is unavailable: {error}"))?,
    };
    let generation = ctx.state.ports.mint_generation();
    let spec = ForwardSpec {
        target: key.target.clone(),
        connect_host: key.remote_host.connect_host().to_string(),
        remote_port: key.remote_port,
        local_port,
        pin_local_port,
    };
    let event_key = key.clone();
    let handle = start_forward(
        spec,
        ctx.state.config.remote.clone(),
        interactive,
        move |event| {
            link.send(crate::Msg::Ports(PortMsg::Event {
                key: event_key.clone(),
                generation,
                event,
            }));
        },
    )
    .map_err(|error| error.to_string())?;

    Ok(PortForwardEntry {
        key,
        local_port,
        port_pinned: pin_local_port,
        local_port_mode,
        browser_scheme,
        state: ForwardState::Starting,
        generation,
        handle: Some(handle),
        error: None,
        pending_opens: Vec::new(),
    })
}

fn commit_started_entry(
    ctx: &mut Context<AppRoot>,
    mut entry: PortForwardEntry,
    mut extra_pending: Vec<PendingLinkOpen>,
) -> Update {
    let mut pending_opens = ctx
        .state
        .ports
        .get_mut(&entry.key)
        .map(|existing| std::mem::take(&mut existing.pending_opens))
        .unwrap_or_default();
    pending_opens.append(&mut extra_pending);
    entry.pending_opens = pending_opens;
    ctx.state.ports.insert(entry);
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn start_forward_entry(
    ctx: &mut Context<AppRoot>,
    key: ForwardKey,
    local_request: LocalPortRequest,
    browser_scheme: Option<BrowserScheme>,
    interactive: bool,
    extra_pending: Vec<PendingLinkOpen>,
) -> Update {
    match try_build_started_entry(ctx, key, local_request, browser_scheme, interactive) {
        Ok(entry) => commit_started_entry(ctx, entry, extra_pending),
        Err(reason) => {
            crate::pane::pty_events::notify_error(ctx, "Could not forward port", reason);
            Update::full()
        }
    }
}

pub(crate) fn handle_event(
    ctx: &mut Context<AppRoot>,
    key: ForwardKey,
    generation: u64,
    event: ForwardEvent,
) -> Update {
    if !ctx.state.ports.matches_generation(&key, generation) {
        return Update::none();
    }
    match event {
        ForwardEvent::Ready { local_port } => {
            let (was_pinned, pending) = {
                let entry = ctx.state.ports.get_mut(&key).expect("matched generation");
                let was_pinned = entry.port_pinned;
                entry.local_port = local_port;
                entry.port_pinned = true;
                entry.state = ForwardState::Ready;
                entry.error = None;
                (was_pinned, std::mem::take(&mut entry.pending_opens))
            };
            let mut update = Update::full();
            if !was_pinned && local_port != key.remote_port {
                let host = ctx.state.remote_target_label(&key.target);
                crate::pane::pty_events::notify_info(
                    ctx,
                    format!(
                        "Forwarded `{host}:{}` → `localhost:{local_port}`",
                        key.remote_port
                    ),
                );
            }
            for open in pending {
                update = update.merge(execute_resolved_open(ctx, open, local_port));
            }
            update
        }
        ForwardEvent::Reconnecting { error } => {
            let entry = ctx.state.ports.get_mut(&key).expect("matched generation");
            entry.state = ForwardState::Reconnecting;
            entry.error = Some(error);
            Update::full()
        }
        ForwardEvent::AuthRequired { error } => {
            let pending = {
                let entry = ctx.state.ports.get_mut(&key).expect("matched generation");
                entry.state = ForwardState::AuthRequired;
                entry.error = Some(error.clone());
                entry.handle = None;
                std::mem::take(&mut entry.pending_opens)
            };
            if !pending.is_empty() {
                crate::pane::pty_events::notify_error(
                    ctx,
                    "Port forward requires authentication",
                    error,
                );
            }
            Update::full()
        }
        ForwardEvent::Failed { error } => {
            let pending = {
                let entry = ctx.state.ports.get_mut(&key).expect("matched generation");
                entry.state = ForwardState::Failed;
                entry.error = Some(error.clone());
                entry.handle = None;
                std::mem::take(&mut entry.pending_opens)
            };
            if !pending.is_empty() {
                crate::pane::pty_events::notify_error(ctx, "Could not forward port", error);
            }
            Update::full()
        }
    }
}

/// Close all tunnels targeting `target` (e.g. on disconnect or forget host).
pub(crate) fn close_target(state: &mut State, target: &RemoteTarget) {
    if !state.ports.remove_target(target).is_empty() {
        state.commands_dirty = true;
    }
    if let Some(overlay) = state.ports_overlay.as_mut() {
        if overlay
            .pending_close
            .as_ref()
            .is_some_and(|key| &key.target == target)
        {
            overlay.pending_close = None;
        }
        if overlay
            .selected
            .as_ref()
            .is_some_and(|key| &key.target == target)
        {
            overlay.selected = None;
        }
    }
    let next_selected = state
        .ports_overlay
        .as_ref()
        .map(|overlay| visible_sorted_keys(state, overlay.query.text()))
        .unwrap_or_else(|| sorted_keys(state))
        .into_iter()
        .next();
    if let Some(overlay) = state.ports_overlay.as_mut()
        && overlay.selected.is_none()
    {
        overlay.selected = next_selected;
    }
}

/// Close all client-owned tunnels (e.g. on exit, hangup, or relaunch).
pub(crate) fn close_all(state: &mut State) {
    if !state.ports.is_empty() {
        state.commands_dirty = true;
    }
    state.ports.clear();
    if let Some(overlay) = state.ports_overlay.as_mut() {
        overlay.pending_close = None;
        overlay.selected = None;
    }
}

/// Forward keys sorted deterministically: active session host first, then host label, remote
/// port, and remote host label.
pub(crate) fn sorted_keys(state: &State) -> Vec<ForwardKey> {
    let current = state.current().remote_target.as_ref();
    let mut keys: Vec<_> = state.ports.iter().map(|(key, _)| key.clone()).collect();
    keys.sort_by(|a, b| {
        let a_current = current == Some(&a.target);
        let b_current = current == Some(&b.target);
        b_current
            .cmp(&a_current)
            .then_with(|| {
                state
                    .remote_target_label(&a.target)
                    .cmp(&state.remote_target_label(&b.target))
            })
            .then_with(|| a.remote_port.cmp(&b.remote_port))
            .then_with(|| {
                a.remote_host
                    .display_label()
                    .cmp(b.remote_host.display_label())
            })
    });
    keys
}

pub(crate) fn forward_status_label(state: ForwardState) -> &'static str {
    match state {
        ForwardState::Ready => "Ready",
        ForwardState::Starting => "Starting…",
        ForwardState::Reconnecting => "Reconnecting…",
        ForwardState::AuthRequired => "Auth required",
        ForwardState::Failed => "Failed",
    }
}

pub(crate) fn forward_label(entry: &PortForwardEntry) -> String {
    let remote_host = entry.key.remote_host.display_label();
    let remote = if remote_host.contains(':') {
        format!("[{remote_host}]:{}", entry.key.remote_port)
    } else {
        format!("{remote_host}:{}", entry.key.remote_port)
    };
    if entry.local_port != 0 {
        format!("127.0.0.1:{} → {remote}", entry.local_port)
    } else {
        remote
    }
}

pub(crate) fn forward_description_text(state: &State, entry: &PortForwardEntry) -> String {
    let host = state.remote_target_label(&entry.key.target);
    let status = forward_status_label(entry.state);
    if matches!(
        entry.state,
        ForwardState::Failed | ForwardState::AuthRequired | ForwardState::Reconnecting
    ) && let Some(error) = entry.error.as_deref()
    {
        let reason = crate::session::discovery::probe_failure_reason(error);
        return crate::view::vocabulary::fields([host.as_str(), reason]);
    }
    crate::view::vocabulary::fields([host.as_str(), status])
}

pub(crate) fn forward_search_item(
    state: &State,
    entry: &PortForwardEntry,
) -> SearchItem<ForwardKey> {
    SearchItem::new(forward_label(entry), entry.key.clone())
        .description(ItemDescription::new().right(forward_description_text(state, entry)))
}

pub(crate) fn visible_sorted_keys(state: &State, query: &str) -> Vec<ForwardKey> {
    let items: Vec<_> = sorted_keys(state)
        .into_iter()
        .filter_map(|key| {
            state
                .ports
                .get(&key)
                .map(|entry| forward_search_item(state, entry))
        })
        .collect();
    tui_lipan::rank_search_palette_indices_with_mode(
        &items,
        query,
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    )
    .into_iter()
    .map(|index| items[index].value.clone())
    .collect()
}

pub(crate) fn forward_can_retry(entry: &PortForwardEntry) -> bool {
    matches!(
        entry.state,
        ForwardState::Failed | ForwardState::AuthRequired
    )
}

pub(crate) fn browser_open_url(entry: &PortForwardEntry) -> Option<String> {
    if entry.state != ForwardState::Ready || entry.local_port == 0 {
        return None;
    }
    let scheme = entry.browser_scheme?;
    let host = match entry.key.remote_host {
        ForwardHost::Localhost => "localhost",
        ForwardHost::Ipv4Loopback | ForwardHost::Ipv6Loopback | ForwardHost::Host(_) => "127.0.0.1",
    };
    Some(format!("{}://{host}:{}", scheme.as_str(), entry.local_port))
}

/// Remote SSH target for manual forward creation from the `Ports…` overlay.
pub(crate) fn default_forward_target(state: &State) -> Option<RemoteTarget> {
    state
        .current()
        .remote_target
        .clone()
        .or_else(|| {
            state
                .ports_overlay
                .as_ref()
                .and_then(|overlay| overlay.selected.as_ref())
                .map(|key| key.target.clone())
        })
        .or_else(|| state.ports.iter().next().map(|(key, _)| key.target.clone()))
        .or_else(|| {
            state
                .background
                .values()
                .find_map(|attachment| attachment.remote_target.clone())
        })
}

pub(crate) fn open_overlay(ctx: &mut Context<AppRoot>) -> Update {
    crate::actions::clear_non_settings_overlays(ctx);
    let selected = sorted_keys(&ctx.state).into_iter().next();
    ctx.state.ports_overlay = Some(PortsOverlayState::new(selected));
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_ports_overlay_focus(ctx);
    Update::full()
}

pub(crate) fn close_overlay(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.ports_overlay = None;
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    let visible = visible_sorted_keys(&ctx.state, &query);
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        overlay.query.set_text(query);
        overlay.pending_close = None;
        if overlay
            .selected
            .as_ref()
            .is_none_or(|key| !visible.contains(key))
        {
            overlay.selected = visible.into_iter().next();
        }
    }
    Update::full()
}

pub(crate) fn select_forward(ctx: &mut Context<AppRoot>, key: ForwardKey) -> Update {
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        if overlay.selected.as_ref() != Some(&key) {
            overlay.pending_close = None;
        }
        overlay.selected = Some(key);
    }
    Update::full()
}

pub(crate) fn activate_forward(ctx: &mut Context<AppRoot>, key: ForwardKey) -> Update {
    if let Some(overlay) = ctx.state.ports_overlay.as_ref()
        && !visible_sorted_keys(&ctx.state, overlay.query.text()).contains(&key)
    {
        return Update::none();
    }
    let Some(entry) = ctx.state.ports.get(&key) else {
        return Update::none();
    };
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        overlay.selected = Some(key.clone());
        overlay.pending_close = None;
    }
    if let Some(url) = browser_open_url(entry) {
        if let Err(error) = tui_lipan::utils::open_url(&url) {
            crate::pane::pty_events::notify_error(
                ctx,
                format!("Could not open `{url}`"),
                error.to_string(),
            );
        }
        return Update::full();
    }
    if forward_can_retry(entry) {
        let local_request = LocalPortRequest::from_entry(entry);
        let browser_scheme = entry.browser_scheme;
        return start_forward_entry(ctx, key, local_request, browser_scheme, true, Vec::new());
    }
    Update::none()
}

pub(crate) fn selected_overlay_key(state: &State) -> Option<ForwardKey> {
    let overlay = state.ports_overlay.as_ref()?;
    let visible = visible_sorted_keys(state, overlay.query.text());
    overlay
        .selected
        .as_ref()
        .filter(|key| visible.contains(key))
        .cloned()
        .or_else(|| visible.into_iter().next())
}

pub(crate) fn retry_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some(key) = selected_overlay_key(&ctx.state) else {
        return Update::none();
    };
    let Some(entry) = ctx.state.ports.get(&key) else {
        return Update::none();
    };
    if !forward_can_retry(entry) {
        return Update::none();
    }
    let local_request = LocalPortRequest::from_entry(entry);
    let browser_scheme = entry.browser_scheme;
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        overlay.pending_close = None;
        overlay.selected = Some(key.clone());
    }
    start_forward_entry(ctx, key, local_request, browser_scheme, true, Vec::new())
}

pub(crate) fn close_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some(key) = selected_overlay_key(&ctx.state) else {
        return Update::none();
    };
    let armed = ctx
        .state
        .ports_overlay
        .as_ref()
        .is_some_and(|overlay| overlay.pending_close.as_ref() == Some(&key));
    if !armed {
        if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
            overlay.pending_close = Some(key);
        }
        return Update::full();
    }

    ctx.state.ports.remove(&key);
    ctx.state.commands_dirty = true;
    let query = ctx
        .state
        .ports_overlay
        .as_ref()
        .map(|overlay| overlay.query.text().to_string())
        .unwrap_or_default();
    let next = visible_sorted_keys(&ctx.state, &query).into_iter().next();
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        overlay.pending_close = None;
        overlay.selected = next;
    }
    let host = ctx.state.remote_target_label(&key.target);
    crate::pane::pty_events::notify_info(
        ctx,
        format!(
            "Closed forward `{host} · {}:{}`",
            key.remote_host.display_label(),
            key.remote_port
        ),
    );
    Update::full()
}

pub(crate) fn open_add_form(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = default_forward_target(&ctx.state) else {
        return Update::none();
    };
    let Some(overlay) = ctx.state.ports_overlay.as_mut() else {
        return Update::none();
    };
    overlay.pending_close = None;
    overlay.form = Some(PortFormState::new(target));
    crate::ops::focus::request_port_form_focus(ctx);
    Update::full()
}

pub(crate) fn close_add_form(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        overlay.form = None;
    }
    crate::ops::focus::request_ports_overlay_focus(ctx);
    Update::full()
}

pub(crate) fn port_form_is_open(state: &State) -> bool {
    state
        .ports_overlay
        .as_ref()
        .is_some_and(|overlay| overlay.form.is_some())
}

pub(crate) fn port_form_cycle_focus(ctx: &mut Context<AppRoot>, forward: bool) -> Update {
    if let Some(form) = ctx
        .state
        .ports_overlay
        .as_mut()
        .and_then(|overlay| overlay.form.as_mut())
    {
        form.cycle_focus(forward);
    }
    crate::ops::focus::request_port_form_focus(ctx);
    Update::full()
}

pub(crate) fn form_changed(
    ctx: &mut Context<AppRoot>,
    field: PortFormField,
    event: InputEvent,
) -> Update {
    if let Some(form) = ctx
        .state
        .ports_overlay
        .as_mut()
        .and_then(|overlay| overlay.form.as_mut())
    {
        form.focus = field;
        event.apply_to(form.input_mut(field));
        form.error = None;
    }
    Update::full()
}

fn reject_port_form(ctx: &mut Context<AppRoot>, error: String) -> Update {
    if let Some(form) = ctx
        .state
        .ports_overlay
        .as_mut()
        .and_then(|overlay| overlay.form.as_mut())
    {
        form.error = Some(error);
    }
    crate::ops::focus::request_port_form_focus(ctx);
    Update::full()
}

enum ForwardRequestOutcome {
    Reused,
    Started(Update),
}

struct ForwardRequestError {
    message: String,
    state_changed: bool,
}

fn ensure_requested_forward(
    ctx: &mut Context<AppRoot>,
    key: ForwardKey,
    requested_exact: Option<u16>,
) -> std::result::Result<ForwardRequestOutcome, ForwardRequestError> {
    let (reuse_existing, preserved_scheme, stop_existing_first) =
        if let Some(existing) = ctx.state.ports.get(&key) {
            let active = !matches!(
                existing.state,
                ForwardState::Failed | ForwardState::AuthRequired
            );
            let same_port = requested_exact == Some(existing.local_port);
            let already_satisfied = active
                && match requested_exact {
                    None => true,
                    Some(_) => existing.local_port_mode == LocalPortMode::Exact && same_port,
                };
            (
                already_satisfied,
                existing.browser_scheme,
                active && same_port && requested_exact.is_some(),
            )
        } else {
            (false, None, false)
        };

    if reuse_existing {
        return Ok(ForwardRequestOutcome::Reused);
    }

    let local_request = match requested_exact {
        Some(port) => LocalPortRequest::Exact(port),
        None => LocalPortRequest::Unpinned,
    };

    let removed_pending = if stop_existing_first {
        if ctx.state.command_link.is_none() {
            return Err(ForwardRequestError {
                message: "Client runtime is not ready".into(),
                state_changed: false,
            });
        }
        // Stop the active Auto forward on `:port` first so the new Exact worker owns the socket
        // with `pin_local_port = true` and a fresh generation.
        let pending = ctx
            .state
            .ports
            .remove(&key)
            .map(|entry| entry.pending_opens)
            .unwrap_or_default();
        ctx.state.commands_dirty = true;
        Some(pending)
    } else {
        None
    };

    match try_build_started_entry(ctx, key.clone(), local_request, preserved_scheme, true) {
        Ok(entry) => Ok(ForwardRequestOutcome::Started(commit_started_entry(
            ctx,
            entry,
            removed_pending.unwrap_or_default(),
        ))),
        Err(reason) => {
            let mut state_changed = false;
            if let Some(pending_opens) = removed_pending {
                let failed_gen = ctx.state.ports.mint_generation();
                ctx.state.ports.insert(PortForwardEntry {
                    key: key.clone(),
                    local_port: requested_exact.unwrap_or_default(),
                    port_pinned: true,
                    local_port_mode: LocalPortMode::Exact,
                    browser_scheme: preserved_scheme,
                    state: ForwardState::Failed,
                    generation: failed_gen,
                    handle: None,
                    error: Some(reason.clone()),
                    pending_opens,
                });
                ctx.state.commands_dirty = true;
                if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
                    overlay.pending_close = None;
                    overlay.selected = Some(key);
                }
                state_changed = true;
            }
            Err(ForwardRequestError {
                message: reason,
                state_changed,
            })
        }
    }
}

pub(crate) fn submit_add_form(ctx: &mut Context<AppRoot>) -> Update {
    let parsed = ctx
        .state
        .ports_overlay
        .as_ref()
        .and_then(|overlay| overlay.form.as_ref())
        .map(PortFormState::parse_request);
    let Some(parsed) = parsed else {
        return Update::none();
    };
    let (key, local_port) = match parsed {
        Ok(pair) => pair,
        Err(error) => return reject_port_form(ctx, error),
    };
    let requested_exact = (local_port != 0).then_some(local_port);

    match ensure_requested_forward(ctx, key.clone(), requested_exact) {
        Ok(outcome) => {
            if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
                overlay.form = None;
                overlay.pending_close = None;
                overlay.selected = Some(key);
            }
            crate::ops::focus::request_ports_overlay_focus(ctx);
            match outcome {
                ForwardRequestOutcome::Reused => Update::full(),
                ForwardRequestOutcome::Started(update) => update,
            }
        }
        Err(err) => reject_port_form(ctx, err.message),
    }
}

fn control_forward_status(state: ForwardState) -> crate::control::PortForwardStatus {
    match state {
        ForwardState::Starting => crate::control::PortForwardStatus::Starting,
        ForwardState::Ready => crate::control::PortForwardStatus::Ready,
        ForwardState::Reconnecting => crate::control::PortForwardStatus::Reconnecting,
        ForwardState::AuthRequired => crate::control::PortForwardStatus::AuthRequired,
        ForwardState::Failed => crate::control::PortForwardStatus::Failed,
    }
}

fn control_forward_info(
    state: &State,
    entry: &PortForwardEntry,
) -> crate::control::PortForwardInfo {
    crate::control::PortForwardInfo {
        target: entry.key.target.to_spec(),
        target_label: state.remote_target_label(&entry.key.target),
        remote_host: entry.key.remote_host.display_label().to_string(),
        remote_port: entry.key.remote_port,
        local_port: (entry.local_port != 0).then_some(entry.local_port),
        state: control_forward_status(entry.state),
        error: entry.error.clone(),
    }
}

pub(crate) fn list_control_forwards(state: &State) -> crate::control::PortForwardListPayload {
    let entries = sorted_keys(state)
        .into_iter()
        .filter_map(|key| state.ports.get(&key))
        .map(|entry| control_forward_info(state, entry))
        .collect();
    crate::control::PortForwardListPayload(entries)
}

fn resolve_control_forward_key(
    state: &State,
    remote: Option<&str>,
    host: Option<&str>,
    port: u16,
    for_close: bool,
) -> std::result::Result<ForwardKey, String> {
    if port == 0 {
        return Err("remote port must be between 1 and 65535".to_string());
    }
    let remote_host = match host {
        Some(raw) => ForwardHost::parse(raw)?,
        None => ForwardHost::Localhost,
    };
    let target = match remote {
        Some(raw) => crate::session::remote::parse_remote_target(raw)?,
        None => {
            if let Some(current) = state.current().remote_target.clone() {
                current
            } else if for_close {
                let mut distinct_targets: Vec<RemoteTarget> = Vec::new();
                for key in sorted_keys(state) {
                    if !distinct_targets.contains(&key.target) {
                        distinct_targets.push(key.target);
                    }
                }
                match distinct_targets.len() {
                    0 => {
                        return Err("no active remote host; pass `--remote <HOST>`".to_string());
                    }
                    1 => distinct_targets.remove(0),
                    _ => {
                        return Err(
                            "multiple remote hosts have active forwards; pass `--remote <HOST>`"
                                .to_string(),
                        );
                    }
                }
            } else {
                return Err("no active remote host; pass `--remote <HOST>`".to_string());
            }
        }
    };
    Ok(ForwardKey {
        target,
        remote_host,
        remote_port: port,
    })
}

pub(crate) fn control_forward(
    ctx: &mut Context<AppRoot>,
    remote: Option<&str>,
    host: Option<&str>,
    port: u16,
    local_port: Option<u16>,
) -> (crate::control::ControlResponse, Update) {
    if local_port == Some(0) {
        return (
            crate::control::ControlResponse::error_with(
                crate::control::ControlErrorCode::InvalidArgument,
                "local port must be between 1 and 65535",
            ),
            Update::none(),
        );
    }
    let key = match resolve_control_forward_key(&ctx.state, remote, host, port, false) {
        Ok(key) => key,
        Err(message) => {
            return (
                crate::control::ControlResponse::error_with(
                    crate::control::ControlErrorCode::InvalidArgument,
                    message,
                ),
                Update::none(),
            );
        }
    };

    match ensure_requested_forward(ctx, key.clone(), local_port) {
        Ok(outcome) => {
            let update = match outcome {
                ForwardRequestOutcome::Reused => Update::none(),
                ForwardRequestOutcome::Started(update) => update,
            };
            let Some(entry) = ctx.state.ports.get(&key) else {
                return (
                    crate::control::ControlResponse::error_with(
                        crate::control::ControlErrorCode::RequestFailed,
                        "port forward state disappeared after start",
                    ),
                    update,
                );
            };
            (
                crate::control::ControlResponse::ok(control_forward_info(&ctx.state, entry)),
                update,
            )
        }
        Err(err) => (
            crate::control::ControlResponse::error_with(
                crate::control::ControlErrorCode::RequestFailed,
                err.message,
            ),
            if err.state_changed {
                Update::full()
            } else {
                Update::none()
            },
        ),
    }
}

pub(crate) fn control_close(
    ctx: &mut Context<AppRoot>,
    remote: Option<&str>,
    host: Option<&str>,
    port: u16,
) -> (crate::control::ControlResponse, Update) {
    let key = match resolve_control_forward_key(&ctx.state, remote, host, port, true) {
        Ok(key) => key,
        Err(message) => {
            return (
                crate::control::ControlResponse::error_with(
                    crate::control::ControlErrorCode::InvalidArgument,
                    message,
                ),
                Update::none(),
            );
        }
    };

    let Some(entry) = ctx.state.ports.remove(&key) else {
        return (
            crate::control::ControlResponse::ok(crate::control::PortForwardClosed {
                removed: false,
                forward: None,
            }),
            Update::none(),
        );
    };

    ctx.state.commands_dirty = true;
    let info = control_forward_info(&ctx.state, &entry);
    drop(entry);
    let next = sorted_keys(&ctx.state).into_iter().next();
    if let Some(overlay) = ctx.state.ports_overlay.as_mut() {
        if overlay.pending_close.as_ref() == Some(&key) {
            overlay.pending_close = None;
        }
        if overlay.selected.as_ref() == Some(&key) {
            overlay.selected = next;
        }
    }
    (
        crate::control::ControlResponse::ok(crate::control::PortForwardClosed {
            removed: true,
            forward: Some(info),
        }),
        Update::full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LinkAction, NamedCommand, UserCommandAction};
    use crate::session::remote::forward::ForwardHandle;
    use crate::state::Pane;
    use tui_lipan::TestBackend;

    fn remote_state(target: RemoteTarget) -> (State, PaneId, PaneId, PaneId) {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().remote_host = Some(target.display_label());
        state.current_mut().remote_target = Some(target);
        let remote_pane_id = 10;
        let nested_ssh_pane_id = 11;
        let scratch_pane_id = 12;
        state.current_mut().workspaces[0].panes.push(Pane::new(
            remote_pane_id,
            100,
            Default::default(),
        ));
        let mut nested = Pane::new(nested_ssh_pane_id, 100, Default::default());
        nested.terminal.cwd_host = Some("prod".into());
        state.current_mut().workspaces[0].panes.push(nested);
        state
            .scratch
            .panes
            .push(Pane::new(scratch_pane_id, 100, Default::default()));
        (state, remote_pane_id, nested_ssh_pane_id, scratch_pane_id)
    }

    #[test]
    fn url_policy_selects_only_remote_loopback_http_and_https_for_client_openers() {
        let workbox = RemoteTarget::Alias("workbox".into());
        let (mut state, remote_pane, nested_pane, scratch_pane) = remote_state(workbox.clone());

        let request = |state: &State, pane, raw, dest: OpenDestination| {
            let parsed = url::Url::parse(raw).unwrap();
            forward_request_for_link(state, pane, &parsed, &dest).map(|req| req.key)
        };

        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://localhost:5173/app",
                OpenDestination::System
            ),
            Some(ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 5173,
            })
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://127.0.0.1:5173",
                OpenDestination::System
            ),
            Some(ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Ipv4Loopback,
                remote_port: 5173,
            })
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://[::1]:5173",
                OpenDestination::System
            ),
            Some(ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Ipv6Loopback,
                remote_port: 5173,
            })
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://0.0.0.0:8000",
                OpenDestination::System
            ),
            Some(ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Ipv4Loopback,
                remote_port: 8000,
            })
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://[::]:8000",
                OpenDestination::System
            ),
            Some(ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Ipv6Loopback,
                remote_port: 8000,
            })
        );
        assert_ne!(
            request(
                &state,
                remote_pane,
                "http://0.0.0.0:8000",
                OpenDestination::System
            ),
            request(
                &state,
                remote_pane,
                "http://[::]:8000",
                OpenDestination::System
            )
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "https://localhost",
                OpenDestination::System
            ),
            Some(ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 443,
            })
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "https://example.com",
                OpenDestination::System
            ),
            None
        );
        assert_eq!(
            request(
                &state,
                scratch_pane,
                "http://localhost:5173",
                OpenDestination::System
            ),
            None
        );
        assert_eq!(
            request(
                &state,
                nested_pane,
                "http://localhost:5173",
                OpenDestination::System
            ),
            None
        );

        // Handler location checks.
        assert!(
            request(
                &state,
                remote_pane,
                "http://localhost:5173",
                OpenDestination::Handler(LinkAction::Inline(UserCommandAction::Exec {
                    command: "firefox \"$ROZI_URL\"".into(),
                })),
            )
            .is_some()
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://localhost:5173",
                OpenDestination::Handler(LinkAction::Inline(UserCommandAction::popup("browser"))),
            ),
            None
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://localhost:5173",
                OpenDestination::Handler(LinkAction::Inline(UserCommandAction::run("browser"))),
            ),
            None
        );

        // Placed remote command runs on SessionHost, so no tunnel is needed.
        state.config.commands.push(NamedCommand {
            id: "ext.browser".into(),
            label: None,
            action: UserCommandAction::Exec {
                command: "browser".into(),
            },
            category: "Browser".into(),
            env: Vec::new(),
            default_key: None,
            hidden: false,
        });
        let bundle = std::sync::Arc::new(
            crate::extension_runtime::bundle::Bundle::from_files(Vec::new()).unwrap(),
        );
        let mut commands = std::collections::BTreeMap::new();
        commands.insert(
            "ext.browser".to_string(),
            crate::config::PlacedCommand {
                placement: crate::config::Placement::ActiveSession,
                launch: crate::config::LaunchTemplate::Shell("browser".into()),
            },
        );
        state.config.extension_placements.insert(
            "ext".into(),
            std::sync::Arc::new(crate::config::ExtensionPlacements {
                id: "ext".into(),
                platforms: Vec::new(),
                local_dir: "/x".into(),
                bundle,
                commands,
                tabs: Default::default(),
                services: Vec::new(),
                client_unsupported: None,
            }),
        );
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://localhost:5173",
                OpenDestination::Handler(LinkAction::Command("ext.browser".into())),
            ),
            None
        );

        // Local session pane never triggers forwarding.
        state.current_mut().remote_host = None;
        state.current_mut().remote_target = None;
        assert_eq!(
            request(
                &state,
                remote_pane,
                "http://localhost:5173",
                OpenDestination::System
            ),
            None
        );
    }

    #[test]
    fn url_rewriting_preserves_path_query_fragment_and_local_loopback_host() {
        assert_eq!(
            rewrite_forwarded_url("http://localhost:5173/foo?q=x#bar", 5173).as_deref(),
            Some("http://localhost:5173/foo?q=x#bar")
        );
        assert_eq!(
            rewrite_forwarded_url("http://localhost:5173/foo?q=x#bar", 49184).as_deref(),
            Some("http://localhost:49184/foo?q=x#bar")
        );
        assert_eq!(
            rewrite_forwarded_url("http://0.0.0.0:8000/api?v=1", 49185).as_deref(),
            Some("http://127.0.0.1:49185/api?v=1")
        );
        assert_eq!(
            rewrite_forwarded_url("http://[::1]:3000/home", 3000).as_deref(),
            Some("http://127.0.0.1:3000/home")
        );
        assert_eq!(
            rewrite_forwarded_url("http://[::]:3000/home", 49187).as_deref(),
            Some("http://127.0.0.1:49187/home")
        );
        assert_eq!(
            rewrite_forwarded_url("https://localhost/login", 49186).as_deref(),
            Some("https://localhost:49186/login")
        );
    }

    #[test]
    fn registry_keys_distinguish_targets_queue_pending_opens_and_fence_stale_generations() {
        crate::test_support::on_large_stack(check_registry_and_delayed_open);
    }

    #[test]
    fn delayed_scratch_run_stays_on_client_after_scratchpad_closes() {
        crate::test_support::on_large_stack(check_delayed_scratch_run_stays_on_client);
    }

    #[cfg(unix)]
    #[test]
    fn ready_drains_queued_link_handlers_with_rewritten_urls_and_original_hints() {
        crate::test_support::on_large_stack(check_ready_drains_exec_handlers);
    }

    #[cfg(unix)]
    #[test]
    fn delayed_exec_pins_originating_session_across_switch_and_cancels_when_closed() {
        crate::test_support::on_large_stack(check_delayed_exec_session_pinning);
    }

    fn check_registry_and_delayed_open() {
        crate::test_support::isolate_user_dirs();

        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = RemoteTarget::Alias("workbox".into());
        let prod = RemoteTarget::Alias("prod".into());
        let source = backend.state().current().workspaces[0].panes[0].id;

        let key_workbox = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Ipv4Loopback,
            remote_port: 5173,
        };
        let key_prod = ForwardKey {
            target: prod.clone(),
            remote_host: ForwardHost::Ipv4Loopback,
            remote_port: 5173,
        };

        let gen1 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Starting,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        let gen_prod = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_prod.clone(),
            local_port: 49200,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Ready,
            generation: gen_prod,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        assert_eq!(backend.state().ports.len(), 2);

        // Configure a non-spawning http handler so Ready drains pending opens offline on all OSes.
        backend.state_mut().current_mut().remote_host = Some("workbox".into());
        backend.state_mut().current_mut().remote_target = Some(workbox.clone());
        backend.state_mut().config.commands.push(NamedCommand {
            id: "test.noop".into(),
            label: None,
            action: UserCommandAction::ExecDirect { argv: Vec::new() },
            category: "Test".into(),
            env: Vec::new(),
            default_key: None,
            hidden: false,
        });
        backend.state_mut().config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Command("test.noop".into()),
        }];

        // Two requests while Starting queue behind the existing entry without spawning another forward.
        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://0.0.0.0:5173/first".into(),
            ))
            .unwrap();
        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://127.0.0.1:5173/second".into(),
            ))
            .unwrap();
        assert_eq!(
            backend
                .state()
                .ports
                .get(&key_workbox)
                .unwrap()
                .pending_opens
                .len(),
            2
        );
        assert_eq!(backend.state().ports.len(), 2);

        // Stale generation event is ignored.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1 + 99,
                event: ForwardEvent::Failed {
                    error: "stale".into(),
                },
            }))
            .unwrap();
        assert_eq!(
            backend.state().ports.get(&key_workbox).unwrap().state,
            ForwardState::Starting
        );

        // Ready transitions the entry, updates local_port, and drains pending opens.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1,
                event: ForwardEvent::Ready { local_port: 49184 },
            }))
            .unwrap();
        let entry = backend.state().ports.get(&key_workbox).unwrap();
        assert_eq!(entry.state, ForwardState::Ready);
        assert_eq!(entry.local_port, 49184);
        assert!(entry.pending_opens.is_empty());

        // Closing target removes only that target's forwards.
        close_target(backend.state_mut(), &workbox);
        assert!(backend.state().ports.get(&key_workbox).is_none());
        assert!(backend.state().ports.get(&key_prod).is_some());

        // A late event for a closed forward is ignored.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1,
                event: ForwardEvent::Ready { local_port: 49184 },
            }))
            .unwrap();
        assert!(backend.state().ports.get(&key_workbox).is_none());

        close_all(backend.state_mut());
        assert!(backend.state().ports.is_empty());
    }

    fn check_delayed_scratch_run_stays_on_client() {
        crate::test_support::isolate_user_dirs();

        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = RemoteTarget::Alias("workbox".into());
        let source = backend.state().current().workspaces[0].panes[0].id;
        let key_workbox = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };
        let gen1 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Starting,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().remote_host = Some("workbox".into());
        state.current_mut().remote_target = Some(workbox);
        state.current_mut().session_instance = Some(
            crate::session::protocol::SessionInstanceId::for_test("remote-inst"),
        );
        state.current_mut().workspaces[0].panes[0].terminal.cwd = Some("/remote/project".into());
        state.scratch_visible = true;
        state.config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(UserCommandAction::Run {
                command: "echo \"$ROZI_URL\"".into(),
                keep_open: false,
            }),
        }];

        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://localhost:5173/app".into(),
            ))
            .unwrap();
        assert_eq!(
            backend
                .state()
                .ports
                .get(&key_workbox)
                .unwrap()
                .pending_opens
                .len(),
            1
        );

        // Close the scratchpad before the tunnel finishes connecting.
        backend.state_mut().scratch_visible = false;

        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox,
                generation: gen1,
                event: ForwardEvent::Ready { local_port: 49184 },
            }))
            .unwrap();

        // Must spawn in client scratchpad, never on the remote session host.
        assert!(backend.state().current().pending_spawns.is_empty());
        assert_eq!(backend.state().scratch.panes.len(), 1);
        let scratch_pane = &backend.state().scratch.panes[0];
        assert_eq!(scratch_pane.identity.cwd, None);
        assert!(
            scratch_pane
                .identity
                .env
                .iter()
                .any(|(k, v)| k == "ROZI_URL" && v == "http://localhost:49184/app")
        );
        assert!(
            scratch_pane
                .identity
                .env
                .iter()
                .any(|(k, v)| k == "ROZI_SOURCE_SESSION" && v == "remote-inst")
        );
    }

    #[cfg(unix)]
    fn check_ready_drains_exec_handlers() {
        crate::test_support::isolate_user_dirs();
        let scratch = tempfile::tempdir().unwrap();
        let out = scratch.path().join("opened");

        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(std::time::Instant::now() < deadline);
            backend.pump().unwrap();
            std::thread::yield_now();
        }

        let workbox = RemoteTarget::Alias("workbox".into());
        let source = backend.state().current().workspaces[0].panes[0].id;
        let key_localhost = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };
        let key_ipv4 = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Ipv4Loopback,
            remote_port: 5173,
        };
        let gen_localhost = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_localhost.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Starting,
            generation: gen_localhost,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        let gen_ipv4 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_ipv4.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Starting,
            generation: gen_ipv4,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        backend.state_mut().current_mut().remote_host = Some("workbox".into());
        backend.state_mut().current_mut().remote_target = Some(workbox);
        let action = UserCommandAction::Exec {
            command: format!(
                "printf '%s\\t%s\\n' \"$ROZI_HINT\" \"$ROZI_URL\" >> '{}'",
                out.display()
            ),
        };
        backend.state_mut().config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(action),
        }];

        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://localhost:5173/first".into(),
            ))
            .unwrap();
        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://127.0.0.1:5173/second".into(),
            ))
            .unwrap();

        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_localhost,
                generation: gen_localhost,
                event: ForwardEvent::Ready { local_port: 49184 },
            }))
            .unwrap();
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_ipv4,
                generation: gen_ipv4,
                event: ForwardEvent::Ready { local_port: 49185 },
            }))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let output = loop {
            backend.pump().unwrap();
            if let Ok(text) = std::fs::read_to_string(&out)
                && text.lines().count() == 2
            {
                break text;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        let mut lines: Vec<_> = output.lines().collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                "http://127.0.0.1:5173/second\thttp://127.0.0.1:49185/second",
                "http://localhost:5173/first\thttp://localhost:49184/first",
            ]
        );
    }

    #[cfg(unix)]
    fn check_delayed_exec_session_pinning() {
        crate::test_support::isolate_user_dirs();
        let scratch = tempfile::tempdir().unwrap();
        let out = scratch.path().join("pinned");

        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(std::time::Instant::now() < deadline);
            backend.pump().unwrap();
            std::thread::yield_now();
        }

        let workbox = RemoteTarget::Alias("workbox".into());
        let source = backend.state().current().workspaces[0].panes[0].id;
        let key_workbox = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };
        let gen1 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Starting,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        {
            let state = backend.state_mut();
            state.runtime_epoch = 10;
            state.current_mut().epoch = 10;
            state.current_mut().session_attached = true;
            state.current_mut().session_name = Some("workbox-dev".into());
            state.current_mut().session_instance = Some(
                crate::session::protocol::SessionInstanceId::for_test("inst-workbox"),
            );
            state.current_mut().remote_host = Some("workbox".into());
            state.current_mut().remote_target = Some(workbox.clone());
            state.config.commands.push(NamedCommand {
                id: "ext.open".into(),
                label: None,
                action: UserCommandAction::Exec {
                    command: format!(
                        "printf '%s\\t%s\\t%s\\t%s\\n' \"$ROZI_URL\" \"$ROZI_SOURCE_SESSION\" \"$ROZI_REMOTE_HOST\" \"$ROZI_SESSION_NAME\" > '{}'",
                        out.display()
                    ),
                },
                category: "Test".into(),
                env: vec![("ROZI_EXTENSION".into(), "ext".into())],
                default_key: None,
                hidden: false,
            });
            state.config.link_handlers = vec![crate::config::LinkHandler {
                schemes: vec!["http".into()],
                files: Vec::new(),
                action: LinkAction::Command("ext.open".into()),
            }];
        }

        // Queue one open while attached to `workbox-dev` (epoch 10).
        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://localhost:5173/parked".into(),
            ))
            .unwrap();

        // Park epoch 10 and switch to a different local session at epoch 11.
        {
            let state = backend.state_mut();
            let mut local = crate::state::Attachment::new();
            local.epoch = 11;
            local.session_attached = true;
            local.session_name = Some("local-shell".into());
            local.session_instance = Some(crate::session::protocol::SessionInstanceId::for_test(
                "inst-local",
            ));
            state.park_current(10, local);
            state.runtime_epoch = 11;
        }

        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1,
                event: ForwardEvent::Ready { local_port: 49184 },
            }))
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let output = loop {
            backend.pump().unwrap();
            if let Ok(text) = std::fs::read_to_string(&out)
                && text.ends_with('\n')
            {
                break text;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(
            output.trim_end(),
            "http://localhost:49184/parked\tinst-workbox\tworkbox\tworkbox-dev"
        );
        std::fs::remove_file(&out).unwrap();

        // Now queue another open on epoch 12 (`workbox`), then drop epoch 12 before Ready.
        let gen2 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Starting,
            generation: gen2,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        {
            let state = backend.state_mut();
            let mut other_remote = crate::state::Attachment::new();
            other_remote.epoch = 12;
            other_remote.session_attached = true;
            other_remote.session_name = Some("workbox-ephemeral".into());
            other_remote.session_instance = Some(
                crate::session::protocol::SessionInstanceId::for_test("inst-ephemeral"),
            );
            other_remote.remote_host = Some("workbox".into());
            other_remote.remote_target = Some(workbox);
            other_remote.workspaces[0]
                .panes
                .push(Pane::new(99, 100, Default::default()));
            state.attachment = other_remote;
            state.runtime_epoch = 12;
        }
        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                99,
                TerminalLinkKind::Url,
                "http://localhost:5173/closed".into(),
            ))
            .unwrap();
        assert_eq!(
            backend
                .state()
                .ports
                .get(&key_workbox)
                .unwrap()
                .pending_opens
                .len(),
            1
        );

        // Close epoch 12 without parking it, leaving epoch 13 active.
        {
            let state = backend.state_mut();
            state.attachment = crate::state::Attachment::new();
            state.runtime_epoch = 13;
            state.current_mut().epoch = 13;
        }
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox,
                generation: gen2,
                event: ForwardEvent::Ready { local_port: 49185 },
            }))
            .unwrap();
        for _ in 0..20 {
            backend.pump().unwrap();
            std::thread::yield_now();
        }
        assert!(
            !out.exists(),
            "delayed handler must cancel when its originating session no longer exists"
        );
    }

    #[test]
    fn reconnecting_forward_queues_click_and_preserves_pinned_port() {
        crate::test_support::on_large_stack(check_reconnecting_forward_queues_click);
    }

    fn check_reconnecting_forward_queues_click() {
        crate::test_support::isolate_user_dirs();

        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = RemoteTarget::Alias("workbox".into());
        let source = backend.state().current().workspaces[0].panes[0].id;
        let key_workbox = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };

        let gen1 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: 49184,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Ready,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        let state = backend.state_mut();
        state.current_mut().remote_host = Some("workbox".into());
        state.current_mut().remote_target = Some(workbox);
        state.scratch_visible = true;
        state.config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(UserCommandAction::Run {
                command: "echo \"$ROZI_URL\"".into(),
                keep_open: false,
            }),
        }];

        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1,
                event: ForwardEvent::Reconnecting {
                    error: "connection reset".into(),
                },
            }))
            .unwrap();
        assert_eq!(
            backend.state().ports.get(&key_workbox).unwrap().state,
            ForwardState::Reconnecting
        );

        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://localhost:5173/reconnected".into(),
            ))
            .unwrap();

        let entry = backend.state().ports.get(&key_workbox).unwrap();
        assert_eq!(entry.generation, gen1);
        assert_eq!(entry.local_port, 49184);
        assert!(entry.port_pinned);
        assert_eq!(entry.state, ForwardState::Reconnecting);
        assert_eq!(entry.pending_opens.len(), 1);

        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1,
                event: ForwardEvent::Ready { local_port: 49184 },
            }))
            .unwrap();

        assert_eq!(backend.state().scratch.panes.len(), 1);
        assert!(
            backend.state().scratch.panes[0]
                .identity
                .env
                .iter()
                .any(|(k, v)| k == "ROZI_URL" && v == "http://localhost:49184/reconnected")
        );
        let entry = backend.state().ports.get(&key_workbox).unwrap();
        assert_eq!(entry.state, ForwardState::Ready);
        assert_eq!(entry.local_port, 49184);
        assert!(entry.pending_opens.is_empty());
    }

    #[test]
    fn retrying_post_ready_auth_required_preserves_pinned_port_when_occupied() {
        crate::test_support::on_large_stack(check_retrying_post_ready_auth_required);
    }

    fn check_retrying_post_ready_auth_required() {
        crate::test_support::isolate_user_dirs();

        let occupied_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied port");
        let occupied_port = occupied_listener.local_addr().expect("local addr").port();

        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for TestBackend command_link"
            );
            backend.pump().unwrap();
            std::thread::yield_now();
        }

        let workbox = RemoteTarget::Alias("workbox".into());
        let source = backend.state().current().workspaces[0].panes[0].id;
        let key_workbox = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };

        let gen1 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: occupied_port,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Ready,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        let state = backend.state_mut();
        state.current_mut().remote_host = Some("workbox".into());
        state.current_mut().remote_target = Some(workbox);
        state.scratch_visible = true;
        state.config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(UserCommandAction::Run {
                command: "echo \"$ROZI_URL\"".into(),
                keep_open: false,
            }),
        }];

        backend
            .dispatch(crate::Msg::Ports(PortMsg::Event {
                key: key_workbox.clone(),
                generation: gen1,
                event: ForwardEvent::AuthRequired {
                    error: "Permission denied".into(),
                },
            }))
            .unwrap();
        let entry = backend.state().ports.get(&key_workbox).unwrap();
        assert_eq!(entry.state, ForwardState::AuthRequired);
        assert_eq!(entry.local_port, occupied_port);
        assert!(entry.port_pinned);

        backend
            .dispatch(crate::Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                "http://localhost:5173/retry".into(),
            ))
            .unwrap();

        let restarted = backend.state().ports.get(&key_workbox).unwrap();
        assert_ne!(restarted.generation, gen1);
        assert_eq!(restarted.local_port, occupied_port);
        assert!(restarted.port_pinned);
        assert!(
            matches!(
                restarted.state,
                ForwardState::Starting | ForwardState::Failed
            ),
            "unexpected initial state after restart: {:?}",
            restarted.state
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            backend.pump().unwrap();
            let entry = backend.state().ports.get(&key_workbox).unwrap();
            if entry.state != ForwardState::Starting {
                assert_eq!(entry.state, ForwardState::Failed);
                assert_eq!(entry.local_port, occupied_port);
                assert!(entry.port_pinned);
                assert!(entry.pending_opens.is_empty());
                assert!(
                    entry
                        .error
                        .as_deref()
                        .is_some_and(|msg| msg.contains(&occupied_port.to_string())),
                    "unexpected error: {:?}",
                    entry.error
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for pinned restart bind conflict failure"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            backend.state().scratch.panes.is_empty(),
            "pinned restart must never emit Ready or open on a different local port"
        );
    }

    #[test]
    fn port_form_validates_fields_and_cycles_focus() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut form = PortFormState::new(target.clone());
        assert_eq!(form.focus, PortFormField::RemotePort);
        assert_eq!(form.remote_host.text(), "localhost");

        // Tab and Shift+Tab cycle cleanly through all three fields in order.
        form.cycle_focus(true);
        assert_eq!(form.focus, PortFormField::LocalPort);
        form.cycle_focus(true);
        assert_eq!(form.focus, PortFormField::RemoteHost);
        form.cycle_focus(false);
        assert_eq!(form.focus, PortFormField::LocalPort);

        // Missing or invalid ports produce clear validation errors.
        assert!(form.parse_request().is_err());
        form.remote_port.set_text("0");
        assert!(form.parse_request().is_err());
        form.remote_port.set_text("5173");
        form.local_port.set_text("not-a-port");
        assert!(form.parse_request().is_err());

        // Blank local port means auto (0); blank remote host falls back to localhost.
        form.local_port.set_text("");
        form.remote_host.set_text("");
        let (key, local_port) = form.parse_request().unwrap();
        assert_eq!(
            key,
            ForwardKey {
                target: target.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 5173,
            }
        );
        assert_eq!(local_port, 0);

        // Explicit non-loopback host and custom local port are preserved.
        form.remote_host.set_text("database.internal");
        form.remote_port.set_text("5432");
        form.local_port.set_text("15432");
        let (key, local_port) = form.parse_request().unwrap();
        assert_eq!(
            key,
            ForwardKey {
                target,
                remote_host: ForwardHost::Host("database.internal".into()),
                remote_port: 5432,
            }
        );
        assert_eq!(local_port, 15432);
    }

    #[test]
    fn ports_overlay_lifecycle_arms_close_and_updates_selection() {
        let workbox = RemoteTarget::Alias("workbox".into());
        let key1 = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 3000,
        };
        let key2 = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };

        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().current_mut().remote_target = Some(workbox);
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key1.clone(),
            local_port: 3000,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Ready,
            generation: 1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key2.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: None,
            state: ForwardState::Failed,
            generation: 2,
            handle: None,
            error: Some("connection refused".into()),
            pending_opens: Vec::new(),
        });

        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
            .unwrap();
        let overlay = backend.state().ports_overlay.as_ref().unwrap();
        assert_eq!(overlay.selected, Some(key1.clone()));
        assert!(overlay.form.is_none());

        // Open and cancel the add-forward form cleanly.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
            .unwrap();
        assert!(
            backend
                .state()
                .ports_overlay
                .as_ref()
                .unwrap()
                .form
                .is_some()
        );
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseAddForm))
            .unwrap();
        assert!(
            backend
                .state()
                .ports_overlay
                .as_ref()
                .unwrap()
                .form
                .is_none()
        );

        // Two-step destructive close: first press arms, second closes and selects next row.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        assert_eq!(
            backend
                .state()
                .ports_overlay
                .as_ref()
                .unwrap()
                .pending_close,
            Some(key1.clone())
        );
        assert_eq!(backend.state().ports.len(), 2);

        // Moving selection clears armed confirmation.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Select(key2.clone())))
            .unwrap();
        assert!(
            backend
                .state()
                .ports_overlay
                .as_ref()
                .unwrap()
                .pending_close
                .is_none()
        );

        // Arm and confirm closing `key2`.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        assert!(backend.state().ports.get(&key2).is_none());
        assert_eq!(
            backend.state().ports_overlay.as_ref().unwrap().selected,
            Some(key1)
        );
    }

    #[test]
    fn explicit_occupied_local_port_fails_without_ephemeral_fallback() {
        crate::test_support::on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let occupied_port = occupied.local_addr().unwrap().port();
            let workbox = RemoteTarget::Alias("workbox".into());

            let mut backend = TestBackend::new(AppRoot::default());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while backend.state().command_link.is_none() {
                assert!(std::time::Instant::now() < deadline);
                backend.pump().unwrap();
                std::thread::yield_now();
            }
            backend.state_mut().current_mut().remote_target = Some(workbox);

            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
                .unwrap();
            backend
                .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
                .unwrap();
            let form = backend
                .state_mut()
                .ports_overlay
                .as_mut()
                .unwrap()
                .form
                .as_mut()
                .unwrap();
            form.remote_port.set_text("5173");
            form.local_port.set_text(occupied_port.to_string());

            backend
                .dispatch(crate::Msg::Ports(PortMsg::SubmitAddForm))
                .unwrap();
            assert!(backend.state().ports.is_empty());
            let form = backend
                .state()
                .ports_overlay
                .as_ref()
                .unwrap()
                .form
                .as_ref()
                .expect("form stays open on occupied explicit port");
            assert!(
                form.error
                    .as_deref()
                    .is_some_and(|err| err.contains(&occupied_port.to_string())),
                "{:?}",
                form.error
            );
        });
    }

    #[test]
    fn zero_match_filter_disables_close_retry_and_activate() {
        let workbox = RemoteTarget::Alias("workbox".into());
        let key_ready = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 3000,
        };
        let key_failed = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };

        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().current_mut().remote_target = Some(workbox);
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_ready.clone(),
            local_port: 3000,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: Some(BrowserScheme::Http),
            state: ForwardState::Ready,
            generation: 1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_failed.clone(),
            local_port: 5173,
            port_pinned: false,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: None,
            state: ForwardState::Failed,
            generation: 2,
            handle: None,
            error: Some("connection refused".into()),
            pending_opens: Vec::new(),
        });

        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
            .unwrap();
        // Arm close before filtering to zero matches.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        backend
            .dispatch(crate::Msg::Ports(PortMsg::QueryChanged("xyz".into())))
            .unwrap();

        assert!(selected_overlay_key(backend.state()).is_none());
        let overlay = backend.state().ports_overlay.as_ref().unwrap();
        assert!(overlay.selected.is_none());
        assert!(overlay.pending_close.is_none());

        // Destructive close (twice), retry, and activate must do nothing when nothing matches.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        backend
            .dispatch(crate::Msg::Ports(PortMsg::CloseSelected))
            .unwrap();
        backend
            .dispatch(crate::Msg::Ports(PortMsg::RetrySelected))
            .unwrap();
        backend
            .dispatch(crate::Msg::Ports(PortMsg::Activate(key_ready.clone())))
            .unwrap();
        assert_eq!(backend.state().ports.len(), 2);
        assert_eq!(backend.state().ports.get(&key_ready).unwrap().generation, 1);
        assert_eq!(
            backend.state().ports.get(&key_failed).unwrap().generation,
            2
        );

        // Clearing the query restores a visible selection.
        backend
            .dispatch(crate::Msg::Ports(PortMsg::QueryChanged(String::new())))
            .unwrap();
        assert_eq!(selected_overlay_key(backend.state()), Some(key_ready));
    }

    #[test]
    fn retry_ignores_healthy_starting_and_reconnecting_entries() {
        let workbox = RemoteTarget::Alias("workbox".into());
        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().current_mut().remote_target = Some(workbox.clone());

        for (port, state, generation) in [
            (3000, ForwardState::Ready, 1),
            (3001, ForwardState::Starting, 2),
            (3002, ForwardState::Reconnecting, 3),
        ] {
            backend.state_mut().ports.insert(PortForwardEntry {
                key: ForwardKey {
                    target: workbox.clone(),
                    remote_host: ForwardHost::Localhost,
                    remote_port: port,
                },
                local_port: port,
                port_pinned: state != ForwardState::Starting,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: None,
                state,
                generation,
                handle: Some(ForwardHandle::dormant()),
                error: None,
                pending_opens: Vec::new(),
            });
        }

        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
            .unwrap();
        for (port, expected_gen) in [(3000, 1), (3001, 2), (3002, 3)] {
            let key = ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: port,
            };
            backend
                .dispatch(crate::Msg::Ports(PortMsg::Select(key.clone())))
                .unwrap();
            backend
                .dispatch(crate::Msg::Ports(PortMsg::RetrySelected))
                .unwrap();
            assert_eq!(
                backend.state().ports.get(&key).unwrap().generation,
                expected_gen
            );
        }
    }

    #[test]
    fn https_forwards_open_with_https_and_manual_tcp_forwards_have_no_browser_url() {
        crate::test_support::on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let workbox = RemoteTarget::Alias("workbox".into());
            let mut backend = TestBackend::new(AppRoot::default());

            let source = backend.state().current().workspaces[0].panes[0].id;
            backend.state_mut().current_mut().remote_host = Some("workbox".into());
            backend.state_mut().current_mut().remote_target = Some(workbox.clone());
            backend.state_mut().config.link_handlers = vec![crate::config::LinkHandler {
                schemes: vec!["http".into(), "https".into()],
                files: Vec::new(),
                action: LinkAction::Inline(UserCommandAction::ExecDirect { argv: Vec::new() }),
            }];

            let https_url = url::Url::parse("https://localhost:8443/dashboard").unwrap();
            let req = forward_request_for_link(
                backend.state(),
                source,
                &https_url,
                &OpenDestination::System,
            )
            .unwrap();
            assert_eq!(req.scheme, BrowserScheme::Https);

            let https_key = req.key;
            let gen_https = backend.state_mut().ports.mint_generation();
            backend.state_mut().ports.insert(PortForwardEntry {
                key: https_key.clone(),
                local_port: 8443,
                port_pinned: false,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: None,
                state: ForwardState::Starting,
                generation: gen_https,
                handle: Some(ForwardHandle::dormant()),
                error: None,
                pending_opens: Vec::new(),
            });

            backend
                .dispatch(crate::Msg::PaneLinkActivate(
                    source,
                    TerminalLinkKind::Url,
                    "https://localhost:8443/dashboard".into(),
                ))
                .unwrap();
            backend
                .dispatch(crate::Msg::Ports(PortMsg::Event {
                    key: https_key.clone(),
                    generation: gen_https,
                    event: ForwardEvent::Ready { local_port: 49210 },
                }))
                .unwrap();
            let https_entry = backend.state().ports.get(&https_key).unwrap();
            assert_eq!(https_entry.browser_scheme, Some(BrowserScheme::Https));
            assert_eq!(
                browser_open_url(https_entry).as_deref(),
                Some("https://localhost:49210")
            );

            let ipv6_entry = PortForwardEntry {
                key: ForwardKey {
                    target: workbox.clone(),
                    remote_host: ForwardHost::Ipv6Loopback,
                    remote_port: 8000,
                },
                local_port: 49211,
                port_pinned: true,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: Some(BrowserScheme::Http),
                state: ForwardState::Ready,
                generation: 98,
                handle: None,
                error: None,
                pending_opens: Vec::new(),
            };
            assert_eq!(
                browser_open_url(&ipv6_entry).as_deref(),
                Some("http://127.0.0.1:49211")
            );
            assert_eq!(forward_label(&ipv6_entry), "127.0.0.1:49211 → [::1]:8000");

            let manual_entry = PortForwardEntry {
                key: ForwardKey {
                    target: workbox,
                    remote_host: ForwardHost::Host("database.internal".into()),
                    remote_port: 5432,
                },
                local_port: 15432,
                port_pinned: true,
                local_port_mode: LocalPortMode::Exact,
                browser_scheme: None,
                state: ForwardState::Ready,
                generation: 99,
                handle: None,
                error: None,
                pending_opens: Vec::new(),
            };
            assert_eq!(browser_open_url(&manual_entry), None);
        });
    }

    #[test]
    fn auto_forward_converted_to_explicit_exact_restarts_worker() {
        crate::test_support::on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let _no_ssh = crate::session::remote::forward::suspend_worker_spawn_for_test();
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let free_port = probe.local_addr().unwrap().port();
            drop(probe);
            let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let occupied_port = occupied.local_addr().unwrap().port();

            let workbox = RemoteTarget::Alias("workbox".into());
            let key = ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 5173,
            };

            let mut backend = TestBackend::new(AppRoot::default());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while backend.state().command_link.is_none() {
                assert!(std::time::Instant::now() < deadline);
                backend.pump().unwrap();
                std::thread::yield_now();
            }
            backend.state_mut().current_mut().remote_target = Some(workbox);

            let gen1 = backend.state_mut().ports.mint_generation();
            backend.state_mut().ports.insert(PortForwardEntry {
                key: key.clone(),
                local_port: free_port,
                port_pinned: true,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: Some(BrowserScheme::Https),
                state: ForwardState::Ready,
                generation: gen1,
                handle: Some(ForwardHandle::dormant()),
                error: None,
                pending_opens: Vec::new(),
            });

            // Submitting the same local port explicitly replaces the existing Auto worker with an
            // Exact worker (`pin_local_port == true`) under a new generation.
            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
                .unwrap();
            backend
                .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
                .unwrap();
            let form = backend
                .state_mut()
                .ports_overlay
                .as_mut()
                .unwrap()
                .form
                .as_mut()
                .unwrap();
            form.remote_port.set_text("5173");
            form.local_port.set_text(free_port.to_string());
            // A concurrent PTY fork can hold the dropped probe until exec; the Exact request
            // needs the port free.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while let Err(error) = crate::session::remote::forward::exact_local_port(free_port) {
                assert!(
                    error.kind() == std::io::ErrorKind::AddrInUse
                        && std::time::Instant::now() < deadline,
                    "port {free_port} is still taken after its probe closed: {error:?}"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            backend
                .dispatch(crate::Msg::Ports(PortMsg::SubmitAddForm))
                .unwrap();

            let updated = backend.state().ports.get(&key).unwrap();
            assert!(updated.generation > gen1);
            assert_eq!(updated.local_port, free_port);
            assert_eq!(updated.local_port_mode, LocalPortMode::Exact);
            assert!(updated.port_pinned);
            assert!(updated.handle.as_ref().is_some_and(|h| h.pin_local_port));
            assert_eq!(updated.browser_scheme, Some(BrowserScheme::Https));
            // Fence against async failure events from the background `ssh workbox` probe before
            // the subsequent dispatches run.
            let gen_exact = backend.state_mut().ports.mint_generation();
            {
                let entry = backend.state_mut().ports.get_mut(&key).unwrap();
                entry.generation = gen_exact;
                entry.handle = Some(ForwardHandle::dormant());
            }

            // Re-submitting the already-Exact forward with the same local port reuses it in place.
            backend
                .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
                .unwrap();
            let form = backend
                .state_mut()
                .ports_overlay
                .as_mut()
                .unwrap()
                .form
                .as_mut()
                .unwrap();
            form.remote_port.set_text("5173");
            form.local_port.set_text(free_port.to_string());
            backend
                .dispatch(crate::Msg::Ports(PortMsg::SubmitAddForm))
                .unwrap();
            assert_eq!(
                backend.state().ports.get(&key).unwrap().generation,
                gen_exact
            );

            // Attempting to change to an occupied different local port preserves the active tunnel.
            backend
                .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
                .unwrap();
            let form = backend
                .state_mut()
                .ports_overlay
                .as_mut()
                .unwrap()
                .form
                .as_mut()
                .unwrap();
            form.remote_port.set_text("5173");
            form.local_port.set_text(occupied_port.to_string());
            backend
                .dispatch(crate::Msg::Ports(PortMsg::SubmitAddForm))
                .unwrap();

            assert_eq!(
                backend.state().ports.get(&key).unwrap().generation,
                gen_exact
            );
            assert_eq!(
                backend.state().ports.get(&key).unwrap().local_port,
                free_port
            );
            assert!(
                backend
                    .state()
                    .ports_overlay
                    .as_ref()
                    .unwrap()
                    .form
                    .as_ref()
                    .is_some_and(|form| form.error.is_some())
            );
        });
    }

    #[test]
    fn replacing_reconnecting_or_starting_forward_preserves_queued_link_opens() {
        crate::test_support::on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let _no_ssh = crate::session::remote::forward::suspend_worker_spawn_for_test();
            let probe_one = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let probe_two = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let free_port_one = probe_one.local_addr().unwrap().port();
            let free_port_two = probe_two.local_addr().unwrap().port();
            let occupied_port = occupied.local_addr().unwrap().port();
            drop(probe_one);
            drop(probe_two);

            let workbox = RemoteTarget::Alias("workbox".into());
            let key = ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 5173,
            };

            let mut backend = TestBackend::new(AppRoot::default());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while backend.state().command_link.is_none() {
                assert!(std::time::Instant::now() < deadline);
                backend.pump().unwrap();
                std::thread::yield_now();
            }

            let source = backend.state().current().workspaces[0].panes[0].id;
            {
                let state = backend.state_mut();
                state.current_mut().remote_host = Some("workbox".into());
                state.current_mut().remote_target = Some(workbox);
                state.scratch_visible = true;
                state.config.link_handlers = vec![crate::config::LinkHandler {
                    schemes: vec!["http".into(), "https".into()],
                    files: Vec::new(),
                    action: LinkAction::Inline(UserCommandAction::Run {
                        command: "echo \"$ROZI_URL\"".into(),
                        keep_open: false,
                    }),
                }];
            }

            let submit_form = |backend: &mut TestBackend<AppRoot>, local: u16| {
                if backend
                    .state()
                    .ports_overlay
                    .as_ref()
                    .and_then(|overlay| overlay.form.as_ref())
                    .is_none()
                {
                    if backend.state().ports_overlay.is_none() {
                        backend
                            .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
                            .unwrap();
                    }
                    backend
                        .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
                        .unwrap();
                }
                let form = backend
                    .state_mut()
                    .ports_overlay
                    .as_mut()
                    .unwrap()
                    .form
                    .as_mut()
                    .unwrap();
                form.remote_port.set_text("5173");
                form.local_port.set_text(local.to_string());
                backend
                    .dispatch(crate::Msg::Ports(PortMsg::SubmitAddForm))
                    .unwrap();
            };

            // 1. Reconnecting Auto forward -> PaneLinkActivate queues open -> Add forward converts
            // same port to Exact -> new generation Ready still executes the queued open.
            let gen_reconn = backend.state_mut().ports.mint_generation();
            backend.state_mut().ports.insert(PortForwardEntry {
                key: key.clone(),
                local_port: free_port_one,
                port_pinned: true,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: Some(BrowserScheme::Http),
                state: ForwardState::Reconnecting,
                generation: gen_reconn,
                handle: Some(ForwardHandle::dormant()),
                error: Some("connection reset".into()),
                pending_opens: Vec::new(),
            });

            backend
                .dispatch(crate::Msg::PaneLinkActivate(
                    source,
                    TerminalLinkKind::Url,
                    "https://localhost:5173/reconn".into(),
                ))
                .unwrap();
            assert_eq!(
                backend.state().ports.get(&key).unwrap().pending_opens.len(),
                1
            );

            submit_form(&mut backend, free_port_one);
            let replaced = backend.state().ports.get(&key).unwrap();
            assert!(replaced.generation > gen_reconn);
            assert_eq!(replaced.local_port_mode, LocalPortMode::Exact);
            assert_eq!(replaced.local_port, free_port_one);
            assert_eq!(replaced.browser_scheme, Some(BrowserScheme::Https));
            assert_eq!(replaced.pending_opens.len(), 1);
            let gen_exact_one = backend.state_mut().ports.mint_generation();
            {
                let entry = backend.state_mut().ports.get_mut(&key).unwrap();
                entry.generation = gen_exact_one;
                entry.handle = Some(ForwardHandle::dormant());
            }

            backend
                .dispatch(crate::Msg::Ports(PortMsg::Event {
                    key: key.clone(),
                    generation: gen_exact_one,
                    event: ForwardEvent::Ready {
                        local_port: free_port_one,
                    },
                }))
                .unwrap();
            assert_eq!(backend.state().scratch.panes.len(), 1);
            assert!(
                backend.state().scratch.panes[0]
                    .identity
                    .env
                    .iter()
                    .any(|(k, v)| {
                        k == "ROZI_URL" && v == &format!("https://localhost:{free_port_one}/reconn")
                    })
            );
            assert!(
                backend
                    .state()
                    .ports
                    .get(&key)
                    .unwrap()
                    .pending_opens
                    .is_empty()
            );

            // 2. Starting Auto forward -> PaneLinkActivate queues open -> rejected occupied port
            // keeps queue intact -> replacing with a different free Exact port preserves queue
            // and opens with the new port when Ready arrives.
            let gen_starting = backend.state_mut().ports.mint_generation();
            backend.state_mut().ports.insert(PortForwardEntry {
                key: key.clone(),
                local_port: free_port_one,
                port_pinned: false,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: None,
                state: ForwardState::Starting,
                generation: gen_starting,
                handle: Some(ForwardHandle::dormant()),
                error: None,
                pending_opens: Vec::new(),
            });

            backend
                .dispatch(crate::Msg::PaneLinkActivate(
                    source,
                    TerminalLinkKind::Url,
                    "http://localhost:5173/start".into(),
                ))
                .unwrap();
            assert_eq!(
                backend.state().ports.get(&key).unwrap().pending_opens.len(),
                1
            );

            submit_form(&mut backend, occupied_port);
            let still_starting = backend.state().ports.get(&key).unwrap();
            assert_eq!(still_starting.generation, gen_starting);
            assert_eq!(still_starting.pending_opens.len(), 1);
            assert!(
                backend
                    .state()
                    .ports_overlay
                    .as_ref()
                    .and_then(|overlay| overlay.form.as_ref())
                    .is_some_and(|form| form.error.is_some())
            );

            submit_form(&mut backend, free_port_two);
            let exact_two = backend.state().ports.get(&key).unwrap();
            assert!(exact_two.generation > gen_starting);
            assert_eq!(exact_two.local_port_mode, LocalPortMode::Exact);
            assert_eq!(exact_two.local_port, free_port_two);
            assert_eq!(exact_two.pending_opens.len(), 1);
            let gen_exact_two = backend.state_mut().ports.mint_generation();
            {
                let entry = backend.state_mut().ports.get_mut(&key).unwrap();
                entry.generation = gen_exact_two;
                entry.handle = Some(ForwardHandle::dormant());
            }

            backend
                .dispatch(crate::Msg::Ports(PortMsg::Event {
                    key: key.clone(),
                    generation: gen_exact_two,
                    event: ForwardEvent::Ready {
                        local_port: free_port_two,
                    },
                }))
                .unwrap();
            assert_eq!(backend.state().scratch.panes.len(), 2);
            assert!(
                backend.state().scratch.panes[1]
                    .identity
                    .env
                    .iter()
                    .any(|(k, v)| {
                        k == "ROZI_URL" && v == &format!("http://localhost:{free_port_two}/start")
                    })
            );
            assert!(
                backend
                    .state()
                    .ports
                    .get(&key)
                    .unwrap()
                    .pending_opens
                    .is_empty()
            );
        });
    }

    #[test]
    fn synchronous_replacement_failure_preserves_pending_opens() {
        crate::test_support::on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let _no_ssh = crate::session::remote::forward::suspend_worker_spawn_for_test();
            let probe_one = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let free_port_one = probe_one.local_addr().unwrap().port();
            drop(probe_one);
            // Keep this port below the usual ephemeral ranges. The test releases and rebinds
            // it, and macOS can otherwise assign it to an unrelated outgoing connection.
            let racing_listener = (1024..32768)
                .find_map(|port| std::net::TcpListener::bind(("127.0.0.1", port)).ok())
                .expect("a free non-ephemeral test port");
            let raced_port = racing_listener.local_addr().unwrap().port();

            let mut backend = TestBackend::new(AppRoot::default());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while backend.state().command_link.is_none() {
                assert!(std::time::Instant::now() < deadline);
                backend.pump().unwrap();
                std::thread::yield_now();
            }

            let source = backend.state().current().workspaces[0].panes[0].id;
            {
                let state = backend.state_mut();
                state.scratch_visible = true;
                state.config.link_handlers = vec![crate::config::LinkHandler {
                    schemes: vec!["http".into(), "https".into()],
                    files: Vec::new(),
                    action: LinkAction::Inline(UserCommandAction::Run {
                        command: "echo \"$ROZI_URL\"".into(),
                        keep_open: false,
                    }),
                }];
            }

            let submit_form = |backend: &mut TestBackend<AppRoot>, local: u16| {
                if backend.state().ports_overlay.is_none() {
                    backend
                        .dispatch(crate::Msg::RunAction(crate::input::Action::OpenPorts))
                        .unwrap();
                }
                if backend
                    .state()
                    .ports_overlay
                    .as_ref()
                    .and_then(|overlay| overlay.form.as_ref())
                    .is_none()
                {
                    backend
                        .dispatch(crate::Msg::Ports(PortMsg::OpenAddForm))
                        .unwrap();
                }
                let form = backend
                    .state_mut()
                    .ports_overlay
                    .as_mut()
                    .unwrap()
                    .form
                    .as_mut()
                    .unwrap();
                form.remote_port.set_text("5173");
                form.local_port.set_text(local.to_string());
                backend
                    .dispatch(crate::Msg::Ports(PortMsg::SubmitAddForm))
                    .unwrap();
            };

            // 1. Different-port replacement: `exact_local_port` succeeds, then `start_forward()`
            // fails synchronously on invalid target. Old entry and `pending_opens` stay intact.
            let invalid_target = RemoteTarget::Alias("\n".into());
            let invalid_key = ForwardKey {
                target: invalid_target.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 5173,
            };
            {
                let state = backend.state_mut();
                state.current_mut().remote_host = Some("bad".into());
                state.current_mut().remote_target = Some(invalid_target);
            }
            let gen_existing = backend.state_mut().ports.mint_generation();
            let different_port = if free_port_one > 1025 {
                free_port_one - 1
            } else {
                free_port_one + 1
            };
            backend.state_mut().ports.insert(PortForwardEntry {
                key: invalid_key.clone(),
                local_port: different_port,
                port_pinned: false,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: None,
                state: ForwardState::Starting,
                generation: gen_existing,
                handle: Some(ForwardHandle::dormant()),
                error: None,
                pending_opens: Vec::new(),
            });

            backend
                .dispatch(crate::Msg::PaneLinkActivate(
                    source,
                    TerminalLinkKind::Url,
                    "http://localhost:5173/keep".into(),
                ))
                .unwrap();
            assert_eq!(
                backend
                    .state()
                    .ports
                    .get(&invalid_key)
                    .unwrap()
                    .pending_opens
                    .len(),
                1
            );

            submit_form(&mut backend, free_port_one);
            let still_present = backend.state().ports.get(&invalid_key).unwrap();
            assert_eq!(still_present.generation, gen_existing);
            assert_eq!(still_present.state, ForwardState::Starting);
            assert_eq!(still_present.pending_opens.len(), 1);
            assert!(
                backend
                    .state()
                    .ports_overlay
                    .as_ref()
                    .and_then(|overlay| overlay.form.as_ref())
                    .is_some_and(|form| form.error.is_some())
            );
            backend
                .dispatch(crate::Msg::Ports(PortMsg::CloseAddForm))
                .unwrap();

            // 2. Same-port Auto -> Exact replacement where `:raced_port` is occupied immediately
            // after stopping the Auto worker: retains a retryable `Failed` entry with
            // `pending_opens`, which still executes once retried after the port frees up.
            let workbox = RemoteTarget::Alias("workbox".into());
            let workbox_key = ForwardKey {
                target: workbox.clone(),
                remote_host: ForwardHost::Localhost,
                remote_port: 5173,
            };
            {
                let state = backend.state_mut();
                state.current_mut().remote_host = Some("workbox".into());
                state.current_mut().remote_target = Some(workbox);
            }
            let gen_reconn = backend.state_mut().ports.mint_generation();
            backend.state_mut().ports.insert(PortForwardEntry {
                key: workbox_key.clone(),
                local_port: raced_port,
                port_pinned: true,
                local_port_mode: LocalPortMode::Auto,
                browser_scheme: Some(BrowserScheme::Https),
                state: ForwardState::Reconnecting,
                generation: gen_reconn,
                handle: Some(ForwardHandle::dormant()),
                error: Some("connection reset".into()),
                pending_opens: Vec::new(),
            });

            backend
                .dispatch(crate::Msg::PaneLinkActivate(
                    source,
                    TerminalLinkKind::Url,
                    "https://localhost:5173/recovered".into(),
                ))
                .unwrap();
            assert_eq!(
                backend
                    .state()
                    .ports
                    .get(&workbox_key)
                    .unwrap()
                    .pending_opens
                    .len(),
                1
            );

            submit_form(&mut backend, raced_port);
            let retained = backend.state().ports.get(&workbox_key).unwrap();
            assert_eq!(retained.state, ForwardState::Failed);
            assert_eq!(retained.local_port, raced_port);
            assert_eq!(retained.local_port_mode, LocalPortMode::Exact);
            assert!(retained.port_pinned);
            assert_eq!(retained.pending_opens.len(), 1);
            assert!(
                backend
                    .state()
                    .ports_overlay
                    .as_ref()
                    .and_then(|overlay| overlay.form.as_ref())
                    .is_some_and(|form| form.error.is_some())
            );

            // Free the raced port, close the form, retry the failed entry, and confirm the
            // recovered queued activation still opens on Ready.
            drop(racing_listener);
            // A concurrent PTY fork can retain the listener until exec, even after our handle
            // closes. Wait for that child to release it before exercising RetrySelected.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                match crate::session::remote::forward::exact_local_port(raced_port) {
                    Ok(_) => break,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AddrInUse
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!(
                        "port {raced_port} is still taken after its listener closed: {error:?}"
                    ),
                }
            }
            backend
                .dispatch(crate::Msg::Ports(PortMsg::CloseAddForm))
                .unwrap();
            backend
                .dispatch(crate::Msg::Ports(PortMsg::RetrySelected))
                .unwrap();
            let retried = backend.state().ports.get(&workbox_key).unwrap();
            assert_eq!(retried.state, ForwardState::Starting);
            assert_eq!(retried.local_port, raced_port);
            assert_eq!(retried.local_port_mode, LocalPortMode::Exact);
            assert_eq!(retried.pending_opens.len(), 1);
            let gen_retry = backend.state_mut().ports.mint_generation();
            {
                let entry = backend.state_mut().ports.get_mut(&workbox_key).unwrap();
                entry.generation = gen_retry;
                entry.handle = Some(ForwardHandle::dormant());
            }

            backend
                .dispatch(crate::Msg::Ports(PortMsg::Event {
                    key: workbox_key.clone(),
                    generation: gen_retry,
                    event: ForwardEvent::Ready {
                        local_port: raced_port,
                    },
                }))
                .unwrap();
            assert_eq!(backend.state().scratch.panes.len(), 1);
            assert!(
                backend.state().scratch.panes[0]
                    .identity
                    .env
                    .iter()
                    .any(|(k, v)| {
                        k == "ROZI_URL" && v == &format!("https://localhost:{raced_port}/recovered")
                    })
            );
            assert!(
                backend
                    .state()
                    .ports
                    .get(&workbox_key)
                    .unwrap()
                    .pending_opens
                    .is_empty()
            );
        });
    }

    #[test]
    fn remote_ports_config_controls_link_forwarding_and_local_port_preference() {
        let (mut state, pane, _, _) = remote_state(RemoteTarget::Alias("workbox".into()));
        let url = url::Url::parse("http://127.0.0.1:5173/").unwrap();
        assert!(
            forward_request_for_link(&state, pane, &url, &OpenDestination::System).is_some(),
            "enabled by default"
        );
        state.config.remote.ports.auto_forward_links = false;
        assert!(
            forward_request_for_link(&state, pane, &url, &OpenDestination::System).is_none(),
            "disabled when auto_forward_links = false"
        );

        assert_eq!(unpinned_preferred_local_port(true, 5173), Some(5173));
        assert_eq!(unpinned_preferred_local_port(false, 5173), None);
    }

    #[test]
    fn control_commands_list_reuse_reject_ambiguous_close_and_remove() {
        crate::test_support::isolate_user_dirs();
        let _no_ssh = crate::session::remote::forward::suspend_worker_spawn_for_test();
        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(std::time::Instant::now() < deadline);
            backend.pump().unwrap();
            std::thread::yield_now();
        }

        let free_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let free_port = free_listener.local_addr().unwrap().port();
        drop(free_listener);

        let workbox = RemoteTarget::Alias("workbox".into());
        let prod = RemoteTarget::Alias("prod".into());
        let key_w = ForwardKey {
            target: workbox,
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };
        let key_p = ForwardKey {
            target: prod,
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };
        let gen_w = backend.state_mut().ports.mint_generation();
        let gen_p = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_w.clone(),
            local_port: free_port,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: None,
            state: ForwardState::Ready,
            generation: gen_w,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_p.clone(),
            local_port: 15173,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: None,
            state: ForwardState::Ready,
            generation: gen_p,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        let dispatch_ctl = |backend: &mut TestBackend<AppRoot>,
                            json: serde_json::Value|
         -> crate::control::ControlResponse {
            let req: crate::control::ControlRequest = serde_json::from_value(json).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            backend
                .dispatch(crate::Msg::ControlRequest(
                    crate::control::ControlEnvelope::local(req, tx),
                ))
                .unwrap();
            rx.try_recv().unwrap()
        };

        let listed = dispatch_ctl(&mut backend, serde_json::json!({ "cmd": "ports-list" }));
        assert!(listed.ok);
        assert_eq!(listed.data.unwrap().as_array().unwrap().len(), 2);

        // Forwarding an already-ready key without changing local_port reuses it in Auto mode.
        let reused = dispatch_ctl(
            &mut backend,
            serde_json::json!({ "cmd": "ports-forward", "remote": "workbox", "port": 5173 }),
        );
        assert!(reused.ok);
        assert_eq!(reused.data.unwrap()["local_port"], free_port);
        let workbox_entry = backend.state().ports.get(&key_w).unwrap();
        assert_eq!(workbox_entry.local_port_mode, LocalPortMode::Auto);
        assert_eq!(workbox_entry.generation, gen_w);

        // Requesting the same port explicitly with `--local-port` upgrades the ready Auto forward
        // to Exact mode with a pinned worker so reconnects and retries keep that exact local port.
        let promoted = dispatch_ctl(
            &mut backend,
            serde_json::json!({
                "cmd": "ports-forward",
                "remote": "workbox",
                "port": 5173,
                "local_port": free_port,
            }),
        );
        assert!(promoted.ok);
        assert_eq!(promoted.data.unwrap()["local_port"], free_port);
        let workbox_entry = backend.state().ports.get(&key_w).unwrap();
        assert_eq!(workbox_entry.local_port_mode, LocalPortMode::Exact);
        assert!(workbox_entry.port_pinned);
        assert!(workbox_entry.generation > gen_w);
        assert!(workbox_entry.handle.as_ref().unwrap().pin_local_port);
        assert_eq!(
            LocalPortRequest::from_entry(workbox_entry),
            LocalPortRequest::Exact(free_port)
        );

        // Explicit --local-port on an occupied loopback port must fail rather than falling back to
        // an ephemeral port, and must leave the existing forward untouched.
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied_port = occupied.local_addr().unwrap().port();
        let conflict = dispatch_ctl(
            &mut backend,
            serde_json::json!({
                "cmd": "ports-forward",
                "remote": "workbox",
                "port": 5173,
                "local_port": occupied_port,
            }),
        );
        assert!(!conflict.ok);
        assert_eq!(
            conflict.code,
            Some(crate::control::ControlErrorCode::RequestFailed)
        );
        assert_eq!(
            backend.state().ports.get(&key_w).unwrap().local_port,
            free_port
        );
        drop(occupied);

        // Closing without --remote when multiple targets have forwards refuses rather than guessing.
        let ambiguous = dispatch_ctl(
            &mut backend,
            serde_json::json!({ "cmd": "ports-close", "port": 5173 }),
        );
        assert!(!ambiguous.ok);

        // Closing with explicit --remote removes the matching entry and is idempotent afterward.
        let closed = dispatch_ctl(
            &mut backend,
            serde_json::json!({ "cmd": "ports-close", "remote": "prod", "port": 5173 }),
        );
        assert!(closed.ok);
        assert_eq!(closed.data.unwrap()["removed"], true);
        assert!(backend.state().ports.get(&key_p).is_none());

        let missing = dispatch_ctl(
            &mut backend,
            serde_json::json!({ "cmd": "ports-close", "remote": "prod", "port": 5173 }),
        );
        assert!(missing.ok);
        assert_eq!(missing.data.unwrap()["removed"], false);
    }

    #[test]
    fn control_forward_without_remote_requires_active_remote_session() {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = RemoteTarget::Alias("workbox".into());
        let prod = RemoteTarget::Alias("prod".into());
        let key_w = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Localhost,
            remote_port: 5173,
        };

        let dispatch_forward = |backend: &mut TestBackend<AppRoot>, port: u16| {
            let req: crate::control::ControlRequest =
                serde_json::from_value(serde_json::json!({ "cmd": "ports-forward", "port": port }))
                    .unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            backend
                .dispatch(crate::Msg::ControlRequest(
                    crate::control::ControlEnvelope::local(req, tx),
                ))
                .unwrap();
            rx.try_recv().unwrap()
        };

        // Local active session + existing forward must still refuse without --remote.
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_w.clone(),
            local_port: 5173,
            port_pinned: true,
            local_port_mode: LocalPortMode::Auto,
            browser_scheme: None,
            state: ForwardState::Ready,
            generation: 1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        let resp = dispatch_forward(&mut backend, 3000);
        assert!(!resp.ok);
        assert_eq!(
            resp.code,
            Some(crate::control::ControlErrorCode::InvalidArgument)
        );
        assert!(resp.error.unwrap_or_default().contains("--remote"));

        // Local active session + selected Ports overlay row must still refuse without --remote.
        backend.state_mut().ports_overlay = Some(PortsOverlayState::new(Some(key_w)));
        let resp = dispatch_forward(&mut backend, 3000);
        assert!(!resp.ok);
        assert_eq!(
            resp.code,
            Some(crate::control::ControlErrorCode::InvalidArgument)
        );
        backend.state_mut().ports_overlay = None;

        // Local active session + multiple background remote attachments must still refuse.
        let mut bg_w = crate::state::Attachment::new();
        bg_w.remote_target = Some(workbox.clone());
        let mut bg_p = crate::state::Attachment::new();
        bg_p.remote_target = Some(prod);
        backend.state_mut().background.insert(1, bg_w);
        backend.state_mut().background.insert(2, bg_p);
        let resp = dispatch_forward(&mut backend, 3000);
        assert!(!resp.ok);
        assert_eq!(
            resp.code,
            Some(crate::control::ControlErrorCode::InvalidArgument)
        );

        // Active remote session resolves `current().remote_target` without `--remote`.
        backend.state_mut().current_mut().remote_target = Some(workbox);
        let resp = dispatch_forward(&mut backend, 5173);
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap()["target"], "workbox");
    }
}
