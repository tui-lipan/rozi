use super::*;

const EXTENSIONS_WIDTH: u16 = 84;
const EXTENSION_DETAIL_WIDTH: u16 = 76;
const EXTENSION_PROGRESS_WIDTH: u16 = 60;

/// The Extensions manager: installed extensions and the public index, one searchable tab each.
pub(crate) fn extensions_overlay(ctx: &Context<AppRoot>) -> Element {
    use crate::state::{ExtensionPickerRow, ExtensionsTab};

    let Some(state) = ctx.state.extensions.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let searching = !state.query.text().trim().is_empty();
    let installed_groups = crate::ops::extensions_manager::installed_groups(state);
    let installed_matches: usize = installed_groups.iter().map(|(_, rows)| rows.len()).sum();
    let catalog_rows = crate::ops::extensions_manager::visible_catalog(state);
    let armed_accent = theme.status.error;
    let armed = state.pending_remove.as_deref();

    let mut items = Vec::new();
    let mut targets = Vec::new();
    let (matches, total) = match state.tab {
        ExtensionsTab::Installed => {
            for (group_index, (title, rows)) in installed_groups.iter().enumerate() {
                if group_index > 0 {
                    items.push(ListItem::spacer());
                    targets.push(None);
                }
                items.push(section_item(title, theme));
                targets.push(None);
                for index in rows {
                    let entry = &state.entries[*index];
                    items.push(if armed == Some(entry.path.as_str()) {
                        super::palette::render_pending_confirm_item(
                            entry.display_name(),
                            armed_accent,
                            "again to remove",
                            true,
                        )
                    } else {
                        installed_row(ctx, state, entry)
                    });
                    targets.push(Some(ExtensionPickerRow::Installed(*index)));
                }
            }
            (installed_matches, state.entries.len())
        }
        ExtensionsTab::Discover => {
            for index in &catalog_rows {
                items.push(catalog_row(ctx, state, &state.catalog_entries[*index]));
                targets.push(Some(ExtensionPickerRow::Catalog(*index)));
            }
            (catalog_rows.len(), state.catalog_entries.len())
        }
    };
    let current = match state.tab {
        ExtensionsTab::Installed => ExtensionPickerRow::Installed(state.selected),
        ExtensionsTab::Discover => {
            ExtensionPickerRow::Catalog(state.catalog_selected.unwrap_or(usize::MAX))
        }
    };
    let selected_index = targets.iter().position(|target| *target == Some(current));
    let selected_installed = match selected_index.and_then(|index| targets[index]) {
        Some(ExtensionPickerRow::Installed(index)) => state.entries.get(index),
        _ => None,
    };
    let selected_catalog = match selected_index.and_then(|index| targets[index]) {
        Some(ExtensionPickerRow::Catalog(index)) => state.catalog_entries.get(index),
        _ => None,
    };
    let row_armed = selected_installed.is_some_and(|entry| armed == Some(entry.path.as_str()));
    let actions = match state.tab {
        ExtensionsTab::Installed => installed_actions(ctx, state, selected_installed, row_armed),
        ExtensionsTab::Discover => vec![
            OverlayAction::new(
                "enter",
                "details",
                Msg::ExtensionsToggleSelected,
                selected_catalog.is_some(),
            ),
            OverlayAction::new(
                "ctrl-i",
                "install from source",
                Msg::ExtensionsOpenInstall,
                true,
            ),
            OverlayAction::new("ctrl-r", "refresh", Msg::ExtensionsReload, true),
        ],
    };

    let targets: Arc<[Option<ExtensionPickerRow>]> = targets.into();
    let select_targets = targets.clone();
    let list_rows = list_rows(ctx, targets.len());
    let page = isize::from(i16::try_from(list_rows.saturating_sub(1).max(1)).unwrap_or(i16::MAX));
    let keys = key_handler(ctx, state.tab, targets, selected_index, page, &actions);
    let search = Input::bound(&state.query)
        .placeholder("Search extensions…")
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
        .on_change(ctx.link().callback(Msg::ExtensionsQueryChanged))
        .key_interceptor(keys)
        .key(extensions_key());
    let tab_labels = ExtensionsTab::ORDER.map(|tab| match tab {
        ExtensionsTab::Installed => crate::ops::extensions_manager::installed_tab_label(state),
        _ => tab.label().to_string(),
    });
    let tabs = super::palette::picker_tabs(
        ctx,
        &tab_labels.each_ref().map(String::as_str),
        state.tab.index(),
        ctx.link()
            .callback(|event: TabsEvent| Msg::ExtensionsTabSelected(event.index)),
    );
    let (selection_left, selection_right) =
        crate::view::picker_selection_cap_glyphs(&ctx.state.config);
    let selection_style = picker_selection_style(theme, row_armed.then_some(armed_accent));
    let list = List::new()
        .items(items)
        .selected(selected_index)
        .border(false)
        .selection_symbol(Some(selection_left))
        .selection_symbol_right(Some(selection_right))
        .selection_symbol_style(crate::view::picker_selection_cap_style(
            theme,
            if row_armed {
                armed_accent
            } else {
                theme.border_active
            },
        ))
        .unselected_symbol(Some(""))
        .selection_full_width(true)
        .selection_style(selection_style)
        .unfocused_selection_style(selection_style)
        .item_hover_style(Style::new().bg(theme.surface.element.elevate_by(0.08)))
        .item_horizontal_padding((0, 1))
        .header_horizontal_padding((0, 1))
        .scroll_wheel(true)
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(theme))
        .empty_text(empty_text(state, searching))
        .empty_text_style(fg_only(&theme.muted))
        .height(Length::Px(list_rows))
        // Keyboard input stays in the search field; the list only takes clicks and the wheel.
        .focusable(false)
        .on_select(ctx.link().callback_opt(move |event: ListEvent| {
            let row = (*select_targets.get(event.index)?)?;
            Some(Msg::ExtensionsSelect(row))
        }))
        .on_activate(
            ctx.link()
                .callback(|_: ListEvent| Msg::ExtensionsToggleSelected),
        );
    let mut body = VStack::new()
        .height(Length::Auto)
        .child(search)
        .child(super::palette::picker_divider(theme))
        .child(tabs)
        .child(Spacer::new().height(Length::Px(1)))
        .child(list);
    if let Some(status) = catalog_status(ctx, state) {
        body = body.child(status);
    }
    body = body.child(overlay_hints(theme, &actions));
    Modal::new()
        .width(Length::Px(EXTENSIONS_WIDTH))
        // Content-sized and capped, with the top edge pinned so filtering shrinks it downward. The
        // reserve matches the install prompt's, which stacks one row below this frame.
        .height(Length::Auto)
        .max_height(Length::Percent(ACTION_PALETTE_MAX_HEIGHT_PERCENT))
        .reserve_height(Length::Percent(ACTION_PALETTE_MAX_HEIGHT_PERCENT))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(theme.surface.element))
        .on_close(ctx.link().callback(|_| Msg::CloseExtensions))
        .child(super::palette::tabbed_picker_panel(
            ctx,
            "Extensions",
            Length::Auto,
            body.into(),
        ))
        .into()
}

