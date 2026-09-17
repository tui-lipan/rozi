use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelpKind {
    Global,
    Direct,
}

/// The two rows that edit the input scheme itself rather than one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchemeRow {
    Prefix,
    Modifier,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HelpRow {
    category: String,
    keys: String,
    label: String,
    kind: HelpKind,
    extra: String,
    config_id: Option<String>,
    scheme: Option<SchemeRow>,
    overridden: bool,
    default_keys: String,
}

impl HelpRow {
    fn global(category: &str, keys: &str, label: &str) -> Self {
        Self {
            category: category.to_string(),
            keys: keys.to_string(),
            label: label.to_string(),
            kind: HelpKind::Global,
            extra: String::new(),
            config_id: None,
            scheme: None,
            overridden: false,
            default_keys: String::new(),
        }
    }

    fn unbound(category: &str, label: &str) -> Self {
        Self {
            extra: "unbound".to_string(),
            ..Self::global(category, "", label)
        }
    }

    fn direct(category: &str, keys: &str, label: &str, extra: &str) -> Self {
        Self {
            category: category.to_string(),
            keys: keys.to_string(),
            label: label.to_string(),
            kind: HelpKind::Direct,
            extra: extra.to_string(),
            config_id: None,
            scheme: None,
            overridden: false,
            default_keys: String::new(),
        }
    }
}

