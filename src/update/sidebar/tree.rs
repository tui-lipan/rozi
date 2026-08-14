use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{SidebarTab, SidebarTabId};
use crate::update::sidebar::polling::{TREE_REFRESH_INTERVAL, tree_active};

/// Keep exactly one low-frequency file-tree refresh chain alive while a Files/Git tab is visible.
/// A generation travels with each tick so hiding and reopening cannot revive an old chain beside
/// the new one.
pub(crate) fn ensure_tree_refresh_armed(ctx: &mut Context<AppRoot>) {
    if !tree_active(ctx) {
        ctx.state.sidebar.tree_refresh_armed_epoch = None;
        return;
    }
    if ctx.state.sidebar.tree_refresh_armed_epoch.is_some() {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    ctx.state.sidebar.tree_refresh_epoch = ctx.state.sidebar.tree_refresh_epoch.wrapping_add(1);
    let epoch = ctx.state.sidebar.tree_refresh_epoch;
    ctx.state.sidebar.tree_refresh_armed_epoch = Some(epoch);
    link.send_after(
        TREE_REFRESH_INTERVAL,
        crate::Msg::SidebarTreeRefresh { epoch },
    );
}

/// Refresh directory entries and Git state, then reschedule only while a tree remains visible.
pub(crate) fn tree_refresh(ctx: &mut Context<AppRoot>, epoch: u64) -> Update {
    if ctx.state.sidebar.tree_refresh_armed_epoch != Some(epoch) || !tree_active(ctx) {
        return Update::none();
    }
    ctx.state.sidebar.tree_entry_refresh_token =
        ctx.state.sidebar.tree_entry_refresh_token.wrapping_add(1);
    ctx.state.sidebar.git_refresh_token = ctx.state.sidebar.git_refresh_token.wrapping_add(1);
    Update::with_command(Command::after(
        TREE_REFRESH_INTERVAL,
        move |link: CommandLink<crate::Msg>| {
            link.send(crate::Msg::SidebarTreeRefresh { epoch });
        },
    ))
}

use super::activation::substitute;

/// Activate a file-tree row: run the tab's `on_click` with `{path}` replaced by the activated path.
///
/// A directory activation only expands the tree (handled in the widget); running the action for it
/// would type the directory's path at the prompt just because it was opened, so directories are
/// dropped here.
pub(crate) fn tree_activate(
    ctx: &mut Context<AppRoot>,
    config_epoch: u64,
    tab_id: SidebarTabId,
    path: String,
    is_dir: bool,
) -> Update {
    if is_dir || config_epoch != ctx.state.sidebar.config_epoch {
        return Update::none();
    }
    let action = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .find_map(|tab| match tab {
            SidebarTab::Tree { config, .. } if tab.id() == tab_id => config.on_click.clone(),
            _ => None,
        });
    action.map_or_else(Update::none, |action| {
        // `send` gets the path substituted as literal keystrokes. `run`/`popup` never do — a path
        // comes from the filesystem and must not compose a command line — so they receive it as
        // `$ROZI_FILE` instead, which a shell expands as one word inside quotes.
        let with_path = substitute(&action, "{path}", &path);
        let env = vec![("ROZI_FILE".to_string(), path)];
        crate::actions::execute_user_command_action_with_env(ctx, &with_path, env)
    })
}

/// Record a directory the user expanded or collapsed, so the tab reopens it the next time the tree
/// mounts. The widget owns the live expansion; this only mirrors it, and is fed back as the seed.
///
/// A collapse forgets that directory alone. What was expanded *inside* it keeps its entry, which is
/// what makes reopening a directory restore the shape it had rather than a single flat level.
pub(crate) fn tree_toggle(
    ctx: &mut Context<AppRoot>,
    tab_id: SidebarTabId,
    path: String,
    expanded: bool,
) -> Update {
    let remembered = ctx.state.sidebar.tree_expanded.entry(tab_id).or_default();
    if expanded {
        remembered.insert(path);
    } else {
        remembered.remove(&path);
    }
    // Nothing on screen depends on this: the tree already drew the toggle itself.
    Update::none()
}

/// The git repository containing `cwd`, found by walking ancestors for a `.git` entry. `.git` is a
/// file rather than a directory inside worktrees and submodules, so this tests existence, not kind.
/// The file tree needs a directory it has no listing for. Ask the session server to read it.
///
/// Deduplicated against in-flight and already-delivered paths: the widget re-emits a request on
/// every rebuild while a directory is still absent from the provided source, so without this an
/// expanded-but-slow directory would issue one `ListDirectory` per frame.
pub(crate) fn tree_entry_request(ctx: &mut Context<AppRoot>, path: String) -> Update {
    if ctx.state.current().remote_host.is_none() {
        return Update::none();
    }
    if ctx.state.sidebar.tree_pending.contains(&path)
        || ctx
            .state
            .sidebar
            .tree_listings
            .iter()
            .any(|listing| &*listing.path == path.as_str())
    {
        return Update::none();
    }
    let Some(client) = ctx.state.current().session_client.as_ref() else {
        return Update::none();
    };
    // Always fetch dotfiles: `show_hidden` is per-tab, and the widget filters provided entries by
    // it anyway, so one listing serves every tab and toggling the option needs no refetch.
    client.list_directory(path.clone(), true);
    ctx.state.sidebar.tree_pending.insert(path);
    Update::none()
}

/// A server-served directory listing arrived. Replaces any previous listing for that path so a
/// refresh overwrites rather than duplicating.
pub(crate) fn tree_directory_listed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    path: String,
    entries: Vec<crate::session::protocol::WireDirEntry>,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch || !ctx.state.sidebar.tree_pending.remove(&path) {
        return Update::none();
    }
    if error.is_some()
        && ctx
            .state
            .sidebar
            .tree_listings
            .iter()
            .any(|listing| &*listing.path == path.as_str() && listing.entries.is_ok())
    {
        return Update::none();
    }
    ctx.state
        .sidebar
        .tree_listings
        .retain(|listing| &*listing.path != path.as_str());
    let listing = match error {
        Some(error) => FileTreeDirectoryListing::error(path, error),
        None => FileTreeDirectoryListing::new(path, entries.into_iter().map(wire_entry_to_widget)),
    };
    ctx.state.sidebar.tree_listings.push(listing);
    Update::full()
}

