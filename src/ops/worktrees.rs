//! Worktree picker actions. Paths received from a remote server remain opaque strings here.

use tui_lipan::prelude::*;

use crate::session::discovery::DiscoveredSession;
use crate::session::protocol::{WorktreeRequest, WorktreeResult};
use crate::state::{
    PendingWorktreeRemove, PendingWorktreeRemoveKind, WorktreeFormField, WorktreeFormState,
    WorktreeOperation, WorktreeOperationKind, WorktreePickerState,
};
use crate::{AppRoot, Msg};

pub(crate) fn request_id(ctx: &mut Context<AppRoot>) -> u64 {
    let id = ctx.state.next_worktree_request_id;
    ctx.state.next_worktree_request_id = id.wrapping_add(1).max(1);
    id
}

fn writable(ctx: &Context<AppRoot>) -> bool {
    ctx.state
        .current()
        .shared
        .as_ref()
        .is_none_or(|shared| !shared.read_only)
}

/// Whether a started create or remove is still running. One whose attachment has gone, or has
/// reconnected, can never be answered: its Git work may still have finished on the host, so it
/// is retired with a note to look, rather than blocking every later operation.
fn operation_in_flight(ctx: &mut Context<AppRoot>) -> bool {
    if ctx.state.worktree_operation.is_none() {
        return false;
    }
    if ctx.state.worktree_operation_reachable() {
        return true;
    }
    if let Some(operation) = ctx.state.worktree_operation.take() {
        let what = match operation.kind {
            WorktreeOperationKind::Create { branch } => format!("creating `{branch}`"),
            WorktreeOperationKind::Remove { path, .. } => format!("removing {path}"),
            WorktreeOperationKind::Unlock { path } => format!("unlocking {path}"),
        };
        crate::pane::pty_events::notify_info(
            ctx,
            format!("Lost the session that was {what}; refresh Worktrees to see the result"),
        );
    }
    false
}

fn selected(picker: &WorktreePickerState) -> Option<&crate::git::worktrees::WorktreeInfo> {
    let query = picker.input.text().trim().to_ascii_lowercase();
    picker.entries.get(picker.selected).filter(|entry| {
        query.is_empty()
            || entry.path.to_ascii_lowercase().contains(&query)
            || entry
                .branch
                .as_deref()
                .is_some_and(|branch| branch.to_ascii_lowercase().contains(&query))
    })
}

/// The repository Worktrees acts on: the focused pane's project root, on the session's host.
///
/// A pane whose shell has moved to a nested SSH host is refused: the session server cannot run
/// Git there.
pub(crate) fn repository_scope(
    state: &crate::state::State,
) -> std::result::Result<(String, Option<crate::session::remote::RemoteTarget>), &'static str> {
    repository_scope_ref(state).map(|(cwd, target)| (cwd.to_string(), target.cloned()))
}

/// Whether the focused pane's repository is not known *yet*, as opposed to known to be absent: no
/// session is connected, an attach has not delivered its panes, or the pane has not reported where
/// it is. A session that has just opened passes through this for a moment before its first pane
/// reports.
///
/// No focused pane on an attached session is an answer, not a gap: closing the last pane leaves
/// no repository to list, and holding the previous one would show checkouts nothing points at.
pub(crate) fn repository_scope_pending(state: &crate::state::State) -> bool {
    if state.current().session_client.is_none() {
        return true;
    }
    let Some(pane) = state
        .focused_pane()
        .and_then(|id| crate::pane::lifecycle::find_pane(state, id))
    else {
        return state.current().pending_session_attach.is_some();
    };
    pane.terminal.cwd.is_none() || pane.terminal.runtime_sequence == 0
}

/// [`repository_scope`] by borrow, for the checks that run after every message.
pub(crate) fn repository_scope_ref(
    state: &crate::state::State,
) -> std::result::Result<(&str, Option<&crate::session::remote::RemoteTarget>), &'static str> {
    let pane = state
        .focused_pane()
        .and_then(|id| crate::pane::lifecycle::find_pane(state, id))
        .ok_or("Focus a pane in a Git repository")?;
    if pane.terminal.cwd_host.is_some() {
        return Err("Pane is on a nested remote host");
    }
    let cwd = pane
        .terminal
        .project_root
        .as_deref()
        .ok_or("Not in a Git repository")?;
    if state.current().session_client.is_none() {
        return Err("Attach to a session first");
    }
    Ok((cwd, state.current().remote_target.as_ref()))
}

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    let (cwd, target) = match repository_scope(&ctx.state) {
        Ok(scope) => scope,
        Err(reason) => {
            crate::pane::pty_events::notify_error(ctx, "Worktrees unavailable", reason);
            return Update::full();
        }
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::none();
    };
    operation_in_flight(ctx);
    let mut picker = WorktreePickerState::new(cwd.clone(), target.clone());
    picker.statuses = ctx.state.worktree_statuses.get(target.as_ref(), &cwd);
    // Open with the last list for this repository and refresh it in place: Git answers quickly,
    // but an empty "loading" frame that then grows into the real list reads as a delay.
    if let Some(cached) = ctx.state.worktree_lists.get(target.as_ref(), &cwd) {
        picker.entries = cached.to_vec();
        picker.selected = cached.iter().position(|tree| tree.path == cwd).unwrap_or(0);
    }
    picker.sessions = crate::ops::session::discovery::immediate_picker_rows(ctx)
        .into_iter()
        .filter(|row| !row.ephemeral && row.remote_target == target)
        .collect();
    let id = request_id(ctx);
    picker.pending_list = Some(id);
    ctx.state.worktree_picker = Some(picker);
    ctx.state.show_palette = false;
    ctx.state.mode = crate::state::Mode::Normal;
    crate::ops::focus::request_worktree_picker_focus(ctx);
    client.worktree(id, WorktreeRequest::List { cwd });
    request_status(ctx, false);
    Update::full()
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.worktree_picker = None;
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.input.set_text(query.clone());
        picker.input.set_cursor(query.len());
        picker.input.set_anchor(None);
        picker.selected = 0;
        picker.pending_remove = None;
    }
    Update::full()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    // The palette reports its selection again as it redraws; only a move disarms a removal.
    if let Some(picker) = ctx.state.worktree_picker.as_mut()
        && picker.selected != index
    {
        picker.selected = index;
        picker.pending_remove = None;
    }
    Update::full()
}

