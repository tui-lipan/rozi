use tui_lipan::prelude::KeyBinding;

/// Format from `canonical_lowercase`, not `canonical()`: tui-lipan prints `ctrl+a` as `Ctrl+A`.
pub fn format_binding(binding: &KeyBinding) -> String {
    binding
        .canonical_lowercase()
        .split_whitespace()
        .map(format_step)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn format_keys(text: &str) -> String {
    text.split(" / ")
        .map(|group| {
            group
                .split(", ")
                .map(|chord| {
                    chord
                        .split_whitespace()
                        .map(format_step)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn format_step(step: &str) -> String {
    let step = match step {
        "←" => "left",
        "→" => "right",
        "↑" => "up",
        "↓" => "down",
        _ => step,
    };
    let mut rest = step;
    let mut modifiers = Vec::new();
    while let Some((modifier, after)) = take_modifier(rest) {
        modifiers.push(modifier.to_string());
        rest = after;
    }
    if modifiers.is_empty() && !is_key(rest) {
        return rest.to_string();
    }

    let rest = match rest {
        "←" => "left",
        "→" => "right",
        "↑" => "up",
        "↓" => "down",
        _ => rest,
    };
    let mut key = named_key(rest).unwrap_or(rest).to_string();
    if is_function_key(&key) {
        key.make_ascii_uppercase();
    }
    let shifted = modifiers.iter().any(|modifier| modifier == "Shift");
    let range = is_numeric_range(&key);
    if shifted && !range {
        if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() {
            key.make_ascii_uppercase();
            modifiers.retain(|modifier| modifier != "Shift");
        } else if let Some(glyph) = shifted_us_layout_glyph(&key) {
            key = glyph.to_string();
            modifiers.retain(|modifier| modifier != "Shift");
        }
    }
    if key.eq_ignore_ascii_case("backtab") {
        key = "Tab".to_string();
        if !modifiers.iter().any(|modifier| modifier == "Shift") {
            modifiers.push("Shift".to_string());
        }
    }
    let mut out = ["Ctrl", "Alt", "Super", "Shift"]
        .into_iter()
        .filter(|wanted| modifiers.iter().any(|modifier| modifier == wanted))
        .map(str::to_string)
        .collect::<Vec<_>>();
    out.push(key);
    out.join("+")
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

fn named_key(key: &str) -> Option<&'static str> {
    Some(match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "esc" | "escape" => "Esc",
        "space" => "Space",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page-up" | "pgup" => "PageUp",
        "pagedown" | "page-down" | "pgdown" => "PageDown",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        _ => return None,
    })
}

fn is_key(key: &str) -> bool {
    named_key(key).is_some()
        || key.eq_ignore_ascii_case("backtab")
        || key.len() == 1
        || is_numeric_range(key)
        || is_function_key(key)
}

fn is_numeric_range(key: &str) -> bool {
    matches!(key.as_bytes(), [start, b'-', end] if start.is_ascii_digit() && end.is_ascii_digit())
}

fn is_function_key(key: &str) -> bool {
    matches!(
        key.as_bytes(),
        [b'f', digit] | [b'F', digit] if (b'1'..=b'9').contains(digit)
    ) || matches!(
        key.as_bytes(),
        [b'f', b'1', digit] | [b'F', b'1', digit] if (b'0'..=b'2').contains(digit)
    )
}

fn shifted_us_layout_glyph(key: &str) -> Option<char> {
    Some(match key {
        "`" => '~',
        "1" => '!',
        "2" => '@',
        "3" => '#',
        "4" => '$',
        "5" => '%',
        "6" => '^',
        "7" => '&',
        "8" => '*',
        "9" => '(',
        "0" => ')',
        "-" => '_',
        "=" => '+',
        "[" => '{',
        "]" => '}',
        "\\" => '|',
        ";" => ':',
        "'" => '"',
        "," => '<',
        "." => '>',
        "/" => '?',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn binding(text: &str) -> KeyBinding {
        KeyBinding::from_str(text).expect("binding parses")
    }

    #[test]
    fn formats_shifted_printable_keys() {
        assert_eq!(format_binding(&binding("a")), "a");
        assert_eq!(format_binding(&binding("shift-a")), "A");
        assert_eq!(format_binding(&binding("shift-e")), "E");
        assert_eq!(format_binding(&binding("ctrl-a")), "Ctrl+a");
        assert_eq!(format_binding(&binding("ctrl-shift-a")), "Ctrl+A");
        assert_eq!(format_binding(&binding("alt-a")), "Alt+a");
        assert_eq!(format_binding(&binding("alt-shift-a")), "Alt+A");
        assert_eq!(format_binding(&binding("ctrl-alt-e")), "Ctrl+Alt+e");
        assert_eq!(format_binding(&binding("ctrl-alt-shift-e")), "Ctrl+Alt+E");
        assert_eq!(format_binding(&binding("shift-/")), "?");
    }

    #[test]
    fn keeps_shift_for_named_keys_and_ranges() {
        assert_eq!(format_binding(&binding("shift-tab")), "Shift+Tab");
        assert_eq!(
            format_binding(&binding("ctrl-shift-left")),
            "Ctrl+Shift+Left"
        );
        assert_eq!(format_binding(&binding("page-up")), "PageUp");
        assert_eq!(format_binding(&binding("page-down")), "PageDown");
        assert_eq!(format_keys("1-9"), "1-9");
        assert_eq!(format_keys("shift+1-9"), "Shift+1-9");
        assert_eq!(format_keys("ctrl+shift+1-9"), "Ctrl+Shift+1-9");
    }

    #[test]
    fn formats_handwritten_keys_without_losing_shifted_glyphs() {
        assert_eq!(format_keys("H / shift+left"), "H / Shift+Left");
        assert_eq!(format_keys("Ctrl+a"), "Ctrl+a");
        assert_eq!(format_keys("Ctrl+A"), "Ctrl+A");
        assert_eq!(format_keys("Ctrl+Shift+←"), "Ctrl+Shift+Left");
        assert_eq!(
            format_keys("arrows / hjkl / drag gap"),
            "arrows / hjkl / drag gap"
        );
        assert_eq!(format_keys("drag"), "drag");
        assert_eq!(format_keys("right-drag"), "right-drag");
    }
}