fn help_rows(ctx: &Context<AppRoot>) -> Vec<HelpRow> {
    let editable = crate::commands::editable_commands(ctx)
        .into_iter()
        .map(|command| {
            (
                command.registry_id,
                (command.config_id, command.overridden, command.default_hint),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut rows = ctx
        .command_registry()
        .entries()
        .into_iter()
        .filter(|entry| {
            entry.id.as_str() != crate::commands::FORWARD_PREFIX_COMMAND_ID
                && entry.id.as_str() != "detach"
                && !entry.id.as_str().starts_with("app.")
                && !entry.id.as_str().starts_with("workspace.")
        })
        .map(|entry| {
            let edit = editable.get(entry.id.as_str());
            let category = entry.category.as_deref().unwrap_or("Other");
            let keys = entry.keybinding_hint.as_deref().unwrap_or("");
            let label = entry.label.as_ref();
            let mut row = if keys.is_empty() {
                HelpRow::unbound(category, label)
            } else {
                HelpRow::global(category, keys, label)
            };
            if let Some((config_id, overridden, default_keys)) = edit {
                row.config_id = Some(config_id.clone());
                row.overridden = *overridden;
                row.default_keys = default_keys.clone();
            }
            row
        })
        .collect::<Vec<_>>();
    rows.splice(0..0, scheme_rows(&ctx.state.config.input));
    rows.extend([
        HelpRow::global("Workspaces", "1-9", "Switch to workspace"),
        HelpRow::global("Workspaces", "Shift+1-9", "Move pane to workspace (follow)"),
        HelpRow::global(
            "Workspaces",
            "Ctrl+Shift+1-9",
            "Move workspace to workspace (follow)",
        ),
        HelpRow::global("Mouse", "drag", "Move pane (left-drag)"),
        HelpRow::global("Mouse", "right-drag", "Resize pane from corner"),
        HelpRow::global("Mouse", "drag gap", "Resize a tiled split"),
    ]);
    rows.extend(direct_mode_rows());
    rows
}

fn direct_mode_rows() -> Vec<HelpRow> {
    const COPY: &str = "Copy mode · DIRECT";
    const SIDEBAR: &str = "Sidebar focused · DIRECT";
    const HELP: &str = "Keybindings open · DIRECT";
    const COPY_EXTRA: &str = "direct copy mode context selection";
    const SIDEBAR_EXTRA: &str = "direct sidebar focused context tree files git panel";
    const HELP_EXTRA: &str = "direct keybindings help overlay context tabs edit";
    let mut rows = vec![
        HelpRow::direct(COPY, "hjkl / arrows", "Move cursor", COPY_EXTRA),
        HelpRow::direct(COPY, "w / b / e", "Word forward / back / end", COPY_EXTRA),
        HelpRow::direct(COPY, "W / B / E", "WORD forward / back / end", COPY_EXTRA),
        HelpRow::direct(
            COPY,
            "0 / ^ / $",
            "Line start / first non-blank / end",
            COPY_EXTRA,
        ),
        HelpRow::direct(COPY, "g / G", "Top / bottom of scrollback", COPY_EXTRA),
        HelpRow::direct(COPY, "Ctrl+u / Ctrl+d", "Half page up / down", COPY_EXTRA),
        HelpRow::direct(COPY, "v / Space", "Start selection", COPY_EXTRA),
        HelpRow::direct(COPY, "y / Enter", "Copy selection & exit", COPY_EXTRA),
        HelpRow::direct(COPY, "Esc / q", "Exit copy mode", COPY_EXTRA),
        HelpRow::direct(SIDEBAR, "↑ / k, ↓ / j", "Move cursor", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "PageUp / PageDown", "Move one page", SIDEBAR_EXTRA),
        HelpRow::direct(
            SIDEBAR,
            "Home / g, End / G",
            "First / last row",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(SIDEBAR, "Enter", "Activate", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "x", "Close the selected row", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "Tab / Shift+Tab", "Cycle tabs", SIDEBAR_EXTRA),
        HelpRow::direct(
            SIDEBAR,
            "← / h, → / l, Space",
            "Collapse / expand / toggle dirs",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Ctrl+Shift+← / Ctrl+Shift+→",
            "Reorder the active tab",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Ctrl+↑ / Ctrl+↓",
            "Focus the other panel",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Ctrl+Shift+↑ / Ctrl+Shift+↓",
            "Move tab to the other panel",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Shift+← / Shift+→",
            "Resize sidebar",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Shift+↑ / Shift+↓",
            "Resize panel split",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(SIDEBAR, "s", "Toggle panels", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "Esc", "Return to pane", SIDEBAR_EXTRA),
        HelpRow::direct(HELP, "type", "Filter", HELP_EXTRA),
        HelpRow::direct(HELP, "← / →, Tab / Shift+Tab", "Switch tabs", HELP_EXTRA),
        HelpRow::direct(
            HELP,
            "↑ / ↓, PageUp / PageDown",
            "Move selection",
            HELP_EXTRA,
        ),
        HelpRow::direct(HELP, "Home / End", "First / last row", HELP_EXTRA),
        HelpRow::direct(HELP, "Enter", "Change keybinding", HELP_EXTRA),
        HelpRow::direct(HELP, "Ctrl+u", "Unbind", HELP_EXTRA),
        HelpRow::direct(HELP, "Ctrl+d", "Reset to default", HELP_EXTRA),
        HelpRow::direct(HELP, "Ctrl+r", "Reset all", HELP_EXTRA),
        HelpRow::direct(HELP, "Esc", "Close", HELP_EXTRA),
    ];
    for row in &mut rows {
        row.extra.push_str(" modes");
    }
    rows
}

fn filtered_help_groups(
    rows: impl IntoIterator<Item = HelpRow>,
    tab: crate::state::HelpTab,
    query: &str,
) -> Vec<(String, Vec<HelpRow>)> {
    let query = normalize_help_query(query);
    let mut groups: Vec<(String, Vec<HelpRow>)> = Vec::new();
    for row in rows {
        let include = match tab {
            crate::state::HelpTab::Global => row.kind == HelpKind::Global && !row.keys.is_empty(),
            crate::state::HelpTab::Modes => row.kind == HelpKind::Direct,
            crate::state::HelpTab::Unbound => row.kind == HelpKind::Global && row.keys.is_empty(),
            crate::state::HelpTab::All => true,
        };
        if !include {
            continue;
        }
        if !query.is_empty() && !help_row_matches(&row, &query) {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(category, _)| *category == row.category)
        {
            Some((_, entries)) => entries.push(row),
            None => groups.push((row.category.clone(), vec![row])),
        }
    }
    groups.sort_by_key(|(category, _)| help_category_priority(category));
    groups
}

/// Selectable row ids in the current tab and query, in list order.
fn visible_keybinding_ids(ctx: &Context<AppRoot>) -> Vec<String> {
    let Some(keybindings) = ctx.state.keybindings.as_ref() else {
        return Vec::new();
    };
    filtered_help_groups(help_rows(ctx), keybindings.tab, keybindings.query.text())
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| help_row_id(&row))
        .collect()
}

/// The row that should stay highlighted after `id` leaves the current tab. Prefers the next
/// remaining row, then the previous. `None` when `id` is the only visible row.
pub(crate) fn neighbor_keybinding_id(ctx: &Context<AppRoot>, id: &str) -> Option<String> {
    let ids = visible_keybinding_ids(ctx);
    let index = ids.iter().position(|row| row == id)?;
    ids.get(index + 1)
        .or_else(|| index.checked_sub(1).and_then(|i| ids.get(i)))
        .cloned()
}

fn normalize_help_query(query: &str) -> String {
    collapse_ws(
        &query
            .trim()
            .to_ascii_lowercase()
            .replace(['+', '/', ',', '·'], " "),
    )
}

fn help_row_matches(row: &HelpRow, query: &str) -> bool {
    help_row_haystack(row).contains(query)
}

fn help_row_haystack(row: &HelpRow) -> String {
    collapse_ws(&format!(
        "{} {} {} {}",
        row.category
            .to_ascii_lowercase()
            .replace(['·', '+', '/'], " "),
        normalize_help_keys(&row.keys),
        row.label.to_ascii_lowercase(),
        row.extra.to_ascii_lowercase()
    ))
}

fn normalize_help_keys(keys: &str) -> String {
    keys.to_ascii_lowercase()
        .replace('←', " left ")
        .replace('→', " right ")
        .replace('↑', " up ")
        .replace('↓', " down ")
        .replace(['+', '/', ',', '·'], " ")
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The Prefix and Mod rows. Mod stays listed while its layer is off, reading `Off`, so it can be
/// turned back on from here.
fn scheme_rows(input: &crate::config::InputConfig) -> Vec<HelpRow> {
    let prefix = input.prefix.label();
    let modifier = crate::state::ModifierChoice::from_input(input).label();
    vec![
        HelpRow {
            extra: "prefix then key scheme".to_string(),
            scheme: Some(SchemeRow::Prefix),
            ..HelpRow::global("", &prefix, "Prefix · then key")
        },
        HelpRow {
            extra: "mod hold key scheme modifier".to_string(),
            scheme: Some(SchemeRow::Modifier),
            ..HelpRow::global("", modifier, "Mod · hold + key")
        },
    ]
}

pub(crate) fn keybinding_editor_dialog_overlay(ctx: &Context<AppRoot>) -> Element {
    match ctx
        .state
        .keybindings
        .as_ref()
        .map(|keybindings| &keybindings.stage)
    {
        Some(crate::state::KeybindingEditorStage::Capture { .. })
        | Some(crate::state::KeybindingEditorStage::Review { .. })
        | Some(crate::state::KeybindingEditorStage::Conflict { .. }) => {
            keybinding_change_overlay(ctx)
        }
        Some(crate::state::KeybindingEditorStage::Modifier { .. }) => {
            keybinding_modifier_overlay(ctx)
        }
        Some(crate::state::KeybindingEditorStage::ResetAll) => keybinding_reset_all_overlay(ctx),
        None | Some(crate::state::KeybindingEditorStage::List) => Text::new("").into(),
    }
}

/// The search field is the overlay's only focus target, as in `shared_search_palette`: typing
/// filters, and `keys` (the interceptor) drives the list, the tab strip, and the row actions.
fn help_search(
    ctx: &Context<AppRoot>,
    keybindings: &crate::state::KeybindingsState,
    matches: usize,
    total: usize,
    keys: KeyHandler,
) -> Element {
    let theme = &ctx.state.theme;
    Input::bound(&keybindings.query)
        .placeholder("Search keybindings…")
        .suffix(format!("{matches}/{total}"))
        .style(fg_only(&theme.muted))
        .focus_style(Style::new().fg(theme.border_active))
        .placeholder_style(fg_only(&theme.muted))
        .suffix_style(fg_only(&theme.primary))
        .focus_suffix_style(fg_only(&theme.primary))
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .border(false)
        .padding((0, 1))
        .on_change(ctx.link().callback(Msg::HelpQueryChanged))
        .key_interceptor(keys)
        .key(help_filter_key())
}

/// Every key the overlay understands, resolved against the rows as rendered.
///
/// Horizontal arrows belong to the tab strip rather than the caret: the query is a short filter
/// that is edited from its end, and one meaning per key is easier to learn than a caret-dependent
/// one. `Home`/`End` likewise jump through the list, matching `SearchPalette`.
fn keybindings_key_handler(
    ctx: &Context<AppRoot>,
    tab: crate::state::HelpTab,
    targets: Arc<[Option<HelpTarget>]>,
    selected: Option<usize>,
    page: isize,
    actions: &[OverlayAction],
) -> KeyHandler {
    let actions = actions
        .iter()
        .filter(|action| action.enabled && action.intercept)
        .map(|action| (action.key.clone(), action.msg.clone()))
        .collect::<Vec<_>>();
    ctx.link().key_handler(move |key| {
        let plain = !key.mods.ctrl && !key.mods.alt && !key.mods.super_key;
        let navigation = match key.code {
            KeyCode::Esc if plain => Some(Msg::CloseHelp),
            KeyCode::Tab if plain && !key.mods.shift => Some(help_tab_msg(tab, 1)),
            KeyCode::BackTab | KeyCode::Tab if plain => Some(help_tab_msg(tab, -1)),
            KeyCode::Left if plain && !key.mods.shift => Some(help_tab_msg(tab, -1)),
            KeyCode::Right if plain && !key.mods.shift => Some(help_tab_msg(tab, 1)),
            code if plain && !key.mods.shift => {
                let delta = match code {
                    KeyCode::Up => Some(-1),
                    KeyCode::Down => Some(1),
                    KeyCode::PageUp => Some(-page),
                    KeyCode::PageDown => Some(page),
                    KeyCode::Home => Some(isize::MIN),
                    KeyCode::End => Some(isize::MAX),
                    _ => None,
                };
                delta.and_then(|delta| {
                    let index = stepped_help_row(&targets, selected, delta)?;
                    let target = targets[index].as_ref()?;
                    Some(Msg::KeybindingSelect(target.id.clone()))
                })
            }
            _ => None,
        };
        navigation.or_else(|| {
            actions
                .iter()
                .find(|(binding, _)| binding.matches_sequence(&[key]))
                .map(|(_, msg)| msg.clone())
        })
    })
}

fn help_tab_msg(tab: crate::state::HelpTab, steps: isize) -> Msg {
    Msg::HelpTabSelected(tab.stepped(steps).index())
}

/// A selectable editor row. `id` is stable across renders.
#[derive(Clone)]
struct HelpTarget {
    id: String,
    edit: RowEdit,
    overridden: bool,
}

/// What `Enter` edits on a row. Generated ranges, mouse gestures, and mode references are
/// reference-only.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RowEdit {
    None,
    Action(String),
    Prefix,
    Modifier,
}

impl RowEdit {
    fn of(row: &HelpRow) -> Self {
        match (row.scheme, &row.config_id) {
            (Some(SchemeRow::Prefix), _) => Self::Prefix,
            (Some(SchemeRow::Modifier), _) => Self::Modifier,
            (None, Some(id)) => Self::Action(id.clone()),
            (None, None) => Self::None,
        }
    }

    fn change_msg(&self) -> Option<Msg> {
        match self {
            Self::None => None,
            Self::Action(id) => Some(Msg::KeybindingCapture(id.clone())),
            Self::Prefix => Some(Msg::KeybindingCapturePrefix),
            Self::Modifier => Some(Msg::KeybindingEditModifier),
        }
    }

    fn action_id(&self) -> Option<&str> {
        match self {
            Self::Action(id) => Some(id),
            _ => None,
        }
    }
}

fn help_row_id(row: &HelpRow) -> String {
    row.config_id
        .clone()
        .unwrap_or_else(|| format!("{}\u{1f}{}", row.category, row.label))
}

/// The selectable row `delta` rows away from `current`, clamped to the ends. Headers and spacers
/// (`None`) are skipped, and an unknown `current` counts as the first row, which is what the list
/// highlights in that case.
fn stepped_help_row(
    targets: &[Option<HelpTarget>],
    current: Option<usize>,
    delta: isize,
) -> Option<usize> {
    let rows = targets
        .iter()
        .enumerate()
        .filter_map(|(index, target)| target.as_ref().map(|_| index))
        .collect::<Vec<_>>();
    let last = rows.len().checked_sub(1)?;
    let position = current
        .and_then(|current| rows.iter().position(|row| *row == current))
        .unwrap_or(0);
    Some(rows[position.saturating_add_signed(delta).min(last)])
}

fn help_group_row_count(groups: &[(String, Vec<HelpRow>)]) -> usize {
    groups.iter().map(|(_, rows)| rows.len()).sum()
}

fn help_tabs(ctx: &Context<AppRoot>, tab: crate::state::HelpTab) -> Element {
    super::palette::picker_tabs(
        ctx,
        &["Global", "Modes", "Unbound", "All"],
        tab.index(),
        ctx.link()
            .callback(|event: TabsEvent| Msg::HelpTabSelected(event.index)),
    )
}

/// The Keybindings overlay: one searchable, tabbed list that is both the reference and the editor.
pub(crate) fn help_overlay(
    ctx: &Context<AppRoot>,
    keybindings: &crate::state::KeybindingsState,
) -> Element {
    let theme = &ctx.state.theme;
    let rows = help_rows(ctx);
    let total = help_group_row_count(&filtered_help_groups(rows.clone(), keybindings.tab, ""));
    let groups = filtered_help_groups(rows, keybindings.tab, keybindings.query.text());
    let matches = help_group_row_count(&groups);
    let mut items = Vec::new();
    let mut targets = Vec::new();
    for (group_index, (category, rows)) in groups.iter().enumerate() {
        if !category.is_empty() {
            if group_index > 0 {
                items.push(ListItem::spacer());
                targets.push(None);
            }
            items.push(help_section_item(category, theme));
            targets.push(None);
        }
        for row in rows {
            items.push(editable_help_row(row, theme));
            targets.push(Some(HelpTarget {
                id: help_row_id(row),
                edit: RowEdit::of(row),
                overridden: row.overridden,
            }));
        }
    }
    let selected_index = keybindings
        .selected
        .as_ref()
        .and_then(|selected| {
            targets
                .iter()
                .position(|target| target.as_ref().is_some_and(|target| &target.id == selected))
        })
        .or_else(|| targets.iter().position(Option::is_some));
    let selected = selected_index.and_then(|index| targets[index].as_ref());
    let change = selected.and_then(|target| target.edit.change_msg());
    let action_id = selected
        .and_then(|target| target.edit.action_id())
        .map(str::to_string);
    let selected_overridden = selected.is_some_and(|target| target.overridden);
    let target = action_id.clone().unwrap_or_default();
    let actions = vec![
        OverlayAction::new(
            "enter",
            "change",
            change.clone().unwrap_or(Msg::KeybindingCancelCapture),
            change.is_some(),
        ),
        OverlayAction::new(
            "ctrl-u",
            "unbind",
            Msg::KeybindingUnbind(target.clone()),
            action_id.is_some(),
        ),
        OverlayAction::new(
            "ctrl-d",
            "reset",
            Msg::KeybindingReset(target),
            selected_overridden,
        ),
        OverlayAction::new(
            "ctrl-r",
            "reset all",
            Msg::KeybindingResetAll,
            !ctx.state.config.key_sources.is_empty(),
        ),
    ];
    let targets: Arc<[Option<HelpTarget>]> = targets.into();
    let select_targets = targets.clone();
    let activate_targets = targets.clone();
    let list_rows = help_list_rows(ctx, targets.len());
    let page = isize::from(i16::try_from(list_rows.saturating_sub(1).max(1)).unwrap_or(i16::MAX));
    let keys = keybindings_key_handler(
        ctx,
        keybindings.tab,
        targets.clone(),
        selected_index,
        page,
        &actions,
    );
    let search = help_search(ctx, keybindings, matches, total, keys);
    let (selection_left, selection_right) =
        crate::view::picker_selection_cap_glyphs(&ctx.state.config);
    let list = List::new()
        .items(items)
        .selected(selected_index)
        .border(false)
        .selection_symbol(Some(selection_left))
        .selection_symbol_right(Some(selection_right))
        .selection_symbol_style(crate::view::picker_selection_cap_style(
            theme,
            theme.border_active,
        ))
        .unselected_symbol(Some(""))
        .selection_full_width(true)
        .selection_style(picker_selection_style(theme, None))
        .unfocused_selection_style(picker_selection_style(theme, None))
        .item_hover_style(Style::new().bg(theme.surface.element.elevate_by(0.08)))
        .item_horizontal_padding(crate::view::picker_list_item_horizontal_padding(
            &ctx.state.config,
        ))
        .header_horizontal_padding(0)
        .scroll_wheel(true)
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(theme))
        .empty_text("No matches")
        .empty_text_style(fg_only(&theme.muted))
        .height(Length::Px(list_rows))
        // Keyboard input stays in the search field; the list only takes clicks and the wheel.
        .focusable(false)
        .on_select(ctx.link().callback_opt(move |event: ListEvent| {
            let target = select_targets.get(event.index)?.as_ref()?;
            Some(Msg::KeybindingSelect(target.id.clone()))
        }))
        .on_activate(ctx.link().callback_opt(move |event: ListEvent| {
            let target = activate_targets.get(event.index)?.as_ref()?;
            target.edit.change_msg()
        }));
    let hints = overlay_hints(theme, &actions);
    let body = VStack::new()
        .height(Length::Auto)
        .child(search)
        .child(super::palette::picker_divider(theme))
        .child(help_tabs(ctx, keybindings.tab))
        .child(Spacer::new().height(Length::Px(1)))
        .child(list)
        .child(hints);
    Modal::new()
        .width(Length::Px(HELP_MODAL_WIDTH))
        // Content-sized so filtering down to a handful of rows shrinks the modal, capped at 70% of
        // the viewport. `reserve_height` pins the top edge, so the modal shrinks downward while you
        // type rather than re-centering under the cursor.
        .height(Length::Auto)
        .max_height(Length::Percent(HELP_MAX_HEIGHT_PERCENT))
        .reserve_height(Length::Percent(HELP_MAX_HEIGHT_PERCENT))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(theme.surface.element))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::CloseHelp))
        .child(super::palette::tabbed_picker_panel(
            ctx,
            "Keybindings",
            Length::Auto,
            body.into(),
        ))
        .into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeybindingCaptureMode {
    Recording,
    Review,
    Conflict,
}

struct KeybindingChangeView {
    target: crate::state::KeybindingTarget,
    mode: KeybindingCaptureMode,
    candidate: Option<KeyBinding>,
    conflicts: Vec<crate::state::KeybindingConflict>,
    replaceable: bool,
    conversion: Option<crate::state::LiteralConversion>,
    held_modifiers: KeyMods,
}

fn keybinding_change_view(ctx: &Context<AppRoot>) -> Option<KeybindingChangeView> {
    use crate::state::KeybindingEditorStage;
    let editor = ctx.state.keybindings.as_ref()?;
    match editor.stage.clone() {
        KeybindingEditorStage::Capture { target } => Some(KeybindingChangeView {
            target,
            mode: KeybindingCaptureMode::Recording,
            candidate: None,
            conflicts: Vec::new(),
            replaceable: false,
            conversion: None,
            held_modifiers: editor.held_modifiers,
        }),
        KeybindingEditorStage::Review {
            target,
            binding,
            conversion,
        } => Some(KeybindingChangeView {
            target,
            mode: KeybindingCaptureMode::Review,
            candidate: Some(binding),
            conflicts: Vec::new(),
            replaceable: false,
            conversion,
            held_modifiers: KeyMods::NONE,
        }),
        KeybindingEditorStage::Conflict {
            target,
            binding,
            conflicts,
            replaceable,
        } => Some(KeybindingChangeView {
            target,
            mode: KeybindingCaptureMode::Conflict,
            candidate: Some(binding),
            conflicts,
            replaceable,
            conversion: None,
            held_modifiers: KeyMods::NONE,
        }),
        _ => None,
    }
}

/// Current and default keys as keycap pills: the current binding on the accent, the default quiet.
/// A `current / default` string with several alternatives becomes one pill per alternative.
fn keybinding_current_defaults(ctx: &Context<AppRoot>, current: &str, default: &str) -> Element {
    let theme = &ctx.state.theme;
    let current_pills = keycap_pills(
        ctx,
        current,
        Style::new()
            .fg(theme.surface.backdrop)
            .bg(theme.border_active)
            .bold(),
    );
    let muted = fg_only(&theme.muted)
        .fg
        .map_or(theme.border_active, |fg| fg.color());
    let default_pills = keycap_pills(
        ctx,
        default,
        Style::new()
            .fg(muted)
            .bg(theme.surface.element.elevate_by(0.1)),
    );
    let group = |text: &str, pills: Vec<Span>| {
        let label = Span::new(format!("{text}  ")).style(fg_only(&theme.muted));
        Text::from_spans(std::iter::once(label).chain(pills)).width(Length::Auto)
    };
    // Side by side at opposite edges while both fit; the flow wraps Default under Current when
    // several bindings make the row too wide.
    Flow::new()
        .justify(Justify::SpaceBetween)
        .gap(2)
        .row_gap(0)
        .padding((0, 1))
        .height(Length::Auto)
        .child(group("Current", current_pills))
        .child(group("Default", default_pills))
        .into()
}

/// Width of the Change keybinding and Change modifier cards.
const KEYBINDING_CARD_WIDTH: u16 = 52;

/// Keys drawn as keycaps, capped with the workbar tab style so they read like the tab strip.
fn keycap_pills(ctx: &Context<AppRoot>, keys: &str, style: Style) -> Vec<Span> {
    /// Columns of pill fill on each side of the key, inside the caps.
    const PILL_PADDING: usize = 3;
    let pad = " ".repeat(PILL_PADDING);
    let caps = ctx
        .state
        .config
        .effective_cap_style(ctx.state.config.pane.workbar_tab_style)
        .glyphs();
    let behind = ctx.state.theme.surface.element;
    let cap_style = Style::new()
        .fg(style.bg.map_or(behind, |bg| bg.color()))
        .bg(behind)
        .contrast_policy(ContrastPolicy::Off);
    let mut spans = Vec::new();
    for (index, key) in keys.split(" / ").enumerate() {
        if index > 0 {
            spans.push(Span::new(" "));
        }
        let label = Span::new(format!("{pad}{key}{pad}")).style(style);
        match caps {
            Some((left, right)) => {
                spans.push(Span::new(left).style(cap_style));
                spans.push(label);
                spans.push(Span::new(right).style(cap_style));
            }
            None => spans.push(label),
        }
    }
    spans
}

/// The rule between a card's heading and its input: structure, not content, so it stays quiet.
fn keybinding_card_divider(theme: &Theme) -> Element {
    Divider::horizontal()
        .join_frame(false)
        .style(fg_only(&theme.border))
        .into()
}

/// The invisible focus target that owns every key while a capture or modifier card is open.
fn keybinding_card_keys(ctx: &Context<AppRoot>) -> Element {
    let stage = ctx
        .state
        .keybindings
        .as_ref()
        .map(|editor| editor.stage.clone())
        .unwrap_or_default();
    Element::from(
        KeyCapture::new().tab_stop(false).on_key(
            ctx.link()
                .key_handler(move |key| crate::update::keybindings::capture_key_msg(&stage, key)),
        ),
    )
    .key(keybinding_capture_key())
}

fn keybinding_field_body(capture: Element, leading: Option<Element>, center: Element) -> Element {
    let mut overlay = HStack::new()
        .width(Length::Auto)
        .padding((0, 1))
        .child(capture);
    if let Some(leading) = leading {
        overlay = overlay.child(leading);
    }
    ZStack::new()
        .passthrough(true)
        .child(
            HStack::new()
                .padding((0, 1))
                .justify(Justify::Center)
                .child(center),
        )
        .child(overlay)
        .into()
}

fn keybinding_capture_field(
    ctx: &Context<AppRoot>,
    candidate_label: &str,
    mode: KeybindingCaptureMode,
) -> Element {
    let theme = &ctx.state.theme;
    let recording = mode == KeybindingCaptureMode::Recording;
    let leading = recording.then(|| {
        Text::new("● REC")
            .style(Style::new().fg(theme.status.error).bold())
            .into()
    });
    let label = Text::new(candidate_label.to_string()).style(if recording {
        fg_only(&theme.muted)
    } else {
        fg_only(&theme.primary).bold()
    });
    let frame = Frame::new()
        .border(true)
        .border_style(overlay_border_style(ctx))
        .style(Style::new().bg(theme.surface.element))
        .height(Length::Px(3))
        .padding(0)
        .child(keybinding_field_body(
            keybinding_card_keys(ctx),
            leading,
            label.into(),
        ));
    // Listening is the one state where keys are consumed, so it is marked like a recording light.
    if mode == KeybindingCaptureMode::Recording {
        let recording = Style::new()
            .fg(theme.status.error)
            .bg(theme.surface.element);
        frame.style(recording).focus_style(recording).into()
    } else {
        frame.into()
    }
}

/// Who the candidate collides with. A command change names the other commands, adding that they
/// are config-only when the editor cannot take their keys; a scheme change names colliding pairs.
fn keybinding_conflict_details(
    theme: &Theme,
    conflicts: &[crate::state::KeybindingConflict],
    lead: &str,
    suffix: &str,
) -> Element {
    let details = VStack::new().height(Length::Auto);
    if conflicts.is_empty() {
        return details.into();
    }
    let labels = conflicts
        .iter()
        .map(|conflict| conflict.label.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    // Wrapped, not truncated: every colliding command has to stay readable.
    details
        .child(
            HStack::new().height(Length::Auto).padding((0, 1)).child(
                Text::from_spans([
                    Span::new("⚠ ").style(fg_only(&theme.accent).bold()),
                    Span::new(lead.to_string()).style(fg_only(&theme.primary)),
                    Span::new(labels).style(fg_only(&theme.primary).bold()),
                    Span::new(suffix.to_string()).style(fg_only(&theme.muted)),
                ])
                .width(Length::Flex(1))
                .overflow(Overflow::Wrap),
            ),
        )
        .into()
}

/// The offer to re-express literal bindings that spell the old prefix or Mod. Off by default.
fn keybinding_conversion_details(
    theme: &Theme,
    conversion: Option<crate::state::LiteralConversion>,
    previous: &str,
    scheme: &str,
) -> Element {
    let Some(conversion) = conversion else {
        return VStack::new().height(Length::Auto).into();
    };
    let count = conversion.count;
    let noun = if count == 1 { "binding" } else { "bindings" };
    let outcome = if conversion.enabled {
        format!(" will follow the new {scheme}")
    } else {
        format!(" stay on {previous}")
    };
    HStack::new()
        .height(Length::Px(1))
        .padding((0, 1))
        .child(
            Text::from_spans([
                Span::new(format!("{count} fixed {noun}")).style(fg_only(&theme.primary).bold()),
                Span::new(outcome).style(fg_only(&theme.muted)),
            ])
            .overflow(Overflow::Ellipsis),
        )
        .into()
}

/// The conversion toggle, only when a conversion is offered, so an absent hint leaves no gap.
fn with_conversion_hint(
    row: Flow,
    theme: &Theme,
    conversion: Option<crate::state::LiteralConversion>,
) -> Flow {
    match conversion {
        Some(conversion) if conversion.enabled => row.child(hint_pill(theme, "keep fixed", "tab")),
        Some(_) => row.child(hint_pill(theme, "convert", "tab")),
        None => row,
    }
}

fn keybinding_change_hints(theme: &Theme, change: &KeybindingChangeView) -> Element {
    let row = hint_row();
    let row = match change.mode {
        KeybindingCaptureMode::Conflict if change.replaceable => {
            row.child(hint_pill(theme, "replace", "enter"))
        }
        KeybindingCaptureMode::Review => with_conversion_hint(
            row.child(hint_pill(theme, "save", "enter")),
            theme,
            change.conversion,
        ),
        _ => row,
    };
    let esc = if change.mode == KeybindingCaptureMode::Recording {
        "cancel"
    } else {
        "record again"
    };
    row.child(hint_pill(theme, esc, "esc")).into()
}

struct ChangeHeading {
    title: &'static str,
    label: String,
    current: String,
    default: String,
    /// What is being changed, as the prompt names it: `keybinding` or `prefix`.
    noun: &'static str,
}

/// The line above the field, following the card's state so it always says what to do next.
fn change_prompt(change: &KeybindingChangeView, noun: &str) -> String {
    match change.mode {
        KeybindingCaptureMode::Recording => format!("Press new {noun}…"),
        KeybindingCaptureMode::Review => format!("New {noun}"),
        KeybindingCaptureMode::Conflict if change.replaceable => {
            format!("Replace existing {noun}?")
        }
        KeybindingCaptureMode::Conflict => {
            let mut noun = noun.to_string();
            noun[..1].make_ascii_uppercase();
            format!("{noun} unavailable")
        }
    }
}

fn change_heading(
    ctx: &Context<AppRoot>,
    target: &crate::state::KeybindingTarget,
) -> ChangeHeading {
    use crate::view::keys_display::format_keys;
    match target {
        crate::state::KeybindingTarget::Prefix => ChangeHeading {
            title: "Change prefix",
            label: "Prefix · then key".to_string(),
            current: ctx.state.config.input.prefix.label(),
            default: crate::config::InputConfig::default().prefix.label(),
            noun: "prefix",
        },
        crate::state::KeybindingTarget::Action(id) => {
            let command = crate::commands::editable_commands(ctx)
                .into_iter()
                .find(|command| &command.config_id == id);
            let hint = |hint: Option<&str>| {
                hint.filter(|hint| !hint.is_empty())
                    .map(format_keys)
                    .unwrap_or_else(|| "—".to_string())
            };
            ChangeHeading {
                title: "Change keybinding",
                label: command
                    .as_ref()
                    .map_or_else(|| id.clone(), |command| command.label.clone()),
                current: hint(
                    command
                        .as_ref()
                        .map(|command| command.current_hint.as_str()),
                ),
                default: hint(
                    command
                        .as_ref()
                        .map(|command| command.default_hint.as_str()),
                ),
                noun: "keybinding",
            }
        }
    }
}

fn keybinding_change_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(change) = keybinding_change_view(ctx) else {
        return Text::new("").into();
    };
    let heading = change_heading(ctx, &change.target);
    let candidate_label = change
        .candidate
        .as_ref()
        .map(KeyBinding::label)
        .unwrap_or_else(|| crate::view::keys_display::format_held_modifiers(change.held_modifiers));
    let (lead, suffix) = match (&change.target, change.replaceable) {
        (crate::state::KeybindingTarget::Prefix, _) => ("Collides: ", ""),
        (_, true) => ("Already bound to ", ""),
        (_, false)
            if change
                .conflicts
                .iter()
                .all(crate::state::KeybindingConflict::is_prefix) =>
        {
            ("Already bound to ", "")
        }
        (_, false) => ("Already bound to ", " · config-only"),
    };
    let theme = &ctx.state.theme;
    let body =
        VStack::new()
            .height(Length::Auto)
            .padding((1, 0, 0, 0))
            .child(
                HStack::new()
                    .height(Length::Px(1))
                    .padding((0, 1))
                    .child(Text::new(heading.label.clone()).style(fg_only(&theme.primary).bold())),
            )
            .child(keybinding_current_defaults(
                ctx,
                &heading.current,
                &heading.default,
            ))
            .child(keybinding_card_divider(theme))
            .child(HStack::new().height(Length::Px(1)).padding((0, 1)).child(
                Text::new(change_prompt(&change, heading.noun)).style(fg_only(&theme.primary)),
            ))
            .child(
                HStack::new()
                    .height(Length::Px(3))
                    .padding((0, 1))
                    .child(keybinding_capture_field(ctx, &candidate_label, change.mode)),
            )
            .child(keybinding_conflict_details(
                theme,
                &change.conflicts,
                lead,
                suffix,
            ))
            .child(keybinding_conversion_details(
                theme,
                change.conversion,
                &heading.current,
                "prefix",
            ))
            .child(keybinding_change_hints(theme, &change));
    nested_action_palette_modal(ctx, heading.title, HELP_MAX_HEIGHT_PERCENT)
        .width(Length::Px(KEYBINDING_CARD_WIDTH))
        .backdrop_style(Style::new().tint_by(theme.surface.backdrop, BACKDROP_RECESSION))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::KeybindingCancelCapture))
        .child(body)
        .into()
}

