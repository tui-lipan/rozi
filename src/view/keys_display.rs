//! Display helpers that sit on top of tui-lipan's key notation.
//!
//! Parsed bindings use `KeyBinding::label`. This module only formats handwritten help text
//! (ranges, mouse gestures, mixed descriptions) and the recorder's in-progress modifier preview.

use tui_lipan::format_binding;
use tui_lipan::prelude::KeyMods;

/// The recorder's preview while only modifiers are held (`Ctrl+Shift+`), in the same order the
/// completed chord's `KeyBinding::label` will use. tui-lipan writes the names; the trailing `+`
/// and the empty-state ellipsis are the recorder's.
pub fn format_held_modifiers(modifiers: KeyMods) -> String {
    match modifiers.label().as_str() {
        "" => "…".to_string(),
        label => format!("{label}+"),
    }
}

/// Format a handwritten key legend: alternatives separated by ` / `, chords by `, `.
///
/// Parseable chords go through tui-lipan's [`format_binding`]. The rest keeps Rozi's help-text
/// rules: workspace ranges (`1-9`), mouse gestures, and mixed descriptions stay as written, and
/// arrow glyphs become named keys (`Ctrl+Shift+←` → `Ctrl+Shift+Left`).
pub fn format_keys(text: &str) -> String {
    text.split(" / ")
        .map(|group| {
            group
                .split(", ")
                .map(format_handwritten_chord)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn format_handwritten_chord(chord: &str) -> String {
    chord
        .split_whitespace()
        .map(format_handwritten_step)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_handwritten_step(step: &str) -> String {
    let normalized = step
        .replace('←', "left")
        .replace('→', "right")
        .replace('↑', "up")
        .replace('↓', "down");
    // A lone letter is already display text (`H` vs `h` in copy mode). Routing it through
    // KeyBinding would canonicalize it to the unshifted character.
    if !is_standalone_letter(step)
        && let Ok(label) = format_binding(&normalized)
    {
        return label;
    }
    let mut rest = normalized.as_str();
    let mut modifiers = Vec::new();
    while let Some((modifier, after)) = take_modifier(rest) {
        modifiers.push(modifier);
        rest = after;
    }
    if modifiers.is_empty() {
        return step.to_string();
    }
    let mut parts: Vec<&str> = ["Ctrl", "Alt", "Super", "Shift"]
        .into_iter()
        .filter(|wanted| modifiers.contains(wanted))
        .collect();
    parts.push(rest);
    parts.join("+")
}

fn is_standalone_letter(step: &str) -> bool {
    let mut chars = step.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic()) && chars.next().is_none()
}

fn take_modifier(step: &str) -> Option<(&'static str, &str)> {
    let index = step.find(['+', '-'])?;
    let (modifier, rest) = step.split_at(index);
    let rest = &rest[1..];
    Some((
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl",
            "alt" | "option" => "Alt",
            "super" | "cmd" | "win" | "meta" => "Super",
            "shift" => "Shift",
            _ => return None,
        },
        rest,
    ))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use tui_lipan::prelude::KeyBinding;

    use super::*;

    fn binding(text: &str) -> KeyBinding {
        KeyBinding::from_str(text).expect("binding parses")
    }

    #[test]
    fn parsed_bindings_use_tui_lipan_labels() {
        assert_eq!(binding("a").label(), "a");
        assert_eq!(binding("shift-a").label(), "A");
        assert_eq!(binding("shift-/").label(), "?");
        assert_eq!(binding("ctrl-a q").label(), "Ctrl+A q");
        assert_eq!(binding("ctrl-shift-a").label(), "Ctrl+Shift+A");
        assert_eq!(binding("cmd-shift-p").label(), "Super+Shift+P");
        assert_eq!(binding("ctrl-shift-/").label(), "Ctrl+Shift+/");
        assert_eq!(binding("shift-tab").label(), "Shift+Tab");
        assert_eq!(binding("page-up").label(), "PageUp");
    }

    #[test]
    fn recorder_preview_is_a_prefix_of_the_finished_chord() {
        let held = KeyMods {
            ctrl: true,
            shift: true,
            ..KeyMods::NONE
        };
        assert_eq!(format_held_modifiers(KeyMods::NONE), "…");
        assert_eq!(format_held_modifiers(KeyMods::CTRL), "Ctrl+");
        assert_eq!(format_held_modifiers(held), "Ctrl+Shift+");
        assert!(
            binding("ctrl-shift-a")
                .label()
                .starts_with(&format_held_modifiers(held))
        );
    }

    #[test]
    fn handwritten_keys_keep_ranges_and_descriptions() {
        assert_eq!(format_keys("shift+?"), "?");
        assert_eq!(format_keys("1-9"), "1-9");
        assert_eq!(format_keys("shift+1-9"), "Shift+1-9");
        assert_eq!(format_keys("ctrl+shift+1-9"), "Ctrl+Shift+1-9");
        assert_eq!(format_keys("enter / esc"), "Enter / Esc");
        assert_eq!(format_keys("H / shift+left"), "H / Shift+Left");
        assert_eq!(format_keys("Ctrl+a"), "Ctrl+A");
        assert_eq!(format_keys("Ctrl+Shift+A"), "Ctrl+Shift+A");
        assert_eq!(format_keys("S"), "S");
        assert_eq!(format_keys("Ctrl+Shift+←"), "Ctrl+Shift+Left");
        assert_eq!(format_keys("←/→"), "←/→");
        assert_eq!(
            format_keys("arrows / hjkl / drag gap"),
            "arrows / hjkl / drag gap"
        );
        assert_eq!(format_keys("drag"), "drag");
        assert_eq!(format_keys("right-drag"), "right-drag");
    }
}
