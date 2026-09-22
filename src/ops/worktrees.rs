//! Worktree picker actions. Paths received from a remote server remain opaque strings here.

use tui_lipan::prelude::*;

use crate::session::discovery::DiscoveredSession;
use crate::session::protocol::{WorktreeRequest, WorktreeResult};
use crate::state::{
    WorktreeFormField, WorktreeFormState, WorktreeOperation, WorktreeOperationKind,
    WorktreePickerState,
};
use crate::{AppRoot, Msg};

fn request_id(ctx: &mut Context<AppRoot>) -> u64 {
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

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    let Some(pane) = ctx
        .state
        .focused_pane()
        .and_then(|id| crate::pane::lifecycle::find_pane(&ctx.state, id))
    else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Worktrees unavailable",
            "Focus a pane in a Git repository",
        );
        return Update::full();
    };
    if pane.terminal.cwd_host.is_some() {
        crate::pane::pty_events::notify_error(
            ctx,
            "Worktrees unavailable",
            "Pane is on a nested remote host",
        );
        return Update::full();
    }
    let Some(cwd) = pane.terminal.project_root.clone() else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Worktrees unavailable",
            "Focused pane is not in a Git repository",
        );
        return Update::full();
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Worktrees unavailable",
            "Attach to a session first",
        );
        return Update::full();
    };
    let target = ctx.state.current().remote_target.clone();
    let mut picker = WorktreePickerState::new(cwd.clone(), target.clone());
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
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
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
    Update::full()
}

fn matching_sessions(picker: &WorktreePickerState, path: &str) -> Vec<DiscoveredSession> {
    picker
        .sessions
        .iter()
        .filter(|row| {
            row.remote_target == picker.target
                && row
                    .origin
                    .worktree
                    .as_ref()
                    .is_some_and(|tree| tree.path == path)
        })
        .cloned()
        .collect()
}

fn new_session_name(ctx: &Context<AppRoot>, branch: Option<&str>, path: &str) -> String {
    let base = crate::session::worktrees::session_name_base(branch, path);
    let picker = ctx.state.worktree_picker.as_ref();
    let target = picker.and_then(|picker| picker.target.as_ref());
    crate::session::worktrees::unused_session_name(&base, |name| {
        picker.is_some_and(|picker| picker.sessions.iter().any(|row| row.name == name))
            || crate::ops::session::lifecycle::session_name_already_running(ctx, name, target)
    })
    .unwrap_or_else(|| format!("{base}-{}", ctx.state.next_worktree_request_id))
}

fn enter_tree(ctx: &mut Context<AppRoot>, tree: crate::git::worktrees::WorktreeInfo) -> Update {
    let Some(picker) = ctx.state.worktree_picker.as_ref() else {
        return Update::none();
    };
    let matches = matching_sessions(picker, &tree.path);
    if matches.len() == 1 {
        let entry = matches.into_iter().next().unwrap();
        ctx.state.worktree_picker = None;
        return crate::ops::session::activate_discovered_session(ctx, entry);
    }
    if matches.len() > 1 {
        ctx.state.worktree_picker = None;
        ctx.state.session_picker = Some(crate::state::SessionPickerState::new(matches));
        ctx.state.show_session_picker = true;
        crate::ops::focus::request_session_picker_focus(ctx);
        return Update::full();
    }
    let name = new_session_name(ctx, tree.branch.as_deref(), &tree.path);
    let target = picker.target.clone();
    ctx.state.worktree_picker = None;
    crate::ops::session::open::open_named_target(
        ctx,
        name,
        crate::ops::session::open::OpenNamedIntent::CreateInWorktree { path: tree.path },
        target,
    )
}

