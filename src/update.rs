use tui_lipan::prelude::*;

use crate::{HyprmuxApp, Msg};

pub(crate) fn handle_msg(
    _app: &mut HyprmuxApp,
    _msg: Msg,
    _ctx: &mut Context<HyprmuxApp>,
) -> Update {
    unreachable!("update::handle_msg is wired after operation modules are extracted")
}
