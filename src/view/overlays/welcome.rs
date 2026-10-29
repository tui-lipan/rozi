use super::*;

const WELCOME_WIDTH: u16 = 72;
/// Space between a row's term or keys and what it means.
const COLUMN_GAP: usize = 2;

/// A first install's welcome: the words rozi uses, then the keys to start with, as the user has
/// them bound.
pub(crate) fn welcome_overlay(ctx: &Context<AppRoot>) -> Element {
    let actions = vec![
        OverlayAction::new("enter", "start", Msg::CloseWelcome, true),
        OverlayAction::new("ctrl-l", "open guide", Msg::OpenWelcomeGuide, true),
    ];
    let formatter = WelcomeFormatter::new(welcome_sections(ctx), &ctx.state.theme);
    let document = DocumentView::new(formatter.text())
        .height(welcome_height(ctx, &formatter))
        .formatter(formatter)
        .wrap(false)
        .line_numbers(false)
        .border(false)
        .padding((1, 1, 0, 1))
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(&ctx.state.theme))
        // The card's only focusable widget, so it receives Enter, Esc, and the arrows.
        .focusable(true)
        .style(fg_only(&ctx.state.theme.primary))
        .focus_content_style(fg_only(&ctx.state.theme.primary))
        .on_key(overlay_interceptor(ctx, &actions))
        .key(welcome_key());
    let content = VStack::new()
        .child(document)
        .child(overlay_hints(ctx, &actions));

    action_palette_modal_with_width(ctx, "Welcome to rozi", WELCOME_WIDTH)
        .on_close(ctx.link().callback(|_| Msg::CloseWelcome))
        .child(content)
        .key("rozi-welcome-modal")
}

struct WelcomeSection {
    title: &'static str,
    /// Whether the left column holds keys rather than words.
    keys: bool,
    rows: Vec<(String, String)>,
}

fn welcome_sections(ctx: &Context<AppRoot>) -> Vec<WelcomeSection> {
    let prefix = ctx.state.config.input.prefix.label();
    let words = vec![
        (
            "session".to_string(),
            "your terminals; named ones keep running after you leave".to_string(),
        ),
        (
            "pane".to_string(),
            "one terminal; open more and they tile side by side".to_string(),
        ),
        (
            "workspace".to_string(),
            "a screen of panes; each session has nine".to_string(),
        ),
        (
            "prefix".to_string(),
            format!("{prefix}, then a command key"),
        ),
    ];
    let workspaces = crate::view::keys_display::format_keys(&format!("{prefix} 1-9"));
    let keys = [
        (Some("spawn"), "new pane"),
        (None, "move between panes"),
        (Some("close"), "close pane"),
        (None, "switch workspace"),
        (Some("command-palette"), "command palette"),
        (Some("help"), "all keys"),
        (Some("quit"), "leave"),
    ]
    .into_iter()
    .filter_map(|(id, what)| {
        let keys = match (id, what) {
            (Some(id), _) => preferred_shortcut(ctx, id, &prefix)?,
            (None, "switch workspace") => workspaces.clone(),
            (None, _) => focus_keys(ctx, &prefix)?,
        };
        Some((keys, what.to_string()))
    })
    .collect();
    vec![
        WelcomeSection {
            title: "The basics",
            keys: false,
            rows: words,
        },
        WelcomeSection {
            title: "Keys to start with",
            keys: true,
            rows: keys,
        },
    ]
}

/// The command's prefix chord, or its first shortcut when it has none. `None` for a command the
/// user unbound, which then takes no row: a card that names a key that does nothing teaches the
/// wrong thing.
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

/// Content-sized while the card fits, capped so it scrolls in a short terminal instead of pushing
/// the hint row out of the modal.
fn welcome_height(ctx: &Context<AppRoot>, formatter: &WelcomeFormatter) -> Length {
    // Modal border, hint row, and one row of slack.
    const CHROME_ROWS: u16 = 4;

    let cap = (ctx.viewport().h * ACTION_PALETTE_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(CHROME_ROWS)
        .max(3);
    let rows = u16::try_from(formatter.text().lines().count() + 1).unwrap_or(u16::MAX);
    if rows > cap {
        Length::Px(cap)
    } else {
        Length::Auto
    }
}

#[derive(Clone)]
struct WelcomeFormatter {
    lines: Vec<(Vec<(String, Style)>, String)>,
}

impl WelcomeFormatter {
    fn new(sections: Vec<WelcomeSection>, theme: &Theme) -> Self {
        let heading = rozi_fg(theme).bold();
        let term = fg_only(&theme.primary).bold();
        let keys = Style::new().fg(theme.status.info);
        let text = fg_only(&theme.primary);
        let mut lines = Vec::new();
        for (index, section) in sections.iter().enumerate() {
            if index > 0 {
                lines.push((Vec::new(), String::new()));
            }
            lines.push((
                vec![(section.title.to_string(), heading)],
                section.title.to_string(),
            ));
            let lead_width = section
                .rows
                .iter()
                .map(|(lead, _)| unicode_width::UnicodeWidthStr::width(lead.as_str()))
                .max()
                .unwrap_or_default()
                + COLUMN_GAP;
            let lead_style = if section.keys { keys } else { term };
            for (lead, what) in &section.rows {
                let padded = format!("{lead:<lead_width$}");
                lines.push((
                    vec![(padded.clone(), lead_style), (what.clone(), text)],
                    format!("{padded}{what}"),
                ));
            }
        }
        Self { lines }
    }

    /// The card as plain text, which is also what the document copies.
    fn text(&self) -> String {
        self.lines
            .iter()
            .map(|(_, plain)| plain.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl ContentFormatter for WelcomeFormatter {
    fn format(&self, _input: FormatInput<'_>) -> FormattedDocument {
        let lines = self
            .lines
            .iter()
            .enumerate()
            .map(|(source_line, (spans, _))| FormattedLine {
                spans: spans
                    .iter()
                    .map(|(text, style)| Span::new(text.clone()).style(*style))
                    .collect(),
                source_line,
                indent: 0,
                links: Vec::new(),
            })
            .collect();
        FormattedDocument {
            blocks: vec![FormattedBlock::Lines(lines)],
        }
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