pub(crate) fn open_selected(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.worktree_operation.is_some() {
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

pub(crate) fn activate(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    select(ctx, index);
    open_selected(ctx)
}

pub(crate) fn open_form(ctx: &mut Context<AppRoot>) -> Update {
    if !writable(ctx) {
        crate::pane::pty_events::notify_error(ctx, "Create failed", "Client is read-only");
        return Update::full();
    }
    if ctx.state.worktree_operation.is_some() {
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
    let id = request_id(ctx);
    ctx.state.worktree_operation = Some(WorktreeOperation {
        request_id: id,
        epoch: ctx.state.runtime_epoch,
        cwd: cwd.clone(),
        kind: WorktreeOperationKind::Create {
            branch: branch.clone(),
        },
    });
    if let Some(picker) = ctx.state.worktree_picker.as_mut() {
        picker.form = None;
    }
    crate::ops::focus::request_worktree_picker_focus(ctx);
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

pub(crate) fn remove_selected(ctx: &mut Context<AppRoot>) -> Update {
    if !writable(ctx) {
        return Update::none();
    }
    if ctx.state.worktree_operation.is_some() {
        crate::pane::pty_events::notify_info(ctx, "Worktree operation in progress");
        return Update::full();
    }
    let Some((cwd, tree, force)) = ctx.state.worktree_picker.as_ref().and_then(|picker| {
        let tree = selected(picker)?.clone();
        Some((
            picker.cwd.clone(),
            tree.clone(),
            picker.pending_remove.as_deref() == Some(tree.path.as_str()),
        ))
    }) else {
        return Update::none();
    };
    if !tree.linked || tree.bare || tree.locked {
        crate::pane::pty_events::notify_error(
            ctx,
            "Remove failed",
            "Primary or locked worktree cannot be removed",
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
        cwd: cwd.clone(),
        kind: WorktreeOperationKind::Remove {
            path: tree.path.clone(),
            force,
        },
    });
    client.worktree(
        id,
        WorktreeRequest::Remove {
            cwd,
            path: tree.path,
            force,
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
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let operation = ctx
        .state
        .worktree_operation
        .take_if(|op| op.epoch == epoch && op.request_id == request_id);
    if let Some(operation) = operation {
        let picker_here = ctx
            .state
            .worktree_picker
            .as_ref()
            .is_some_and(|picker| picker.cwd == operation.cwd);
        match (operation.kind, result) {
            (WorktreeOperationKind::Create { .. }, WorktreeResult::Created { worktree }) => {
                if picker_here {
                    return enter_tree(ctx, worktree);
                }
                crate::pane::pty_events::notify_info(
                    ctx,
                    format!("Created worktree {}", worktree.path),
                );
            }
            (WorktreeOperationKind::Remove { .. }, WorktreeResult::Removed { path }) => {
                if picker_here {
                    return refresh(ctx);
                }
                crate::pane::pty_events::notify_info(ctx, format!("Removed worktree {path}"));
            }
            (
                WorktreeOperationKind::Remove { path, force: false },
                WorktreeResult::Failed { message },
            ) => {
                if picker_here
                    && message.contains("--force")
                    && let Some(picker) = ctx.state.worktree_picker.as_mut()
                {
                    picker.pending_remove = Some(path);
                }
                crate::pane::pty_events::notify_error(ctx, "Remove failed", message);
            }
            (_, WorktreeResult::Failed { message }) => {
                crate::pane::pty_events::notify_error(ctx, "Worktree operation failed", message);
            }
            _ => {}
        }
        return Update::full();
    }
    let Some(picker) = ctx.state.worktree_picker.as_mut() else {
        return Update::none();
    };
    if picker.pending_list == Some(request_id) {
        picker.pending_list = None;
        match result {
            WorktreeResult::Listed { worktrees } => {
                picker.entries = worktrees;
                picker.selected = picker.selected.min(picker.entries.len().saturating_sub(1));
                picker.error = None;
            }
            WorktreeResult::Failed { message } => picker.error = Some(message),
            _ => {}
        }
        return Update::full();
    }
    if let Some(form) = picker.form.as_mut()
        && form.pending_preview == Some(request_id)
    {
        form.pending_preview = None;
        match result {
            WorktreeResult::Previewed { path } if !form.path_edited => {
                form.path.set_text(path);
            }
            WorktreeResult::Failed { message } => form.error = Some(message),
            _ => {}
        }
        return Update::full();
    }
    Update::none()
}
