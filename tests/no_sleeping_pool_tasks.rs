//! `Command::spawn` runs on tui-lipan's fixed 2-8 worker pool, so sleeping inside one holds a
//! worker for the whole delay. A couple of recurring ticks (a clock segment, a picker refresh) are
//! enough to park every worker on a low-core machine and stall unrelated background work behind
//! them — session discovery, config reload, workbar commands.
//!
//! `Command::after` / `CommandLink::send_after` wait on a shared timer thread instead. This guards
//! the invariant, because the sleeping form looks perfectly reasonable in review.

use std::path::Path;

/// Lines to look ahead from a `Command::spawn(` for a sleep in its body.
const WINDOW: usize = 6;

fn rust_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_command_spawn_sleeps_on_a_pool_worker() {
    let mut sources = Vec::new();
    rust_sources(Path::new("src"), &mut sources);
    assert!(!sources.is_empty(), "expected to find crate sources");

    let mut offenders = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).expect("read source");
        let lines: Vec<&str> = text.lines().collect();
        // Unit tests legitimately sleep to drive timing, so stop at the test module. Match only a
        // `#[cfg(test)]` that introduces a `mod`: several files gate a helper fn or `use` on
        // `cfg(test)` well above their tests, and cutting there would blind the scan to most of
        // the file (`src/session/client.rs` gates a helper at line 40).
        let end = lines
            .iter()
            .enumerate()
            .position(|(index, line)| {
                line.trim_start().starts_with("#[cfg(test)]")
                    && lines
                        .get(index + 1)
                        .is_some_and(|next| next.trim_start().starts_with("mod "))
            })
            .unwrap_or(lines.len());

        for (index, line) in lines[..end].iter().enumerate() {
            if !line.contains("Command::spawn") {
                continue;
            }
            let window_end = (index + WINDOW).min(end);
            if let Some(offset) = lines[index..window_end]
                .iter()
                .position(|body| body.contains("thread::sleep"))
            {
                offenders.push(format!("{}:{}", path.display(), index + offset + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these tasks sleep on a pool worker; use Command::after or link.send_after instead:\n  {}",
        offenders.join("\n  ")
    );
}
