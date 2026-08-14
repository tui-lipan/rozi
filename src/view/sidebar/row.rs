use tui_lipan::prelude::*;

use crate::{AppRoot, Msg};

pub(crate) use crate::state::RowTarget;

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

/// One row: how it renders, what activating it does, and what its ✕ destroys if it has one.
pub(crate) struct SidebarRow {
    pub kind: RowKind,
    pub target: RowTarget,
    pub close: Option<crate::state::SidebarClose>,
}

impl SidebarRow {
    pub(super) fn header(element: impl Into<Element>) -> Self {
        Self {
            kind: RowKind::Header(Box::new(element.into())),
            target: RowTarget::Inert,
            close: None,
        }
    }

    pub(super) fn spacer() -> Self {
        Self {
            kind: RowKind::Spacer,
            target: RowTarget::Inert,
            close: None,
        }
    }

    pub(super) fn item(row: Row, target: RowTarget) -> Self {
        Self {
            kind: RowKind::Item(Box::new(row)),
            target,
            close: None,
        }
    }

    /// Give the row a ✕: revealed on hover, destroying `close` after a confirming second click.
    pub(super) fn closable(mut self, close: crate::state::SidebarClose) -> Self {
        self.close = Some(close);
        self
    }

    /// Rows the keyboard cursor is allowed to land on.
    pub(crate) fn selectable(&self) -> bool {
        !matches!(self.target, RowTarget::Inert)
    }
}

/// How a row's ✕ should render this frame. Absent when the row has no ✕, or has one that is
/// neither hovered nor armed.
#[derive(Clone, Copy)]
pub(super) struct CloseAffordance {
    /// Sidebar panel containing the row.
    pub panel: usize,
    /// Row index, so the click resolves the same way `SidebarRowActivate` does.
    pub index: usize,
    /// Armed for the confirming second click.
    pub armed: bool,
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
    group_level: bool,
    glyph: Option<Element>,
    title: String,
    title_style: Style,
    badge: Option<Element>,
    meta: Option<Element>,
    detail: Vec<(String, Style)>,
}

impl Row {
    pub(super) fn new(title: impl Into<String>) -> Self {
        Self {
            active: false,
            group_level: false,
            glyph: None,
            title: title.into(),
            title_style: Style::default(),
            badge: None,
            meta: None,
            detail: Vec::new(),
        }
    }

    /// Fills the gutter marker: the row is the focused pane, the current session, and so on.
    pub(super) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Align this row with a section heading rather than its children.
    pub(super) fn group_level(mut self) -> Self {
        self.group_level = true;
        self
    }

