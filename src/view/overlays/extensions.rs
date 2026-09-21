use super::*;

const EXTENSIONS_WIDTH: u16 = 84;
const EXTENSION_DETAIL_WIDTH: u16 = 76;

pub(crate) fn extensions_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(state) = ctx.state.extensions.as_ref() else {
        return Text::new("").into();
    };
    // One description per row, shared by the group entries and the row renderer so the text a
    // query matches is the text the row shows.
    let descriptions: Vec<String> = state
        .entries
        .iter()
        .map(|entry| crate::ops::extensions_manager::extension_description(entry, state))
        .collect();
    let installed_ids = state
        .entries
        .iter()
        .filter_map(|entry| entry.id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    let catalog_descriptions: Vec<String> = state
        .catalog_entries
        .iter()
        .map(crate::ops::extensions_manager::catalog_description)
        .collect();
    let mut groups = Vec::new();
    groups.extend([
        extension_group("Active", state, &descriptions, |status| {
            status == crate::config::ExtensionStatus::Loaded
        }),
        extension_group("Disabled", state, &descriptions, |status| {
            status == crate::config::ExtensionStatus::Disabled
        }),
        extension_group("Problems", state, &descriptions, |status| {
            !matches!(
                status,
                crate::config::ExtensionStatus::Loaded | crate::config::ExtensionStatus::Disabled
            )
        }),
    ]);
    groups.push((
        "Discover",
        state
            .catalog_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !installed_ids.contains(entry.id.as_str()))
            .map(|(index, entry)| {
                SearchEntry::item(
                    entry.title.clone(),
                    crate::state::ExtensionPickerRow::Catalog(index),
                )
                .description(picker_description(fit_description(
                    &entry.title,
                    &catalog_descriptions[index],
                    EXTENSIONS_WIDTH,
                )))
            })
            .collect(),
    ));
    let entries = search_entries_with_groups(groups);
    let selected = state
        .catalog_selected
        .map(crate::state::ExtensionPickerRow::Catalog)
        .unwrap_or(crate::state::ExtensionPickerRow::Installed(state.selected));
    let selected_installed = state
        .catalog_selected
        .is_none()
        .then(|| state.entries.get(state.selected))
        .flatten();
    let selected_catalog = state
        .catalog_selected
        .and_then(|index| state.catalog_entries.get(index));
    let toggle = selected_installed.is_some_and(|entry| {
        matches!(
            entry.status,
            crate::config::ExtensionStatus::Loaded | crate::config::ExtensionStatus::Disabled
        )
    });
    let manifest = selected_installed
        .is_some_and(|entry| state.manifest_entries.contains(entry.path.as_str()));
    let removable = state.updating_id.is_none()
        && selected_installed
            .is_some_and(|entry| state.removable_entries.contains(entry.path.as_str()));
    let updatable = selected_installed
        .and_then(|entry| entry.id.as_deref())
        .is_some_and(|id| {
            state.installation_kinds.get(id)
                == Some(&crate::extension_installation::InstallKind::Git)
                && state.updating_id.is_none()
        });
    let armed = selected_installed
        .filter(|entry| state.pending_remove.as_deref() == Some(entry.path.as_str()))
        .map(|_| crate::state::ExtensionPickerRow::Installed(state.selected));
    let actions = vec![
        OverlayAction::new(
            "enter",
            if selected_catalog.is_some() {
                "details"
            } else if selected_installed
                .is_some_and(|entry| entry.status == crate::config::ExtensionStatus::Disabled)
            {
                "enable"
            } else {
                "disable"
            },
            Msg::ExtensionsToggleSelected,
            toggle || selected_catalog.is_some(),
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-d",
            "details",
            Msg::ExtensionsOpenDetail,
            selected_installed.is_some() || selected_catalog.is_some(),
        ),
        OverlayAction::new("ctrl-i", "install", Msg::ExtensionsOpenInstall, true),
        OverlayAction::new(
            "ctrl-u",
            if state.updating_id.is_some() {
                "updating"
            } else {
                "update"
            },
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
            if armed.is_some() {
                "confirm remove"
            } else {
                "remove"
            },
            Msg::ExtensionsRemoveSelected,
            removable,
        )
        .confirm_if(
            armed.is_some(),
            "again to remove",
            ctx.state.theme.status.error,
            true,
        ),
    ];
    let item_style = fg_only(&ctx.state.theme.primary);
    let muted_style = fg_only(&ctx.state.theme.muted);
    let rows = state.entries.clone();
    let catalog_rows = state.catalog_entries.clone();
    let catalog_descriptions_for_render = catalog_descriptions.clone();
    let updating_id = state.updating_id.clone();
    let updating_style = Style::new().fg(ctx.state.theme.status.info);
    let selected_index = entries
        .iter()
        .filter_map(|entry| match entry {
            SearchEntry::Item(item) => Some(item.value),
            _ => None,
        })
        .position(|row| row == selected);

    OverlayPalette::new(
        "Extensions",
        extensions_key(),
        Msg::CloseExtensions,
        EXTENSIONS_WIDTH,
    )
    .entries(entries)
    .actions(actions)
    .armed_row(armed)
    .placeholder("Search extensions…")
    // The status row already reports a running fetch or a failed one.
    .empty_text(if state.catalog_loading {
        ""
    } else {
        "No extensions available"
    })
    .status(catalog_status(ctx, state))
    .initial_query(state.restore_query.clone())
    .preserve_groups(true)
    .selected(selected_index)
    .render_item(Arc::new(
        move |item: &SearchItem<crate::state::ExtensionPickerRow>, _highlight| {
            let crate::state::ExtensionPickerRow::Installed(index) = item.value else {
                let crate::state::ExtensionPickerRow::Catalog(index) = item.value else {
                    unreachable!()
                };
                let entry = &catalog_rows[index];
                let incompatible = entry.incompatibility().is_some();
                return Some(picker_row(
                    [Span::new(item.label.as_ref()).style(if incompatible {
                        muted_style
                    } else {
                        item_style
                    })],
                    fit_description(
                        item.label.as_ref(),
                        &catalog_descriptions_for_render[index],
                        EXTENSIONS_WIDTH,
                    ),
                    if incompatible {
                        muted_style
                    } else {
                        item_style
                    },
                ));
            };
            let entry = &rows[index];
            let problem = !matches!(
                entry.status,
                crate::config::ExtensionStatus::Loaded | crate::config::ExtensionStatus::Disabled
            );
            let label_style = if problem { muted_style } else { item_style };
            let description_style = if problem { muted_style } else { item_style };
            let updating = entry
                .id
                .as_deref()
                .is_some_and(|id| updating_id.as_deref() == Some(id));
            let row = picker_row(
                [Span::new(item.label.as_ref()).style(label_style)],
                fit_description(item.label.as_ref(), &descriptions[index], EXTENSIONS_WIDTH),
                description_style,
            );
            Some(if updating {
                row.description(crate::ops::extensions_manager::EXTENSION_UPDATING_LABEL)
                    .description_style(description_style)
                    .description_spinner(crate::view::session_status::picker_circle_spinner(
                        updating_style,
                    ))
            } else {
                row
            })
        },
    ))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::ExtensionsQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<crate::state::ExtensionPickerRow>| {
                Msg::ExtensionsSelect(event.item.value)
            }),
    )
    .on_activate(
        ctx.link()
            .callback(|_: SearchEvent<crate::state::ExtensionPickerRow>| {
                Msg::ExtensionsToggleSelected
            }),
    )
    .render(ctx)
}

