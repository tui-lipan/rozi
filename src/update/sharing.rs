use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::sharing;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Close,
    QueryChanged(String),
    Select(u32),
    GrantFromQuery,
    CycleRole(u32),
    RevokeSelected,
}

pub(crate) fn handle(ctx: &mut Context<AppRoot>, msg: Msg) -> Update {
    match msg {
        Msg::Close => sharing::close(ctx),
        Msg::QueryChanged(query) => sharing::query_changed(ctx, query),
        Msg::Select(uid) => sharing::select(ctx, uid),
        Msg::GrantFromQuery => sharing::grant_from_query(ctx),
        Msg::CycleRole(uid) => sharing::cycle_role(ctx, uid),
        Msg::RevokeSelected => sharing::revoke_selected(ctx),
    }
}