pub(crate) fn refresh(ctx: &mut Context<AppRoot>) -> Update {
    let Some((client, cwd)) = ctx.state.worktree_picker.as_ref().and_then(|picker| {
        Some((
            ctx.state.current().session_client.clone()?,
            picker.cwd.clone(),
        ))
    }) else {
        return Update::none();
    };
    let id = request_id(ctx);
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.pending_list = Some(id);
        picker.pending_remove = None;
        picker.error = None;
    }
    client.worktree(id, WorktreeRequest::List { cwd });
    request_status(ctx, true);
    Update::full()
}

fn request_status(ctx: &mut Context<AppRoot>, refresh: bool) {
    let Some(picker) = ctx.state.worktree_picker.as_ref() else {
        return;
    };
    if picker.standalone_form || (picker.pending_status.is_some() && !refresh) {
        return;
    }
    let cwd = picker.cwd.clone();
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    let id = request_id(ctx);
    ctx.state.worktree_picker.as_mut().unwrap().pending_status = Some(id);
    client.worktree(id, WorktreeRequest::Status { cwd, refresh });
}

pub(crate) fn copy_path(ctx: &mut Context<AppRoot>) -> Update {
    let Some(path) = ctx
        .state
        .worktree_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(picker.selected))
        .map(|tree| tree.path.clone())
    else {
        return Update::none();
    };
    match ctx.clipboard().copy(&path) {
        Ok(()) => crate::pane::pty_events::notify_info(ctx, "Copied worktree path"),
        Err(error) => crate::pane::pty_events::notify_error(ctx, "Copy failed", error.to_string()),
    };
    Update::full()
}

/// Sessions among `sessions` that record the checkout at `path` on `target` as their origin.
fn sessions_for_checkout(
    sessions: &[DiscoveredSession],
    target: Option<&crate::session::remote::RemoteTarget>,
    path: &str,
) -> Vec<DiscoveredSession> {
    sessions
        .iter()
        .filter(|row| {
            row.remote_target.as_ref() == target
                && row
                    .origin
                    .worktree
                    .as_ref()
                    .is_some_and(|tree| tree.path == path)
        })
        .cloned()
        .collect()
}

fn new_session_name(
    ctx: &Context<AppRoot>,
    branch: Option<&str>,
    path: &str,
    target: Option<&crate::session::remote::RemoteTarget>,
    known: &[DiscoveredSession],
) -> String {
    let base = crate::session::worktrees::session_name_base(branch, path);
    crate::session::worktrees::unused_session_name(&base, |name| {
        known.iter().any(|row| row.name == name)
            || crate::ops::session::lifecycle::session_name_already_running(ctx, name, target)
    })
    .unwrap_or_else(|| format!("{base}-{}", ctx.state.next_worktree_request_id))
}

/// Open a checkout: switch to the one session recording it as its origin, let the user choose
/// among several, or create a named session whose first shell starts in it.
///
/// `known` is the discovered sessions to match against and avoid names from; `checkouts` are the
/// repository's worktrees, which a `[worktrees] profile` is rebased from. Whatever overlay led
/// here is closed first.
fn enter_checkout(
    ctx: &mut Context<AppRoot>,
    tree: crate::git::worktrees::WorktreeInfo,
    target: Option<crate::session::remote::RemoteTarget>,
    mut checkouts: Vec<String>,
    known: Vec<DiscoveredSession>,
) -> Update {
    ctx.state.worktree_picker = None;
    let mut matches = sessions_for_checkout(&known, target.as_ref(), &tree.path);
    if matches.len() == 1 {
        return crate::ops::session::activate_discovered_session(ctx, matches.remove(0));
    }
    if matches.len() > 1 {
        ctx.state.session_picker =
            Some(crate::state::SessionPickerState::new(matches).on_tab(target));
        ctx.state.show_session_picker = true;
        crate::ops::focus::request_session_picker_focus(ctx);
        return Update::full();
    }
    let name = new_session_name(
        ctx,
        tree.branch.as_deref(),
        &tree.path,
        target.as_ref(),
        &known,
    );
    if !checkouts.contains(&tree.path) {
        checkouts.push(tree.path.clone());
    }
    crate::ops::session::open::open_named_target(
        ctx,
        name,
        crate::ops::session::open::OpenNamedIntent::CreateInWorktree {
            path: tree.path,
            checkouts,
        },
        target,
    )
}

fn enter_tree(ctx: &mut Context<AppRoot>, tree: crate::git::worktrees::WorktreeInfo) -> Update {
    let Some(picker) = ctx.state.worktree_picker.as_ref() else {
        return Update::none();
    };
    let target = picker.target.clone();
    let checkouts = picker
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let known = picker.sessions.clone();
    enter_checkout(ctx, tree, target, checkouts, known)
}

