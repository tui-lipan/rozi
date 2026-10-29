use super::*;

/// The rozi mark, sampled from `assets/logo.png`: two pixel rows per text row, drawn with half
/// blocks. `#` is the mark, `.` the background.
const LOGO: [&str; 24] = [
    "........##..##........",
    "......####..####......",
    "....######..######....",
    "..########..########..",
    ".#########..#########.",
    "##########..##########",
    "##########..##########",
    "##########..##########",
    "##########..##########",
    "########......########",
    "######..........######",
    "####....##........####",
    "###......##........###",
    "..........##..........",
    "..........##..........",
    "##.......##.........##",
    "####....##.####...####",
    "######..........######",
    ".######........######.",
    "...######....######...",
    ".....############.....",
    "......##########......",
    "........######........",
    "..........##..........",
];
/// The mark's own gradient, top to bottom: brand colours, the same in every theme.
const LOGO_TOP: (u8, u8, u8) = (0xFD, 0x4A, 0x80);
const LOGO_BOTTOM: (u8, u8, u8) = (0x98, 0x2B, 0xF2);
const LOGO_WIDTH: u16 = 22;
/// The logo column: the mark with breathing room on both sides.
const LOGO_COLUMN: u16 = LOGO_WIDTH + 6;

const WIDE_WIDTH: u16 = 92;
const NARROW_WIDTH: u16 = 72;
/// The smallest screen the full card fits on: the logo beside the words, the keys below. On a
/// smaller one the card becomes a single document that scrolls, without the logo.
const WIDE_MIN_VIEWPORT: (u16, u16) = (WIDE_WIDTH + 4, 38);
/// Rows the compact card takes at its natural height, and the rows around its document: modal
/// border, top padding, and the hint row.
const COMPACT_ROWS: u16 = 28;
const COMPACT_CHROME: u16 = 6;

/// A first install's welcome: the mark, the words rozi uses, and the keys to start with, as the user
/// has them bound.
pub(crate) fn welcome_overlay(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let viewport = ctx.viewport();
    let wide = viewport.w >= WIDE_MIN_VIEWPORT.0 && viewport.h >= WIDE_MIN_VIEWPORT.1;
    let actions = vec![
        OverlayAction::new("enter", "got it", Msg::CloseWelcome, true),
        OverlayAction::new("ctrl-l", "open guide", Msg::OpenWelcomeGuide, true),
    ];

    let mut content = VStack::new().height(Length::Auto).padding((1, 1, 0, 1));
    if wide {
        let keys = welcome_document(ctx, keys_markdown(ctx), true)
            .padding((1, 1, 0, 1))
            .on_key(overlay_interceptor(ctx, &actions))
            .key(welcome_key());
        content = content
            .child(
                HStack::new()
                    .height(Length::Auto)
                    .child(logo(theme))
                    .child(welcome_document(ctx, intro_markdown(), false)),
            )
            .child(keys);
    } else {
        let room = viewport.h.saturating_sub(COMPACT_CHROME).max(3);
        let height = if room >= COMPACT_ROWS {
            Length::Auto
        } else {
            Length::Px(room)
        };
        let markdown = format!("{}\n\n{}", intro_markdown(), keys_markdown(ctx));
        content = content.child(
            welcome_document(ctx, markdown, true)
                // A line under each header only: the rows it saves are worth more here than the grid.
                .table_row_separators(TableRowSeparators::Header)
                .height(height)
                .scrollbar(true)
                .scrollbar_config(modal_scrollbar_config(theme))
                .on_key(overlay_interceptor(ctx, &actions))
                .key(welcome_key()),
        );
    }
    let content = content.child(overlay_hints(ctx, &actions));

    styled_modal(ctx, "", if wide { WIDE_WIDTH } else { NARROW_WIDTH })
        .animation(crate::view::animation::picker_animation(
            ctx.state.config.animations,
        ))
        .height(Length::Auto)
        .on_close(ctx.link().callback(|_| Msg::CloseWelcome))
        .child(content)
        .key("rozi-welcome-modal")
}

/// Markdown in the card's own table style: rounded frames with lines between every column, so the
/// words and keys read as a grid rather than a report.
fn welcome_document(ctx: &Context<AppRoot>, markdown: String, focusable: bool) -> DocumentView {
    let theme = &ctx.state.theme;
    let heading = rozi_fg(theme).bold();
    let styles = DocumentStyles {
        heading_styles: [heading; 6],
        strong_style: fg_only(&theme.primary).bold(),
        code_inline_style: Style::new().fg(theme.status.info).bold(),
        table_header_style: fg_only(&theme.muted),
        ..DocumentStyles::default()
    };
    DocumentView::new(markdown)
        .formatter(MarkdownFormatter::default().styles(styles))
        .height(Length::Auto)
        .wrap(true)
        .line_numbers(false)
        .border(false)
        .padding((0, 1, 0, 1))
        .table_width_mode(DocumentTableWidthMode::Fill)
        .table_wrap(true)
        .table_outer_frame(true)
        .table_column_separators(true)
        .table_row_separators(TableRowSeparators::All)
        .table_border_variant(BorderStyle::Rounded)
        .table_border_style(fg_only(&theme.border))
        .table_cell_padding(1)
        .focusable(focusable)
        .style(fg_only(&theme.primary))
        .focus_content_style(fg_only(&theme.primary))
}

