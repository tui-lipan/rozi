//! The Agent conversations view: the requests between the session's agents, and one chain of them
//! drawn as a sequence diagram.

use tui_lipan::prelude::*;

use crate::app::AppRoot;
use crate::state::AgentConversationsState;

pub(crate) fn open(ctx: &mut Context<AppRoot>) -> Update {
    // Land on the newest request still waiting, else the newest one at all.
    let requests = crate::view::agent_requests::requests(&ctx.state);
    let selected = requests
        .iter()
        .rev()
        .find(|request| !request.state.is_resolved())
        .or_else(|| requests.last())
        .map(|request| request.id.clone());
    ctx.state.agent_conversations = Some(AgentConversationsState::new(selected));
    ctx.state.commands_dirty = true;
    request_focus(ctx);
    Update::full()
}

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.agent_conversations.take().is_none() {
        return Update::none();
    }
    ctx.state.commands_dirty = true;
    if ctx.state.is_launcher() {
        return Update::full();
    }
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

/// Focus whichever of the list or the diagram is showing.
pub(crate) fn request_focus(ctx: &mut Context<AppRoot>) {
    crate::ops::focus::request_agent_conversations_focus(ctx);
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(view) = ctx.state.agent_conversations.as_mut() {
        view.input.set_text(query);
    }
    Update::full()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, id: String) -> Update {
    if let Some(view) = ctx.state.agent_conversations.as_mut() {
        view.selected = Some(id);
    }
    Update::full()
}

pub(crate) fn open_chain(ctx: &mut Context<AppRoot>, id: String) -> Update {
    let root = crate::view::agent_requests::requests(&ctx.state)
        .iter()
        .find(|request| request.id == id)
        .map(|request| request.root.clone());
    let Some(view) = ctx.state.agent_conversations.as_mut() else {
        return Update::none();
    };
    view.selected = Some(id);
    view.chain = root;
    request_focus(ctx);
    Update::full()
}

pub(crate) fn back(ctx: &mut Context<AppRoot>) -> Update {
    let Some(view) = ctx.state.agent_conversations.as_mut() else {
        return Update::none();
    };
    if view.chain.take().is_none() {
        return close(ctx);
    }
    request_focus(ctx);
    Update::full()
}

/// Focus the agent request `id` was sent to, and close the view.
pub(crate) fn go_to(ctx: &mut Context<AppRoot>, id: String) -> Update {
    let pane = crate::view::agent_requests::requests(&ctx.state)
        .iter()
        .find(|request| request.id == id)
        .and_then(|request| crate::view::agent_requests::pane_of(&ctx.state, &request.to));
    let Some(pane) = pane else {
        return Update::none();
    };
    // Landed on before the view closes, so the focus the close returns to is the agent's pane.
    crate::ops::focus::focus_pane_anywhere(ctx, pane);
    close(ctx)
}