/// Discovered sessions to match a checkout against, on the host the sidebar lists. Discovery runs
/// here, at the moment of the click, rather than on every refresh of the tab.
fn discovered_sessions(
    ctx: &mut Context<AppRoot>,
    target: Option<&crate::session::remote::RemoteTarget>,
) -> Vec<DiscoveredSession> {
    crate::ops::session::discovery::immediate_picker_rows(ctx)
        .into_iter()
        .filter(|row| !row.ephemeral && row.remote_target.as_ref() == target)
        .collect()
}

/// A checkout row in the Worktrees tab was activated.
pub(crate) fn open_from_sidebar(ctx: &mut Context<AppRoot>, path: String) -> Update {
    let listing = &ctx.state.sidebar.worktrees;
    let Some((target, _)) = listing.source.clone() else {
        return Update::none();
    };
    let Some(tree) = listing
        .entries
        .iter()
        .find(|tree| tree.path == path)
        .cloned()
    else {
        return Update::none();
    };
    let checkouts = listing
        .entries
        .iter()
        .map(|tree| tree.path.clone())
        .collect();
    let known = discovered_sessions(ctx, target.as_ref());
    enter_checkout(ctx, tree, target, checkouts, known)
}

/// The Worktrees tab's "New worktree" row: the new-worktree form on its own, with no list behind
/// it. Submitting it opens the new checkout once Git has made it.
pub(crate) fn open_form_from_sidebar(ctx: &mut Context<AppRoot>) -> Update {
    if !writable(ctx) {
        crate::pane::pty_events::notify_error(ctx, "Create failed", "Client is read-only");
        return Update::full();
    }
    if operation_in_flight(ctx) {
        crate::pane::pty_events::notify_info(ctx, "Worktree operation in progress");
        return Update::full();
    }
    let Some((_, cwd)) = ctx.state.sidebar.worktrees.source.clone() else {
        return Update::none();
    };
    let target = ctx.state.current().remote_target.clone();
    let mut picker = WorktreePickerState::new(cwd, target);
    picker.standalone_form = true;
    picker.form = Some(WorktreeFormState::new());
    ctx.state.worktree_picker = Some(picker);
    ctx.state.show_palette = false;
    ctx.state.mode = crate::state::Mode::Normal;
    crate::ops::focus::request_worktree_form_focus(ctx);
    Update::full()
}

pub(crate) fn open_selected(ctx: &mut Context<AppRoot>) -> Update {
    if operation_in_flight(ctx) {
        crate::pane::pty_events::notify_info(ctx, "Worktree operation in progress");
        return Update::full();
    }
    let tree = ctx
        .state
        .worktree_picker
        .as_ref()
        .and_then(selected)
        .cloned();
    tree.map_or(Update::none(), |tree| enter_tree(ctx, tree))
}

pub(crate) fn can_open_pane(state: &crate::state::State) -> bool {
    state.is_controller()
        && !state.scratch_visible
        && state.current().session_client.is_some()
        && state
            .current()
            .shared
            .as_ref()
            .is_none_or(|shared| !shared.read_only)
        && state.worktree_picker.as_ref().is_some_and(|picker| {
            picker.form.is_none()
                && picker.target == state.current().remote_target
                && picker
                    .entries
                    .get(picker.selected)
                    .is_some_and(|tree| !tree.bare && !tree.prunable)
        })
}

/// Open an ordinary interactive pane in this session; host paths pass unchanged to its server.
pub(crate) fn open_pane(ctx: &mut Context<AppRoot>) -> Update {
    if !can_open_pane(&ctx.state) || operation_in_flight(ctx) {
        return Update::none();
    }
    let picker = ctx.state.worktree_picker.as_ref().unwrap();
    let path = picker.entries[picker.selected].path.clone();
    let workspace = ctx.state.current().active_workspace;
    let source = ctx.state.current().workspaces[workspace].focused_pane;
    ctx.state.worktree_picker = None;
    ctx.state.current_mut().engaged = true;
    crate::pane::lifecycle::spawn_interactive_pane(
        ctx,
        workspace,
        source,
        crate::state::PaneIdentity {
            cwd: Some(path),
            ..Default::default()
        },
    )
    .1
}

pub(crate) fn activate(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    select(ctx, index);
    open_selected(ctx)
}

pub(crate) fn open_form(ctx: &mut Context<AppRoot>) -> Update {
    if !writable(ctx) {
        crate::pane::pty_events::notify_error(ctx, "Create failed", "Client is read-only");
        return Update::full();
    }
    if operation_in_flight(ctx) {
        crate::pane::pty_events::notify_info(ctx, "Worktree operation in progress");
        return Update::full();
    }
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.form = Some(WorktreeFormState::new());
        crate::ops::focus::request_worktree_form_focus(ctx);
    }
    Update::full()
}

pub(crate) fn close_form(ctx: &mut Context<AppRoot>) -> Update {
    if ctx
        .state
        .worktree_picker
        .as_ref()
        .is_some_and(|picker| picker.standalone_form)
    {
        return close(ctx);
    }
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.form = None;
    }
    crate::ops::focus::request_worktree_picker_focus(ctx);
    Update::full()
}

