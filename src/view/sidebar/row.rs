use tui_lipan::prelude::*;

use crate::HyprmuxApp;

/// The row shape shared by every sidebar tab: an accent marker gutter that fills for the current
/// row, an optional glyph column, then a title over an optional dimmed detail line. One builder is
/// what keeps user-defined tabs reading as the same list as the built-in ones rather than as bare
/// text pinned to column zero.
pub(super) struct Row {
    marked: bool,
    indent: bool,
    glyph: Option<Element>,
    title: String,
    title_style: Style,
    detail: Vec<(String, Style)>,
}

impl Row {
    pub(super) fn new(title: impl Into<String>) -> Self {
        Self {
            marked: false,
            indent: false,
            glyph: None,
            title: title.into(),
            title_style: Style::default(),
            detail: Vec::new(),
        }
    }

    /// Fills the gutter marker: the row is the focused pane, the current session, and so on.
    pub(super) fn marked(mut self, marked: bool) -> Self {
        self.marked = marked;
        self
    }

    /// Nests the row one cell under a section header, moving the glyph into the header's label
    /// column so the section reads as a tree.
    pub(super) fn indent(mut self, indent: bool) -> Self {
        self.indent = indent;
        self
    }

    pub(super) fn glyph(mut self, glyph: impl Into<Element>) -> Self {
        self.glyph = Some(glyph.into());
        self
    }

    pub(super) fn title_style(mut self, style: Style) -> Self {
        self.title_style = style;
        self
    }

    pub(super) fn detail(mut self, text: impl Into<String>, style: Style) -> Self {
        self.detail.push((text.into(), style));
        self
    }

    pub(super) fn build(self, ctx: &Context<HyprmuxApp>) -> Element {
        let lines = if self.detail.is_empty() { 1 } else { 2 };
        let marker = if self.marked { "▎" } else { " " };
        let gutter = (0..lines).fold(
            VStack::new()
                .gap(0)
                .width(Length::Auto)
                .height(Length::Px(lines)),
            |gutter, _| {
                gutter.child(
                    Text::new(marker)
                        .height(Length::Px(1))
                        .style(super::super::fg_only(&ctx.state.theme.accent)),
                )
            },
        );

        // The glyph column carries its own leading cell, so a glyph or indent row butts up against
        // the gutter; a plain row needs the separating space from the outer stack instead.
        let leading = self.glyph.is_some() || self.indent;
        let mut cells = HStack::new().gap(1).height(Length::Px(lines));
        if self.indent {
            cells = cells.child(Text::new(" "));
        }
        if let Some(glyph) = self.glyph {
            cells = cells.child(glyph);
        }

        let mut text = VStack::new()
            .gap(0)
            .child(Text::new(self.title).style(self.title_style));
        if !self.detail.is_empty() {
            text = text.child(self.detail.into_iter().fold(
                HStack::new().gap(1).height(Length::Px(1)),
                |line, (value, style)| line.child(Text::new(value).style(style)),
            ));
        }

        HStack::new()
            .gap(u16::from(!leading))
            .height(Length::Px(lines))
            .style(if self.marked {
                Style::new().bg(ctx.state.theme.surface.element.elevate(0.04))
            } else {
                Style::default()
            })
            .child(gutter)
            .child(cells.child(text))
            .into()
    }
}

/// One-line section header, aligned with the glyph column so headed rows read as a tree.
pub(super) fn header(ctx: &Context<HyprmuxApp>, label: impl Into<String>, muted: bool) -> Element {
    let style = if muted {
        super::super::fg_only(&ctx.state.theme.muted).bold()
    } else {
        super::super::fg_only(&ctx.state.theme.accent).bold()
    };
    Text::new(format!(" {}", label.into()))
        .style(style)
        .height(Length::Px(1))
        .into()
}

/// Shortens a value to fit a sidebar detail line, which is narrow and never worth wrapping.
pub(super) fn truncate(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut short = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    short.push('…');
    short
}

/// The muted "nothing here" body a tab shows in place of rows.
pub(super) fn empty(ctx: &Context<HyprmuxApp>, text: &str) -> Element {
    VStack::new()
        .padding((0, 0, 0, 1))
        .child(Text::new(text.to_string()).style(super::super::fg_only(&ctx.state.theme.muted)))
        .into()
}
