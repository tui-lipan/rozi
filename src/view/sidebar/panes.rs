use tui_lipan::prelude::*;

use crate::{HyprmuxApp, Msg};

pub(super) fn panes_tab(ctx: &Context<HyprmuxApp>) -> Element {
    let mut body = VStack::new().gap(1);
    let mut any = false;
    for (workspace_index, workspace) in ctx.state.workspaces.iter().enumerate() {
        let panes: Vec<_> = workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .collect();
        if panes.is_empty() {
            continue;
        }
        any = true;
        let workspace_label = workspace.name.as_deref().map_or_else(
            || format!("Workspace {}", workspace_index + 1),
            |name| format!("{}  {}", workspace_index + 1, name),
        );
        let mut section = VStack::new().gap(0).child(
            Text::new(format!(" {workspace_label}"))
                .style(super::super::fg_only(&ctx.state.theme.accent).bold())
                .height(Length::Px(1)),
        );
        for pane in panes {
            let id = pane.id;
            let focused = ctx.state.focused_pane == Some(id);
            let marker = if focused { "▎" } else { " " };
            let title = pane.display_title(pane.terminal.title());
            let program = pane
                .terminal
                .foreground_program
                .as_deref()
                .unwrap_or("shell");
            let row = HStack::new()
                .gap(1)
                .height(Length::Px(2))
                .style(if focused {
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
                            Text::new(title).style(super::super::fg_only(&ctx.state.theme.primary)),
                        )
                        .child(
                            Text::new(program.to_string())
                                .style(super::super::fg_only(&ctx.state.theme.muted)),
                        ),
                );
            section = section.child(
                MouseRegion::new()
                    .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
                    .on_click(ctx.link().callback(move |_| Msg::SidebarFocusPane(id)))
                    .child(row)
                    .key(format!("sidebar-pane-{id}")),
            );
        }
        body = body.child(section);
    }
    if any {
        body.into()
    } else {
        VStack::new()
            .padding((0, 0, 0, 1))
            .child(Text::new("No panes").style(super::super::fg_only(&ctx.state.theme.muted)))
            .into()
    }
}
