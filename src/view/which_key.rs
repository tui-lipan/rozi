//! The which-key strip: what the prefix can do next, shown while a chord is pending.
//!
//! This is chrome, not an overlay. It carries no `State` flag and takes no focus, because
//! [`crate::commands::commands_active`] disables every command while a modal is up - a which-key
//! that registered as an overlay would switch off the very chord it is advertising. Everything it
//! shows is read at render time from the live command registry, so `[keys]` overrides, unbound
//! commands, and per-context availability are always truthful.
//!
//! Three rules keep it a strip rather than a second help overlay:
//!
//! 1. **Families collapse.** Four rows reading `h/j/k/l Focus left/down/up/right` say one thing;
//!    they render as `hjkl Focus pane`. Collapsing is refused the moment a member is rebound, so a
//!    customized binding is never misreported (see [`collapse_keys`]).
//! 2. **Unavailable commands drop out.** `CommandEntry::enabled` already folds in
//!    `command_available`, so commands that cannot act right now (no control lease, no exited pane)
//!    disappear without a second gate that could drift from real dispatch.
//! 3. **Height is capped.** Whatever does not fit is counted in the header rather than paged, since
//!    a paging key would have to steal a chord from the very table it is scrolling. The overflow
//!    hint points at the keybindings overlay, which is already bound and already lists everything.

use std::collections::HashSet;
use std::str::FromStr;

use tui_lipan::prelude::*;

use super::fg_only;
use crate::AppRoot;
use crate::state::WORKBAR_HEIGHT;

/// Blank columns between two packed columns.
const COLUMN_GAP: usize = 3;
/// Rows the strip is allowed to occupy, before the viewport-height cap applies.
const MAX_ROWS: usize = 8;
/// Share of the viewport height the strip may take. It is transient chrome sitting on top of live
/// panes, so it yields screen before it yields to the overflow count.
const HEIGHT_SHARE: usize = 5;
/// Rows the strip needs before it is worth drawing at all.
const MIN_ROWS: usize = 2;

/// Commands that are registered but never belong in the prefix strip as ordinary rows: framework
/// commands rozi re-implements, the detach alias that duplicates quit, the digit families that
/// [`workspace_row`] and the keybindings overlay summarize instead, and the forward-prefix command,
/// which is registered without a label and gets its own row from [`forward_prefix_row`].
fn skipped(id: &str) -> bool {
    id.starts_with("app.")
        || id == "detach"
        || id == crate::commands::FORWARD_PREFIX_COMMAND_ID
        || id.starts_with("workspace.move.")
        || id.starts_with("workspace.relocate.")
}

/// Commands that need a second tile to mean anything.
///
/// `command_available` deliberately leaves these enabled in an unsplit workspace: a disabled
/// command is an unregistered chord, so disabling them would send `<prefix> h` straight through to
/// the shell. The strip hides them on its own instead of advertising nine no-ops - which is also
/// what keeps it small in exactly the case where somebody is most likely to be reading it.
const NEEDS_NEIGHBOUR: &[&str] = &[
    "focus-left",
    "focus-down",
    "focus-up",
    "focus-right",
    "swap-left",
    "swap-down",
    "swap-up",
    "swap-right",
    "move-left",
    "move-down",
    "move-up",
    "move-right",
    "cycle-focus-next",
    "cycle-focus-prev",
    "grow-split",
    "shrink-split",
    "flip-split",
    "promote-to-master",
];

/// Whether the active workspace holds more than one live pane. Closing panes are already on their
/// way out, so they do not keep the relational commands on screen.
fn has_neighbour(ctx: &Context<AppRoot>) -> bool {
    let current = ctx.state.current();
    current.workspaces[current.active_workspace]
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .count()
        > 1
}

/// A directional or paired command family that reads better as one row.
struct Family {
    ids: &'static [&'static str],
    label: &'static str,
    /// Placed between the member keys. An empty joiner also allows the shared-modifier form
    /// (`ctrl+h ctrl+j …` -> `ctrl+hjkl`); a non-empty one joins verbatim.
    joiner: &'static str,
}