/// Alt, Super, or Off, chosen with the arrows rather than recorded: Mod is a choice between known
/// values, not a key.
fn keybinding_modifier_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(crate::state::KeybindingEditorStage::Modifier {
        choice,
        conversion,
        conflicts,
    }) = ctx
        .state
        .keybindings
        .as_ref()
        .map(|editor| editor.stage.clone())
    else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let input = &ctx.state.config.input;
    let current = crate::state::ModifierChoice::from_input(input);
    let default = crate::state::ModifierChoice::from_input(&crate::config::InputConfig::default());
    let mut options = vec![Span::new("‹  ").style(fg_only(&theme.muted))];
    for (index, option) in crate::state::ModifierChoice::ORDER.into_iter().enumerate() {
        if index > 0 {
            options.push(Span::new("   "));
        }
        let style = if option == choice {
            Style::new().fg(theme.border_active).bold()
        } else {
            fg_only(&theme.muted)
        };
        options.push(Span::new(option.label()).style(style));
    }
    options.push(Span::new("  ›").style(fg_only(&theme.muted)));
    let field = Frame::new()
        .border(true)
        .border_style(overlay_border_style(ctx))
        .style(Style::new().bg(theme.surface.element))
        .height(Length::Px(3))
        .padding(0)
        .child(keybinding_field_body(
            keybinding_card_keys(ctx),
            None,
            Text::from_spans(options).into(),
        ));
    let hints = with_conversion_hint(
        hint_row()
            .child(hint_pill(theme, "choose", "←/→"))
            .child(hint_pill(theme, "save", "enter")),
        theme,
        conversion,
    )
    .child(hint_pill(theme, "cancel", "esc"));
    let body = VStack::new()
        .height(Length::Auto)
        .padding((1, 0, 0, 0))
        .child(
            HStack::new()
                .height(Length::Px(1))
                .padding((0, 1))
                .child(Text::new("Mod · hold + key").style(fg_only(&theme.primary).bold())),
        )
        .child(keybinding_current_defaults(
            ctx,
            current.label(),
            default.label(),
        ))
        .child(keybinding_card_divider(theme))
        .child(
            HStack::new().height(Length::Px(1)).padding((0, 1)).child(
                Text::new(if conflicts.is_empty() {
                    "Choose modifier…"
                } else {
                    "Modifier unavailable"
                })
                .style(fg_only(&theme.primary)),
            ),
        )
        .child(
            HStack::new()
                .height(Length::Px(3))
                .padding((0, 1))
                .child(field),
        )
        .child(keybinding_conflict_details(
            theme,
            &conflicts,
            "Collides: ",
            "",
        ))
        .child(keybinding_conversion_details(
            theme,
            conversion,
            input.modifier.label(),
            "modifier",
        ))
        .child(hints);
    nested_action_palette_modal(ctx, "Change modifier", HELP_MAX_HEIGHT_PERCENT)
        .width(Length::Px(KEYBINDING_CARD_WIDTH))
        .backdrop_style(Style::new().tint_by(theme.surface.backdrop, BACKDROP_RECESSION))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::KeybindingCancelCapture))
        .child(body)
        .into()
}