pub(crate) fn form_changed(
    ctx: &mut Context<AppRoot>,
    field: WorktreeFormField,
    event: InputEvent,
) -> Update {
    let Some(form) = ctx
        .state
        .worktree_picker
        .as_mut()
        .and_then(|picker| picker.form.as_mut())
    else {
        return Update::none();
    };
    form.focus = field;
    event.apply_to(form.input_mut(field));
    form.error = None;
    if field == WorktreeFormField::Path {
        form.path_edited = true;
        // The warning described the previewed path, not this one; a create still reports it.
        form.unignored = None;
    }
    if field == WorktreeFormField::Branch && !form.path_edited {
        form.preview_revision = form.preview_revision.wrapping_add(1);
        form.pending_preview = None;
        let revision = form.preview_revision;
        let epoch = ctx.state.runtime_epoch;
        return Update::with_command(Command::after(
            std::time::Duration::from_millis(250),
            move |link: CommandLink<Msg>| {
                link.send(Msg::WorktreePreviewTick { epoch, revision });
            },
        ));
    }
    Update::full()
}

pub(crate) fn form_cycle(ctx: &mut Context<AppRoot>, forward: bool) -> Update {
    if let Some(form) = ctx
        .state
        .worktree_picker
        .as_mut()
        .and_then(|picker| picker.form.as_mut())
    {
        form.cycle_focus(forward);
        crate::ops::focus::request_worktree_form_focus(ctx);
    }
    Update::full()
}

pub(crate) fn preview_tick(ctx: &mut Context<AppRoot>, epoch: u64, revision: u64) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let Some((cwd, branch)) = ctx.state.worktree_picker.as_ref().and_then(|picker| {
        let form = picker.form.as_ref()?;
        (form.preview_revision == revision
            && !form.path_edited
            && !form.branch.text().trim().is_empty())
        .then(|| (picker.cwd.clone(), form.branch.text().trim().to_string()))
    }) else {
        return Update::none();
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::none();
    };
    let id = request_id(ctx);
    if let Some(form) = ctx
        .state
        .worktree_picker
        .as_mut()
        .and_then(|picker| picker.form.as_mut())
    {
        form.pending_preview = Some(id);
    }
    client.worktree(id, WorktreeRequest::Preview { cwd, branch });
    Update::none()
}

pub(crate) fn submit_form(ctx: &mut Context<AppRoot>) -> Update {
    if !writable(ctx) {
        return Update::none();
    }
    let Some((cwd, branch, base, path)) = ctx.state.worktree_picker.as_ref().and_then(|picker| {
        let form = picker.form.as_ref()?;
        Some((
            picker.cwd.clone(),
            form.branch.text().trim().to_string(),
            form.base.text().trim().to_string(),
            form.path.text().trim().to_string(),
        ))
    }) else {
        return Update::none();
    };
    if branch.is_empty() || base.is_empty() {
        if let Some(form) = ctx
            .state
            .worktree_picker
            .as_mut()
            .and_then(|picker| picker.form.as_mut())
        {
            form.error = Some("Branch and base are required".into());
        }
        return Update::full();
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::none();
    };
    let standalone = ctx
        .state
        .worktree_picker
        .as_ref()
        .is_some_and(|picker| picker.standalone_form);
    let id = request_id(ctx);
    ctx.state.worktree_operation = Some(WorktreeOperation {
        request_id: id,
        epoch: ctx.state.runtime_epoch,
        connection: client.connection_token(),
        cwd: cwd.clone(),
        kind: WorktreeOperationKind::Create {
            branch: branch.clone(),
        },
        open_when_done: standalone,
    });
    if standalone {
        // Nothing stays open for the result: the Worktrees tab shows the checkout being created,
        // and its session opens when Git is done.
        ctx.state.worktree_picker = None;
        crate::ops::focus::request_current_pane_focus(ctx);
    } else {
        if let Some(picker) = ctx.state.worktree_picker.as_mut() {
            picker.form = None;
        }
        crate::ops::focus::request_worktree_picker_focus(ctx);
    }
    client.worktree(
        id,
        WorktreeRequest::Create {
            cwd,
            branch,
            base,
            path: (!path.is_empty()).then_some(path),
        },
    );
    Update::full()
}

/// Ctrl+K in the picker. The first press arms, like every other destructive picker action, and
/// the second removes; a stale lock arms as such, so the lift is a step the user sees, and a dirty
/// checkout re-arms as forced once Git has refused it.
pub(crate) fn remove_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some((cwd, tree, armed)) = ctx.state.worktree_picker.as_ref().and_then(|picker| {
        let tree = selected(picker)?.clone();
        let armed = picker
            .pending_remove
            .as_ref()
            .filter(|pending| pending.path == tree.path)
            .map(|pending| pending.kind);
        Some((picker.cwd.clone(), tree, armed))
    }) else {
        return Update::none();
    };
    let stale = tree.lock.as_ref().is_some_and(|lock| lock.stale);
    // A checkout that cannot be removed skips the arming and goes straight to the refusal toast.
    let removable = tree.linked && !tree.bare && (tree.lock.is_none() || stale);
    if armed.is_none() && removable && writable(ctx) && !operation_in_flight(ctx) {
        if let Some(picker) = ctx.state.worktree_picker.as_mut() {
            picker.pending_remove = Some(PendingWorktreeRemove {
                path: tree.path,
                kind: if stale {
                    PendingWorktreeRemoveKind::StaleLock
                } else {
                    PendingWorktreeRemoveKind::Clean
                },
            });
        }
        return crate::ops::confirm::arm(ctx);
    }
    start_remove(
        ctx,
        cwd,
        tree,
        armed == Some(PendingWorktreeRemoveKind::Dirty),
    )
}