fn intro_markdown() -> String {
    "## Welcome to rozi\n\n\
     Your terminals, tiled side by side, and still running when you come back. \
     Here are the few words and keys that get you going.\n\n\
     | in rozi | means |\n\
     | --- | --- |\n\
     | **session** | your terminals; named ones keep running |\n\
     | **pane** | one terminal; more tile side by side |\n\
     | **workspace** | a screen of panes; nine per session |"
        .to_string()
}

/// The keys, two to a row, after a line that names the prefix once: `Enter` under it reads better
/// than `Ctrl+A Enter` seven times. A key the user moved off the prefix keeps its whole chord, and a
/// command they unbound has no cell, since a key that does nothing teaches nothing.
fn keys_markdown(ctx: &Context<AppRoot>) -> String {
    let prefix = ctx.state.config.input.prefix.label();
    let workspaces = crate::view::keys_display::format_keys(&format!("{prefix} 1-9"));
    let keys: Vec<(String, &str)> = [
        (Some("spawn"), "open a new pane"),
        (None, "move between panes"),
        (Some("close"), "close the pane"),
        (None, "switch workspace"),
        (Some("command-palette"), "find any command"),
        (Some("help"), "see every key"),
        (Some("quit"), "leave rozi"),
    ]
    .into_iter()
    .filter_map(|(id, what)| {
        let keys = match (id, what) {
            (Some(id), _) => preferred_shortcut(ctx, id, &prefix)?,
            (None, "switch workspace") => workspaces.clone(),
            (None, _) => focus_keys(ctx, &prefix)?,
        };
        Some((keys, what))
    })
    .collect();
    let cell = |(keys, what): &(String, &str)| {
        let keys = keys
            .strip_prefix(prefix.as_str())
            .map_or(keys.as_str(), str::trim_start);
        let keys = keys
            .split(' ')
            .map(|key| format!("`{key}`"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{keys} | {what}")
    };
    let half = keys.len().div_ceil(2);
    let mut markdown = format!(
        "These come after the **prefix**, `{prefix}`: press it, let go, then press the key.\n\n\
         | key | does | key | does |\n\
         | --- | --- | --- | --- |\n"
    );
    for row in 0..half {
        let left = cell(&keys[row]);
        let right = keys.get(row + half).map_or(" | ".to_string(), cell);
        markdown.push_str(&format!("| {left} | {right} |\n"));
    }
    markdown.trim_end().to_string()
}

/// The command's prefix chord, or its first shortcut when it has none. `None` for a command the
/// user unbound.
fn preferred_shortcut(ctx: &Context<AppRoot>, id: &str, prefix: &str) -> Option<String> {
    let shortcuts = crate::commands::command_shortcut_labels(ctx, id);
    shortcuts
        .iter()
        .find(|label| label.starts_with(prefix))
        .or_else(|| shortcuts.first())
        .cloned()
}

/// The four focus directions as one chord, `Ctrl+A h j k l`, when each is a prefix chord of one
/// key; otherwise the next-pane cycle, which is a single command and always reads plainly.
fn focus_keys(ctx: &Context<AppRoot>, prefix: &str) -> Option<String> {
    let directions = ["focus-left", "focus-down", "focus-up", "focus-right"]
        .into_iter()
        .map(|id| {
            let shortcut = preferred_shortcut(ctx, id, prefix)?;
            let key = shortcut.strip_prefix(prefix)?.strip_prefix(' ')?;
            (!key.contains(' ')).then(|| key.to_string())
        })
        .collect::<Option<Vec<_>>>();
    match directions {
        Some(keys) => Some(format!("{prefix} {}", keys.join(" "))),
        None => preferred_shortcut(ctx, "cycle-focus-next", prefix),
    }
}

/// The mark, drawn with half blocks in its own gradient, centred in its column.
fn logo(theme: &Theme) -> Element {
    let color = |row: usize| {
        let t = row as f32 / (LOGO.len() - 1) as f32;
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Color::Rgb(
            mix(LOGO_TOP.0, LOGO_BOTTOM.0),
            mix(LOGO_TOP.1, LOGO_BOTTOM.1),
            mix(LOGO_TOP.2, LOGO_BOTTOM.2),
        )
    };
    let lines = LOGO.chunks(2).enumerate().map(|(index, pair)| {
        let (top, bottom) = (pair[0].as_bytes(), pair[1].as_bytes());
        let (top_color, bottom_color) = (color(index * 2), color(index * 2 + 1));
        let spans =
            (0..usize::from(LOGO_WIDTH)).map(|x| match (top[x] == b'#', bottom[x] == b'#') {
                (true, true) => Span::new("▀").style(Style::new().fg(top_color).bg(bottom_color)),
                (true, false) => Span::new("▀").style(Style::new().fg(top_color)),
                (false, true) => Span::new("▄").style(Style::new().fg(bottom_color)),
                (false, false) => Span::new(" "),
            });
        Text::from_spans(spans.collect::<Vec<_>>()).into()
    });
    let mark = lines.fold(
        VStack::new()
            .width(Length::Px(LOGO_WIDTH))
            .height(Length::Auto),
        |stack, line: Element| stack.child(line),
    );
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let version = Text::new(format!(
        "{version:^width$}",
        width = usize::from(LOGO_WIDTH)
    ))
    .style(fg_only(&theme.muted))
    .width(Length::Px(LOGO_WIDTH));
    VStack::new()
        .width(Length::Px(LOGO_COLUMN))
        .height(Length::Auto)
        .padding((1, 3, 0, 3))
        .gap(1)
        .child(mark)
        .child(version)
        .into()
}
