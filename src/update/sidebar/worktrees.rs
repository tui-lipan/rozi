//! The Worktrees tab: the checkouts of the focused pane's repository, on the session's host.
//!
//! It follows the focused pane the way the Files and Git tabs do, and refreshes on the same signal
//! they do — a new root, a finished command, or the repository refresh tick — so a checkout added
//! from a shell appears without a manual refresh. Git runs on the session host's worktree worker;
//! nothing here blocks the UI.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::session::protocol::{WorktreeRequest, WorktreeResult};
use crate::state::SidebarWorktrees;

/// The tab's stable id, as `[sidebar] tabs` and panel placement name it.
pub(crate) const TAB_ID: &str = "worktrees";

pub(crate) fn worktrees_active(ctx: &Context<AppRoot>) -> bool {
    ctx.state.sidebar_visible
        && ctx
            .state
            .sidebar
            .active_tabs()
            .any(|id| id.as_str() == TAB_ID)
}

/// Whether the repository the tab lists is still the focused pane's. Compared by borrow: this runs
/// after every message, so the steady state must not allocate.
fn source_is_current(ctx: &Context<AppRoot>) -> bool {
    let listing = &ctx.state.sidebar.worktrees;
    match crate::ops::worktrees::repository_scope_ref(&ctx.state) {
        Ok((cwd, target)) => listing
            .source
            .as_ref()
            .is_some_and(|(listed_target, listed_cwd)| {
                listed_cwd == cwd && listed_target.as_ref() == target
            }),
        Err(reason) => listing.source.is_none() && listing.unavailable.as_deref() == Some(reason),
    }
}

/// Chokepoint: keep the tab on the focused pane's repository and ask for its checkouts whenever
/// the repository refresh signal moves. Returns whether what the tab shows changed.
pub(crate) fn sync_worktrees_tab(ctx: &mut Context<AppRoot>) -> bool {
    if !worktrees_active(ctx) {
        return false;
    }
    let mut changed = false;
    if !source_is_current(ctx) {
        let listing = match crate::ops::worktrees::repository_scope(&ctx.state) {
            Ok((cwd, target)) => {
                // Start from the last list for this repository rather than an empty one, so moving
                // between projects swaps rows instead of flashing a loading state.
                let cached = ctx.state.worktree_lists.get(target.as_ref(), &cwd);
                SidebarWorktrees {
                    loaded: cached.is_some(),
                    entries: cached.map(<[_]>::to_vec).unwrap_or_default(),
                    source: Some((target, cwd)),
                    ..SidebarWorktrees::default()
                }
            }
            Err(reason) => SidebarWorktrees {
                unavailable: Some(reason.to_string()),
                ..SidebarWorktrees::default()
            },
        };
        ctx.state.sidebar.worktrees = listing;
        changed = true;
    }
    let listing = &ctx.state.sidebar.worktrees;
    if listing.source.is_some()
        && listing.pending.is_none()
        && listing.requested_token != Some(ctx.state.sidebar.git_refresh_token)
    {
        request_list(ctx);
    }
    changed
}

/// Ask the session host for the listed repository's checkouts, and which sessions use them.
pub(crate) fn request_list(ctx: &mut Context<AppRoot>) {
    let Some((_, cwd)) = ctx.state.sidebar.worktrees.source.clone() else {
        return;
    };
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    let id = crate::ops::worktrees::request_id(ctx);
    let listing = &mut ctx.state.sidebar.worktrees;
    listing.pending = Some(id);
    listing.requested_token = Some(ctx.state.sidebar.git_refresh_token);
    client.worktree(id, WorktreeRequest::List { cwd });
}

