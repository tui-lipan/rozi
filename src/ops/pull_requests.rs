//! Keeps the pull requests of the checkouts agents work in current while a view shows them.
//!
//! Each host looks up its own repositories, so a path is only ever read by the machine it belongs
//! to: this machine through a lookup the client runs, another host through its monitor, which
//! asks once per poll. Nothing is looked up while no agent view is open.

use std::collections::{BTreeMap, HashMap};

use tui_lipan::prelude::*;

use crate::git::pull_requests::{Lookup, WorktreeStatuses};
use crate::session::remote::RemoteTarget;
use crate::{AppRoot, Msg};

/// How often the open views' repositories are read again, so an agent that moves to another
/// checkout is followed. The hosts pace their own lookups.
const TICK: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether a view that shows agents' pull requests is on screen: the Agents view, or the sidebar's
/// Agents tab.
fn visible(state: &crate::state::State) -> bool {
    state.agent_picker.is_some()
        || (state.sidebar_shown()
            && state
                .visible_sidebar_tabs()
                .any(|id| *id == crate::config::SidebarTab::agents().id()))
}

/// The repositories each host is asked about: every one an agent it runs works in.
fn wanted(state: &crate::state::State) -> HashMap<Option<RemoteTarget>, Vec<String>> {
    let mut wanted: HashMap<Option<RemoteTarget>, Vec<String>> = HashMap::new();
    for source in crate::view::agents::global_agent_rows(state)
        .into_iter()
        .filter_map(|row| row.pull_request_source)
    {
        let repositories = wanted.entry(source.host).or_default();
        if !repositories.contains(&source.repository) {
            repositories.push(source.repository);
        }
    }
    wanted
}

pub(crate) fn ensure_tick(ctx: &mut Context<AppRoot>) {
    if !ctx.state.agent_pull_requests.tick_armed
        && visible(&ctx.state)
        && let Some(link) = ctx.state.command_link.as_ref()
    {
        ctx.state.agent_pull_requests.tick_armed = true;
        link.send(Msg::PullRequestTick);
    }
}

pub(crate) fn tick(ctx: &mut Context<AppRoot>) -> Update {
    if !visible(&ctx.state) {
        ctx.state.agent_pull_requests.tick_armed = false;
        want(ctx, HashMap::new());
        return Update::none();
    }
    let wanted = wanted(&ctx.state);
    want(ctx, wanted);
    Update::command_only(Command::after(TICK, |link| link.send(Msg::PullRequestTick)))
}

/// Tell every host which of its repositories to look up, and the rest to stop.
fn want(ctx: &mut Context<AppRoot>, mut wanted: HashMap<Option<RemoteTarget>, Vec<String>>) {
    let local = wanted.remove(&None).unwrap_or_default();
    let pull_requests = &mut ctx.state.agent_pull_requests;
    if pull_requests.local.is_none()
        && !local.is_empty()
        && let Some(link) = ctx.state.command_link.clone()
    {
        pull_requests.local = Some(Lookup::start(move |repositories| {
            link.send(Msg::PullRequests {
                target: None,
                generation: None,
                repositories,
            });
        }));
    }
    if let Some(lookup) = &pull_requests.local {
        lookup.want(local);
    }
    for monitor in &ctx.state.remote.monitors {
        monitor.want_pull_requests(
            wanted
                .remove(&Some(monitor.target.clone()))
                .unwrap_or_default(),
        );
    }
}

/// What a host found. A remote answer counts only from the monitor that is still current.
pub(crate) fn apply(
    ctx: &mut Context<AppRoot>,
    target: Option<RemoteTarget>,
    generation: Option<u64>,
    repositories: BTreeMap<String, WorktreeStatuses>,
) -> Update {
    if let Some(target) = &target
        && !ctx
            .state
            .remote
            .monitors
            .iter()
            .any(|monitor| &monitor.target == target && Some(monitor.generation) == generation)
    {
        return Update::none();
    }
    let hosts = &mut ctx.state.agent_pull_requests.hosts;
    if hosts.get(&target) == Some(&repositories) {
        return Update::none();
    }
    hosts.insert(target, repositories);
    Update::full()
}