fn empty_text(state: &crate::state::ExtensionsState, searching: bool) -> &'static str {
    match state.tab {
        _ if searching => "No matches",
        crate::state::ExtensionsTab::Installed => "No extensions installed",
        // The status row already reports a running fetch or a failed one.
        crate::state::ExtensionsTab::Discover
            if state.catalog_loading || state.catalog_error.is_some() =>
        {
            ""
        }
        crate::state::ExtensionsTab::Discover => "No extensions available",
    }
}

fn section_item(title: &str, theme: &Theme) -> ListItem {
    ListItem::header(title).style(rozi_fg(theme).bold())
}

fn installed_row(
    ctx: &Context<AppRoot>,
    state: &crate::state::ExtensionsState,
    entry: &crate::config::ExtensionInfo,
) -> ListItem {
    let theme = &ctx.state.theme;
    let problem = !matches!(
        entry.status,
        crate::config::ExtensionStatus::Loaded | crate::config::ExtensionStatus::Disabled
    );
    let style = fg_only(if problem {
        &theme.muted
    } else {
        &theme.primary
    });
    let label = entry.display_name();
    let id = entry.id.as_deref();
    let updating = id.is_some_and(|id| state.updating_id.as_deref() == Some(id));
    let checking = id.is_some_and(|id| {
        state.update_checks.get(id) == Some(&crate::state::ExtensionUpdateCheck::Checking)
    });
    let description = crate::ops::extensions_manager::extension_description(entry, state);
    let description = fit_description(label, &description, EXTENSIONS_WIDTH);
    let row = picker_row([Span::new(label).style(style)], description.as_str(), style);
    if updating {
        row.description(crate::ops::extensions_manager::EXTENSION_UPDATING_LABEL)
            .description_style(style)
            .description_spinner(crate::view::session_status::picker_circle_spinner(
                Style::new().fg(theme.status.info),
            ))
    } else if checking {
        // The spinner's slot replaces picker_row's leading separator, keeping the row's width and
        // sitting one cell from the version.
        row.description(description).description_spinner(
            crate::view::session_status::picker_circle_spinner(fg_only(&theme.muted)),
        )
    } else {
        row
    }
}