    /// A short marker pinned to the right edge of the title line — a workspace number, a client
    /// count, session connection chrome. The title yields to it, so a long title truncates rather
    /// than pushing it off.
    pub(super) fn badge(mut self, badge: impl Into<Element>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// A short token riding immediately after the title, before the badge — an agent's elapsed
    /// run, say. Unlike the badge it does not pin to the right edge, so it stays attached to the
    /// name it qualifies instead of drifting away from it on a wide sidebar.
    pub(super) fn meta(mut self, text: impl Into<String>, style: Style) -> Self {
        self.meta = Some(Text::new(text.into()).style(style).into());
        self
    }

    /// Text badge helper for the common string + style case.
    pub(super) fn badge_text(mut self, text: impl Into<String>, style: Style) -> Self {
        self.badge = Some(Text::new(text.into()).style(style).into());
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

    pub(super) fn build(
        mut self,
        ctx: &Context<AppRoot>,
        selected: bool,
        close: Option<CloseAffordance>,
    ) -> Element {
        let theme = &ctx.state.theme;

        // An armed row spells the confirmation out the way the session picker's pending kill does:
        // the title strikes through, because that is the thing about to go, and the detail line
        // gives up whatever it was saying to ask for the second click. The ✕ itself stays a plain ✕
        // — it is the target, and a target that changes shape under the pointer is one you can miss.
        if close.is_some_and(|close| close.armed) {
            self.title_style = self.title_style.strikethrough();
            self.detail = vec![(
                "Click again to confirm".to_string(),
                Style::new().fg(theme.status.error),
            )];
        }

        let lines = if self.detail.is_empty() { 1 } else { 2 };
        let marker = if self.active { "▍" } else { " " };
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
        //
        // A visible ✕ takes that slot instead of the badge rather than fighting it for width. The
        // badge is ambient (what the pane runs) and comes back the moment the pointer leaves; the ✕
        // is the thing being aimed at, so in a column this narrow it gets the space.
        let trailing = match close {
            Some(close) => Some(close_affordance(ctx, close)),
            None => self.badge,
        };
        // The name keeps only the width it needs when something rides beside it, so the meta
        // token stays attached to the name rather than drifting to the far edge; the flex moves to
        // the gap between them and the badge.
        let name = Text::new(self.title).style(self.title_style);
        let title: Element = match (self.meta, trailing) {
            (None, None) => name.into(),
            (None, Some(trailing)) => HStack::new()
                .gap(1)
                .height(Length::Px(1))
                .child(name.width(Length::Flex(1)))
                .child(trailing)
                .into(),
            (Some(meta), trailing) => {
                let mut line = HStack::new()
                    .gap(1)
                    .height(Length::Px(1))
                    .child(name)
                    .child(meta);
                if let Some(trailing) = trailing {
                    line = line
                        .child(Spacer::new().width(Length::Flex(1)))
                        .child(trailing);
                }
                line.into()
            }
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
            .gap(if self.group_level {
                0
            } else {
                u16::from(!leading)
            })
            .height(Length::Px(lines))
            .style(if selected {
                super::row_highlight(theme)
            } else if self.active {
                Style::new().bg(theme.surface.element.elevate_by(0.04))
            } else {
                Style::default()
            })
            .child(gutter)
            .child(cells.child(text))
            .into()
    }
}

/// The ✕ pinned to a row's title line: click to arm, click again to destroy.
///
/// It renders the same armed or not — the row says what is about to happen (struck-through title, a
/// "Click again to confirm" detail line) while the glyph stays exactly where and what it was, since
/// the confirming click has to land back on it. Deliberately *not* recolored while armed: red is
/// what hovering it means, so an armed ✕ painted red would answer the pointer with no change at all
/// and stop reading as a thing you can click.
///
/// The cell of padding to its right holds it off the panel edge and off the scrollbar that appears
/// once the list scrolls; it is inside the region, so it widens the target rather than shrinking it.
///
/// Its own `MouseRegion` nested inside the row's is what separates "close this" from the row's
/// ordinary click (focus the pane, attach the session): the nearest enclosing region wins a click,
/// so the row's `on_click` never fires for it. That nesting is also why it reports hover itself —
/// hover resolves to a single innermost node, so without this the pointer reaching the ✕ would read
/// as leaving the row and take the ✕ away with it.
fn close_affordance(ctx: &Context<AppRoot>, close: CloseAffordance) -> Element {
    let panel = close.panel;
    let index = close.index;
    let style = super::super::fg_only(&ctx.state.theme.muted);
    let region: Element = MouseRegion::new()
        .on_click(
            ctx.link()
                .callback(move |_| Msg::SidebarRowClose { panel, index }),
        )
        .on_mouse_move(
            ctx.link()
                .callback(move |_| Msg::SidebarPointerMoved(panel)),
        )
        .on_hover_change(ctx.link().callback(move |hovered| Msg::SidebarRowHover {
            panel,
            index,
            hovered,
        }))
        .hover_effect(VisualEffect::transform_fg(ColorTransform::Tint(
            ctx.state.theme.status.error,
            1.0,
        )))
        .child(
            // `Auto` width is load-bearing: a flexing wrapper would compete with the title for the
            // row and drag the ✕ into the middle of it instead of pinning it to the edge.
            HStack::new()
                .padding((0, 1, 0, 0))
                .width(Length::Auto)
                .height(Length::Px(1))
                .child(Text::new("✕").style(style).height(Length::Px(1))),
        )
        .into();
    region.key(close_hover_key(panel, index))
}

/// Stable identity for the nested close region, allowing the parent row to compose its own hover
/// effect while this child is the framework's single hover owner.
pub(super) fn close_hover_key(panel: usize, index: usize) -> String {
    format!("sidebar-{panel}-row-close-{index}")
}

/// One-line section header, aligned with the glyph column so headed rows read as a tree.
pub(super) fn header(ctx: &Context<AppRoot>, label: impl Into<String>, muted: bool) -> Element {
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

/// A section header carrying a right-aligned note — the project header's branch. The label flexes,
/// so an overlong project name truncates rather than pushing the note off the edge: the note is a
/// fixed short thing that is either fully readable or worthless, while a clipped project name is
/// still recognizable.
pub(super) fn header_with_note(
    ctx: &Context<AppRoot>,
    label: impl Into<String>,
    note: impl Into<String>,
) -> Element {
    HStack::new()
        .gap(1)
        .height(Length::Px(1))
        // Left cell aligns with the plain header's leading space. Nothing on the right: the note
        // lands in the same column as the rows' badges below it, which is what makes the two read
        // as one right-hand rail rather than two near-misses.
        .padding((0, 0, 0, 1))
        .child(
            Text::new(label.into())
                .style(super::super::fg_only(&ctx.state.theme.accent).bold())
                .width(Length::Flex(1)),
        )
        .child(Text::new(note.into()).style(super::super::fg_only(&ctx.state.theme.muted).dim()))
        .into()
}

/// Shortens a value by dropping characters from the *front*, keeping the tail. For a path, the tail
/// is the part that identifies it — clipping `~/Work/Projects/rozi` to `~/Work/Project…` throws
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
