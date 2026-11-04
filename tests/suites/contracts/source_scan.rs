//! Shared reading of crate sources for the contract tests that scan them.
//!
//! A scan must skip unit-test modules, which legitimately build `State` or sleep to drive timing,
//! but nothing else. Cutting a file at its first `#[cfg(test)] mod` would blind the scan to every
//! production item declared after that module, so this blanks each test module's own lines and
//! keeps the rest, with line numbers unchanged.

use std::path::{Path, PathBuf};

/// Every `.rs` file at or beneath `path`.
pub fn rust_sources(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read source dir") {
            rust_sources(&entry.expect("dir entry").path(), out);
        }
    } else if path.extension().is_some_and(|ext| ext == "rs") {
        out.push(path.to_path_buf());
    }
}

/// `text`'s lines with every `#[cfg(test)]` module replaced by an empty line, so index `n` is
/// still line `n + 1` of the file.
///
/// Panics when a test module's braces never close: a scan that guessed where it ends could hide
/// production code, so the contract fails rather than passing on a misread.
pub fn production_lines<'a>(path: &Path, text: &'a str) -> Vec<&'a str> {
    let mut lines: Vec<&str> = text.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let opens_test_mod = lines[index].trim_start().starts_with("#[cfg(test)]")
            && lines
                .get(index + 1)
                .is_some_and(|next| is_mod_declaration(next.trim_start()));
        if !opens_test_mod {
            index += 1;
            continue;
        }
        let end = test_module_end(&lines, index + 1).unwrap_or_else(|| {
            panic!(
                "{}:{}: could not find where this test module ends",
                path.display(),
                index + 1
            )
        });
        for line in &mut lines[index..=end] {
            *line = "";
        }
        index = end + 1;
    }
    lines
}

fn is_mod_declaration(line: &str) -> bool {
    let line = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    line.starts_with("mod ")
}

/// The index of the line that closes the module declared on `start`: the same line for
/// `mod tests;`, otherwise the line holding its matching `}`. Braces inside comments, strings, raw
/// strings and character literals do not count.
fn test_module_end(lines: &[&str], start: usize) -> Option<usize> {
    // One stream from `start` on, each char tagged with its line, so strings and block comments
    // that span lines read whole.
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(start) {
        chars.extend(line.chars().map(|c| (c, index)));
        chars.push(('\n', index));
    }
    let char_at = |i: usize| chars.get(i).map(|(c, _)| *c);
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    // A `b`, `c`, `br` or `cr` prefix only counts when it starts a token.
    let starts_token = |i: usize| i == 0 || !char_at(i - 1).is_some_and(is_ident);

    let mut depth = 0usize;
    let mut opened = false;
    let mut at = 0;
    while let Some(c) = char_at(at) {
        let next = char_at(at + 1);
        match c {
            '/' if next == Some('/') => {
                while char_at(at).is_some_and(|c| c != '\n') {
                    at += 1;
                }
                continue;
            }
            '/' if next == Some('*') => {
                let mut nesting = 1;
                at += 2;
                while nesting > 0 {
                    match (char_at(at)?, char_at(at + 1)) {
                        ('*', Some('/')) => {
                            nesting -= 1;
                            at += 1;
                        }
                        ('/', Some('*')) => {
                            nesting += 1;
                            at += 1;
                        }
                        _ => {}
                    }
                    at += 1;
                }
                continue;
            }
            // A raw string: `r"…"` or `r#"…"#`, optionally `b`- or `c`-prefixed.
            'r' if starts_token(at)
                || (at > 0
                    && matches!(char_at(at - 1), Some('b' | 'c'))
                    && starts_token(at - 1)) =>
            {
                let mut hashes = 0;
                let mut open = at + 1;
                while char_at(open) == Some('#') {
                    hashes += 1;
                    open += 1;
                }
                if char_at(open) == Some('"') {
                    at = open + 1;
                    while !(char_at(at)? == '"'
                        && (1..=hashes).all(|k| char_at(at + k) == Some('#')))
                    {
                        at += 1;
                    }
                    at += hashes;
                }
            }
            '"' => {
                at += 1;
                loop {
                    match char_at(at)? {
                        '\\' => at += 1,
                        '"' => break,
                        _ => {}
                    }
                    at += 1;
                }
            }
            // A character literal (`'{'`, `'\''`); a lifetime has no closing quote.
            '\'' if next == Some('\\') => {
                at += 3;
                while char_at(at).is_some_and(|c| c != '\'') {
                    at += 1;
                }
            }
            '\'' if char_at(at + 2) == Some('\'') => at += 2,
            ';' if !opened => return Some(chars[at].1),
            '{' => {
                depth += 1;
                opened = true;
            }
            '}' => {
                depth = depth.checked_sub(1)?;
                if opened && depth == 0 {
                    return Some(chars[at].1);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

#[test]
fn production_lines_skip_only_the_test_modules() {
    let source = r##"use crate::a;
#[cfg(test)]
mod early_tests {
    const OPEN: char = '{';
    const QUOTE: char = '\'';
    const TEXT: &str = "}} {\"";
    const RAW: &str = r#"say "}" {"#;
    const BYTES: &[u8] = br"}";
    const SPAN: &str = "{
    ";
    // }
    /* } /* } */ */
    fn f<'a>(s: &'a str) -> &'a str { s }
}
pub fn after_early_tests() {}
#[cfg(test)]
mod external_tests;
pub fn after_external() {}
#[cfg(test)]
pub(crate) mod helpers {
    fn h() {}
}
#[cfg(test)]
fn gated_helper() {}
"##;
    let lines = production_lines(Path::new("sample.rs"), source);
    let kept: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .map(|(index, line)| (index + 1, *line))
        .collect();
    assert_eq!(
        kept,
        vec![
            (1, "use crate::a;"),
            (15, "pub fn after_early_tests() {}"),
            (18, "pub fn after_external() {}"),
            (23, "#[cfg(test)]"),
            (24, "fn gated_helper() {}"),
        ]
    );
}

#[test]
#[should_panic(expected = "could not find where this test module ends")]
fn an_unclosed_test_module_fails_the_scan() {
    production_lines(
        Path::new("sample.rs"),
        "#[cfg(test)]\nmod tests {\n    fn f() {}\n",
    );
}
