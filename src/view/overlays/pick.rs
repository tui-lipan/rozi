/// Room the modal frame and its padding take before a row's own text starts.
const PICK_ROW_CHROME: usize = 4;
/// Blank cells kept between a label and the description right-aligned after it. The description is
/// right-aligned, so this is what the reader actually sees between the two.
const PICK_DESCRIPTION_GAP: usize = 3;
/// A description shorter than this says nothing worth crowding the label for.
const PICK_MIN_DESCRIPTION: usize = 8;

/// Fit a producer-supplied description into what the label leaves behind.
///
/// Every other picker in rozi puts a short fixed token in this slot — `busy`, `restorable`, a
/// marker list — so none of them can overflow it. `pick` is the only one relaying arbitrary text
/// from another program, and a long enough description pushed the label out of its own row
/// entirely. The label is the thing being chosen between, so it is served first; the description
/// takes what is left, loses its tail to an ellipsis, and is dropped outright when the remainder is
/// too small to carry meaning.
fn fit_description(label: &str, description: &str, width: u16) -> String {
    let available = usize::from(width).saturating_sub(PICK_ROW_CHROME);
    let budget = available
        .saturating_sub(label.chars().count())
        .saturating_sub(PICK_DESCRIPTION_GAP);
    if budget < PICK_MIN_DESCRIPTION {
        return String::new();
    }
    if description.chars().count() <= budget {
        return description.to_string();
    }
    let mut fitted: String = description.chars().take(budget.saturating_sub(1)).collect();
    fitted.push('…');
    fitted
}

pub(crate) fn pick_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(pick) = ctx.state.pick.as_ref() else {
        return Text::new("").into();
    };

    let title = pick.title.clone();
    let placeholder = pick.placeholder.clone();
    let width = pick.width;
    let restore_query = pick.restore_query.clone();
    let rows = pick.rows.clone();
    let has_groups = rows.iter().any(|r| r.group.is_some());
    let selected_index = Some(pick.selected);

    let entries = if has_groups {
        let mut groups: Vec<(String, Vec<SearchEntry<usize>>)> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let group_name = row.group.as_deref().unwrap_or("Other").to_string();
            let mut item = SearchItem::new(row.label.clone(), index).active(row.active);
            if let Some(priority) = row.priority {
                item = item.priority(priority);
            }
            let description = fit_description(
                &row.label,
                row.disabled
                    .as_deref()
                    .or(row.description.as_deref())
                    .unwrap_or(""),
                width,
            );
            if !description.is_empty() {
                item = item.description(ItemDescription::new().right(description));
            }
            let entry = SearchEntry::Item(item);
            if let Some((_, items)) = groups.iter_mut().find(|(name, _)| *name == group_name) {
                items.push(entry);
            } else {
                groups.push((group_name, vec![entry]));
            }
        }
        search_entries_with_groups(groups)
    } else {
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let mut item = SearchItem::new(row.label.clone(), index).active(row.active);
                if let Some(priority) = row.priority {
                    item = item.priority(priority);
                }
                let description = fit_description(
                    &row.label,
                    row.disabled
                        .as_deref()
                        .or(row.description.as_deref())
                        .unwrap_or(""),
                    width,
                );
                if !description.is_empty() {
                    item = item.description(ItemDescription::new().right(description));
                }
                SearchEntry::Item(item)
            })
            .collect::<Vec<_>>()
    };

    let disabled_style = fg_only(&ctx.state.theme.muted);
    let item_style = fg_only(&ctx.state.theme.primary);
    let description_style = fg_only(&ctx.state.theme.muted);
    // An armed `confirm` action paints its row exactly like the session picker's armed kill, so
    // the affordance is one the user has already learned somewhere else.
    let armed = pick.pending_action.clone().and_then(|(index, row)| {
        pick.actions
            .get(index)
            .map(|action| (row, format!("again to {}", action.label)))
    });
    let error_bg = ctx.state.theme.status.error;

    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder(placeholder)
        // Rebuilt, not un-hidden: the picker unmounts while a prompt is up, so it comes back
        // seeded with the filter that was typed before.
        .initial_query(restore_query)
        .on_query_change(
            ctx.link()
                .callback(|query: std::sync::Arc<str>| Msg::PickQueryChanged(query.to_string())),
        )
        .preserve_groups(has_groups)
        .initial_selected_item_index(selected_index)
        .sync_selection(true)
        .render_item(Arc::new(
            move |item: &SearchItem<usize>, _highlight| {
                let row = &rows[item.value];
                if let Some((armed_row, cue)) = armed.as_ref()
                    && row.id.as_ref().unwrap_or(&row.label) == armed_row
                {
                    return render_pending_confirm_item(
                        item.label.as_ref(),
                        error_bg,
                        cue,
                        true,
                    )
                    .into();
                }
                let disabled_reason = row.disabled.as_deref();
                // Fitted here as well as on the entry: this custom renderer is what the palette
                // actually draws, and it reads the row out of state rather than using the
                // description the entry was built with.
                let status = fit_description(
                    item.label.as_ref(),
                    disabled_reason.or(row.description.as_deref()).unwrap_or(""),
                    width,
                );
                let style = if disabled_reason.is_some() {
                    disabled_style
                } else {
                    item_style
                };
                ListItem::from_spans(vec![Span::new(item.label.as_ref()).style(style)])
                    .description(status.as_str())
                    .description_style(if disabled_reason.is_some() {
                        disabled_style
                    } else {
                        description_style
                    })
                    .into()
            },
        ))
        .on_select(ctx.link().callback(|event: SearchEvent<usize>| {
            Msg::PickSelect(event.item.value)
        }))
        .on_activate(ctx.link().callback(|event: SearchEvent<usize>| {
            Msg::PickActivate(event.item.value)
        }));

    let palette = palette.input_key_interceptor(pick_key_interceptor(ctx));

    let body = VStack::new()
        .height(Length::Auto)
        .child(palette)
        .child(pick_hints(ctx));

    action_palette(ctx, &title, pick_key(), Msg::ClosePick, body, width)
}