fn keybinding_reset_all_overlay(ctx: &Context<AppRoot>) -> Element {
    let buttons = [
        DialogButton::new(
            "Cancel",
            Msg::KeybindingConfirmResetAll(false),
            Msg::KeybindingFocusAnswer(DIALOG_REFUSE),
        ),
        DialogButton::new(
            "Reset all",
            Msg::KeybindingConfirmResetAll(true),
            Msg::KeybindingFocusAnswer(DIALOG_AFFIRM),
        ),
    ];
    dialog_overlay(
        ctx,
        DialogChrome {
            title: "Reset all keybindings?",
            detail: Some("Remove every keybinding override and restore current defaults."),
            highlight: None,
            caption: None,
            dim_behind: false,
        },
        Msg::KeybindingConfirmResetAll(false),
        &buttons,
    )
}

/// Cap and reserved band for the Keybindings list overlay. Nested cards (Change keybinding)
/// reserve two fewer rows so they sit one row below this top edge.
const HELP_MAX_HEIGHT_PERCENT: u16 = 70;
const HELP_MODAL_WIDTH: u16 = 64;

/// Height cap for the keybinding list: every row is one line, so the count is exact, and the list
/// is only capped where the modal would otherwise outgrow the viewport. Overflow shrinks this Px
/// sibling so a wrapping hint footer can keep its wrap height.
fn help_list_rows(ctx: &Context<AppRoot>, rows: usize) -> u16 {
    // Frame borders, search, divider, tabs, list gap, and a one-row footer reserve. Extra
    // wrapped hint rows come out of the list, not this cap.
    const ABOVE_LIST: u16 = 8;

    let cap = (ctx.viewport().h * HELP_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(ABOVE_LIST)
        .max(3);
    u16::try_from(rows).unwrap_or(u16::MAX).clamp(1, cap)
}

fn help_category_priority(category: &str) -> usize {
    match category {
        "" => 0,
        "App" => 1,
        "Session" => 2,
        "Collaboration" => 3,
        "Panes" => 4,
        "Focus" => 5,
        "Workspace" => 6,
        "Workspaces" => 7,
        "Copy mode · DIRECT" => 8,
        "Profile" => 9,
        "Settings" => 10,
        "Mouse" => 11,
        "Sidebar" => 12,
        "Sidebar focused · DIRECT" => 13,
        "Keybindings open · DIRECT" => 14,
        "Custom" => usize::MAX,
        _ => 15,
    }
}

fn help_section_item(title: &str, theme: &Theme) -> ListItem {
    ListItem::from_spans([Span::new(title.to_uppercase()).style(fg_only(&theme.accent).bold())])
        .role(ListItemRole::Header)
        .rule(fg_only(&theme.muted))
}

/// Every binding row is selectable, including ones the editor cannot persist: those simply leave
/// the change, unbind, and reset actions disabled.
fn editable_help_row(row: &HelpRow, theme: &Theme) -> ListItem {
    ListItem::from_spans(help_key_spans(row, theme))
        .description_spans([Span::new(row.label.clone()).style(fg_only(&theme.primary))])
        .primary_truncate_description_first(true)
}

fn help_key_spans(row: &HelpRow, theme: &Theme) -> Vec<Span> {
    let (current, current_style) = if row.keys.is_empty() {
        ("—".to_string(), fg_only(&theme.muted))
    } else {
        (
            crate::view::keys_display::format_keys(&row.keys),
            Style::new().fg(theme.border_active).bold(),
        )
    };
    let mut spans = vec![Span::new(current).style(current_style)];
    if row.overridden {
        let default = if row.default_keys.is_empty() {
            "—".to_string()
        } else {
            crate::view::keys_display::format_keys(&row.default_keys)
        };
        spans.push(Span::new(" ← ").style(fg_only(&theme.muted)));
        spans.push(Span::new(default).style(fg_only(&theme.muted)));
    }
    spans
}

#[cfg(test)]
mod palette_alias_tests {
    use super::super::commands::{command_entries_with_groups, command_palette_aliases};
    use super::{
        HelpRow, filtered_help_groups, help_category_priority, scheme_rows,
        settings_palette_aliases,
    };
    use crate::state::{HelpTab, SettingsAction};
    use tui_lipan::prelude::SearchEntry;

    /// `settings_palette_aliases` always appends the group name, so a row with no aliases of its
    /// own still returns a one-element list. Assert on the aliases *besides* the group, or the
    /// check passes for every row whether or not anyone wrote one.
    #[test]
    fn every_settings_action_has_search_aliases() {
        for action in SettingsAction::all().iter().copied() {
            let group = "Test group";
            let aliases = settings_palette_aliases(group, action);
            let own: Vec<_> = aliases
                .iter()
                .filter(|alias| alias.as_ref() != group)
                .collect();
            assert!(
                !own.is_empty(),
                "{action:?} is searchable only by its group name; give it aliases of its own"
            );
        }
    }

    #[test]
    fn extensions_command_keeps_plugin_aliases() {
        let aliases = command_palette_aliases("extensions");
        for term in ["plugins", "addons", "extension manager"] {
            assert!(
                aliases.iter().any(|alias| alias.as_ref() == term),
                "the Extensions command is unreachable by `{term}`"
            );
        }
    }

    /// Every row must also be reachable from the Commands palette, where `Settings` is the only
    /// entry standing in for all of them - typing what you want to change has to find the door.
    #[test]
    fn settings_command_aliases_cover_every_group() {
        let aliases = command_palette_aliases("settings");
        for group in [
            "theme",
            "titlebar",
            "workbar",
            "sidebar",
            "which key",
            "alerts",
            "notifications",
            "sounds",
            "session startup",
        ] {
            assert!(
                aliases.iter().any(|alias| alias.as_ref() == group),
                "the Settings command is unreachable by `{group}`"
            );
        }
    }

    #[test]
    fn settings_command_aliases_cover_nested_controls() {
        let aliases = command_palette_aliases("settings");
        for term in [
            "padding",
            "powerline",
            "workbar badge",
            "workbar tab",
            "workbar style",
            "titlebar style",
            "focused titlebar",
            "nerd icons",
            "floating border",
            "scratchpad border",
            "fullscreen border",
            "picker border",
            "picker tab",
            "picker selection",
            "sidebar gap",
            "sidebar background",
            "sidebar tab",
            "workbar background",
        ] {
            assert!(
                aliases.iter().any(|alias| alias.as_ref() == term),
                "missing settings alias: {term}"
            );
        }
    }

    #[test]
    fn toggle_sidebar_has_sidebar_alias() {
        let aliases = command_palette_aliases("toggle-sidebar");
        assert!(
            aliases.iter().any(|alias| alias.as_ref() == "sidebar"),
            "toggle-sidebar must keep an exact sidebar alias for Hybrid ranking"
        );
        for id in [
            "toggle-sidebar-split",
            "focus-sidebar",
            "sidebar-next-tab",
            "sidebar-prev-tab",
        ] {
            assert!(
                command_palette_aliases(id).is_empty(),
                "{id} needs no redundant sidebar alias"
            );
        }
    }

    #[test]
    fn command_groups_have_exactly_one_spacer_between_them() {
        let entries = command_entries_with_groups([
            ("Panes", vec![SearchEntry::item("Pane", 1)]),
            ("Workspace", vec![SearchEntry::item("Workspace", 2)]),
            ("App", vec![SearchEntry::item("App", 3)]),
        ]);
        assert!(matches!(entries[0], SearchEntry::Header(_)));
        assert!(matches!(entries[1], SearchEntry::Item(_)));
        assert!(matches!(entries[2], SearchEntry::Spacer));
        assert!(matches!(entries[3], SearchEntry::Header(_)));
        assert!(matches!(entries[4], SearchEntry::Item(_)));
        assert!(matches!(entries[5], SearchEntry::Spacer));
        assert!(matches!(entries[6], SearchEntry::Header(_)));
        assert!(matches!(entries[7], SearchEntry::Item(_)));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, SearchEntry::Spacer))
                .count(),
            2
        );
    }

    #[test]
    fn help_categories_put_custom_last() {
        let categories = [
            "Settings",
            "Workspace",
            "Session",
            "Collaboration",
            "App",
            "Panes",
            "Profile",
            "Sidebar",
            "Custom",
            "Other",
        ];
        let mut sorted = categories;
        sorted.sort_by_key(|category| help_category_priority(category));
        assert_eq!(
            sorted,
            [
                "App",
                "Session",
                "Collaboration",
                "Panes",
                "Workspace",
                "Profile",
                "Settings",
                "Sidebar",
                "Other",
                "Custom",
            ]
        );
    }

    #[test]
    fn unbound_tab_keeps_empty_keys_out_of_global() {
        let rows = vec![
            HelpRow::unbound("App", "Edit scrollback"),
            HelpRow::unbound("App", "Open config file"),
            HelpRow::unbound("Workspace", "Kill workspace"),
            HelpRow::global("Panes", "Enter", "New pane"),
            HelpRow::direct("Copy mode · DIRECT", "Esc / q", "Exit copy mode", "direct"),
        ];
        let unbound = filtered_help_groups(rows.clone(), HelpTab::Unbound, "");
        let global = filtered_help_groups(rows.clone(), HelpTab::Global, "");
        let modes = filtered_help_groups(rows, HelpTab::Modes, "");
        assert!(
            unbound
                .iter()
                .flat_map(|(_, rows)| rows)
                .all(|row| row.keys.is_empty())
        );
        assert!(
            global
                .iter()
                .flat_map(|(_, rows)| rows)
                .all(|row| !row.keys.is_empty() && row.kind == super::HelpKind::Global)
        );
        assert!(
            modes
                .iter()
                .flat_map(|(_, rows)| rows)
                .all(|row| row.kind == super::HelpKind::Direct)
        );
    }

    #[test]
    fn scrollback_filter_finds_an_unbound_row() {
        let groups = filtered_help_groups(
            [HelpRow::unbound("App", "Edit scrollback")],
            HelpTab::Unbound,
            "SCROLLBACK",
        );
        assert_eq!(groups[0].1[0].label, "Edit scrollback");
    }

    #[test]
    fn search_matches_keys_groups_and_mode_names() {
        let rows = vec![
            HelpRow::global("Panes", "Shift+Enter", "New floating pane"),
            HelpRow::direct(
                "Sidebar focused · DIRECT",
                "Ctrl+Shift+←",
                "Reorder the active tab",
                "direct sidebar focused",
            ),
            HelpRow::unbound("App", "Open config file"),
        ];
        let floating = filtered_help_groups(rows.clone(), HelpTab::Global, "shift enter");
        assert_eq!(floating[0].1[0].label, "New floating pane");
        let sidebar = filtered_help_groups(rows.clone(), HelpTab::Modes, "ctrl shift left");
        assert_eq!(sidebar[0].1[0].label, "Reorder the active tab");
        let unbound = filtered_help_groups(rows, HelpTab::Unbound, "unbound");
        assert_eq!(unbound[0].1[0].label, "Open config file");
    }

    #[test]
    fn help_tab_defaults_to_global() {
        assert_eq!(HelpTab::default(), HelpTab::Global);
    }

    #[test]
    fn scheme_rows_lead_global_and_all_without_a_group() {
        let rows = scheme_rows(&crate::config::InputConfig::default());
        assert_eq!(rows[0].category, "");
        assert_eq!(rows[0].keys, "Ctrl+A");
        assert_eq!(rows[0].label, "Prefix · then key");
        assert_eq!(rows[1].label, "Mod · hold + key");
        assert_eq!(rows[1].keys, "Alt");
        let global = filtered_help_groups(rows.clone(), HelpTab::Global, "");
        assert_eq!(global[0].0, "");
        assert_eq!(global[0].1.len(), 2);
        assert!(filtered_help_groups(rows.clone(), HelpTab::Modes, "").is_empty());
        assert!(filtered_help_groups(rows.clone(), HelpTab::Unbound, "").is_empty());
        assert_eq!(filtered_help_groups(rows, HelpTab::All, "")[0].0, "");
    }

    /// The Mod row is how the layer is turned back on, so it stays listed while off.
    #[test]
    fn scheme_rows_keep_mod_reading_off_when_modifier_shortcuts_are_off() {
        let input = crate::config::InputConfig {
            modifier_shortcuts: false,
            ..crate::config::InputConfig::default()
        };
        let rows = scheme_rows(&input);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Prefix · then key");
        assert_eq!(rows[1].label, "Mod · hold + key");
        assert_eq!(rows[1].keys, "Off");
        assert_eq!(super::RowEdit::of(&rows[0]), super::RowEdit::Prefix);
        assert_eq!(super::RowEdit::of(&rows[1]), super::RowEdit::Modifier);
    }
}
