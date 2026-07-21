use tui_lipan::prelude::*;

use super::row::{self, Row};
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
        let mut section = VStack::new()
            .gap(0)
            .child(row::header(ctx, workspace_label, false));
        for pane in panes {
            let id = pane.id;
            let program = pane
                .terminal
                .foreground_program
                .as_deref()
                .unwrap_or("shell");
            let content = Row::new(pane.display_title(pane.terminal.title()))
                .marked(ctx.state.focused_pane == Some(id))
                .title_style(super::super::fg_only(&ctx.state.theme.primary))
                .detail(program, super::super::fg_only(&ctx.state.theme.muted))
                .build(ctx);
            section = section.child(
                MouseRegion::new()
                    .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
                    .on_click(ctx.link().callback(move |_| Msg::SidebarFocusPane(id)))
                    .child(content)
                    .key(format!("sidebar-pane-{id}")),
            );
        }
        body = body.child(section);
    }
    if any {
        body.into()
    } else {
        row::empty(ctx, "No panes")
    }
}
