use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::ports::{self, PortMsg};

pub(crate) fn handle(ctx: &mut Context<AppRoot>, msg: PortMsg) -> Update {
    match msg {
        PortMsg::Event {
            key,
            generation,
            event,
        } => ports::handle_event(ctx, key, generation, event),
    }
}
