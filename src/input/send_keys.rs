//! tmux-style key-name parsing for control-socket `send-keys`.

use std::str::FromStr;

use tui_lipan::prelude::{KeyBinding, KeyEvent};

/// Parse one `send-keys` argument into either a discrete key event or literal text.
///
/// Named keys use tmux notation (`C-c`, `M-x`, `Enter`, `F1`, …). Anything that does not match a
/// known key name is treated as literal text to forward as UTF-8 bytes.
pub fn parse_send_keys_arg(raw: &str, force_literal: bool) -> Result<SendKeysItem, String> {
    if force_literal {
        return Ok(SendKeysItem::Text(raw.to_string()));
    }
    if let Some(event) = try_parse_key_name(raw)? {
        return Ok(SendKeysItem::Key(event));
    }
    Ok(SendKeysItem::Text(raw.to_string()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendKeysItem {
    Key(KeyEvent),
    Text(String),
}

fn try_parse_key_name(raw: &str) -> Result<Option<KeyEvent>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    // Single printable character (including space via the Space alias below) is a key when it is
    // a known alias or a single-char token; multi-char bare text falls through to literal.
    if let Some(aliased) = alias_key_name(trimmed) {
        return parse_lipan_binding(&aliased).map(Some);
    }

    if looks_like_tmux_chord(trimmed) {
        let converted = tmux_chord_to_binding(trimmed)?;
        return parse_lipan_binding(&converted).map(Some);
    }

    // Bare single character → key event; longer bare strings stay literal.
    if trimmed.chars().count() == 1 {
        return parse_lipan_binding(trimmed).map(Some);
    }

    Ok(None)
}

fn looks_like_tmux_chord(raw: &str) -> bool {
    let mut parts = raw.split('-').filter(|part| !part.is_empty());
    let Some(first) = parts.next() else {
        return false;
    };
    parts.next().is_some() && matches!(first, "C" | "c" | "M" | "m" | "S" | "s")
}

fn tmux_chord_to_binding(raw: &str) -> Result<String, String> {
    let parts: Vec<&str> = raw.split('-').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 {
        return Err(format!("invalid key name `{raw}`"));
    }

    let mut mods = Vec::new();
    let mut idx = 0;
    // Modifiers consume leading C/M/S tokens only while at least one key token remains.
    while idx < parts.len().saturating_sub(1) {
        match parts[idx] {
            "C" | "c" => mods.push("ctrl"),
            "M" | "m" => mods.push("alt"),
            "S" | "s" => mods.push("shift"),
            _ => break,
        }
        idx += 1;
    }
    let key_parts = &parts[idx..];
    if key_parts.is_empty() {
        return Err(format!("invalid key name `{raw}`"));
    }

    let key = key_parts.join("-");
    let key = alias_key_name(&key).unwrap_or_else(|| key.to_ascii_lowercase());
    if mods.is_empty() {
        Ok(key)
    } else {
        Ok(format!("{}-{}", mods.join("-"), key))
    }
}

fn alias_key_name(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let aliased = match lower.as_str() {
        "enter" | "return" | "cr" => "enter",
        "escape" | "esc" => "esc",
        "space" => "space",
        "tab" => "tab",
        "bspace" | "backspace" | "bs" => "backspace",
        "dc" | "delete" | "del" => "delete",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        "home" => "home",
        "end" => "end",
        "pgup" | "pageup" | "ppage" => "pageup",
        "pgdn" | "pagedown" | "npage" => "pagedown",
        "ic" | "insert" => "insert",
        other if other.len() >= 2 && other.starts_with('f') && other[1..].parse::<u8>().is_ok() => {
            let n: u8 = other[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                return Some(format!("f{n}"));
            }
            return None;
        }
        _ => return None,
    };
    Some(aliased.to_string())
}

fn parse_lipan_binding(raw: &str) -> Result<KeyEvent, String> {
    let binding = KeyBinding::from_str(raw).map_err(|err| err.to_string())?;
    let events = binding.key_events().map_err(|err| err.to_string())?;
    events
        .into_iter()
        .next()
        .ok_or_else(|| format!("key `{raw}` produced no events"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::prelude::{KeyCode, KeyMods};

    fn key(code: KeyCode, mods: KeyMods) -> KeyEvent {
        KeyEvent { code, mods }
    }

    fn parse_key_name(raw: &str) -> Result<KeyEvent, String> {
        match parse_send_keys_arg(raw, false)? {
            SendKeysItem::Key(event) => Ok(event),
            SendKeysItem::Text(_) => Err(format!("not a key name: `{raw}`")),
        }
    }

    #[test]
    fn parses_tmux_modifier_chords() {
        assert_eq!(
            parse_key_name("C-c").unwrap(),
            key(
                KeyCode::Char('c'),
                KeyMods {
                    ctrl: true,
                    ..KeyMods::NONE
                }
            )
        );
        assert_eq!(
            parse_key_name("M-x").unwrap(),
            key(
                KeyCode::Char('x'),
                KeyMods {
                    alt: true,
                    ..KeyMods::NONE
                }
            )
        );
        assert_eq!(
            parse_key_name("C-M-a").unwrap(),
            key(
                KeyCode::Char('a'),
                KeyMods {
                    ctrl: true,
                    alt: true,
                    ..KeyMods::NONE
                }
            )
        );
    }

    #[test]
    fn parses_named_aliases() {
        assert_eq!(
            parse_key_name("Enter").unwrap(),
            key(KeyCode::Enter, KeyMods::NONE)
        );
        assert_eq!(
            parse_key_name("Escape").unwrap(),
            key(KeyCode::Esc, KeyMods::NONE)
        );
        assert_eq!(
            parse_key_name("Space").unwrap(),
            key(KeyCode::Char(' '), KeyMods::NONE)
        );
        assert_eq!(
            parse_key_name("BSpace").unwrap(),
            key(KeyCode::Backspace, KeyMods::NONE)
        );
        assert_eq!(
            parse_key_name("Up").unwrap(),
            key(KeyCode::Up, KeyMods::NONE)
        );
        assert_eq!(
            parse_key_name("PgUp").unwrap(),
            key(KeyCode::PageUp, KeyMods::NONE)
        );
        assert_eq!(
            parse_key_name("F12").unwrap(),
            key(KeyCode::F(12), KeyMods::NONE)
        );
    }

    #[test]
    fn rejects_unknown_function_keys_as_names_but_keeps_literal() {
        assert!(parse_key_name("F99").is_err());
        assert_eq!(
            parse_send_keys_arg("F99", false).unwrap(),
            SendKeysItem::Text("F99".into())
        );
    }

    #[test]
    fn force_literal_bypasses_key_names() {
        assert_eq!(
            parse_send_keys_arg("C-c", true).unwrap(),
            SendKeysItem::Text("C-c".into())
        );
        assert_eq!(
            parse_send_keys_arg("Enter", true).unwrap(),
            SendKeysItem::Text("Enter".into())
        );
    }

    #[test]
    fn bare_text_stays_literal() {
        assert_eq!(
            parse_send_keys_arg("echo hi", false).unwrap(),
            SendKeysItem::Text("echo hi".into())
        );
        assert_eq!(
            parse_send_keys_arg("a", false).unwrap(),
            SendKeysItem::Key(key(KeyCode::Char('a'), KeyMods::NONE))
        );
    }

    #[test]
    fn invalid_modifier_only_chord_errors() {
        assert!(parse_key_name("C-").is_err());
    }
}
