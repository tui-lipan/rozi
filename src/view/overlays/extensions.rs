const EXTENSIONS_WIDTH: u16 = 72;
const EXTENSION_DETAIL_WIDTH: u16 = 76;

pub(crate) fn extensions_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(state) = ctx.state.extensions.as_ref() else {
        return Text::new("").into();
    };
    let entries = search_entries_with_groups([
        extension_group("Active", &state.entries, |status| {
            status == crate::config::ExtensionStatus::Loaded
        }),
        extension_group("Disabled", &state.entries, |status| {
            status == crate::config::ExtensionStatus::Disabled
        }),
        extension_group("Problems", &state.entries, |status| {
            !matches!(
                status,
                crate::config::ExtensionStatus::Loaded
                    | crate::config::ExtensionStatus::Disabled
            )
        }),
    ]);
    let selected = state.entries.get(state.selected);
    let toggle = selected.is_some_and(|entry| {
        matches!(
            entry.status,
            crate::config::ExtensionStatus::Loaded
                | crate::config::ExtensionStatus::Disabled
        )
    });
    let manifest =
        selected.is_some_and(|entry| state.manifest_entries.contains(entry.path.as_str()));
    let removable =
        selected.is_some_and(|entry| state.removable_entries.contains(entry.path.as_str()));
    let armed = selected
        .filter(|entry| state.pending_remove.as_deref() == Some(entry.path.as_str()))
        .map(|_| state.selected);
    let actions = vec![
        OverlayAction::new(
            "enter",
            if selected.is_some_and(|entry| {
                entry.status == crate::config::ExtensionStatus::Disabled
            }) {
                "enable"
            } else {
                "disable"
            },
            Msg::ExtensionsToggleSelected,
            toggle,
        )
        .hint_only(),
        OverlayAction::new("ctrl-d", "details", Msg::ExtensionsOpenDetail, selected.is_some()),
        OverlayAction::new("ctrl-r", "reload", Msg::ExtensionsReload, true),
        OverlayAction::new("ctrl-o", "open manifest", Msg::ExtensionsOpenManifest, manifest),
        OverlayAction::new("ctrl-y", "copy report", Msg::ExtensionsCopyReport, selected.is_some()),
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
    let selected_index = entries
        .iter()
        .filter_map(|entry| match entry {
            SearchEntry::Item(item) => Some(item.value),
            _ => None,
        })
        .position(|entry_index| entry_index == state.selected);

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
        .empty_text("No extensions installed")
        .initial_query(state.restore_query.clone())
        .preserve_groups(true)
        .selected(selected_index)
        .render_item(Arc::new(move |item: &SearchItem<usize>, _highlight| {
            let entry = &rows[item.value];
            let problem = !matches!(
                entry.status,
                crate::config::ExtensionStatus::Loaded
                    | crate::config::ExtensionStatus::Disabled
            );
            let label_style = if problem {
                muted_style
            } else {
                item_style
            };
            let description_style = if problem {
                muted_style
            } else {
                item_style
            };
            Some(picker_row(
                [Span::new(item.label.as_ref()).style(label_style)],
                fit_description(
                    item.label.as_ref(),
                    &extension_description(entry),
                    EXTENSIONS_WIDTH,
                ),
                description_style,
            ))
        }))
        .on_query_change(
            ctx.link()
                .callback(|query: Arc<str>| Msg::ExtensionsQueryChanged(query.to_string())),
        )
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::ExtensionsSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|_: SearchEvent<usize>| Msg::ExtensionsToggleSelected),
        )
        .render(ctx)
}

fn extension_group(
    title: &'static str,
    entries: &[crate::config::ExtensionInfo],
    include: impl Fn(crate::config::ExtensionStatus) -> bool,
) -> (&'static str, Vec<SearchEntry<usize>>) {
    let rows = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| include(entry.status))
        .map(|(index, entry)| {
            SearchEntry::item(entry.display_name().to_string(), index).description(
                picker_description(fit_description(
                    entry.display_name(),
                    &extension_description(entry),
                    EXTENSIONS_WIDTH,
                )),
            )
        })
        .collect();
    (title, rows)
}

fn extension_description(entry: &crate::config::ExtensionInfo) -> String {
    match entry.version.as_deref() {
        Some(version) => format!("{version} · {}", entry.status_detail()),
        None => entry.status_detail(),
    }
}

pub(crate) fn extension_detail_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some((state, detail)) = ctx
        .state
        .extensions
        .as_ref()
        .and_then(|state| state.detail.as_ref().map(|detail| (state, detail)))
    else {
        return Text::new("").into();
    };
    let Some(entry) = state
        .entries
        .iter()
        .find(|entry| entry.path == detail.path)
    else {
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
    let document = DocumentView::new(crate::config::report_text(&detail.sections))
        .formatter(ExtensionReportFormatter::new(
            detail.sections.clone(),
            &ctx.state.theme,
        ))
        .wrap(true)
        .line_numbers(false)
        .border(false)
        .height(Length::Auto)
        // ScrollView currently reserves one content column for a standalone scrollbar even when
        // its renderer mounts the integrated scrollbar on the ancestor frame. Left-only padding
        // keeps the visible inset balanced until tui-lipan uses the same mode in both passes.
        .padding((0, 0, 0, 1))
        .scrollbar(false)
        .focusable(true)
        .tab_stop(false)
        .style(fg_only(&ctx.state.theme.primary))
        .focus_content_style(fg_only(&ctx.state.theme.primary))
        .on_key(overlay_interceptor(ctx, &actions))
        .key(extension_detail_key());
    let report = ScrollView::new()
        .children(vec![document])
        .scrollbar(true)
        .scrollbar_config(
            modal_scrollbar_config(&ctx.state.theme).variant(ScrollbarVariant::Integrated),
        )
        .scroll_keys(ScrollKeymap::DEFAULT)
        .focusable(false)
        .ambient_page_scroll(true)
        .height(Length::Flex(1));
    let content = VStack::new()
        .child(report)
        .child(overlay_hints(&ctx.state.theme, &actions));
    let title = format!("Extensions · {}", entry.display_name());

    action_palette_modal_with_width(ctx, &title, EXTENSION_DETAIL_WIDTH)
        .on_close(ctx.link().callback(|_| Msg::CloseExtensionDetail))
        .child(action_palette_frame(content))
        .into()
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
                _ => (
                    vec![Span::new(row.label.as_str()).style(tone.bold())],
                    1,
                ),
            };
            push_report_line(
                lines,
                source_line,
                0,
                label,
            );
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
