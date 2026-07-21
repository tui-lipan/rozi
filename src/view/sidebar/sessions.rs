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
    ctx.state
        .sidebar
        .sessions
        .iter()
        .cloned()
        .map(|entry| {
            let current = ctx.state.session_name.as_deref() == Some(entry.name.as_str());
            let pending =
                ctx.state.sidebar.pending_session_open.as_deref() == Some(entry.name.as_str());
            let label = if entry.ephemeral {
                "ephemeral".to_string()
            } else {
                entry.name.clone()
            };
            let detail = if pending {
                "press again · ends temporary session".to_string()
            } else {
                session_detail(&entry, current)
            };
            let detail_style = if pending {
                Style::new().fg(ctx.state.theme.status.warning)
            } else {
                super::super::fg_only(&ctx.state.theme.muted)
            };
            let row = Row::new(label)
                .active(current)
                .title_style(super::super::fg_only(&ctx.state.theme.primary))
                .detail(detail, detail_style);
            SidebarRow::item(row, RowTarget::Session(Box::new(entry)))
        })
        .collect()
}
