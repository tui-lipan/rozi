use tui_lipan::prelude::*;

use super::row::{Row, RowTarget, SidebarRow};
use crate::HyprmuxApp;
use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};

fn session_detail(entry: &DiscoveredSession, current: bool) -> String {
    match &entry.status {
        DiscoveredSessionStatus::Running {
            panes,
            clients,
            created_from_profile,
            ..
        } => {
            let mut detail = format!("{panes} pane{}", if *panes == 1 { "" } else { "s" });
            let others = clients.saturating_sub(u32::from(current));
            if others > 0 {
                detail.push_str(&format!(" · {others} other"));
            }
            if let Some(profile) = created_from_profile {
                detail.push_str(&format!(" · from {profile}"));
            }
            detail
        }
        DiscoveredSessionStatus::Busy => "busy".to_string(),
        DiscoveredSessionStatus::Unknown => "incompatible or unavailable".to_string(),
    }
}

pub(super) fn sessions_rows(ctx: &Context<HyprmuxApp>) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    let mut last_group: Option<Option<&str>> = None;
    for entry in &ctx.state.sidebar.sessions {
        let group = entry.host.as_deref();
        if last_group != Some(group) {
            if last_group.is_some() {
                rows.push(SidebarRow::spacer());
            }
            rows.push(SidebarRow::header(
                Text::new(match group {
                    Some(host) => format!("REMOTE · {host}"),
                    None => "LOCAL".to_string(),
                })
                .style(super::super::fg_only(&ctx.state.theme.muted).bold()),
            ));
            last_group = Some(group);
        }
        let entry = entry.clone();
        let current = ctx.state.current().session_name.as_deref() == Some(entry.name.as_str())
            && ctx.state.current().remote_host == entry.host
            && ctx.state.current().remote_target == entry.remote_target;
        let connection = ctx
            .state
            .attachment_by_identity(&entry.name, entry.remote_target.as_ref())
            .map(|attachment| attachment.connection);
        let mut label = if entry.ephemeral {
            "ephemeral".to_string()
        } else {
            entry.name.clone()
        };
        if let Some(host) = entry.host.as_deref() {
            label.push('@');
            label.push_str(host);
        }
        let row = Row::new(label)
            .active(current)
            .title_style(super::super::fg_only(&ctx.state.theme.primary))
            .detail(
                match (current, connection) {
                    (false, Some(crate::state::ConnectionState::Connected)) => {
                        format!("attached · {}", session_detail(&entry, current))
                    }
                    (
                        false,
                        Some(
                            crate::state::ConnectionState::Connecting
                            | crate::state::ConnectionState::Reconnecting,
                        ),
                    ) => format!("reconnecting · {}", session_detail(&entry, current)),
                    (false, Some(_)) => format!("offline · {}", session_detail(&entry, current)),
                    _ => session_detail(&entry, current),
                },
                super::super::fg_only(&ctx.state.theme.muted),
            );
        rows.push(SidebarRow::item(row, RowTarget::Session(Box::new(entry))));
    }
    rows
}
