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
        PortMsg::CloseOverlay => ports::close_overlay(ctx),
        PortMsg::QueryChanged(query) => ports::query_changed(ctx, query),
        PortMsg::Select(key) => ports::select_forward(ctx, key),
        PortMsg::Activate(key) => ports::activate_forward(ctx, key),
        PortMsg::OpenAddForm => ports::open_add_form(ctx),
        PortMsg::CloseAddForm => ports::close_add_form(ctx),
        PortMsg::FormChanged(field, event) => ports::form_changed(ctx, field, event),
        PortMsg::SubmitAddForm => ports::submit_add_form(ctx),
        PortMsg::RetrySelected => ports::retry_selected(ctx),
        PortMsg::CloseSelected => ports::close_selected(ctx),
    }
}
