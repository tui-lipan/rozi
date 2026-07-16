use tui_lipan::prelude::*;

use crate::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use crate::{HyprmuxApp, Msg};

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

pub(super) fn sessions_tab(ctx: &Context<HyprmuxApp>) -> Element {
    if ctx.state.sidebar.sessions.is_empty() {
        return Text::new("No sessions discovered")
            .style(super::super::fg_only(&ctx.state.theme.muted))
            .into();
    }
    ctx.state
        .sidebar
        .sessions
        .iter()
        .cloned()
        .fold(VStack::new().gap(0).padding((1, 0)), |body, entry| {
            let current = ctx.state.session_name.as_deref() == Some(entry.name.as_str());
            let pending =
                ctx.state.sidebar.pending_session_open.as_deref() == Some(entry.name.as_str());
            let label = if entry.ephemeral {
                "ephemeral".to_string()
            } else {
                entry.name.clone()
            };
            let marker = if current { "▎" } else { " " };
            let detail = if pending {
                "click again · ends temporary session".to_string()
            } else {
                session_detail(&entry, current)
            };
            let detail_style = if pending {
                Style::new().fg(ctx.state.theme.status.warning)
            } else {
                super::super::fg_only(&ctx.state.theme.muted)
            };
            let key_name = entry.name.clone();
            let content = HStack::new()
                .gap(1)
                .height(Length::Px(2))
                .style(if current {
                    Style::new().bg(ctx.state.theme.surface.element.elevate(0.04))
                } else {
                    Style::default()
                })
                .child(
                    VStack::new()
                        .gap(0)
                        .width(Length::Auto)
                        .height(Length::Px(2))
                        .child(
                            Text::new(marker)
                                .height(Length::Px(1))
                                .style(super::super::fg_only(&ctx.state.theme.accent)),
                        )
                        .child(
                            Text::new(marker)
                                .height(Length::Px(1))
                                .style(super::super::fg_only(&ctx.state.theme.accent)),
                        ),
                )
                .child(
                    VStack::new()
                        .gap(0)
                        .child(
                            Text::new(label).style(super::super::fg_only(&ctx.state.theme.primary)),
                        )
                        .child(Text::new(detail).style(detail_style)),
                );
            body.child(
                MouseRegion::new()
                    .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
                    .on_click(
                        ctx.link()
                            .callback(move |_| Msg::SidebarSessionActivate(entry.clone())),
                    )
                    .child(content)
                    .key(format!("sidebar-session-{key_name}")),
            )
        })
        .into()
}
