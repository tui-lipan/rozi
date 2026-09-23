//! Shared named-session opening and seed selection.

use crate::AppRoot;
use crate::profiles::load_profile;
use tui_lipan::prelude::*;

#[derive(Clone, Debug)]
pub(crate) enum OpenNamedIntent {
    ResolveProfile {
        profile: String,
        path: std::path::PathBuf,
    },
    CreateFresh,
    CreateFromProfile {
        profile: String,
        path: std::path::PathBuf,
    },
    /// A new session for the checkout at `path`; `checkouts` are the repository's worktrees, which
    /// `[worktrees] profile` pane directories are rebased from.
    CreateInWorktree {
        path: String,
        checkouts: Vec<String>,
    },
}

/// Open `name` on this machine, or on `remote` when one is given. A created session starts from the
/// same seed either way, so a session made on a host opens with a shell just like a local one.
pub(crate) fn open_named_target(
    ctx: &mut Context<AppRoot>,
    name: String,
    intent: OpenNamedIntent,
    remote: Option<crate::session::remote::RemoteTarget>,
) -> Update {
    if !crate::session::discovery::valid_session_name(&name) {
        crate::pane::pty_events::notify_error(ctx, "Invalid name", "Use letters, numbers, _ or -");
        return Update::full();
    }
    let exists = match remote.as_ref() {
        None => crate::session::discovery::discover_session(&name)
            .ok()
            .flatten()
            .is_some(),
        Some(target) => {
            crate::ops::session::lifecycle::session_name_already_running(ctx, &name, Some(target))
        }
    };
    let explicit_create = matches!(
        intent,
        OpenNamedIntent::CreateFresh
            | OpenNamedIntent::CreateFromProfile { .. }
            | OpenNamedIntent::CreateInWorktree { .. }
    );
    if explicit_create && exists {
        crate::pane::pty_events::notify_error(
            ctx,
            "Create failed",
            format!("Session `{name}` is already running"),
        );
        return Update::full();
    }
    let remote_host = remote
        .as_ref()
        .map(crate::session::remote::RemoteTarget::display_label);
    if !explicit_create && exists {
        return crate::ops::session::attach_session_by_name(ctx, name, remote_host, remote, false);
    }
    if ctx.state.is_attached_to(&name, remote.as_ref()) {
        return Update::none();
    }
    if let Some(pending) = ctx.state.current().pending_session_attach.as_ref() {
        if pending.name == name && ctx.state.current().remote_target == remote {
            return Update::none();
        }
        crate::pane::pty_events::notify_info(ctx, "Attach already in progress");
        return Update::full();
    }
    // Past the guards this session is being opened, which retires any dialog we were raised from -
    // Settings included. The rejections above keep their parent, since nothing happened.
    crate::ops::overlay_return::leave(ctx);

    let seed = match intent {
        OpenNamedIntent::ResolveProfile { profile, path } => {
            if !path.exists() {
                crate::pane::pty_events::notify_error(
                    ctx,
                    "Not found",
                    format!("No session or profile `{name}`"),
                );
                return Update::full();
            }
            SessionSeed::Profile { profile, path }
        }
        OpenNamedIntent::CreateFresh => SessionSeed::Default,
        OpenNamedIntent::CreateFromProfile { profile, path } => {
            SessionSeed::Profile { profile, path }
        }
        OpenNamedIntent::CreateInWorktree { path, checkouts } => {
            SessionSeed::FreshWorktree { path, checkouts }
        }
    };
    let (attachment, attach_intent) = match seed {
        SessionSeed::Profile { profile, path } => {
            let loaded = match load_profile(&path) {
                Ok(loaded) => loaded,
                Err(message) => {
                    crate::pane::pty_events::notify_error(ctx, "Profile load failed", message);
                    return Update::full();
                }
            };
            let attachment = crate::profiles::attachment_from_profile(&ctx.state.config, loaded)
                .unwrap_or_else(|| crate::state::fresh_default_attachment(&ctx.state.config));
            (
                attachment,
                crate::state::AttachIntent::ProfileSeed { profile, path },
            )
        }
        SessionSeed::Default => crate::profiles::default_session_seed(&ctx.state.config),
        SessionSeed::FreshWorktree { path, checkouts } => {
            // A broken worktree profile should not keep the checkout from opening at all.
            let profile = crate::profiles::load_worktree_profile(
                &ctx.state.config,
                &checkouts,
                remote.is_some(),
            )
            .unwrap_or_else(|message| {
                crate::pane::pty_events::notify_error(ctx, "Worktree profile skipped", message);
                None
            });
            crate::profiles::worktree_session_seed(&ctx.state.config, &path, &checkouts, profile)
        }
    };
    let epoch = ctx.state.mint_attachment_id();
    let (parked_epoch, left) =
        if explicit_create {
            // Creating a session parks the current one, like switching to a session or creating one on
            // a remote host: it stays live in the background, instant to return to.
            crate::ops::session::park_current_and_install(ctx, attachment, epoch)
        } else {
            // Resolving a profile *replaces* the current session — a deliberate, confirmed action (see
            // `[confirm].load_profile`) — so release it rather than park.
            let left = ctx.state.current().session_name.clone().map(|left_name| {
                crate::state::LeftSession {
                    name: left_name,
                    was_ephemeral_shutdown: crate::ops::session::may_shutdown_ephemeral(&ctx.state),
                }
            });
            crate::ops::session::release_current_session(ctx);
            crate::ops::session::install_fresh_attachment(ctx, attachment);
            (None, left)
        };
    ctx.state.current_mut().remote_host = remote_host.clone();
    ctx.state.current_mut().remote_target = remote.clone();
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        reconnect: false,
        remote_host,
        intent: attach_intent.clone(),
        left,
        parked_epoch,
    });
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
    let remote_config = ctx.state.config.remote.clone();
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || match remote {
            Some(target) => crate::session::bootstrap::attach_remote_session_client(
                epoch,
                name,
                false,
                explicit_create,
                target,
                remote_config,
                // Explicit request: fail fast rather than blocking the UI on a dead host.
                crate::session::bootstrap::RemoteAttachMode::Initial,
                link,
            ),
            None if explicit_create => {
                crate::session::bootstrap::create_session_client(epoch, name, false, link)
            }
            None => {
                crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
            }
        });
    }))
}

enum SessionSeed {
    Default,
    Profile {
        profile: String,
        path: std::path::PathBuf,
    },
    FreshWorktree {
        path: String,
        checkouts: Vec<String>,
    },
}
