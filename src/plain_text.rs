/// Remove terminal escape sequences and Unicode control characters from untrusted display text.
/// Callers remain responsible for applying context-specific length limits.
pub(crate) fn sanitize(input: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Text,
        Escape,
        EscapeIntermediate,
        Csi,
        Osc,
        OscEscape,
        String,
        StringEscape,
    }

    let mut state = State::Text;
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        state = match state {
            State::Text => match ch {
                '\u{1b}' => State::Escape,
                '\u{9b}' => State::Csi,
                '\u{9d}' => State::Osc,
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => State::String,
                _ if ch.is_control() => State::Text,
                _ => {
                    output.push(ch);
                    State::Text
                }
            },
            State::Escape => match ch {
                '[' => State::Csi,
                ']' => State::Osc,
                'P' | 'X' | '^' | '_' => State::String,
                ' '..='/' => State::EscapeIntermediate,
                _ => State::Text,
            },
            State::EscapeIntermediate => {
                if ('0'..='~').contains(&ch) {
                    State::Text
                } else {
                    State::EscapeIntermediate
                }
            }
            State::Csi => {
                if ('@'..='~').contains(&ch) {
                    State::Text
                } else {
                    State::Csi
                }
            }
            State::Osc => match ch {
                '\u{7}' | '\u{9c}' => State::Text,
                '\u{1b}' => State::OscEscape,
                _ => State::Osc,
            },
            State::OscEscape => {
                if ch == '\\' {
                    State::Text
                } else {
                    State::Osc
                }
            }
            State::String => match ch {
                '\u{9c}' => State::Text,
                '\u{1b}' => State::StringEscape,
                _ => State::String,
            },
            State::StringEscape => {
                if ch == '\\' {
                    State::Text
                } else {
                    State::String
                }
            }
        };
    }
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escape_sequences_and_controls() {
        assert_eq!(
            sanitize(" \u{1b}[31mred\u{1b}[0m\r\n\u{1b}]0;title\u{7}ok\u{1b}(B\u{0} "),
            "redok"
        );
    }

    #[test]
    fn strips_c1_sequences() {
        assert_eq!(sanitize("a\u{9b}31mb\u{9d}title\u{9c}c"), "abc");
    }
}
