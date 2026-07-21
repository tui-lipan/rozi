use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::SidebarTabId;
use crate::state::PaneId;

/// What activating a row does. Rows are built as a pure function of `State`, so the update side can
/// rebuild the same list and resolve an index back to one of these — which is what lets Enter and a
/// click share a single code path instead of two callbacks that can drift.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowTarget {
    /// Headers, spacers, and error rows: present in the list, never selected or activated.
    Inert,
    Pane(PaneId),
    Session(Box<crate::session::discovery::DiscoveredSession>),
    Launcher {
        config_epoch: u64,
        tab_id: SidebarTabId,
        entry_index: usize,
    },
    CommandRow {
        config_epoch: u64,
        tab_id: SidebarTabId,
        output_epoch: u64,
        line: String,
    },
}

/// What a row renders as. Items are kept unbuilt so the list can hand each one its selection state
/// at build time; headers carry their own glyphs and are finished elements already.
pub(crate) enum RowKind {
    /// Blank separator between groups.
    Spacer,
    /// Boxed alongside `Item` because `Element` is large; an unboxed variant would make every row
    /// in the vector pay for the biggest one.
    Header(Box<Element>),
    Item(Box<Row>),
}

/// One row: how it renders, and what activating it does.
pub(crate) struct SidebarRow {
    pub kind: RowKind,
    pub target: RowTarget,
}

impl SidebarRow {
    pub(super) fn header(element: impl Into<Element>) -> Self {
        Self {
            kind: RowKind::Header(Box::new(element.into())),
            target: RowTarget::Inert,
        }
    }

    pub(super) fn spacer() -> Self {
        Self {
            kind: RowKind::Spacer,
            target: RowTarget::Inert,
        }
    }

    pub(super) fn item(row: Row, target: RowTarget) -> Self {
        Self {
            kind: RowKind::Item(Box::new(row)),
            target,
        }
    }

    /// Rows the keyboard cursor is allowed to land on.
    pub(crate) fn selectable(&self) -> bool {
        !matches!(self.target, RowTarget::Inert)
    }
}

/// The row shape shared by every sidebar tab: an accent marker gutter that fills for the current
/// row, an optional glyph column, then a title over an optional dimmed detail line. One builder is
/// what keeps user-defined tabs reading as the same list as the built-in ones rather than as bare
/// text pinned to column zero.
///
/// `active` (the current pane, the attached session) and `selected` (the keyboard cursor) are
/// independent: the marker bar and the cursor highlight can both be on the same row.
pub(crate) struct Row {
    active: bool,
    glyph: Option<Element>,
    title: String,
    title_style: Style,
    badge: Option<(String, Style)>,
    detail: Vec<(String, Style)>,
}

impl Row {
    pub(super) fn new(title: impl Into<String>) -> Self {
        Self {
            active: false,
            glyph: None,
            title: title.into(),
            title_style: Style::default(),
            badge: None,
            detail: Vec::new(),
        }
    }

    /// Fills the gutter marker: the row is the focused pane, the current session, and so on.
    pub(super) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// A short marker pinned to the right edge of the title line — a workspace number, a client
    /// count. The title yields to it, so a long title truncates rather than pushing it off.
    pub(super) fn badge(mut self, text: impl Into<String>, style: Style) -> Self {
        self.badge = Some((text.into(), style));
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

    pub(super) fn build(self, ctx: &Context<HyprmuxApp>, selected: bool) -> Element {
        let theme = &ctx.state.theme;
        let lines = if self.detail.is_empty() { 1 } else { 2 };
        let marker = if self.active { "▎" } else { " " };
        // The marker repeats down the row so a two-line entry gets a full-height bar rather than a
        // tick beside its first line.
        let marker_style = super::super::fg_only(&theme.accent);
        let gutter = (0..lines).fold(
            VStack::new()
                .gap(0)
                .width(Length::Auto)
                .height(Length::Px(lines)),
            |gutter, _| gutter.child(Text::new(marker).height(Length::Px(1)).style(marker_style)),
        );

        // The glyph column carries its own leading cell, so a glyph row butts up against the
        // gutter; a plain row needs the separating space from the outer stack instead.
        let leading = self.glyph.is_some();
        let mut cells = HStack::new().gap(1).height(Length::Px(lines));
        if let Some(glyph) = self.glyph {
            cells = cells.child(glyph);
        }

        // A badge pins itself to the right edge, with the title flexing into whatever is left. The
        // title has to be the one that gives way — a workspace number pushed off the edge is worse
        // than a truncated name, since the name is usually recoverable from the row beside it.
        let title: Element = match self.badge {
            None => Text::new(self.title).style(self.title_style).into(),
            Some((badge, badge_style)) => HStack::new()
                .gap(1)
                .height(Length::Px(1))
                .child(
                    Text::new(self.title)
                        .style(self.title_style)
                        .width(Length::Flex(1)),
                )
                .child(Text::new(badge).style(badge_style))
                .into(),
        };

        // The cursor changes the background only; every span keeps the color that carries its
        // meaning, so agent status, git state, and error red stay readable underneath it.
        let mut text = VStack::new().gap(0).child(title);
        if !self.detail.is_empty() {
            text = text.child(self.detail.into_iter().fold(
                HStack::new().gap(1).height(Length::Px(1)),
                |line, (value, style)| line.child(Text::new(value).style(style)),
            ));
        }

        HStack::new()
            .gap(u16::from(!leading))
            .height(Length::Px(lines))
            .style(if selected {
                super::row_highlight(theme)
            } else if self.active {
                Style::new().bg(theme.surface.element.elevate(0.04))
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

/// Shortens a value by dropping characters from the *front*, keeping the tail. For a path, the tail
/// is the part that identifies it — clipping `~/Work/Projects/hyprmux` to `~/Work/Project…` throws
/// away the only word that distinguishes it from its neighbours.
pub(super) fn truncate_start(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut short = String::from("…");
    short.extend(value.chars().skip(count - keep));
    short
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