/// Ctrl+U in the picker: lift the selected checkout's lock, stale or not.
pub(crate) fn unlock_selected(ctx: &mut Context<AppRoot>) -> Update {
    let Some((cwd, tree)) = ctx
        .state
        .worktree_picker
        .as_ref()
        .and_then(|picker| Some((picker.cwd.clone(), selected(picker)?.clone())))
    else {
        return Update::none();
    };
    if !writable(ctx) {
        crate::pane::pty_events::notify_error(ctx, "Unlock failed", "Client is read-only");
        return Update::full();
    }
    if operation_in_flight(ctx) {
        crate::pane::pty_events::notify_info(ctx, "Worktree operation in progress");
        return Update::full();
    }
    if tree.lock.is_none() {
        return Update::none();
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::none();
    };
    let id = request_id(ctx);
    ctx.state.worktree_operation = Some(WorktreeOperation {
        request_id: id,
        epoch: ctx.state.runtime_epoch,
        connection: client.connection_token(),
        cwd: cwd.clone(),
        kind: WorktreeOperationKind::Unlock {
            path: tree.path.clone(),
        },
        open_when_done: false,
    });
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.pending_remove = None;
    }
    client.worktree(
        id,
        WorktreeRequest::Unlock {
            cwd,
            path: tree.path,
        },
    );
    Update::full()
}

/// A checkout's ✕ in the Worktrees tab was confirmed.
pub(crate) fn remove_from_sidebar(ctx: &mut Context<AppRoot>, path: String, force: bool) -> Update {
    let listing = &ctx.state.sidebar.worktrees;
    let Some((_, cwd)) = listing.source.clone() else {
        return Update::none();
    };
    let Some(tree) = listing
        .entries
        .iter()
        .find(|tree| tree.path == path)
        .cloned()
    else {
        return Update::none();
    };
    start_remove(ctx, cwd, tree, force)
}

/// Ask the session host to remove a linked checkout. The host refuses a checkout any session
/// records as its origin, and `force` only relaxes Git's dirty-checkout check. A stale lock is
/// lifted on the way, once the host has confirmed for itself that its owner is gone.
fn start_remove(
    ctx: &mut Context<AppRoot>,
    cwd: String,
    tree: crate::git::worktrees::WorktreeInfo,
    force: bool,
) -> Update {
    if !writable(ctx) {
        crate::pane::pty_events::notify_error(ctx, "Remove failed", "Client is read-only");
        return Update::full();
    }
    if operation_in_flight(ctx) {
        crate::pane::pty_events::notify_info(ctx, "Worktree operation in progress");
        return Update::full();
    }
    let stale = tree.lock.as_ref().is_some_and(|lock| lock.stale);
    if !tree.linked || tree.bare {
        crate::pane::pty_events::notify_error(
            ctx,
            "Remove failed",
            "The primary worktree cannot be removed",
        );
        return Update::full();
    }
    if tree.lock.is_some() && !stale {
        crate::pane::pty_events::notify_error(
            ctx,
            "Remove failed",
            "Worktree is locked; unlock it first",
        );
        return Update::full();
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::none();
    };
    let id = request_id(ctx);
    ctx.state.worktree_operation = Some(WorktreeOperation {
        request_id: id,
        epoch: ctx.state.runtime_epoch,
        connection: client.connection_token(),
        cwd: cwd.clone(),
        kind: WorktreeOperationKind::Remove {
            path: tree.path.clone(),
            force,
        },
        open_when_done: false,
    });
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.pending_remove = None;
    }
    client.worktree(
        id,
        WorktreeRequest::Remove {
            cwd,
            path: tree.path,
            force,
            unlock_stale: stale,
        },
    );
    Update::full()
}