/// The host answered the tab's list request.
pub(crate) fn listed(ctx: &mut Context<AppRoot>, result: WorktreeResult) -> Update {
    let listing = &mut ctx.state.sidebar.worktrees;
    listing.pending = None;
    let Some((target, cwd)) = listing.source.clone() else {
        return Update::none();
    };
    match result {
        WorktreeResult::Listed {
            worktrees,
            sessions,
        } => {
            if listing
                .force_remove
                .as_ref()
                .is_some_and(|path| !worktrees.iter().any(|tree| &tree.path == path))
            {
                listing.force_remove = None;
            }
            let unchanged = listing.loaded
                && listing.error.is_none()
                && listing.entries == worktrees
                && listing.sessions == sessions;
            listing.entries = worktrees.clone();
            listing.sessions = sessions;
            listing.loaded = true;
            listing.error = None;
            ctx.state.worktree_lists.put(target, cwd, worktrees);
            if unchanged {
                // The periodic refresh usually finds nothing new; skip the repaint.
                return Update::none();
            }
        }
        WorktreeResult::Failed { message } => {
            listing.entries.clear();
            listing.sessions.clear();
            listing.loaded = true;
            listing.error = Some(message);
            ctx.state.worktree_lists.forget(target.as_ref(), &cwd);
        }
        _ => return Update::none(),
    }
    Update::full()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use tui_lipan::TestBackend;

    use crate::config::{SidebarTab, SidebarTabId};
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::{ClientMessage, WorktreeRequest, WorktreeResult};
    use crate::{AppRoot, Msg};

    const REPO: &str = "/src/repo";
    const LINKED: &str = "/src/repo-worktrees/feat";

    fn on_large_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .unwrap()
            .join()
            .unwrap();
    }

    fn tree(path: &str, linked: bool) -> crate::git::worktrees::WorktreeInfo {
        crate::git::worktrees::WorktreeInfo {
            path: path.into(),
            branch: Some(if linked { "feat" } else { "main" }.into()),
            detached: false,
            bare: false,
            prunable: false,
            linked,
            locked: false,
        }
    }

    /// A Worktrees tab on screen, following a focused pane in `REPO`, on a session whose outgoing
    /// messages the test reads.
    fn backend() -> (TestBackend<AppRoot>, Receiver<ClientOutbound>) {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, outbound) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.sidebar.tabs = vec![SidebarTab::Worktrees];
        state.sidebar.panels[0].tabs = vec![SidebarTabId::new(super::TAB_ID)];
        state.sidebar.panels[0].active_tab = Some(SidebarTabId::new(super::TAB_ID));
        state.current_mut().session_client = Some(client);
        let focused = state.focused_pane().expect("a focused pane");
        crate::pane::lifecycle::find_pane_mut(state, focused)
            .unwrap()
            .terminal
            .project_root = Some(REPO.into());
        (backend, outbound)
    }

    /// Any message runs the chokepoint that keeps the tab in step with the focused pane.
    fn nudge(backend: &mut TestBackend<AppRoot>) {
        backend.dispatch(Msg::SidebarPointerMoved(0)).unwrap();
    }

    fn sent_worktree_requests(outbound: &Receiver<ClientOutbound>) -> Vec<(u64, WorktreeRequest)> {
        outbound
            .try_iter()
            .filter_map(|message| match message {
                ClientOutbound::Control(ClientMessage::Worktree {
                    request_id,
                    request,
                }) => Some((request_id, request)),
                _ => None,
            })
            .collect()
    }

    fn list(backend: &mut TestBackend<AppRoot>, request_id: u64) {
        let epoch = backend.state().runtime_epoch;
        backend
            .dispatch(Msg::SessionWorktreeResult {
                epoch,
                request_id,
                result: WorktreeResult::Listed {
                    worktrees: vec![tree(REPO, false), tree(LINKED, true)],
                    sessions: Default::default(),
                },
            })
            .unwrap();
    }

    #[test]
    fn the_tab_follows_the_focused_repository_and_caches_its_checkouts() {
        on_large_stack(|| {
            let (mut backend, outbound) = backend();
            nudge(&mut backend);
            let requests = sent_worktree_requests(&outbound);
            let [(id, WorktreeRequest::List { cwd })] = requests.as_slice() else {
                panic!("one list request, got {requests:?}");
            };
            assert_eq!(cwd, REPO);
            assert_eq!(
                backend.state().sidebar.worktrees.source,
                Some((None, REPO.into()))
            );

            list(&mut backend, *id);
            let state = backend.state();
            assert_eq!(state.sidebar.worktrees.entries.len(), 2);
            assert_eq!(
                state.worktree_lists.get(None, REPO).map(<[_]>::len),
                Some(2)
            );
            // Nothing moved the refresh signal, so no second list is asked for.
            nudge(&mut backend);
            assert!(sent_worktree_requests(&outbound).is_empty());
        });
    }

    #[test]
    fn a_checkout_is_removed_on_the_second_press_and_a_dirty_one_rearms_to_force() {
        on_large_stack(|| {
            let (mut backend, outbound) = backend();
            nudge(&mut backend);
            let (id, _) = sent_worktree_requests(&outbound).remove(0);
            list(&mut backend, id);
            // Rows: header, primary, linked checkout, "New worktree".
            let linked_row = 2;
            let close = Msg::SidebarRowClose {
                panel: 0,
                index: linked_row,
            };

            backend.dispatch(close.clone()).unwrap();
            assert_eq!(
                backend.state().sidebar.pending_row_close,
                Some(crate::state::SidebarClose::Worktree {
                    path: LINKED.into(),
                    force: false
                }),
                "the first press only arms"
            );
            assert!(sent_worktree_requests(&outbound).is_empty());

            backend.dispatch(close.clone()).unwrap();
            let requests = sent_worktree_requests(&outbound);
            let [(remove_id, WorktreeRequest::Remove { path, force, .. })] = requests.as_slice()
            else {
                panic!("one remove request, got {requests:?}");
            };
            assert_eq!((path.as_str(), *force), (LINKED, false));

            let epoch = backend.state().runtime_epoch;
            backend
                .dispatch(Msg::SessionWorktreeResult {
                    epoch,
                    request_id: *remove_id,
                    result: WorktreeResult::Failed {
                        message: "fatal: contains modified files, use --force to delete it".into(),
                    },
                })
                .unwrap();
            let state = backend.state();
            assert_eq!(
                state.sidebar.worktrees.force_remove.as_deref(),
                Some(LINKED)
            );
            assert_eq!(
                state.sidebar.pending_row_close,
                Some(crate::state::SidebarClose::Worktree {
                    path: LINKED.into(),
                    force: true
                }),
                "Git's refusal re-arms the row as a forced removal"
            );

            backend.dispatch(close).unwrap();
            let forced = sent_worktree_requests(&outbound)
                .into_iter()
                .find_map(|(_, request)| match request {
                    WorktreeRequest::Remove { force, .. } => Some(force),
                    _ => None,
                });
            assert_eq!(forced, Some(true));
        });
    }
}
