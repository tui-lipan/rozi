//! The domain cores (`layout/`, `pane/`, `session/`) sit beneath the application layers. A core
//! file that imports `state`, `config`, `view`, `app`, `ops` or `update` — or names `AppRoot` —
//! pulls its whole module back above the layer it is meant to support, and the dependency cycle
//! that follows only shows up as a build that cannot be split or tested alone.
//!
//! This guards the files that have already been cut free. Add a file here when it stops importing
//! the layers; never remove one to make a change pass.

use std::path::Path;

use crate::source_scan::{production_lines, rust_sources};

/// Files and directories (relative to the crate root) that must not reach into the layers.
const FLOOR: &[&str] = &[
    "src/layout/model.rs",
    "src/layout/tree_ser.rs",
    "src/layout/geometry.rs",
    "src/pane/id.rs",
    "src/pane/chrome.rs",
    "src/control/pick.rs",
    "src/layout/shared/mod.rs",
    "src/layout/shared/validate.rs",
    "src/layout/shared/edit.rs",
    "src/layout/shared/placement.rs",
    "src/control.rs",
    "src/control",
    "src/session/protocol",
    "src/session/naming.rs",
    // Vocabulary the wire carries, reached from `session/protocol` and `control`.
    "src/runtime_metrics.rs",
    "src/extension_runtime/protocol.rs",
    "src/recording/format.rs",
];

/// First path segments under `crate::` that a floor file must not reach.
const FORBIDDEN_ROOTS: &[&str] = &["state", "config", "view", "app", "ops", "update"];

/// Rust spells one path many ways: `crate::state::X`, `crate::{state::X, pane::Y}`,
/// `crate :: state`, and a `use` split across lines. This reads the path after each `crate::`
/// rather than matching one spelling, so a grouped import cannot slip past the guard.
///
/// Returns the 1-based line of every forbidden `crate::<root>` path, and of every `AppRoot`.
fn forbidden_references(lines: &[&str]) -> Vec<(usize, String)> {
    // Comment-free text with each char's line, so a path split across lines still reads whole.
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        chars.extend(code.chars().map(|c| (c, index + 1)));
        chars.push(('\n', index + 1));
    }
    let text: Vec<char> = chars.iter().map(|(c, _)| *c).collect();
    let skip_ws = |mut i: usize| {
        while text.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        i
    };
    let ident_at = |i: usize| -> (String, usize) {
        let end = (i..text.len())
            .find(|&j| !(text[j].is_alphanumeric() || text[j] == '_'))
            .unwrap_or(text.len());
        (text[i..end].iter().collect(), end)
    };

    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let (word, end) = ident_at(i);
        if word.is_empty() {
            i += 1;
            continue;
        }
        let next = skip_ws(end);
        if word == "AppRoot" {
            found.push((chars[i].1, word.clone()));
        } else if word == "crate"
            && text.get(next) == Some(&':')
            && text.get(next + 1) == Some(&':')
        {
            let mut at = skip_ws(next + 2);
            let mut roots = Vec::new();
            if text.get(at) == Some(&'{') {
                // Grouped: take the first segment of each top-level item.
                let mut depth = 0;
                let mut item_start = true;
                while let Some(&c) = text.get(at) {
                    match c {
                        '{' => {
                            depth += 1;
                            item_start = depth == 1;
                        }
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        ',' if depth == 1 => item_start = true,
                        c if c.is_whitespace() => {}
                        _ if item_start && depth == 1 => {
                            let (name, name_end) = ident_at(at);
                            roots.push((name, at));
                            item_start = false;
                            at = name_end;
                            continue;
                        }
                        _ => {}
                    }
                    at += 1;
                }
            } else {
                roots.push((ident_at(at).0, at));
            }
            for (root, pos) in roots {
                if FORBIDDEN_ROOTS.contains(&root.as_str()) {
                    found.push((chars[pos].1, format!("crate::{root}")));
                }
            }
        }
        i = end.max(i + 1);
    }
    found
}

#[test]
fn domain_floor_does_not_import_the_application_layers() {
    let mut sources = Vec::new();
    for entry in FLOOR {
        let path = Path::new(entry);
        assert!(path.exists(), "floor entry {entry} no longer exists");
        rust_sources(path, &mut sources);
    }

    let mut offenders = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).expect("read source");
        // Unit tests may build `State` to exercise the core, so their modules are skipped.
        let lines = production_lines(&path, &text);
        for (line_number, what) in forbidden_references(&lines) {
            offenders.push(format!(
                "{}:{line_number}: {what}: {}",
                path.display(),
                lines[line_number - 1].trim()
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "domain-floor files must not depend on the application layers:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_scan_sees_every_spelling_of_a_forbidden_path() {
    let hits = |source: &str| {
        let lines: Vec<&str> = source.lines().collect();
        forbidden_references(&lines).len()
    };
    assert_eq!(hits("use crate::state::State;"), 1);
    assert_eq!(hits("use crate::{state::State, pane::id::PaneId};"), 1);
    assert_eq!(hits("use crate::{config, state};"), 2);
    assert_eq!(
        hits("use crate::{\n    pane::id::PaneId,\n    ops::focus,\n};"),
        1
    );
    assert_eq!(hits("let x: crate :: view::Thing;"), 1);
    assert_eq!(hits("use crate::state::{A, B};"), 1);
    assert_eq!(hits("fn f(ctx: &mut Context<AppRoot>) {}"), 1);
    assert_eq!(
        hits("use crate::{layout::{model::X}, pane::id::PaneId};"),
        0
    );
    assert_eq!(hits("// use crate::state::State;"), 0);
    assert_eq!(hits("use crate::pane::id::PaneId; // not crate::state"), 0);
}