const FAMILIES: &[Family] = &[
    Family {
        ids: &["focus-left", "focus-down", "focus-up", "focus-right"],
        label: "Focus pane",
        joiner: "",
    },
    Family {
        ids: &["swap-left", "swap-down", "swap-up", "swap-right"],
        label: "Swap pane",
        joiner: "",
    },
    Family {
        ids: &["move-left", "move-down", "move-up", "move-right"],
        label: "Move pane",
        joiner: "",
    },
    Family {
        ids: &["grow-split", "shrink-split"],
        label: "Resize split",
        joiner: "/",
    },
    Family {
        ids: &["cycle-focus-next", "cycle-focus-prev"],
        label: "Cycle focus",
        joiner: "/",
    },
];

/// Order in which categories are *kept* when the strip runs out of height, which is the reverse of
/// what matters in the keybindings overlay: that one is scrollable and ordered for reading, this
/// one truncates and is ordered so the window-manager keys a hesitating user is most likely
/// hunting for survive the cut.
fn category_priority(category: &str) -> usize {
    match category {
        "Panes" => 0,
        "Focus" => 1,
        "Workspace" => 2,
        "Workspaces" => 3,
        "App" => 4,
        "Sidebar" => 5,
        "Session" => 6,
        "Profile" => 7,
        "Collaboration" => 8,
        "Custom" => 9,
        _ => 10,
    }
}

struct Row {
    keys: String,
    label: String,
    priority: usize,
}

impl Row {
    fn width(&self) -> usize {
        self.keys.chars().count() + 1 + self.label.chars().count()
    }
}

/// The keys a chord presses *after* the prefix, or `None` when this binding is not a prefix chord
/// at all. Reading the continuation off the live binding rather than off `keybinding_hint` is what
/// keeps a `[keys]` override that moved a command off the prefix out of the strip entirely.
fn prefix_continuation(binding: &KeyBinding, prefix: &str) -> Option<String> {
    let canonical = binding.canonical_lowercase();
    let mut steps = canonical.split_whitespace();
    if steps.next()? != prefix {
        return None;
    }
    let rest = steps.collect::<Vec<_>>();
    if rest.is_empty() {
        return None;
    }
    KeyBinding::from_str(&rest.join(" "))
        .ok()
        .map(|binding| binding.compact_display())
}

/// Fold a family's member keys into one display string, or `None` when they no longer form a set
/// worth collapsing - which is exactly when one of them has been rebound, and when spelling them
/// out individually is the only honest thing to do.
fn collapse_keys(parts: &[String], joiner: &str) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    if !joiner.is_empty() {
        return Some(parts.join(joiner));
    }
    // Every member must be the same modifier combination applied to a single character, so the
    // combination can be lifted out front: `h j k l` -> `hjkl`, `ctrl+h …` -> `ctrl+hjkl`.
    let split = |key: &String| -> Option<(String, char)> {
        let mut chars = key.chars();
        let last = chars.next_back()?;
        let head: String = chars.collect();
        (head.is_empty() || head.ends_with('+')).then_some((head, last))
    };
    let split: Vec<_> = parts.iter().map(split).collect::<Option<_>>()?;
    let (head, _) = split.first()?;
    if !split.iter().all(|(candidate, _)| candidate == head) {
        return None;
    }
    let tail: String = split.iter().map(|(_, key)| *key).collect();
    Some(format!("{head}{tail}"))
}

/// One candidate command: a registry entry that is currently actionable and currently reachable
/// from the prefix.
struct Candidate {
    id: String,
    keys: String,
    label: String,
    category: String,
}

fn candidates(ctx: &Context<AppRoot>) -> Vec<Candidate> {
    let prefix = ctx.state.config.input.prefix.canonical_lowercase();
    let neighbour = has_neighbour(ctx);
    ctx.command_registry()
        .entries()
        .into_iter()
        .filter(|entry| entry.enabled && !skipped(entry.id.as_str()))
        .filter(|entry| neighbour || !NEEDS_NEIGHBOUR.contains(&entry.id.as_str()))
        .filter_map(|entry| {
            let keys = entry
                .shortcuts
                .iter()
                .find_map(|binding| prefix_continuation(binding, &prefix))?;
            Some(Candidate {
                id: entry.id.as_str().to_string(),
                keys,
                label: crate::commands::builtin_label(entry.id.as_str())
                    .map_or_else(|| entry.label.to_string(), str::to_string),
                category: entry.category.as_deref().unwrap_or("Other").to_string(),
            })
        })
        .collect()
}

