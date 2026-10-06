use super::*;

use crate::state::{PendingWorktreeRemoveKind, WorktreeFormField, WorktreeFormState};

pub(crate) fn worktree_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.worktree_picker.as_ref() else {
        return Text::new("").into();
    };
    if let Some(form) = picker.form.as_ref() {
        return worktree_form(ctx, form);
    }
    let selected = picker.selected_entry();
    let writable = ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_none_or(|shared| !shared.read_only);
    let busy = ctx.state.worktree_operation_reachable();
    let armed = picker
        .pending_remove
        .as_ref()
        .filter(|pending| selected.is_some_and(|tree| tree.path == pending.path))
        .map(|pending| pending.kind);
    let (remove_label, confirm) = match armed {
        Some(PendingWorktreeRemoveKind::Dirty) => {
            ("force remove", "again to force (dirty checkout)")
        }
        Some(PendingWorktreeRemoveKind::StaleLock) => (
            "unlock & remove",
            "again to unlock and remove (lock owner gone)",
        ),
        Some(PendingWorktreeRemoveKind::Clean) | None => ("remove", "again to remove"),
    };
    let removable = selected.is_some_and(|tree| {
        tree.linked && !tree.bare && tree.lock.as_ref().is_none_or(|lock| lock.stale)
    });
    let armed = armed.is_some();
    let actions = vec![
        OverlayAction::new(
            "enter",
            "open",
            Msg::WorktreeOpenSelected,
            selected.is_some() && !busy,
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-enter",
            "pane",
            Msg::WorktreeOpenPane,
            selected.is_some() && !busy && crate::ops::worktrees::can_open_pane(&ctx.state),
        ),
        OverlayAction::new(
            "ctrl-c",
            "copy path",
            Msg::WorktreeCopyPath,
            selected.is_some(),
        ),
        OverlayAction::new("ctrl-n", "new", Msg::WorktreeNew, writable && !busy),
        OverlayAction::new("ctrl-r", "refresh", Msg::WorktreeRefresh, !busy),
        OverlayAction::destructive(
            ctx,
            remove_label,
            Msg::WorktreeRemoveSelected,
            writable && !busy && removable,
            armed,
        )
        .with_confirm_cue(confirm),
        OverlayAction::new(
            "ctrl-u",
            "unlock",
            Msg::WorktreeUnlockSelected,
            writable && !busy && selected.is_some_and(|tree| tree.lock.is_some()),
        ),
        OverlayAction::new("esc", "close", Msg::CloseWorktrees, true).hide_hint(),
    ];
    let rows: Vec<WorktreeRow> = picker
        .entries
        .iter()
        .map(|tree| WorktreeRow::new(tree, picker))
        .collect();
    let widest_row = rows
        .iter()
        .map(|row| MARKER_WIDTH + row.branch.chars().count() + row.description().chars().count())
        .max()
        .unwrap_or(0);
    let entries = (0..rows.len())
        .map(|index| {
            // The label is what search ranks; paths still match through the aliases.
            SearchEntry::Item(picker.search_item(index).expect("listed worktree"))
        })
        .collect();
    let theme = &ctx.state.theme;
    let branch_style = fg_only(&theme.primary).bold();
    let marker_style = fg_only(&theme.accent);
    let muted = fg_only(&theme.muted);
    let status_styles = rows
        .iter()
        .map(|row| {
            row.status.map_or(muted, |status| {
                crate::view::worktree_status_style(theme, status)
            })
        })
        .collect::<Vec<_>>();
    let rows = Arc::new(rows);
    let render_item: OverlayItemRenderer<usize> =
        Arc::new(move |item: &SearchItem<usize>, _highlight| {
            let row = rows.get(item.value)?;
            let mut description = Vec::new();
            if !row.work_status.is_empty() {
                description
                    .push(Span::new(row.work_status.clone()).style(status_styles[item.value]));
            }
            if !row.state.is_empty() {
                if !description.is_empty() {
                    description.push(Span::new(" · ").style(muted));
                }
                description.push(Span::new(row.state.clone()).style(muted));
            }
            Some(
                ListItem::from_spans([
                    Span::new(if row.current { "● " } else { "  " }).style(marker_style),
                    Span::new(row.branch.clone()).style(branch_style),
                ])
                .description_spans(description)
                .primary_truncate_description_first(false),
            )
        });
    let empty = if let Some(error) = picker.error.as_deref() {
        format!("Git: {error}")
    } else if picker.pending_list.is_some() {
        "Loading worktrees…".to_string()
    } else if picker.entries.is_empty() {
        "No Git worktrees".to_string()
    } else {
        "No worktrees match".to_string()
    };
    let host = picker
        .target
        .as_ref()
        .map_or_else(|| "local".to_string(), |target| target.display_label());
    OverlayPalette::new(
        format!("Worktrees · {host}"),
        crate::view::worktree_picker_key(),
        Msg::CloseWorktrees,
        picker_width(widest_row),
    )
    .entries(entries)
    .render_item(render_item)
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
                } else if key.code == KeyCode::Char('e') && key.mods.ctrl && !key.mods.alt {
                    Some(Msg::WorktreeExclude)
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
    if let Some(directory) = form.unignored.as_deref() {
        body = body.child(
            HStack::new()
                .height(Length::Auto)
                .padding((0, 1, 0, 2))
                .child(
                    Text::new(format!("{directory}/ is not ignored by Git"))
                        .style(Style::new().fg(theme.status.warning)),
                ),
        );
    }
    let mut hints = hint_row()
        .child(hint_button(ctx, "create", "enter", Msg::WorktreeFormSubmit))
        .child(hint_pill(theme, "next field", "tab"));
    if form.unignored.is_some() && form.pending_exclude.is_none() {
        hints = hints.child(hint_button(ctx, "exclude", "ctrl+e", Msg::WorktreeExclude));
    }
    body = body.child(hints);
    action_palette_modal(ctx, "New worktree")
        .on_close(ctx.link().callback(|_| Msg::WorktreeFormClose))
        .child(body)
        .key("rozi-worktree-form-modal")
}

