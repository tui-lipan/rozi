use super::*;

use crate::state::{PendingWorktreeRemoveKind, WorktreeFormField, WorktreeFormState};

pub(crate) fn worktree_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.worktree_picker.as_ref() else {
        return Text::new("").into();
    };
    if let Some(form) = picker.form.as_ref() {
        if form.choosing_branch {
            return branch_picker(ctx, form);
        }
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
        .map(|tree| {
            let mut row = WorktreeRow::new(tree, picker);
            row.removing = ctx
                .state
                .worktree_removing(&tree.path, picker.target.as_ref());
            row
        })
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
    // Keep the marker column outside the label, which confirmation rendering replaces.
    let marker_rows = Arc::clone(&rows);
    let render_gutter = Arc::new(move |item: &SearchItem<usize>, _: &SearchHighlight| {
        let row = marker_rows.get(item.value)?;
        Some(ListItemGutter::from_spans([Span::new(if row.current {
            "● "
        } else {
            "  "
        })
        .style(marker_style)]))
    });
    let render_item: OverlayItemRenderer<usize> =
        Arc::new(move |item: &SearchItem<usize>, _highlight| {
            let row = rows.get(item.value)?;
            if row.removing {
                return Some(
                    ListItem::from_spans([Span::new(row.branch.clone()).style(branch_style)])
                        .description("removing…")
                        .description_style(muted)
                        .description_spinner(crate::view::session_status::picker_circle_spinner(
                            muted,
                        )),
                );
            }
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
                ListItem::from_spans([Span::new(row.branch.clone()).style(branch_style)])
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
    let repo = ctx.state.worktree_lists.repository_label(
        picker.target.as_ref(),
        &picker.repository,
        &picker.entries,
    );
    let show_host = picker.target.is_some()
        || ctx.state.remote.hosts.iter().any(|host| {
            matches!(
                host.probe,
                crate::state::HostProbe::Reached | crate::state::HostProbe::InFlight
            )
        })
        || ctx.state.background.values().any(|attachment| {
            attachment.remote_target.is_some() && attachment.session_client.is_some()
        });
    let title = if show_host {
        let host = picker.target.as_ref().map_or_else(
            || "local".to_string(),
            |target| ctx.state.remote_target_label(target),
        );
        format!("Worktrees · {repo} · {host}")
    } else {
        format!("Worktrees · {repo}")
    };
    let title = if picker.error.is_some() {
        format!("{title} · Git unavailable")
    } else if picker.statuses.unavailable {
        format!("{title} · PR unavailable")
    } else {
        title
    };
    OverlayPalette::new(
        title,
        crate::view::worktree_picker_key(),
        Msg::CloseWorktrees,
        picker_width(widest_row),
    )
    .entries(entries)
    .item_gutter(render_gutter)
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

fn branch_picker(ctx: &Context<AppRoot>, form: &WorktreeFormState) -> Element {
    let mut entries: Vec<_> = form
        .branches
        .iter()
        .enumerate()
        .map(|(index, branch)| {
            SearchEntry::Item(SearchItem::new(branch.name.clone(), index).description(
                ItemDescription::new().right(if branch.checkout.is_some() {
                    "checked out"
                } else {
                    "existing branch"
                }),
            ))
        })
        .collect();
    let query = form.branch_query.text().trim();
    if !query.is_empty() && !form.branches.iter().any(|branch| branch.name == query) {
        entries.push(SearchEntry::Item(
            SearchItem::new(query.to_string(), form.branches.len())
                .description(ItemDescription::new().right("create branch")),
        ));
    }
    let branches = form.branches.clone();
    let muted = fg_only(&ctx.state.theme.muted);
    let primary = fg_only(&ctx.state.theme.primary);
    let empty = form
        .error
        .as_deref()
        .unwrap_or(if form.pending_branches.is_some() {
            "Loading branches…"
        } else {
            "Type a new branch name"
        });
    OverlayPalette::new(
        "New worktree · Branch",
        crate::view::worktree_picker_key(),
        Msg::WorktreeBranchesClose,
        72,
    )
    .entries(entries)
    .placeholder("Search or create a branch…")
    .initial_query(form.branch_query.text().to_string())
    .selected(Some(form.branch_selected))
    .empty_text(empty.to_string())
    .actions(vec![
        OverlayAction::new(
            "enter",
            "choose",
            Msg::WorktreeBranchActivate(form.branch_selected),
            true,
        )
        .hint_only(),
    ])
    .render_item(Arc::new(move |item: &SearchItem<usize>, _| {
        let checked_out = branches
            .get(item.value)
            .is_some_and(|branch| branch.checkout.is_some());
        Some(
            ListItem::from_spans([Span::new(item.label.to_string()).style(if checked_out {
                muted
            } else {
                primary
            })])
            .description_spans([Span::new(if checked_out {
                "checked out"
            } else if item.value == branches.len() {
                "create branch"
            } else {
                "existing branch"
            })
            .style(muted)]),
        )
    }))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::WorktreeBranchQuery(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::WorktreeBranchSelect(event.item.value)),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::WorktreeBranchActivate(event.item.value)),
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
                } else if key.code == KeyCode::Char('b') && key.mods.ctrl && !key.mods.alt {
                    Some(Msg::WorktreeBranchesOpen)
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
        .child(hint_pill(theme, "next field", "tab"))
        .child(hint_button(
            ctx,
            "branches",
            "ctrl+b",
            Msg::WorktreeBranchesOpen,
        ));
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
    removing: bool,
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
            Some(pr) => format!(
                "#{} {}{}",
                pr.number,
                pr.status.picker_label(),
                if picker.statuses.unavailable {
                    " · stale"
                } else {
                    ""
                }
            ),
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
            removing: false,
        }
    }

    fn description(&self) -> String {
        if self.removing {
            return "  removing…".into();
        }
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
