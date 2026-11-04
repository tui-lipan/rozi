//! Shared reading of crate sources for the contract tests that scan them.
//!
//! A scan must skip unit-test modules, which legitimately build `State` or sleep to drive timing,
//! but nothing else. A test module is any `mod` gated on `#[cfg(test)]` or `#[cfg(all(test, …))]`,
//! at any visibility. An inline one has its own lines blanked, keeping line numbers and every
//! production item around it. An external one (`mod tests;`) has its file, and any directory of
//! submodules beneath it, left out of the source set.

use std::path::{Path, PathBuf};

/// The `.rs` files at or beneath each of `roots` that are not test modules, with test modules
/// declared anywhere under `src_root` left out.
///
/// Panics when a declared external test module has no file where Rust would look for it: a scan
/// that could not tell which file is test-only would end up scanning it as production.
pub fn production_sources(src_root: &Path, roots: &[&Path]) -> Vec<PathBuf> {
    let mut everything = Vec::new();
    rust_sources(src_root, &mut everything);
    let mut excluded = Vec::new();
    for file in &everything {
        let text = std::fs::read_to_string(file).expect("read source");
        for name in scan(file, &text).external_test_modules {
            let dir = module_dir(file);
            let as_file = dir.join(format!("{name}.rs"));
            let as_dir = dir.join(&name);
            assert!(
                as_file.is_file() || as_dir.join("mod.rs").is_file(),
                "{}: test module `{name}` has no {} or {}",
                file.display(),
                as_file.display(),
                as_dir.join("mod.rs").display()
            );
            excluded.push(as_file);
            excluded.push(as_dir);
        }
    }

    let mut sources = Vec::new();
    for root in roots {
        rust_sources(root, &mut sources);
    }
    sources.retain(|path| !excluded.iter().any(|test| path.starts_with(test)));
    sources
}

/// `text`'s lines with every inline test module replaced by an empty line, so index `n` is still
/// line `n + 1` of the file.
///
/// Panics when a test module's braces never close: a scan that guessed where it ends could hide
/// production code, so the contract fails rather than passing on a misread.
pub fn production_lines<'a>(path: &Path, text: &'a str) -> Vec<&'a str> {
    scan(path, text).lines
}

struct Scan<'a> {
    lines: Vec<&'a str>,
    external_test_modules: Vec<String>,
}

fn scan<'a>(path: &Path, text: &'a str) -> Scan<'a> {
    let mut lines: Vec<&str> = text.lines().collect();
    let mut external_test_modules = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !is_test_gate(lines[index]) {
            index += 1;
            continue;
        }
        // Other attributes may sit between the gate and the `mod`.
        let mut decl = index + 1;
        while lines.get(decl).is_some_and(|line| {
            let line = line.trim_start();
            line.starts_with("#[") || line.starts_with("//")
        }) {
            decl += 1;
        }
        let Some(name) = lines.get(decl).and_then(|line| mod_name(line)) else {
            index += 1;
            continue;
        };
        let end = test_module_end(&lines, decl).unwrap_or_else(|| {
            panic!(
                "{}:{}: could not find where this test module ends",
                path.display(),
                index + 1
            )
        });
        if end == decl && lines[decl].trim_end().ends_with(';') {
            // A file module's path depends on every inline module around it; only the plain
            // top-level layout is mapped, and anything else fails rather than being misread.
            assert!(
                !lines[decl].starts_with(char::is_whitespace),
                "{}:{}: external test module `{name}` inside another module; teach \
                 source_scan where its file is",
                path.display(),
                decl + 1
            );
            external_test_modules.push(name.to_string());
        }
        for line in &mut lines[index..=end] {
            *line = "";
        }
        index = end + 1;
    }
    Scan {
        lines,
        external_test_modules,
    }
}

/// Every `.rs` file at or beneath `path`.
fn rust_sources(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read source dir") {
            rust_sources(&entry.expect("dir entry").path(), out);
        }
    } else if path.extension().is_some_and(|ext| ext == "rs") {
        out.push(path.to_path_buf());
    }
}

