//! `Command::spawn` runs on tui-lipan's fixed 2-8 worker pool, so sleeping inside one holds a
//! worker for the whole delay. A couple of recurring ticks (a clock segment, a picker refresh) are
//! enough to park every worker on a low-core machine and stall unrelated background work behind
//! them — session discovery, config reload, workbar commands.
//!
//! `Command::after` / `CommandLink::send_after` wait on a shared timer thread instead. This guards
//! the invariant, because the sleeping form looks perfectly reasonable in review.

use std::path::Path;

use crate::source_scan::{production_lines, rust_sources};

/// Lines to look ahead from a `Command::spawn(` for a sleep in its body.
const WINDOW: usize = 6;

#[test]
fn no_command_spawn_sleeps_on_a_pool_worker() {
    let mut sources = Vec::new();
    rust_sources(Path::new("src"), &mut sources);
    assert!(!sources.is_empty(), "expected to find crate sources");

    let mut offenders = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).expect("read source");
        // Unit tests legitimately sleep to drive timing, so their modules are skipped.
        let lines = production_lines(&path, &text);
        let end = lines.len();
        for (index, line) in lines.iter().enumerate() {
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