fn catalog_row(
    ctx: &Context<AppRoot>,
    state: &crate::state::ExtensionsState,
    entry: &crate::extension_catalog::CatalogEntry,
) -> ListItem {
    let theme = &ctx.state.theme;
    let installed = crate::ops::extensions_manager::catalog_entry_installed(state, entry);
    let style = fg_only(if installed || entry.incompatibility().is_some() {
        &theme.muted
    } else {
        &theme.primary
    });
    let description = crate::ops::extensions_manager::catalog_description(entry, installed);
    picker_row(
        [Span::new(entry.title.as_str()).style(style)],
        fit_description(&entry.title, &description, EXTENSIONS_WIDTH),
        style,
    )
}

fn installed_actions(
    ctx: &Context<AppRoot>,
    state: &crate::state::ExtensionsState,
    selected: Option<&crate::config::ExtensionInfo>,
    armed: bool,
) -> Vec<OverlayAction> {
    let toggle = selected.is_some_and(|entry| {
        matches!(
            entry.status,
            crate::config::ExtensionStatus::Loaded | crate::config::ExtensionStatus::Disabled
        )
    });
    let manifest =
        selected.is_some_and(|entry| state.manifest_entries.contains(entry.path.as_str()));
    let removable = state.updating_id.is_none()
        && selected.is_some_and(|entry| state.removable_entries.contains(entry.path.as_str()));
    let selected_id = selected.and_then(|entry| entry.id.as_deref());
    let git = selected_id.is_some_and(|id| {
        state.installation_kinds.get(id) == Some(&crate::extension_installation::InstallKind::Git)
    });
    let check = selected_id.and_then(|id| state.update_checks.get(id));
    let checking = matches!(check, Some(crate::state::ExtensionUpdateCheck::Checking));
    let updatable = git && state.updating_id.is_none() && !checking;
    let update_label = if matches!(
        check,
        Some(crate::state::ExtensionUpdateCheck::Available { .. })
    ) {
        "update"
    } else {
        "check"
    };
    vec![
        OverlayAction::new(
            "enter",
            if selected
                .is_some_and(|entry| entry.status == crate::config::ExtensionStatus::Disabled)
            {
                "enable"
            } else {
                "disable"
            },
            Msg::ExtensionsToggleSelected,
            toggle,
        ),
        OverlayAction::new(
            "ctrl-d",
            "details",
            Msg::ExtensionsOpenDetail,
            selected.is_some(),
        ),
        OverlayAction::new("ctrl-i", "install", Msg::ExtensionsOpenInstall, true),
        OverlayAction::new(
            "ctrl-u",
            update_label,
            Msg::ExtensionsUpdateSelected,
            updatable,
        ),
        OverlayAction::new("ctrl-r", "reload", Msg::ExtensionsReload, true),
        OverlayAction::new(
            "ctrl-o",
            "open manifest",
            Msg::ExtensionsOpenManifest,
            manifest,
        ),
        OverlayAction::new(
            "ctrl-k",
            if armed { "confirm remove" } else { "remove" },
            Msg::ExtensionsRemoveSelected,
            removable,
        )
        .confirm_if(armed, "again to remove", ctx.state.theme.status.error, true),
    ]
}