/// Cells before the branch: the current-checkout marker.
const MARKER_WIDTH: usize = 2;

/// The path remains an alias, while the right edge carries work and checkout status.
struct WorktreeRow {
    branch: String,
    work_status: String,
    status: Option<crate::git::pull_requests::WorkStatus>,
    state: String,
    current: bool,
}

impl WorktreeRow {
    fn new(
        tree: &crate::git::worktrees::WorktreeInfo,
        picker: &crate::state::WorktreePickerState,
    ) -> Self {
        let branch = match (&tree.branch, tree.bare) {
            (_, true) => "(bare)".to_string(),
            (Some(branch), false) => branch.clone(),
            (None, false) => "(detached)".to_string(),
        };
        let state = if !tree.linked {
            "primary"
        } else if tree.lock.as_ref().is_some_and(|lock| lock.stale) {
            "stale lock"
        } else if tree.lock.is_some() {
            "locked"
        } else if tree.prunable {
            "prunable"
        } else {
            ""
        }
        .to_string();
        let pr = picker.statuses.checkouts.get(&tree.path);
        let work_status = match pr {
            Some(pr) => format!("#{} {}", pr.number, pr.status.picker_label()),
            None if tree.linked && tree.branch.is_some() && picker.statuses.unavailable => {
                "PR unavailable".into()
            }
            None => String::new(),
        };
        Self {
            branch,
            work_status,
            status: pr.map(|pr| pr.status),
            state,
            current: tree.path == picker.cwd,
        }
    }

    fn description(&self) -> String {
        [self.work_status.as_str(), self.state.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Wide enough for the widest branch and status, within a
/// range that keeps a short list compact and a long one from spanning an ultrawide screen. The
/// modal is clamped to the viewport either way.
fn picker_width(widest_row: usize) -> u16 {
    // Frame border, row padding, and the gap between a label and its description.
    const CHROME: usize = 8;
    (widest_row + CHROME).clamp(72, 160) as u16
}

#[cfg(test)]
mod tests {
    use super::picker_width;

    #[test]
    fn the_picker_grows_with_its_widest_row_within_bounds() {
        assert_eq!(picker_width(20), 72);
        assert_eq!(picker_width(90), 98);
        assert_eq!(picker_width(400), 160);
    }
}