/// `1-9 Workspace`, but only while all nine switches still sit on their own digit. Move and
/// relocate stay out: they are two more digit rows that say almost the same thing, and the
/// keybindings overlay already spells all three families out.
fn workspace_row(candidates: &[Candidate]) -> Option<Row> {
    let mut keys = Vec::with_capacity(9);
    for index in 1..=9 {
        let id = format!("workspace.switch.{index}");
        let candidate = candidates.iter().find(|candidate| candidate.id == id)?;
        let digit = candidate.keys.chars().next()?;
        if candidate.keys.chars().count() != 1 || digit != char::from_digit(index, 10)? {
            return None;
        }
        keys.push(digit);
    }
    (keys.len() == 9).then(|| Row {
        keys: "1-9".to_string(),
        label: "Workspace".to_string(),
        priority: category_priority("Workspaces"),
    })
}

/// `<prefix> <prefix>` sends the prefix key to the pane. It is registered without a label (it is
/// not a palette command), so the strip supplies one - this is precisely the binding nobody
/// discovers on their own.
fn forward_prefix_row(ctx: &Context<AppRoot>) -> Option<Row> {
    let prefix = ctx.state.config.input.prefix.canonical_lowercase();
    let entry = ctx
        .command_registry()
        .entries()
        .into_iter()
        .find(|entry| entry.id.as_str() == crate::commands::FORWARD_PREFIX_COMMAND_ID)?;
    if !entry.enabled {
        return None;
    }
    let keys = entry
        .shortcuts
        .iter()
        .find_map(|binding| prefix_continuation(binding, &prefix))?;
    Some(Row {
        keys,
        label: "Send prefix".to_string(),
        priority: category_priority("App"),
    })
}