pub(crate) fn apply_result(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    request_id: u64,
    result: WorktreeResult,
) -> Update {
    // A create or remove belongs to the attachment that sent it, which may since have moved to the
    // background: its reply is matched and reported wherever it comes from, or the operation
    // would stay "in progress" forever.
    let operation = ctx
        .state
        .worktree_operation
        .take_if(|op| op.epoch == epoch && op.request_id == request_id);
    if let Some(operation) = operation {
        let foreground = epoch == ctx.state.runtime_epoch;
        let picker_here = foreground
            && ctx
                .state
                .worktree_picker
                .as_ref()
                .is_some_and(|picker| picker.cwd == operation.cwd);
        let sidebar_here = foreground
            && ctx
                .state
                .sidebar
                .worktrees
                .source
                .as_ref()
                .is_some_and(|(_, cwd)| *cwd == operation.cwd);
        if sidebar_here {
            // The checkout list changed on the host; show it without waiting for the next tick.
            crate::update::sidebar::worktrees::request_list(ctx);
        }
        match (operation.kind, result) {
            (
                WorktreeOperationKind::Create { .. },
                WorktreeResult::Created {
                    worktree,
                    unignored,
                },
            ) => {
                if let Some(directory) = unignored {
                    warn_unignored(ctx, &directory);
                }
                if picker_here {
                    return enter_tree(ctx, worktree);
                }
                if operation.open_when_done && foreground {
                    let target = ctx.state.current().remote_target.clone();
                    let checkouts = ctx
                        .state
                        .worktree_lists
                        .get(target.as_ref(), &operation.cwd)
                        .map(|trees| trees.iter().map(|tree| tree.path.clone()).collect())
                        .unwrap_or_default();
                    let known = discovered_sessions(ctx, target.as_ref());
                    return enter_checkout(ctx, worktree, target, checkouts, known);
                }
                crate::pane::pty_events::notify_path_info(
                    ctx,
                    "Created worktree",
                    worktree.path.clone(),
                    worktree.path,
                );
            }
            (WorktreeOperationKind::Remove { .. }, WorktreeResult::Removed { path }) => {
                if picker_here {
                    return refresh(ctx);
                }
                crate::pane::pty_events::notify_info(ctx, format!("Removed worktree {path}"));
            }
            (WorktreeOperationKind::Unlock { .. }, WorktreeResult::Unlocked { path }) => {
                if picker_here {
                    return refresh(ctx);
                }
                crate::pane::pty_events::notify_info(ctx, format!("Unlocked worktree {path}"));
            }
            (
                WorktreeOperationKind::Remove { path, force: false },
                WorktreeResult::Failed { message },
            ) => {
                let dirty = message.contains("--force");
                if picker_here
                    && dirty
                    && let Some(picker) = ctx.state.worktree_picker.as_mut()
                {
                    picker.pending_remove = Some(PendingWorktreeRemove {
                        path: path.clone(),
                        kind: PendingWorktreeRemoveKind::Dirty,
                    });
                }
                crate::pane::pty_events::notify_error(ctx, "Remove failed", message);
                if sidebar_here && dirty {
                    // Git refused a dirty checkout: the row re-arms as a forced removal, so the
                    // next ✕ says what it will do and then does it.
                    ctx.state.sidebar.worktrees.force_remove = Some(path.clone());
                    ctx.state.sidebar.pending_row_close =
                        Some(crate::state::SidebarClose::Worktree { path, force: true });
                    return crate::ops::confirm::arm(ctx);
                }
            }
            (_, WorktreeResult::Failed { message }) => {
                crate::pane::pty_events::notify_error(ctx, "Worktree operation failed", message);
            }
            _ => {}
        }
        return Update::full();
    }
    // List and preview replies only ever feed what is on screen.
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let sidebar_status = ctx.state.sidebar.worktrees.pending_status == Some(request_id);
    let picker_status = ctx
        .state
        .worktree_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_status == Some(request_id));
    if sidebar_status || picker_status {
        let (target, cwd) = if sidebar_status {
            ctx.state.sidebar.worktrees.pending_status = None;
            let Some(source) = ctx.state.sidebar.worktrees.source.clone() else {
                return Update::none();
            };
            source
        } else {
            let picker = ctx.state.worktree_picker.as_mut().unwrap();
            picker.pending_status = None;
            (picker.target.clone(), picker.cwd.clone())
        };
        let statuses = match result {
            WorktreeResult::Statuses { statuses } => statuses,
            WorktreeResult::Failed { .. } => crate::git::pull_requests::WorktreeStatuses {
                unavailable: true,
                ..Default::default()
            },
            _ => return Update::none(),
        };
        let same_repository = |entries: &[crate::git::worktrees::WorktreeInfo]| {
            entries.iter().any(|tree| tree.path == cwd)
        };
        let mut changed = false;
        if let Some(picker) = ctx.state.worktree_picker.as_mut()
            && picker.target == target
            && (picker.cwd == cwd || same_repository(&picker.entries))
            && picker.pending_status.is_none_or(|id| id <= request_id)
            && picker.statuses != statuses
        {
            changed = true;
            picker.statuses = statuses.clone();
        }
        let sidebar = &mut ctx.state.sidebar.worktrees;
        if sidebar.source.as_ref().is_some_and(|(host, root)| {
            *host == target && (*root == cwd || same_repository(&sidebar.entries))
        }) && sidebar.pending_status.is_none_or(|id| id <= request_id)
            && sidebar.statuses != statuses
        {
            changed = true;
            sidebar.statuses = statuses.clone();
        }
        ctx.state.worktree_statuses.put(target, cwd, statuses);
        return if changed {
            Update::full()
        } else {
            Update::none()
        };
    }
    if ctx.state.sidebar.worktrees.pending == Some(request_id) {
        return crate::update::sidebar::worktrees::listed(ctx, result);
    }
    let Some(picker) = ctx.state.worktree_picker.as_mut() else {
        return Update::none();
    };
    if picker.pending_list == Some(request_id) {
        picker.pending_list = None;
        match result {
            WorktreeResult::Listed { worktrees, .. } => {
                // A refresh may reorder or drop rows; stay on the same checkout when it remains.
                let selected_path = picker
                    .entries
                    .get(picker.selected)
                    .map(|tree| tree.path.clone());
                picker.selected = selected_path
                    .and_then(|path| worktrees.iter().position(|tree| tree.path == path))
                    .unwrap_or_else(|| {
                        worktrees
                            .iter()
                            .position(|tree| tree.path == picker.cwd)
                            .unwrap_or(0)
                    });
                picker.entries = worktrees;
                picker.error = None;
                let (target, cwd, list) = (
                    picker.target.clone(),
                    picker.cwd.clone(),
                    picker.entries.clone(),
                );
                ctx.state.worktree_lists.put(target, cwd, list);
            }
            WorktreeResult::Failed { message } => {
                picker.entries.clear();
                picker.error = Some(message);
                let (target, cwd) = (picker.target.clone(), picker.cwd.clone());
                ctx.state.worktree_lists.forget(target.as_ref(), &cwd);
            }
            _ => {}
        }
        return Update::full();
    }
    if let Some(form) = picker.form.as_mut()
        && form.pending_preview == Some(request_id)
    {
        form.pending_preview = None;
        match result {
            WorktreeResult::Previewed { path, unignored } if !form.path_edited => {
                form.path.set_text(path);
                form.unignored = unignored;
            }
            WorktreeResult::Failed { message } => form.error = Some(message),
            _ => {}
        }
        return Update::full();
    }
    if let Some(form) = picker.form.as_mut()
        && form.pending_exclude == Some(request_id)
    {
        form.pending_exclude = None;
        match result {
            WorktreeResult::Excluded { directory } => {
                form.unignored = None;
                crate::pane::pty_events::notify_info(
                    ctx,
                    format!("Added `{directory}/` to .git/info/exclude"),
                );
            }
            WorktreeResult::Failed { message } => form.error = Some(message),
            _ => {}
        }
        return Update::full();
    }
    Update::none()
}