/// A server-served change scan arrived, backing the `Changes` tab under `--remote`.
pub(crate) fn tree_changes_listed(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    root: String,
    changes: Vec<crate::session::protocol::WireChange>,
    error: Option<String>,
) -> Update {
    if epoch != ctx.state.runtime_epoch
        || ctx.state.sidebar.tree_changes_pending_root.as_deref() != Some(root.as_str())
    {
        return Update::none();
    }
    ctx.state.sidebar.tree_changes_pending_root = None;
    if let Some(error) = error {
        if ctx.state.sidebar.tree_changes_root.as_deref() == Some(root.as_str()) {
            return Update::none();
        }
        ctx.state.sidebar.tree_changes.clear();
        ctx.state.sidebar.tree_changes_root = None;
        ctx.state.sidebar.tree_changes_error = Some(error);
        return Update::full();
    }
    ctx.state.sidebar.tree_changes_error = None;
    ctx.state.sidebar.tree_changes = changes.into_iter().map(wire_change_to_widget).collect();
    ctx.state.sidebar.tree_changes_root = Some(root);
    Update::full()
}

fn wire_entry_to_widget(entry: crate::session::protocol::WireDirEntry) -> FileTreeEntry {
    let mut out = FileTreeEntry::new(entry.name, entry.is_dir)
        .symlink(entry.is_symlink)
        .ignored(entry.ignored);
    if entry.git_staged.is_some() || entry.git_unstaged.is_some() {
        out = out.git_status(GitFileStatus::new(
            entry.git_staged.map(wire_state_to_change),
            entry.git_unstaged.map(wire_state_to_change),
        ));
    }
    out
}

fn wire_change_to_widget(change: crate::session::protocol::WireChange) -> FileTreeChange {
    FileTreeChange::new(change.path, wire_state_to_status(change.state)).staged(change.staged)
}

fn wire_state_to_change(state: crate::session::protocol::WireChangeState) -> GitChangeState {
    use crate::session::protocol::WireChangeState as Wire;
    match state {
        Wire::Added => GitChangeState::Added,
        Wire::Modified => GitChangeState::Modified,
        Wire::Deleted => GitChangeState::Deleted,
        Wire::Renamed => GitChangeState::Renamed,
        Wire::Untracked => GitChangeState::Untracked,
        Wire::Conflicted => GitChangeState::Conflicted,
    }
}

fn wire_state_to_status(state: crate::session::protocol::WireChangeState) -> FileTreeChangeStatus {
    use crate::session::protocol::WireChangeState as Wire;
    match state {
        Wire::Added => FileTreeChangeStatus::Added,
        Wire::Modified => FileTreeChangeStatus::Modified,
        Wire::Deleted => FileTreeChangeStatus::Deleted,
        Wire::Renamed => FileTreeChangeStatus::Renamed,
        Wire::Untracked => FileTreeChangeStatus::Untracked,
        Wire::Conflicted => FileTreeChangeStatus::Conflicted,
    }
}