fn rows(ctx: &Context<AppRoot>) -> Vec<Row> {
    let candidates = candidates(ctx);

    // Resolve every family first, then emit each one at the position of its leading member, so a
    // collapsed row keeps the place the registry gave it instead of jumping to the front of its
    // category ahead of commands that are registered earlier.
    let mut collapsed: Vec<(&'static str, String, &'static str)> = Vec::new();
    let mut claimed: HashSet<&str> = HashSet::new();
    for family in FAMILIES {
        let members: Option<Vec<&Candidate>> = family
            .ids
            .iter()
            .map(|id| candidates.iter().find(|candidate| candidate.id == *id))
            .collect();
        let Some(members) = members else { continue };
        let keys: Vec<String> = members.iter().map(|member| member.keys.clone()).collect();
        let Some(keys) = collapse_keys(&keys, family.joiner) else {
            continue;
        };
        claimed.extend(family.ids.iter().copied());
        collapsed.push((family.ids[0], keys, family.label));
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut workspace_row = workspace_row(&candidates);
    for candidate in &candidates {
        let id = candidate.id.as_str();
        if let Some((_, keys, label)) = collapsed.iter().find(|(lead, _, _)| *lead == id) {
            rows.push(Row {
                keys: keys.clone(),
                label: (*label).to_string(),
                priority: category_priority(&candidate.category),
            });
            continue;
        }
        if claimed.contains(id) {
            continue;
        }
        if id.starts_with("workspace.") {
            // The nine digit switches stand in for one another; emit the summary at the first.
            rows.extend(workspace_row.take());
            continue;
        }
        rows.push(Row {
            keys: candidate.keys.clone(),
            label: candidate.label.clone(),
            priority: category_priority(&candidate.category),
        });
    }

    if let Some(row) = forward_prefix_row(ctx) {
        rows.push(row);
    }

    rows.sort_by_key(|row| row.priority);
    rows
}

/// Column-major packing: the fewest rows that fits everything within `width`, or `max_rows` with a
/// truncated tail. Returns the columns actually drawn and how many rows were left out.
fn pack(rows: &[Row], width: usize, max_rows: usize) -> (Vec<Vec<&Row>>, usize) {
    let columns_for = |rows_per_column: usize| -> Vec<Vec<&Row>> {
        rows.chunks(rows_per_column)
            .map(<[Row]>::iter)
            .map(Iterator::collect)
            .collect()
    };
    let total_width = |columns: &[Vec<&Row>]| -> usize {
        columns
            .iter()
            .map(|column| column.iter().map(|row| row.width()).max().unwrap_or(0) + COLUMN_GAP)
            .sum::<usize>()
            .saturating_sub(COLUMN_GAP)
    };

    for rows_per_column in 1..=max_rows {
        let columns = columns_for(rows_per_column);
        if total_width(&columns) <= width {
            return (columns, 0);
        }
    }

    // Nothing fits whole: keep full columns from the front until the width runs out.
    let columns = columns_for(max_rows);
    let mut kept: Vec<Vec<&Row>> = Vec::new();
    for column in columns {
        let mut candidate = kept.clone();
        candidate.push(column);
        if total_width(&candidate) > width {
            break;
        }
        kept = candidate;
    }
    let shown: usize = kept.iter().map(Vec::len).sum();
    (kept, rows.len().saturating_sub(shown))
}

/// The continuation that opens the keybindings overlay, for the overflow hint. `None` when help is
/// unbound or has been moved off the prefix, in which case the hint is simply not offered.
fn help_key(ctx: &Context<AppRoot>) -> Option<String> {
    let prefix = ctx.state.config.input.prefix.canonical_lowercase();
    ctx.command_registry()
        .entries()
        .into_iter()
        .find(|entry| entry.id.as_str() == "help")?
        .shortcuts
        .iter()
        .find_map(|binding| prefix_continuation(binding, &prefix))
}

/// The strip and the rect it occupies, in content-viewport coordinates, or `None` when it is
/// disabled, no chord is pending, or the viewport has no room for it.
pub(crate) fn layer(ctx: &Context<AppRoot>, content: Rect) -> Option<(Rect, Element)> {
    // `command_chord_revealed` rather than `command_chord_pending`: the strip waits out
    // `[input] which_key_delay_ms` so a chord finished from muscle memory never flashes it. The
    // runtime schedules the frame at which the delay elapses, so nothing here needs a timer.
    if !ctx.state.config.input.which_key || !ctx.command_chord_revealed() {
        return None;
    }
    let rows = rows(ctx);
    if rows.is_empty() {
        return None;
    }

    let theme = &ctx.state.theme;
    let bar = if ctx.state.config.pane.show_workbar {
        WORKBAR_HEIGHT
    } else {
        0
    };
    // Border on both sides, then a column of padding inside each border.
    let inner_width = usize::from(content.w).saturating_sub(4);
    let available_rows = usize::from(content.h.saturating_sub(bar)).saturating_sub(2);
    let max_rows = (usize::from(content.h) / HEIGHT_SHARE).clamp(MIN_ROWS, MAX_ROWS);
    if inner_width < 12 || available_rows < MIN_ROWS {
        return None;
    }
    let (columns, hidden) = pack(&rows, inner_width, max_rows.min(available_rows));
    if columns.is_empty() {
        return None;
    }

    let drawn_rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let height = u16::try_from(drawn_rows).ok()?.saturating_add(2);
    let y = if ctx.state.config.pane.workbar_at_bottom {
        content.h.checked_sub(bar + height)?
    } else {
        bar
    };

    let key_style = Style::new().fg(theme.border_active).bold();
    let label_style = fg_only(&theme.primary);
    let mut grid = HStack::new()
        .gap(COLUMN_GAP as u16)
        .height(Length::Px(height - 2));
    for column in &columns {
        let column_width = column.iter().map(|row| row.width()).max().unwrap_or(0);
        let mut stack = VStack::new().width(Length::Px(u16::try_from(column_width).ok()?));
        for row in column {
            stack = stack.child(
                Text::from_spans([
                    Span::new(format!("{} ", row.keys)).style(key_style),
                    Span::new(row.label.clone()).style(label_style),
                ])
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
            );
        }
        grid = grid.child(stack);
    }

    // Structured chrome, not a sentence: the left label is what is being held, the right one is
    // the count that did not fit plus the key that shows the rest.
    let overflow = match (hidden, help_key(ctx)) {
        (0, _) => None,
        (hidden, Some(key)) => Some(format!(" +{hidden} · {key} all ")),
        (hidden, None) => Some(format!(" +{hidden} ")),
    };
    // No header label naming the prefix: the workbar's `PREFIX` badge already says which mode this
    // is, and the strip only ever appears in that mode. The header keeps only the overflow count,
    // which is the one thing nothing else on screen reports.
    let mut frame = Frame::new()
        .border(true)
        .border_style(BorderStyle::Rounded)
        // The strip floats over live panes rather than tiling with them, so its border must not
        // fuse with the pane borders it crosses. Merging is the right default for frames that share
        // a seam; here it would draw `┼─┼` junctions into a panel that is not joined to anything.
        .border_merge_mode(BorderMergeMode::Replace)
        .style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.backdrop),
        )
        .padding((0, 1))
        .child(grid);
    if let Some(overflow) = overflow {
        frame = frame.header_right(FrameLabel::new(overflow).style(fg_only(&theme.muted)));
    }

    let rect = FloatRect {
        x: 0.0,
        y: f32::from(y),
        w: f32::from(content.w),
        h: f32::from(height),
    }
    .to_rect();
    Some((rect, frame.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(keys: &str, label: &str) -> Row {
        Row {
            keys: keys.to_string(),
            label: label.to_string(),
            priority: 0,
        }
    }

    #[test]
    fn bare_directional_keys_collapse_into_one_token() {
        let keys = ["h", "j", "k", "l"].map(String::from);
        assert_eq!(collapse_keys(&keys, "").as_deref(), Some("hjkl"));
    }

    #[test]
    fn shared_modifier_lifts_out_of_a_collapsed_family() {
        let keys = ["ctrl+h", "ctrl+j", "ctrl+k", "ctrl+l"].map(String::from);
        assert_eq!(collapse_keys(&keys, "").as_deref(), Some("ctrl+hjkl"));
    }

    #[test]
    fn a_rebound_member_refuses_to_collapse() {
        // Collapsing here would advertise `hjFl`, a chord that does nothing.
        let keys = ["h", "j", "f5", "l"].map(String::from);
        assert_eq!(collapse_keys(&keys, ""), None);
        let mixed = ["ctrl+h", "j", "ctrl+k", "ctrl+l"].map(String::from);
        assert_eq!(collapse_keys(&mixed, ""), None);
    }

    #[test]
    fn paired_families_join_verbatim() {
        let keys = ["=", "-"].map(String::from);
        assert_eq!(collapse_keys(&keys, "/").as_deref(), Some("=/-"));
    }

    #[test]
    fn continuation_is_read_off_the_chord_and_rejects_non_prefix_bindings() {
        let chord = KeyBinding::from_str("ctrl-a shift-n").expect("chord parses");
        assert_eq!(
            prefix_continuation(&chord, "ctrl+a").as_deref(),
            // `compact_display` renders a shifted letter as the glyph it produces.
            Some("N")
        );
        let held = KeyBinding::from_str("alt-n").expect("binding parses");
        assert_eq!(prefix_continuation(&held, "ctrl+a"), None);
        let other_prefix = KeyBinding::from_str("ctrl-b n").expect("chord parses");
        assert_eq!(prefix_continuation(&other_prefix, "ctrl+a"), None);
    }

    #[test]
    fn packing_prefers_the_fewest_rows_that_still_fits() {
        let rows = vec![row("a", "One"), row("b", "Two"), row("c", "Three")];
        let (columns, hidden) = pack(&rows, 80, 10);
        assert_eq!(hidden, 0);
        assert_eq!(columns.len(), 3, "a wide strip lays out on a single row");
        assert!(columns.iter().all(|column| column.len() == 1));
    }

    #[test]
    fn packing_truncates_to_whole_columns_and_counts_the_remainder() {
        let rows: Vec<Row> = (0..20).map(|i| row("x", &format!("Command {i}"))).collect();
        let (columns, hidden) = pack(&rows, 24, 2);
        let shown: usize = columns.iter().map(Vec::len).sum();
        assert!(shown > 0 && hidden > 0);
        assert_eq!(shown + hidden, 20);
        assert!(columns.iter().all(|column| column.len() == 2));
    }
}