/// A checkout inside the repository that Git does not ignore shows up in `git status` and is swept
/// up by `git add -A`. Said once it exists; the fix stays the user's call.
fn warn_unignored(ctx: &mut Context<AppRoot>, directory: &str) {
    crate::pane::pty_events::notify_warning(
        ctx,
        "Worktree not ignored by Git",
        format!(
            "`{directory}/` shows in git status. Add it to .git/info/exclude with Ctrl+E in New \
             worktree, or `rozi worktrees exclude`."
        ),
    );
}

/// Add the directory the new-worktree form warned about to the repository's `.git/info/exclude`.
/// Only on request: creating a checkout never edits Git's ignore rules by itself.
pub(crate) fn exclude_from_form(ctx: &mut Context<AppRoot>) -> Update {
    if !writable(ctx) {
        crate::pane::pty_events::notify_error(ctx, "Exclude failed", "Client is read-only");
        return Update::full();
    }
    let Some((cwd, directory)) = ctx.state.worktree_picker.as_ref().and_then(|picker| {
        let form = picker.form.as_ref()?;
        Some((picker.cwd.clone(), form.unignored.clone()?))
    }) else {
        return Update::none();
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::none();
    };
    let id = request_id(ctx);
    if let Some(form) = ctx
        .state
        .worktree_picker
        .as_mut()
        .and_then(|picker| picker.form.as_mut())
    {
        form.pending_exclude = Some(id);
    }
    client.worktree(id, WorktreeRequest::Exclude { cwd, directory });
    Update::full()
}

#[cfg(test)]
mod tests {
    use tui_lipan::TestBackend;

    use crate::session::client::SessionClient;
    use crate::session::protocol::WorktreeResult;
    use crate::state::{WorktreeOperation, WorktreeOperationKind, WorktreePickerState};
    use crate::{AppRoot, Msg};

    fn on_large_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn pane_action_keeps_the_session_and_passes_host_paths_to_its_server() {
        on_large_stack(|| {
            use crate::session::client::ClientOutbound;
            use crate::session::protocol::ClientMessage;
            use crate::session::remote::RemoteTarget;
            use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
            use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods};

            crate::test_support::isolate_user_dirs();
            for (remote, click) in [(false, false), (true, false), (true, true)] {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, outbound) = SessionClient::test_channel();
                let target = remote.then(|| RemoteTarget::Alias("workbox".into()));
                let path = if remote {
                    "C:\\code\\worktrees\\feature with spaces"
                } else {
                    "/src/repo-worktrees/feature with spaces"
                };
                let state = backend.state_mut();
                state.config.animations.picker = crate::layout::anim::PickerAnimationStyle::Off;
                state.config.animations.enabled = false;
                let current = state.current_mut();
                current.session_client = Some(client);
                current.session_attached = true;
                current.session_name = Some("dev".into());
                current.remote_target = target.clone();
                current.remote_host = remote.then(|| "workbox".into());
                current.active_workspace = 1;
                let mut picker = WorktreePickerState::new("/src/repo".into(), target);
                picker.entries.push(crate::git::worktrees::WorktreeInfo {
                    path: path.into(),
                    branch: Some("feat/worktree-pane".into()),
                    detached: false,
                    bare: false,
                    prunable: false,
                    linked: true,
                    lock: None,
                });
                state.worktree_picker = Some(picker);
                let epoch = state.runtime_epoch;
                backend.render();
                let frame = backend.capture_frame().plain_text();
                assert!(frame.contains("pane Ctrl+Enter"), "{frame}");
                // Optional visual artifact from the same attached-session fixture that
                // verifies the action, rather than a disconnected picker sketch.
                #[cfg(feature = "ui-snapshot")]
                if !remote && let Ok(path) = std::env::var("ROZI_TEST_WORKTREE_PANE_CAPTURE") {
                    let png = backend.capture_ui_snapshot().to_png_default().unwrap();
                    std::fs::write(path, png).unwrap();
                }
                assert!(backend.focus_key(&"rozi-worktree-picker".into()));
                if click {
                    let lines = backend.capture_frame().to_fixed_grid_lines();
                    let (y, x) = lines
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(y, line)| {
                            let byte = line.find("pane Ctrl+Enter")?;
                            Some((y as u16, line[..byte].chars().count() as u16))
                        })
                        .unwrap();
                    for kind in [
                        MouseKind::Down(MouseButton::Left),
                        MouseKind::Up(MouseButton::Left),
                    ] {
                        backend
                            .send_mouse(MouseEvent {
                                x,
                                y,
                                kind,
                                mods: KeyMods::NONE,
                            })
                            .unwrap();
                    }
                } else {
                    backend
                        .send_key(KeyEvent {
                            code: KeyCode::Enter,
                            mods: KeyMods::CTRL,
                        })
                        .unwrap();
                }
                let state = backend.state();
                assert!(state.worktree_picker.is_none());
                assert_eq!(state.runtime_epoch, epoch);
                assert_eq!(state.current().session_name.as_deref(), Some("dev"));
                assert_eq!(state.current().active_workspace, 1);
                let workspace = &state.current().workspaces[1];
                let pane = workspace.panes.last().unwrap();
                assert_eq!(pane.identity.cwd.as_deref(), Some(path));
                assert_eq!(workspace.focused_pane, Some(pane.id));
                assert!(state.current().engaged);
                let spawns: Vec<_> = outbound
                    .try_iter()
                    .filter_map(|message| match message {
                        ClientOutbound::Control(ClientMessage::SpawnPane {
                            cwd,
                            local,
                            launch,
                            ..
                        }) => Some((cwd, local, launch)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(spawns.len(), 1);
                assert_eq!(spawns[0].0.as_deref(), Some(path));
                assert!(!spawns[0].1);
                assert!(spawns[0].2.is_none());
            }
        });
    }