/// The directory a file's child modules live in: beside a `mod.rs`, `lib.rs` or `main.rs`, and in
/// a directory named after any other file.
fn module_dir(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new(""));
    match file.file_name().and_then(|name| name.to_str()) {
        Some("mod.rs" | "lib.rs" | "main.rs") => parent.to_path_buf(),
        _ => parent.join(file.file_stem().expect("source file stem")),
    }
}

/// Whether `line` is a `#[cfg]` that only holds under test: `test` itself, or an `all(…)` that
/// includes it. `any(test, …)` and `not(test)` also compile outside tests, so they do not count.
fn is_test_gate(line: &str) -> bool {
    let Some(predicate) = line
        .trim()
        .strip_prefix("#[cfg(")
        .and_then(|rest| rest.strip_suffix(")]"))
    else {
        return false;
    };
    let predicate = predicate.trim();
    if predicate == "test" {
        return true;
    }
    let Some(terms) = predicate
        .strip_prefix("all(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let mut depth = 0usize;
    let mut start = 0;
    let mut has_test = false;
    for (at, c) in terms.char_indices().chain([(terms.len(), ',')]) {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                has_test |= terms[start..at].trim() == "test";
                start = at + 1;
            }
            _ => {}
        }
    }
    has_test
}

/// The module name a `mod` declaration introduces, at any visibility: `pub`, `pub(crate)`,
/// `pub(super)`, `pub(in path)`.
fn mod_name(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    if let Some(scope) = rest.strip_prefix("pub(") {
        rest = scope.split_once(')')?.1;
    } else if let Some(after_pub) = rest.strip_prefix("pub ") {
        rest = after_pub;
    }
    let rest = rest.trim_start().strip_prefix("mod")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
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
#[cfg(test)]
pub(super) mod super_helpers {
    use crate::state::State;
}
#[cfg(all(test, unix))]
#[allow(dead_code)]
pub(in crate::ops) mod unix_tests {
    fn u() {}
}
#[cfg(not(test))]
mod outside_tests {}
#[cfg(any(test, feature = "x"))]
mod sometimes {}
#[cfg(test)]
pub use helpers::h;
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
            (34, "#[cfg(not(test))]"),
            (35, "mod outside_tests {}"),
            (36, "#[cfg(any(test, feature = \"x\"))]"),
            (37, "mod sometimes {}"),
            (38, "#[cfg(test)]"),
            (39, "pub use helpers::h;"),
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

/// External test modules drop out of the source set, as a file or as a directory of submodules,
/// whichever file declares them.
#[test]
fn production_sources_leave_out_external_test_modules() {
    let root = tempfile::tempdir().expect("temp dir");
    let src = root.path().join("src");
    let files = [
        ("lib.rs", "mod a;\nmod b;\n#[cfg(test)]\nmod crate_tests;\n"),
        ("crate_tests.rs", ""),
        (
            "a/mod.rs",
            "#[cfg(test)]\npub(super) mod tests;\npub fn a() {}\n",
        ),
        ("a/tests.rs", ""),
        (
            "b.rs",
            "#[cfg(all(test, unix))]\nmod suite;\nmod tests_helper;\n",
        ),
        ("b/suite/mod.rs", "mod nested;\n"),
        ("b/suite/nested.rs", ""),
        ("b/tests_helper.rs", ""),
    ];
    for (path, text) in files {
        let path = src.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    let mut found: Vec<String> = production_sources(&src, &[&src])
        .iter()
        .map(|path| {
            path.strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    found.sort();
    assert_eq!(found, ["a/mod.rs", "b.rs", "b/tests_helper.rs", "lib.rs"]);
}

#[test]
#[should_panic(expected = "test module `missing` has no")]
fn an_external_test_module_without_a_file_fails_the_scan() {
    let root = tempfile::tempdir().expect("temp dir");
    std::fs::write(root.path().join("lib.rs"), "#[cfg(test)]\nmod missing;\n").unwrap();
    production_sources(root.path(), &[root.path()]);
}
