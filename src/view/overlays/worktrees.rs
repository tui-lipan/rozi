use super::*;

use crate::state::{WorktreeFormField, WorktreeFormState};

pub(crate) fn worktree_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.worktree_picker.as_ref() else {
        return Text::new("").into();
    };
    if let Some(form) = picker.form.as_ref() {
        return worktree_form(ctx, form);
    }
    let query = picker.input.text().trim().to_ascii_lowercase();
    let selected = picker.entries.get(picker.selected).filter(|tree| {
        query.is_empty()
            || tree.path.to_ascii_lowercase().contains(&query)
            || tree
                .branch
                .as_deref()
                .is_some_and(|branch| branch.to_ascii_lowercase().contains(&query))
    });
    let writable = ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_none_or(|shared| !shared.read_only);
    let busy = ctx.state.worktree_operation.is_some();
    let armed =
        selected.is_some_and(|tree| picker.pending_remove.as_deref() == Some(tree.path.as_str()));
    let actions = vec![
        OverlayAction::new(
            "enter",
            "open",
            Msg::WorktreeOpenSelected,
            selected.is_some() && !busy,
        )
        .hint_only(),
        OverlayAction::new("ctrl-n", "new", Msg::WorktreeNew, writable && !busy),
        OverlayAction::new("ctrl-r", "refresh", Msg::WorktreeRefresh, !busy),
        OverlayAction::new(
            "ctrl-k",
            if armed { "force remove" } else { "remove" },
            Msg::WorktreeRemoveSelected,
            writable
                && !busy
                && selected.is_some_and(|tree| tree.linked && !tree.bare && !tree.locked),
        )
        .confirm_if(
            armed,
            "again to force (dirty checkout)",
            ctx.state.theme.status.error,
            true,
        ),
        OverlayAction::new("esc", "close", Msg::CloseWorktrees, true),
    ];
    let entries = picker
        .entries
        .iter()
        .enumerate()
        .map(|(index, tree)| {
            let branch =
                tree.branch
                    .as_deref()
                    .unwrap_or(if tree.detached { "detached" } else { "bare" });
            let sessions = picker
                .sessions
                .iter()
                .filter(|row| {
                    row.remote_target == picker.target
                        && row
                            .origin
                            .worktree
                            .as_ref()
                            .is_some_and(|origin| origin.path == tree.path)
                })
                .count();
            let label = format!("{branch}  {}", tree.path);
            let description = if sessions > 0 {
                format!("{sessions} session{}", if sessions == 1 { "" } else { "s" })
            } else if !tree.linked {
                "primary".to_string()
            } else if tree.locked {
                "locked".to_string()
            } else if tree.prunable {
                "prunable".to_string()
            } else {
                String::new()
            };
            SearchEntry::item(label, index).description(picker_description(description))
        })
        .collect();
    let empty = if let Some(error) = picker.error.as_deref() {
        format!("Git: {error}")
    } else if picker.pending_list.is_some() {
        "Loading worktrees…".to_string()
    } else if picker.entries.is_empty() {
        "No Git worktrees".to_string()
    } else {
        "No worktrees match".to_string()
    };
    OverlayPalette::new(
        "Worktrees",
        crate::view::worktree_picker_key(),
        Msg::CloseWorktrees,
        72,
    )
    .header_right(
        picker
            .target
            .as_ref()
            .map_or("local".to_string(), |target| target.display_label()),
    )
    .entries(entries)
    .actions(actions)
    .armed_row(armed.then_some(picker.selected))
    .placeholder("Search branches or paths…")
    .initial_query(picker.input.text().to_string())
    .selected(Some(picker.selected))
    .empty_text(empty)
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::WorktreeQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::WorktreeSelect(event.item.value)),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::WorktreeActivate(event.item.value)),
    )
    .render(ctx)
}

fn form_row(ctx: &Context<AppRoot>, form: &WorktreeFormState, field: WorktreeFormField) -> Element {
    let theme = &ctx.state.theme;
    let focused = form.focus == field;
    let input: Element = if focused {
        Input::bound(form.input(field))
            .placeholder(match field {
                WorktreeFormField::Branch => "feat/my-task",
                WorktreeFormField::Base => "HEAD",
                WorktreeFormField::Path => "server default",
            })
            .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
            .focus_style(
                Style::new()
                    .fg(theme.border_active)
                    .bg(theme.surface.element),
            )
            .selection_style(theme.text_selection)
            .width(Length::Flex(1))
            .border(false)
            .padding((0, 0))
            .on_change(
                ctx.link()
                    .callback(move |event: InputEvent| Msg::WorktreeFormChanged(field, event)),
            )
            .on_key(ctx.link().key_handler(|key| {
                if key.is(KeyCode::Esc) {
                    Some(Msg::WorktreeFormClose)
                } else if key.code == KeyCode::Enter && !key.mods.ctrl && !key.mods.alt {
                    Some(Msg::WorktreeFormSubmit)
                } else {
                    None
                }
            }))
            .key(crate::view::worktree_form_input_key())
    } else {
        let value = form.input(field).text();
        let empty = value.is_empty();
        Text::new(if empty {
            match field {
                WorktreeFormField::Branch => "feat/my-task",
                WorktreeFormField::Base => "HEAD",
                WorktreeFormField::Path => "server default",
            }
        } else {
            value
        })
        .overflow(Overflow::Clip)
        .width(Length::Flex(1))
        .style(if empty {
            fg_only(&theme.muted).italic()
        } else {
            fg_only(&theme.primary)
        })
        .into()
    };
    HStack::new()
        .height(Length::Auto)
        .padding((0, 1, 0, 0))
        .child(
            Text::new(if focused { "› " } else { "  " })
                .width(Length::Px(2))
                .style(fg_only(&theme.accent)),
        )
        .child(
            Text::new(field.label())
                .width(Length::Px(8))
                .style(if focused {
                    fg_only(&theme.primary).bold()
                } else {
                    fg_only(&theme.muted)
                }),
        )
        .child(input)
        .into()
}

fn worktree_form(ctx: &Context<AppRoot>, form: &WorktreeFormState) -> Element {
    let theme = &ctx.state.theme;
    let mut body = VStack::new().height(Length::Auto).padding((1, 0, 0, 0));
    for field in WorktreeFormField::ORDER {
        body = body.child(form_row(ctx, form, field));
    }
    if let Some(error) = form.error.as_deref() {
        body = body.child(Text::new(error).style(Style::new().fg(theme.status.warning)));
    }
    body = body.child(
        hint_row()
            .child(hint_pill(theme, "create", "enter"))
            .child(hint_pill(theme, "next field", "tab"))
            .child(hint_pill(theme, "cancel", "esc")),
    );
    action_palette_modal(ctx, "New worktree")
        .on_close(ctx.link().callback(|_| Msg::WorktreeFormClose))
        .child(body)
        .into()
}