fn list_rows(ctx: &Context<AppRoot>, rows: usize) -> u16 {
    // Frame border, search, divider, tabs, and spacer above; status, hints, and border below.
    const CHROME_ROWS: u16 = 8;
    let cap = (ctx.viewport().h * ACTION_PALETTE_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(CHROME_ROWS)
        .max(3);
    u16::try_from(rows).unwrap_or(u16::MAX).clamp(1, cap)
}

/// Keys for the search field, which keeps focus: the arrows and Page keys move the selection,
/// `Tab`, `Shift+Tab`, `Left`, and `Right` switch tabs, and the footer actions take their keys.
fn key_handler(
    ctx: &Context<AppRoot>,
    tab: crate::state::ExtensionsTab,
    targets: Arc<[Option<crate::state::ExtensionPickerRow>]>,
    selected: Option<usize>,
    page: isize,
    actions: &[OverlayAction],
) -> KeyHandler {
    let actions = actions
        .iter()
        .filter(|action| action.enabled && action.intercept)
        .map(|action| (action.key.clone(), action.msg.clone()))
        .collect::<Vec<_>>();
    let switch = move |steps: isize| Msg::ExtensionsTabSelected(tab.stepped(steps).index());
    ctx.link().key_handler(move |key| {
        let plain = !key.mods.ctrl && !key.mods.alt && !key.mods.super_key;
        let navigation = match key.code {
            KeyCode::Tab if plain && !key.mods.shift => Some(switch(1)),
            KeyCode::BackTab | KeyCode::Tab if plain => Some(switch(-1)),
            KeyCode::Left if plain && !key.mods.shift => Some(switch(-1)),
            KeyCode::Right if plain && !key.mods.shift => Some(switch(1)),
            code if plain && !key.mods.shift => {
                let step = match code {
                    KeyCode::Up => Some((-1, true)),
                    KeyCode::Down => Some((1, true)),
                    KeyCode::PageUp => Some((-page, false)),
                    KeyCode::PageDown => Some((page, false)),
                    KeyCode::Home => Some((isize::MIN, false)),
                    KeyCode::End => Some((isize::MAX, false)),
                    _ => None,
                };
                step.and_then(|(delta, wrap)| {
                    let index =
                        super::palette::stepped_selectable_row(&targets, selected, delta, wrap)?;
                    targets[index].map(Msg::ExtensionsSelect)
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

/// The discovery status row: a spinner while the index is fetched, the failure when it could not
/// be. A group cannot carry either, since the palette hides a group with no selectable rows.
fn catalog_status(
    ctx: &Context<AppRoot>,
    state: &crate::state::ExtensionsState,
) -> Option<Element> {
    if state.tab != crate::state::ExtensionsTab::Discover {
        return None;
    }
    let theme = &ctx.state.theme;
    let listed = !state.catalog_entries.is_empty();
    let content: Element = if state.catalog_loading {
        Spinner::new()
            .spinner_style(SpinnerStyle::Dots)
            .label(if listed {
                "refreshing index"
            } else {
                "loading index"
            })
            .style(Style::new().fg(theme.status.info))
            .label_style(fg_only(&theme.muted))
            .into()
    } else {
        let error = state.catalog_error.as_deref()?;
        let label = if listed {
            "index not refreshed"
        } else {
            "discovery unavailable"
        };
        Text::new(format!("{label} · {error}"))
            .overflow(Overflow::Ellipsis)
            .style(Style::new().fg(theme.status.warning))
            .into()
    };
    Some(
        HStack::new()
            .height(Length::Px(1))
            .padding((0, 1, 0, 1))
            .child(content)
            .into(),
    )
}

/// Shown in place of the report or prompt that started an installation, until it finishes or the
/// user hides it with `Esc`.
pub(crate) fn extension_install_progress_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(install) = crate::ops::extensions_manager::visible_install(&ctx.state) else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let actions = vec![OverlayAction::new(
        "esc",
        "hide",
        Msg::ExtensionsHideInstall,
        true,
    )];
    let mut body = VStack::new()
        .height(Length::Auto)
        .padding((1, 1, 0, 1))
        .child(
            HStack::new()
                .height(Length::Px(1))
                .gap(1)
                .child(
                    Spinner::new()
                        .spinner_style(SpinnerStyle::Dots)
                        .style(Style::new().fg(theme.status.info)),
                )
                .child(
                    Text::new(install.label.clone())
                        .overflow(Overflow::Ellipsis)
                        .width(Length::Flex(1))
                        .style(fg_only(&theme.primary).bold()),
                ),
        );
    if let Some(detail) = install.detail.as_deref() {
        // Indented past the spinner so it reads as belonging to the label.
        body = body.child(
            HStack::new()
                .height(Length::Px(1))
                .padding((0, 0, 0, 2))
                .child(
                    Text::new(detail.to_string())
                        .overflow(Overflow::Ellipsis)
                        .width(Length::Flex(1))
                        .style(fg_only(&theme.muted)),
                ),
        );
    }
    let content = VStack::new()
        .height(Length::Auto)
        .child(body)
        .child(Spacer::new().height(Length::Px(1)))
        .child(overlay_hints(theme, &actions));
    styled_modal(ctx, "Installing extension", EXTENSION_PROGRESS_WIDTH)
        .height(Length::Auto)
        .padding(0)
        .on_close(ctx.link().callback(|_| Msg::ExtensionsHideInstall))
        .child(content)
        .key(crate::view::widget_keys::extension_install_progress_key())
}

pub(crate) fn extension_detail_overlay(ctx: &Context<AppRoot>) -> Element {
    if ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.catalog_detail.as_ref())
        .is_some()
    {
        return catalog_extension_detail_overlay(ctx);
    }
    let Some((state, detail)) = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.detail.as_ref().map(|detail| (state, detail)))
    else {
        return Text::new("").into();
    };
    let Some(entry) = state.entries.iter().find(|entry| entry.path == detail.path) else {
        return Text::new("").into();
    };
    let actions = vec![
        OverlayAction::new("ctrl-y", "copy report", Msg::ExtensionsCopyReport, true),
        OverlayAction::new(
            "ctrl-o",
            "open manifest",
            Msg::ExtensionsOpenManifest,
            state.manifest_entries.contains(entry.path.as_str()),
        ),
        OverlayAction::new(
            "ctrl-l",
            "open homepage",
            Msg::ExtensionsOpenLink,
            entry.homepage.is_some(),
        ),
    ];
    let formatter = ExtensionReportFormatter::new(detail.sections.clone(), &ctx.state.theme);
    let document = DocumentView::new(crate::config::report_text(&detail.sections))
        .height(report_height(ctx, &formatter))
        .formatter(formatter)
        .wrap(true)
        .line_numbers(false)
        .border(false)
        .padding((0, 1, 0, 1))
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(&ctx.state.theme))
        // The report is the modal's only focusable widget, so it has to be its tab stop: an
        // overlay whose focus ring is empty swallows every key before dispatch, which is what
        // used to leave the arrows and Page keys inert over a report too long to fit.
        .focusable(true)
        .style(fg_only(&ctx.state.theme.primary))
        .focus_content_style(fg_only(&ctx.state.theme.primary))
        .on_key(overlay_interceptor(ctx, &actions))
        .key(extension_detail_key());
    let content = VStack::new()
        .child(document)
        .child(overlay_hints(&ctx.state.theme, &actions));
    let title = format!("Extensions · {}", entry.display_name());

    action_palette_modal_with_width(ctx, &title, EXTENSION_DETAIL_WIDTH)
        .on_close(ctx.link().callback(|_| Msg::CloseExtensionDetail))
        .child(content)
        .into()
}

fn catalog_extension_detail_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(detail) = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.catalog_detail.as_ref())
    else {
        return Text::new("").into();
    };
    let entry = &detail.entry;
    let compatible = entry.incompatibility().is_none();
    let installed =
        ctx.state.extensions.as_ref().is_some_and(|state| {
            crate::ops::extensions_manager::catalog_entry_installed(state, entry)
        });
    // The report is replaced by the progress modal while its own installation runs, so an install
    // already in flight here is always someone else's.
    let busy = ctx.state.extension_install.is_some();
    let actions = vec![
        OverlayAction::new(
            "enter",
            "install",
            Msg::ExtensionsSubmitCatalogInstall,
            compatible && !installed && !busy,
        ),
        OverlayAction::new("ctrl-l", "open source", Msg::ExtensionsOpenLink, true),
    ];
    let sections = crate::ops::extensions_manager::catalog_report_sections(
        entry,
        installed,
        detail.error.as_deref(),
    );
    let formatter = ExtensionReportFormatter::new(sections.clone(), &ctx.state.theme);
    let document = DocumentView::new(crate::config::report_text(&sections))
        .height(report_height(ctx, &formatter))
        .formatter(formatter)
        .wrap(true)
        .line_numbers(false)
        .border(false)
        .padding((0, 1, 0, 1))
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(&ctx.state.theme))
        .focusable(true)
        .style(fg_only(&ctx.state.theme.primary))
        .focus_content_style(fg_only(&ctx.state.theme.primary))
        .on_key(overlay_interceptor(ctx, &actions))
        .key(extension_detail_key());
    let content = VStack::new()
        .child(document)
        .child(overlay_hints(&ctx.state.theme, &actions));
    let title = if installed {
        format!("Extensions · {}", entry.title)
    } else {
        format!("Install extension · {}", entry.title)
    };

    action_palette_modal_with_width(ctx, &title, EXTENSION_DETAIL_WIDTH)
        .on_close(ctx.link().callback(|_| Msg::CloseExtensionDetail))
        .child(content)
        .into()
}

/// Height for the report body: content-sized while the report fits, capped once it does not.
///
/// `action_palette_modal_with_width` caps the modal at 65% of the viewport, and a `Length::Auto`
/// document squeezed by that cap clips its tail rather than scrolling - the hint row then
/// overdraws the last visible line. So the cap is applied here instead, from an estimate that
/// leans the safe way: the row count rounds up (word wrapping breaks earlier than this counts,
/// never later) and the budget rounds down, so a report near the boundary takes the capped branch,
/// where the document scrolls. Over-capping costs a few unused rows; under-capping costs the hints.
fn report_height(ctx: &Context<AppRoot>, formatter: &ExtensionReportFormatter) -> Length {
    // Modal border, hint row, and one row of slack.
    const CHROME_ROWS: u16 = 4;
    // Modal border, the document's own horizontal padding, and its scrollbar column.
    const TEXT_WIDTH: u16 = EXTENSION_DETAIL_WIDTH - 5;

    let cap = (ctx.viewport().h * 65 / 100)
        .saturating_sub(CHROME_ROWS)
        .max(3);
    let rows: u32 = formatter
        .document()
        .blocks
        .iter()
        .map(|block| match block {
            FormattedBlock::Lines(lines) => lines
                .iter()
                .map(|line| {
                    let budget = usize::from(TEXT_WIDTH.saturating_sub(line.indent)).max(1);
                    tui_lipan::utils::spans::line_width(&line.spans)
                        .div_ceil(budget)
                        .max(1) as u32
                })
                .sum(),
            _ => 0,
        })
        .sum();
    if rows > u32::from(cap) {
        Length::Px(cap)
    } else {
        Length::Auto
    }
}

#[derive(Clone)]
struct ExtensionReportFormatter {
    sections: Vec<crate::config::ReportSection>,
    heading: Style,
    label: Style,
    value: Style,
    muted: Style,
    success: Style,
    warning: Style,
    error: Style,
    home: Option<String>,
}

impl ExtensionReportFormatter {
    fn new(sections: Vec<crate::config::ReportSection>, theme: &Theme) -> Self {
        Self {
            sections,
            heading: rozi_fg(theme).bold(),
            label: fg_only(&theme.primary).bold(),
            value: fg_only(&theme.primary),
            muted: fg_only(&theme.muted),
            success: Style::new().fg(theme.status.success),
            warning: Style::new().fg(theme.status.warning),
            error: Style::new().fg(theme.status.error),
            home: crate::platform::paths::home_directory(),
        }
    }

    fn document(&self) -> FormattedDocument {
        let mut lines = Vec::new();
        let mut source_line = 0;
        for (section_index, section) in self.sections.iter().enumerate() {
            if section_index > 0 {
                push_report_line(&mut lines, &mut source_line, 0, vec![Span::new("")]);
            }
            push_report_line(
                &mut lines,
                &mut source_line,
                0,
                vec![Span::new(section.title).style(self.heading)],
            );
            let label_width = section
                .rows
                .iter()
                .map(|row| row.label.chars().count())
                .max()
                .unwrap_or_default();
            for row in &section.rows {
                self.push_row(&mut lines, &mut source_line, label_width, row);
            }
        }
        FormattedDocument {
            blocks: vec![FormattedBlock::Lines(lines)],
        }
    }

    fn push_row(
        &self,
        lines: &mut Vec<FormattedLine>,
        source_line: &mut usize,
        label_width: usize,
        row: &crate::config::ReportRow,
    ) {
        let tone = self.tone(row.tone);
        if row.value.contains('\n') {
            let (label, detail_indent) = match &row.kind {
                crate::config::ReportKind::Command(_) => (
                    vec![
                        Span::new("• ").style(self.heading),
                        Span::new(row.label.as_str()).style(self.label),
                    ],
                    2,
                ),
                _ => (vec![Span::new(row.label.as_str()).style(tone.bold())], 1),
            };
            push_report_line(lines, source_line, 0, label);
            for detail in row.value.lines() {
                let detail = compact_home_paths(detail, self.home.as_deref());
                let spans = match detail.split_once(": ") {
                    Some((key, value)) => vec![
                        Span::new(format!("{key}: ")).style(self.muted),
                        Span::new(value.to_string()).style(self.detail_tone(row.tone)),
                    ],
                    None => vec![Span::new(detail).style(self.detail_tone(row.tone))],
                };
                push_report_line(lines, source_line, detail_indent, spans);
            }
        } else {
            let value = compact_home_paths(&row.value, self.home.as_deref());
            push_report_line(
                lines,
                source_line,
                0,
                vec![
                    Span::new(format!("{:<label_width$}", row.label)).style(self.label),
                    Span::new("   "),
                    Span::new(value).style(tone),
                ],
            );
        }
    }

    fn tone(&self, tone: crate::config::ReportTone) -> Style {
        match tone {
            crate::config::ReportTone::Plain => self.value,
            crate::config::ReportTone::Accent => self.heading,
            crate::config::ReportTone::Success => self.success,
            crate::config::ReportTone::Warning => self.warning,
            crate::config::ReportTone::Error => self.error,
            crate::config::ReportTone::Muted => self.muted,
        }
    }

    fn detail_tone(&self, tone: crate::config::ReportTone) -> Style {
        match tone {
            crate::config::ReportTone::Warning => self.warning,
            crate::config::ReportTone::Error => self.error,
            _ => self.value,
        }
    }
}

fn compact_home_paths(value: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return value.to_string();
    };
    let mut compact = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find(home) {
        let start = cursor + relative;
        let end = start + home.len();
        let prefix_boundary = value[..start].chars().next_back().is_none_or(|previous| {
            matches!(
                previous,
                ' ' | '\t' | '\n' | '\r' | '"' | '\'' | '=' | '[' | '(' | '{' | ',' | ';' | ':'
            )
        });
        let path_boundary = value[end..]
            .chars()
            .next()
            .is_none_or(|next| matches!(next, '/' | '\\'));
        if prefix_boundary && path_boundary {
            compact.push_str(&value[cursor..start]);
            compact.push('~');
            cursor = end;
        } else {
            compact.push_str(&value[cursor..end]);
            cursor = end;
        }
    }
    compact.push_str(&value[cursor..]);
    compact
}

impl ContentFormatter for ExtensionReportFormatter {
    fn format(&self, _input: FormatInput<'_>) -> FormattedDocument {
        self.document()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn ContentFormatter> {
        Box::new(self.clone())
    }
}

fn push_report_line(
    lines: &mut Vec<FormattedLine>,
    source_line: &mut usize,
    indent: u16,
    spans: Vec<Span>,
) {
    lines.push(FormattedLine {
        spans,
        source_line: *source_line,
        indent,
        links: Vec::new(),
    });
    *source_line += 1;
}

#[cfg(test)]
mod extension_report_tests {
    use super::compact_home_paths;

    #[test]
    fn report_paths_collapse_home_prefixes_without_touching_sibling_names() {
        assert_eq!(
            compact_home_paths(
                r#"launch: ["/home/you/bin/tool"] env: ROOT=/home/you/project"#,
                Some("/home/you"),
            ),
            r#"launch: ["~/bin/tool"] env: ROOT=~/project"#
        );
        assert_eq!(
            compact_home_paths("/home/youssef/project", Some("/home/you")),
            "/home/youssef/project"
        );
        assert_eq!(
            compact_home_paths("/prefix/home/you/project", Some("/home/you")),
            "/prefix/home/you/project"
        );
    }
}
