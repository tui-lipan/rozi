//! Client-owned SSH port-forward lifecycle, URL policy, and delayed link opening.

use std::net::IpAddr;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{LinkAction, UserCommandAction};
use crate::pane::id::PaneId;
use crate::session::remote::RemoteTarget;
use crate::session::remote::forward::{
    ForwardEvent, ForwardHost, ForwardKey, ForwardSpec, preferred_local_port, start_forward,
};
use crate::state::{
    ForwardState, OpenDestination, PendingLinkContext, PendingLinkOpen, PortForwardEntry,
    ResolvedLinkDestination, State,
};

use super::links::{Machine, PanePlace};

/// A port-forward event or action routed through [`crate::Msg::Ports`].
#[derive(Clone)]
pub enum PortMsg {
    Event {
        key: ForwardKey,
        generation: u64,
        event: ForwardEvent,
    },
}

/// Forward policy decision for a parsed terminal URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkForwardRequest {
    pub key: ForwardKey,
}

/// Decide whether activating `url` from `source` onto `destination` requires an SSH tunnel.
pub(crate) fn forward_request_for_link(
    state: &State,
    source: PaneId,
    url: &url::Url,
    destination: &OpenDestination,
) -> Option<LinkForwardRequest> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
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
    let port_policy = if let Some(entry) = ctx.state.ports.get_mut(&request.key) {
        match entry.state {
            ForwardState::Ready => {
                let local_port = entry.local_port;
                return execute_resolved_open(ctx, pending, local_port);
            }
            ForwardState::Starting | ForwardState::Reconnecting => {
                entry.pending_opens.push(pending);
                return Update::none();
            }
            ForwardState::AuthRequired | ForwardState::Failed => {
                if entry.port_pinned {
                    LocalPortPolicy::Pinned(entry.local_port)
                } else {
                    LocalPortPolicy::Unpinned
                }
            }
        }
    } else {
        LocalPortPolicy::Unpinned
    };

    start_forward_entry(ctx, request.key, port_policy, true, Some(pending))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalPortPolicy {
    Unpinned,
    Pinned(u16),
}

pub(crate) fn start_forward_entry(
    ctx: &mut Context<AppRoot>,
    key: ForwardKey,
    port_policy: LocalPortPolicy,
    interactive: bool,
    pending: Option<PendingLinkOpen>,
) -> Update {
    let Some(link) = ctx.state.command_link.clone() else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Could not forward port",
            "Client runtime is not ready",
        );
        return Update::full();
    };
    let (local_port, pin_local_port) = match port_policy {
        LocalPortPolicy::Unpinned => match preferred_local_port(Some(key.remote_port)) {
            Ok(port) => (port, false),
            Err(error) => {
                crate::pane::pty_events::notify_error(
                    ctx,
                    "Could not forward port",
                    error.to_string(),
                );
                return Update::full();
            }
        },
        LocalPortPolicy::Pinned(port) => (port, true),
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
    let handle = match start_forward(
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
    ) {
        Ok(handle) => handle,
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Could not forward port", error.to_string());
            return Update::full();
        }
    };

    ctx.state.ports.insert(PortForwardEntry {
        key,
        local_port,
        port_pinned: pin_local_port,
        state: ForwardState::Starting,
        generation,
        handle: Some(handle),
        error: None,
        pending_opens: pending.into_iter().collect(),
    });
    Update::full()
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
    state.ports.remove_target(target);
}

/// Close all client-owned tunnels (e.g. on exit, hangup, or relaunch).
pub(crate) fn close_all(state: &mut State) {
    state.ports.clear();
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

        let scratch = tempfile::tempdir().expect("tempdir");
        let out = scratch.path().join("opened-url");
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
            state: ForwardState::Ready,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        let state = backend.state_mut();
        state.current_mut().remote_host = Some("workbox".into());
        state.current_mut().remote_target = Some(workbox);
        state.config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(UserCommandAction::Exec {
                command: format!("printf '%s\\n' \"$ROZI_URL\" > '{}'", out.display()),
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

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let opened = loop {
            backend.pump().unwrap();
            if let Ok(text) = std::fs::read_to_string(&out)
                && !text.is_empty()
            {
                break text;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for queued link to open after reconnect"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(opened.trim(), "http://localhost:49184/reconnected");
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

        let scratch = tempfile::tempdir().expect("tempdir");
        let out = scratch.path().join("should-not-open");
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
            state: ForwardState::Ready,
            generation: gen1,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });

        let state = backend.state_mut();
        state.current_mut().remote_host = Some("workbox".into());
        state.current_mut().remote_target = Some(workbox);
        state.config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(UserCommandAction::Exec {
                command: format!("printf '%s\\n' \"$ROZI_URL\" > '{}'", out.display()),
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
            !out.exists(),
            "pinned restart must never emit Ready or open on a different local port"
        );
    }
}