fn bump_git_refresh(sidebar: &mut crate::state::SidebarState) {
    sidebar.git_refresh_token = sidebar.git_refresh_token.wrapping_add(1);
}

/// File-tree chokepoint: keep the resolved roots in step with the focused pane, and refresh git
/// status when a command finishes.
///
/// Runs after every message like the focus chokepoint, so the common case must be cheap: it
/// compares the pane's reported directory against the cached one and does nothing when unchanged.
/// The ancestor walk only runs when the directory actually changed, which is user-paced — a shell
/// re-reporting the same directory on every prompt costs one string comparison.
pub(crate) fn sync_tree_roots(ctx: &mut Context<AppRoot>) {
    // Compared as a borrow: this runs per message, including output from off-screen panes that the
    // session handler deliberately makes free, so the steady state must not allocate.
    // Under `--remote` the tree roots at the server's path, not a local one, so this follows
    // `server_cwd_ref`. The repository walk stays local-only: `.git` cannot be probed across the
    // link, and `root_for` already falls back to the cwd when there is no repo root.
    let source_changed = ctx.state.sidebar.tree_source_epoch != Some(ctx.state.runtime_epoch);
    if source_changed
        || crate::pane_lifecycle::focused_server_cwd_ref(&ctx.state)
            != ctx.state.sidebar.tree_cwd.as_deref()
    {
        let cwd = crate::pane_lifecycle::focused_server_cwd_ref(&ctx.state).map(str::to_string);
        ctx.state.sidebar.tree_repo = if ctx.state.current().remote_host.is_some() {
            None
        } else {
            cwd.as_deref()
                .and_then(crate::platform::paths::discover_project_root)
        };
        ctx.state.sidebar.tree_cwd = cwd;
        ctx.state.sidebar.tree_source_epoch = Some(ctx.state.runtime_epoch);
        // A new root invalidates every server-served listing: paths under the old root will never
        // be asked for again, and keeping them would leak one host's tree into another's.
        ctx.state.sidebar.tree_listings.clear();
        ctx.state.sidebar.tree_pending.clear();
        ctx.state.sidebar.tree_changes.clear();
        ctx.state.sidebar.tree_changes_root = None;
        ctx.state.sidebar.tree_changes_pending_root = None;
        ctx.state.sidebar.tree_changes_error = None;
        ctx.state.sidebar.tree_entry_refresh_token =
            ctx.state.sidebar.tree_entry_refresh_token.wrapping_add(1);
        bump_git_refresh(&mut ctx.state.sidebar);
    }

    // A command finishing is the moment the working tree most likely changed, and it is a far
    // better refresh trigger than a timer: no polling while the user reads, immediate feedback
    // after a build, commit, or checkout.
    let phase = ctx.state.current().focused_pane.and_then(|id| {
        crate::pane_lifecycle::find_pane(&ctx.state, id)
            .map(|pane| (id, pane.terminal.command_phase))
    });
    if phase != ctx.state.sidebar.last_command_phase {
        ctx.state.sidebar.last_command_phase = phase;
        if matches!(
            phase,
            Some((
                _,
                crate::session::protocol::PaneCommandPhase::Completed { .. }
            ))
        ) {
            bump_git_refresh(&mut ctx.state.sidebar);
        }
    }

    refresh_remote_tree(ctx);
}

/// Re-ask the session server for tree data whose git state may have gone stale.
///
/// Keyed on `git_refresh_token`, the same signal the local tree refreshes on, so this fires once
/// per root change or completed command rather than per message. Already-known directories are
/// re-requested in place rather than cleared, so the tree does not flash back to loading rows.
fn refresh_remote_tree(ctx: &mut Context<AppRoot>) {
    if ctx.state.current().remote_host.is_none() || !tree_active(ctx) {
        return;
    }
    let token = ctx.state.sidebar.git_refresh_token;
    if token == ctx.state.sidebar.tree_server_token {
        return;
    }
    let Some(root) = ctx.state.sidebar.tree_cwd.clone() else {
        return;
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    ctx.state.sidebar.tree_server_token = token;
    ctx.state.sidebar.tree_changes_pending_root = Some(root.clone());
    client.list_changes(root);
    let known: Vec<String> = ctx
        .state
        .sidebar
        .tree_listings
        .iter()
        .map(|listing| listing.path.to_string())
        .collect();
    for path in known {
        if ctx.state.sidebar.tree_pending.insert(path.clone()) {
            client.list_directory(path, true);
        }
    }
}