/// Footer chords: select, plus whatever the caller declared. Built the same way every other picker
/// builds its own, so a caller-driven one is not visibly a second-class citizen.
fn pick_hints(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let mut row = hint_row();
    if let Some(pick) = ctx.state.pick.as_ref() {
        if pick
            .rows
            .get(pick.selected)
            .is_some_and(|row| row.disabled.is_none())
        {
            row = row.child(hint_pill(theme, "select", "enter"));
        }
        for action in &pick.actions {
            row = row.child(hint_pill(
                theme,
                &action.label,
                &KeyBinding::from_str(&action.key)
                    .map(|binding| crate::view::keys_display::format_binding(&binding))
                    .unwrap_or_else(|_| action.key.clone()),
            ));
        }
    }
    // No `esc` pill: cancelling is the app-wide default for every overlay, and the built-in
    // pickers do not spend footer space restating it.
    row.into()
}

/// Match a declared action chord against the key the palette's input did not consume.
fn pick_key_interceptor(ctx: &Context<AppRoot>) -> KeyHandler {
    let actions: Vec<(usize, KeyBinding)> = ctx
        .state
        .pick
        .as_ref()
        .map(|pick| {
            pick.actions
                .iter()
                .enumerate()
                .filter_map(|(index, action)| {
                    KeyBinding::from_str(&action.key)
                        .ok()
                        .map(|binding| (index, binding))
                })
                .collect()
        })
        .unwrap_or_default();
    ctx.link().key_handler(move |key| {
        actions
            .iter()
            .find(|(_, binding)| binding.matches_sequence(&[key]))
            .map(|(index, _)| Msg::PickActionKey(*index))
    })
}

/// The text prompt an action raised. Rendered above the picker, which stays mounted underneath so
/// cancelling returns to the list with its query and highlight intact.
pub(crate) fn pick_prompt_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(prompt) = ctx.state.pick.as_ref().and_then(|pick| pick.prompt.as_ref()) else {
        return Text::new("").into();
    };
    prompt_overlay(
        ctx,
        PromptChrome::new(&prompt.title, "", &[("submit", "enter")]),
        &prompt.input,
        pick_prompt_input_key(),
        Msg::PickPromptChanged,
        Msg::PickPromptCancel,
        Msg::PickPromptSubmit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row is a choice between labels, so a description never costs the label a cell.
    #[test]
    fn a_long_description_yields_to_its_label() {
        let long = "wasm-pack build wasm/showcase --target web --out-dir pkg";

        // Short label, wide picker: room for both, nothing is touched.
        assert_eq!(fit_description("dev", "npm run build", 60), "npm run build");

        // The overflowing case from a real project: the label survives whole and the description
        // gives up its tail.
        let fitted = fit_description("build:wasm", long, 60);
        assert!(fitted.ends_with('…'));
        assert!(fitted.chars().count() <= 60 - PICK_ROW_CHROME - "build:wasm".chars().count());
        assert!(long.starts_with(&fitted[..fitted.len() - '…'.len_utf8()]));

        // A label that fills the row leaves nothing worth saying, so the description goes.
        assert_eq!(fit_description(&"x".repeat(50), long, 60), "");
        // A narrow picker clips hard but still never spends a cell the label needs.
        let narrow = fit_description("build:wasm", long, 30);
        assert!(narrow.chars().count() <= 30 - PICK_ROW_CHROME - "build:wasm".chars().count());
    }
}
