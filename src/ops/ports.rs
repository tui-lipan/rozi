//! Client-owned SSH port-forward lifecycle, URL policy, and delayed link opening.

use std::net::Ipv4Addr;

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::id::PaneId;
use crate::session::remote::RemoteTarget;
use crate::session::remote::forward::{
    ForwardEvent, ForwardHost, ForwardKey, ForwardSpec, preferred_local_port, start_forward,
};
use crate::state::{ForwardState, OpenDestination, PendingLinkOpen, PortForwardEntry, State};

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
            .then_some(ForwardHost::Loopback),
        url::Host::Ipv4(ip) => {
            if ip == Ipv4Addr::LOCALHOST || ip.is_unspecified() {
                Some(ForwardHost::Loopback)
            } else if ip.is_loopback() {
                Some(ForwardHost::Host(ip.to_string()))
            } else {
                None
            }
        }
        url::Host::Ipv6(ip) => {
            if ip.is_unspecified() {
                Some(ForwardHost::Loopback)
            } else if ip.is_loopback() {
                Some(ForwardHost::Host(ip.to_string()))
            } else if let Some(v4) = ip.to_ipv4_mapped() {
                if v4 == Ipv4Addr::LOCALHOST || v4.is_unspecified() {
                    Some(ForwardHost::Loopback)
                } else if v4.is_loopback() {
                    Some(ForwardHost::Host(v4.to_string()))
                } else {
                    None
                }
            } else {
                None
            }
        }
    }
}

/// Rewrite a remote loopback/wildcard URL onto the local forwarded loopback port, preserving
/// path, query, fragment, and scheme.
pub(crate) fn rewrite_forwarded_url(original: &str, local_port: u16) -> Option<String> {
    let mut url = url::Url::parse(original).ok()?;
    let keep_host = match url.host() {
        Some(url::Host::Domain(domain)) if domain.eq_ignore_ascii_case("localhost") => true,
        Some(url::Host::Ipv4(ip)) if ip == Ipv4Addr::LOCALHOST => true,
        _ => false,
    };
    if !keep_host {
        url.set_host(Some("127.0.0.1")).ok()?;
    }
    url.set_port(Some(local_port)).ok()?;
    Some(url.into())
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
    if let Some(entry) = ctx.state.ports.get_mut(&request.key) {
        match entry.state {
            ForwardState::Ready => {
                let local_port = entry.local_port;
                let rewritten = rewrite_forwarded_url(original_url, local_port)
                    .unwrap_or_else(|| original_url.to_string());
                return super::links::open_destination(
                    ctx,
                    source,
                    &destination,
                    original_url,
                    &rewritten,
                );
            }
            ForwardState::Starting => {
                entry.pending_opens.push(PendingLinkOpen {
                    source,
                    original_url: original_url.to_string(),
                    destination,
                });
                return Update::none();
            }
            ForwardState::Reconnecting | ForwardState::AuthRequired | ForwardState::Failed => {}
        }
    }

    let pending = PendingLinkOpen {
        source,
        original_url: original_url.to_string(),
        destination,
    };
    start_forward_entry(ctx, request.key, None, true, Some(pending))
}

pub(crate) fn start_forward_entry(
    ctx: &mut Context<AppRoot>,
    key: ForwardKey,
    preferred_local: Option<u16>,
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
    let local_port = match preferred_local_port(preferred_local.or(Some(key.remote_port))) {
        Ok(port) => port,
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Could not forward port", error.to_string());
            return Update::full();
        }
    };
    let generation = ctx.state.ports.mint_generation();
    let spec = ForwardSpec {
        target: key.target.clone(),
        connect_host: key.remote_host.connect_host().to_string(),
        remote_port: key.remote_port,
        local_port,
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
            let pending = {
                let entry = ctx.state.ports.get_mut(&key).expect("matched generation");
                entry.local_port = local_port;
                entry.state = ForwardState::Ready;
                entry.error = None;
                std::mem::take(&mut entry.pending_opens)
            };
            let mut update = Update::full();
            if local_port != key.remote_port {
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
                let rewritten = rewrite_forwarded_url(&open.original_url, local_port)
                    .unwrap_or_else(|| open.original_url.clone());
                update = update.merge(super::links::open_destination(
                    ctx,
                    open.source,
                    &open.destination,
                    &open.original_url,
                    &rewritten,
                ));
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
                remote_host: ForwardHost::Loopback,
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
                remote_host: ForwardHost::Loopback,
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
                remote_host: ForwardHost::Host("::1".into()),
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
                remote_host: ForwardHost::Loopback,
                remote_port: 8000,
            })
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
                remote_host: ForwardHost::Loopback,
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
            rewrite_forwarded_url("https://localhost/login", 49186).as_deref(),
            Some("https://localhost:49186/login")
        );
    }

    #[test]
    fn registry_keys_distinguish_targets_queue_pending_opens_and_fence_stale_generations() {
        crate::test_support::on_large_stack(check_registry_and_delayed_open);
    }

    fn check_registry_and_delayed_open() {
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
        let prod = RemoteTarget::Alias("prod".into());
        let source = backend.state().current().workspaces[0].panes[0].id;

        let key_workbox = ForwardKey {
            target: workbox.clone(),
            remote_host: ForwardHost::Loopback,
            remote_port: 5173,
        };
        let key_prod = ForwardKey {
            target: prod.clone(),
            remote_host: ForwardHost::Loopback,
            remote_port: 5173,
        };

        let gen1 = backend.state_mut().ports.mint_generation();
        backend.state_mut().ports.insert(PortForwardEntry {
            key: key_workbox.clone(),
            local_port: 5173,
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
            state: ForwardState::Ready,
            generation: gen_prod,
            handle: Some(ForwardHandle::dormant()),
            error: None,
            pending_opens: Vec::new(),
        });
        assert_eq!(backend.state().ports.len(), 2);

        // Set up remote session context and a local exec handler to verify queued ROZI_HINT/ROZI_URL.
        backend.state_mut().current_mut().remote_host = Some("workbox".into());
        backend.state_mut().current_mut().remote_target = Some(workbox.clone());
        let action = UserCommandAction::Exec {
            command: format!(
                "printf '%s\\n%s\\n' \"$ROZI_HINT\" \"$ROZI_URL\" >> '{}'",
                out.display()
            ),
        };
        backend.state_mut().config.link_handlers = vec![crate::config::LinkHandler {
            schemes: vec!["http".into()],
            files: Vec::new(),
            action: LinkAction::Inline(action),
        }];

        // Two requests while Starting queue behind the existing entry without spawning another forward.
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

        // Ready drains both pending opens with rewritten URLs and preserved ROZI_HINT.
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
                && text.lines().count() == 4
            {
                break text;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(
            lines,
            [
                "http://localhost:5173/first",
                "http://localhost:49184/first",
                "http://127.0.0.1:5173/second",
                "http://127.0.0.1:49184/second",
            ]
        );

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
}