fn extension_group(
    title: &'static str,
    state: &crate::state::ExtensionsState,
    descriptions: &[String],
    include: impl Fn(crate::config::ExtensionStatus) -> bool,
) -> (
    &'static str,
    Vec<SearchEntry<crate::state::ExtensionPickerRow>>,
) {
    let rows = state
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| include(entry.status))
        .map(|(index, entry)| {
            SearchEntry::item(
                entry.display_name().to_string(),
                crate::state::ExtensionPickerRow::Installed(index),
            )
            .description(picker_description(fit_description(
                entry.display_name(),
                &descriptions[index],
                EXTENSIONS_WIDTH,
            )))
        })
        .collect();
    (title, rows)
}

/// The discovery status row: a spinner while the index is fetched, the failure when it could not
/// be. A group cannot carry either, since the palette hides a group with no selectable rows.
fn catalog_status(
    ctx: &Context<AppRoot>,
    state: &crate::state::ExtensionsState,
) -> Option<Element> {
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
    let installing = ctx.state.extension_catalog_install.as_deref();
    let actions = vec![OverlayAction::new(
        "enter",
        if installing == Some(entry.repository.as_str()) {
            "installing"
        } else {
            "install"
        },
        Msg::ExtensionsSubmitCatalogInstall,
        compatible && installing.is_none(),
    )];
    let sections =
        crate::ops::extensions_manager::catalog_report_sections(entry, detail.error.as_deref());
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
    let title = format!("Install extension · {}", entry.title);

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
            heading: fg_only(&theme.accent).bold(),
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