    #[test]
    fn worktree_panes_require_layout_control_on_the_same_host() {
        on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            for denied in [
                "read-only",
                "follower",
                "other-host",
                "bare",
                "prunable",
                "scratch",
            ] {
                let mut backend = TestBackend::new(AppRoot::default());
                let (client, outbound) = SessionClient::test_channel();
                let state = backend.state_mut();
                state.current_mut().session_client = Some(client);
                let mut shared = crate::state::SharedSessionState::new(1);
                shared.controller = Some(if denied == "follower" { 2 } else { 1 });
                shared.read_only = denied == "read-only";
                state.current_mut().shared = Some(shared);
                state.scratch_visible = denied == "scratch";
                let target = (denied == "other-host")
                    .then(|| crate::session::remote::RemoteTarget::Alias("elsewhere".into()));
                let mut picker = WorktreePickerState::new("/src/repo".into(), target);
                picker.entries.push(crate::git::worktrees::WorktreeInfo {
                    path: "/src/worktree".into(),
                    branch: Some("feat/pane".into()),
                    detached: false,
                    bare: denied == "bare",
                    prunable: denied == "prunable",
                    linked: true,
                    lock: None,
                });
                state.worktree_picker = Some(picker);
                let before = state.current().workspaces[0].panes.len();
                backend.dispatch(Msg::WorktreeOpenPane).unwrap();
                assert_eq!(
                    backend.state().current().workspaces[0].panes.len(),
                    before,
                    "{denied}"
                );
                assert!(backend.state().worktree_picker.is_some(), "{denied}");
                assert!(
                    !outbound.try_iter().any(|message| matches!(
                        message,
                        crate::session::client::ClientOutbound::Control(
                            crate::session::protocol::ClientMessage::SpawnPane { .. }
                        )
                    )),
                    "{denied}"
                );
            }
        });
    }

    /// Start a create on the foreground attachment's connection.
    fn start_create(backend: &mut TestBackend<AppRoot>, client: &SessionClient) -> u64 {
        let state = backend.state_mut();
        state.current_mut().session_client = Some(client.clone());
        state.worktree_operation = Some(WorktreeOperation {
            request_id: 41,
            epoch: state.runtime_epoch,
            connection: client.connection_token(),
            cwd: "/src/repo".into(),
            kind: WorktreeOperationKind::Create {
                branch: "feat/x".into(),
            },
            open_when_done: false,
        });
        state.runtime_epoch
    }

    #[test]
    fn a_create_finishing_after_a_session_switch_still_completes() {
        on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            let (client, _outbound) = SessionClient::test_channel();
            let origin = start_create(&mut backend, &client);
            {
                // Switching sessions parks the attachment that sent the create.
                let state = backend.state_mut();
                state.park_current(origin, crate::state::Attachment::new());
                state.runtime_epoch = origin + 100;
            }
            assert!(backend.state().worktree_operation_reachable());

            backend
                .dispatch(Msg::SessionWorktreeResult {
                    epoch: origin,
                    request_id: 41,
                    result: WorktreeResult::Created {
                        worktree: crate::git::worktrees::WorktreeInfo {
                            path: "/src/repo-worktrees/feat-x".into(),
                            branch: Some("feat/x".into()),
                            detached: false,
                            bare: false,
                            prunable: false,
                            linked: true,
                            lock: None,
                        },
                        unignored: None,
                    },
                })
                .unwrap();
            assert!(
                backend.state().worktree_operation.is_none(),
                "a background completion must retire the operation"
            );
        });
    }

    #[test]
    fn an_operation_whose_connection_is_gone_stops_blocking_new_ones() {
        on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            let (sent_on, _outbound) = SessionClient::test_channel();
            start_create(&mut backend, &sent_on);
            // The attachment reconnected: its reply can no longer arrive on the old connection.
            let (reconnected, _outbound) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_client = Some(reconnected);
                state.worktree_picker = Some(WorktreePickerState::new("/src/repo".into(), None));
            }
            assert!(!backend.state().worktree_operation_reachable());

            backend.dispatch(Msg::WorktreeNew).unwrap();
            let state = backend.state();
            assert!(state.worktree_operation.is_none());
            assert!(
                state
                    .worktree_picker
                    .as_ref()
                    .is_some_and(|picker| picker.form.is_some()),
                "the new-worktree form opens instead of reporting an operation in progress"
            );
        });
    }
}
