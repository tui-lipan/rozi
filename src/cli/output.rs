//! Human-facing report formatting, shared by every subcommand that prints a table.
//!
//! JSON forms, publish/subscribe streams, and the version preamble deliberately bypass this
//! module: those are protocols even when a person sometimes reads them.

use crate::control;

/// ANSI palette for human-facing command output.
///
/// JSON forms, publish/subscribe streams, and the version/protocol preamble deliberately bypass
/// this type: those streams are protocols even when a person sometimes reads them. Reports meant
/// for a terminal share this palette and fall back to plain text when colour was disabled through
/// the standard environment variables.
#[derive(Clone, Copy)]
pub(super) struct OutputStyles {
    /// Whether to emit any styling at all.
    color: bool,
    /// Whether the terminal advertised 24-bit colour, so the palette can be sent exactly rather
    /// than approximated into the 256-colour cube.
    truecolor: bool,
}

impl OutputStyles {
    pub(super) const fn plain() -> Self {
        Self {
            color: false,
            truecolor: false,
        }
    }

    /// A fully styled instance, for tests that assert the coloured form directly rather than
    /// depending on the ambient terminal.
    #[cfg(test)]
    pub(super) const fn colored() -> Self {
        Self {
            color: true,
            truecolor: true,
        }
    }

    pub(super) fn detect() -> Self {
        if crate::platform::ansi::stdout_supports_color() {
            Self {
                color: true,
                truecolor: crate::platform::ansi::supports_truecolor(),
            }
        } else {
            Self::plain()
        }
    }

    /// The rozi palette colour for a tone, so `--help`, `update`, and the install scripts all
    /// describe rozi with the colours the app and the logo use.
    fn color_for(tone: OutputTone) -> Option<crate::platform::ansi::Rgb> {
        use crate::platform::ansi::palette;
        match tone {
            OutputTone::Plain => None,
            OutputTone::Accent => Some(palette::ROSE),
            OutputTone::Heading => Some(palette::VIOLET),
            OutputTone::Success => Some(palette::SUCCESS),
            OutputTone::Warning => Some(palette::WARNING),
            OutputTone::Error => Some(palette::ERROR),
            OutputTone::Muted => Some(palette::LAVENDER),
        }
    }

    pub(super) fn paint(self, text: &str, tone: OutputTone) -> String {
        match Self::color_for(tone).filter(|_| self.color) {
            Some(color) => format!(
                "{}{text}{}",
                crate::platform::ansi::fg(color, self.truecolor),
                crate::platform::ansi::RESET
            ),
            None => text.to_string(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum OutputTone {
    Plain,
    Accent,
    Heading,
    Success,
    Warning,
    Error,
    Muted,
}

pub(super) struct TableCell {
    text: String,
    tone: OutputTone,
}

impl TableCell {
    pub(super) fn new(text: impl Into<String>, tone: OutputTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }

    pub(super) fn plain(text: impl Into<String>) -> Self {
        Self::new(text, OutputTone::Plain)
    }
}

/// Format a compact table without tabs, whose terminal tab stops make short rows look ragged.
///
/// Widths are measured before SGR is added and with Unicode display width rather than byte length,
/// so coloured and non-ASCII values align identically. The final column has no trailing padding.
pub(super) fn format_table(
    headers: &[&str],
    rows: &[Vec<TableCell>],
    styles: OutputStyles,
) -> String {
    use unicode_width::UnicodeWidthStr;

    let mut widths: Vec<usize> = headers.iter().map(|header| header.width()).collect();
    for row in rows {
        debug_assert_eq!(row.len(), headers.len());
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.text.width());
        }
    }

    let mut out = String::new();
    let append_row = |out: &mut String, cells: Vec<TableCell>| {
        for (index, cell) in cells.into_iter().enumerate() {
            let width = cell.text.width();
            out.push_str(&styles.paint(&cell.text, cell.tone));
            if index + 1 < headers.len() {
                out.push_str(&" ".repeat(widths[index] - width + 2));
            }
        }
        out.push('\n');
    };
    append_row(
        &mut out,
        headers
            .iter()
            .map(|header| TableCell::new(*header, OutputTone::Heading))
            .collect(),
    );
    for row in rows {
        append_row(
            &mut out,
            row.iter()
                .map(|cell| TableCell::new(cell.text.clone(), cell.tone))
                .collect(),
        );
    }
    out
}

pub(super) fn value_string<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(serde_json::Value::as_str)
}

pub(super) fn value_u64(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key).and_then(serde_json::Value::as_u64)
}

pub(super) fn pane_status_tone(status: &str) -> OutputTone {
    match status {
        "running" | "ready" | "idle" | "done" => OutputTone::Success,
        "working" | "starting" | "busy" => OutputTone::Warning,
        "blocked" | "failed" | "exited" | "error" => OutputTone::Error,
        _ => OutputTone::Plain,
    }
}

pub(super) fn format_panes_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let panes = data
        .and_then(serde_json::Value::as_array)
        .map(|panes| panes.as_slice())
        .unwrap_or_default();
    if panes.is_empty() {
        return format!("{}\n", styles.paint("No panes found.", OutputTone::Muted));
    }

    let session = panes.first().and_then(|pane| value_string(pane, "session"));
    let rows = panes
        .iter()
        .map(|pane| {
            let reported = value_string(pane, "reported_status");
            let agent_state = value_string(pane, "agent_state");
            let state = reported.or(agent_state).unwrap_or("—");
            let size = value_string(pane, "status").unwrap_or("—");
            let command = value_string(pane, "foreground_program")
                .map(str::to_string)
                .or_else(|| value_string(pane, "command").map(str::to_string))
                .or_else(|| {
                    pane.get("argv")
                        .and_then(serde_json::Value::as_array)
                        .map(|argv| {
                            argv.iter()
                                .filter_map(serde_json::Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                })
                .filter(|command| !command.is_empty())
                .unwrap_or_else(|| "—".to_string());
            vec![
                TableCell::new(
                    value_u64(pane, "id")
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                    OutputTone::Accent,
                ),
                TableCell::plain(
                    value_u64(pane, "workspace")
                        .map(|workspace| {
                            if workspace == 0 {
                                "scratch".to_string()
                            } else {
                                workspace.to_string()
                            }
                        })
                        .unwrap_or_else(|| "—".to_string()),
                ),
                TableCell::plain(value_string(pane, "title").unwrap_or("—")),
                TableCell::new(state, pane_status_tone(state)),
                TableCell::plain(value_string(pane, "agent").unwrap_or("—")),
                TableCell::plain(size),
                TableCell::plain(command),
            ]
        })
        .collect::<Vec<_>>();
    let table = format_table(
        &[
            "ID",
            "WORKSPACE",
            "TITLE",
            "STATE",
            "AGENT",
            "SIZE",
            "COMMAND",
        ],
        &rows,
        styles,
    );
    match session {
        Some(session) => format!(
            "{}  {}\n\n{table}",
            styles.paint("Session", OutputTone::Muted),
            styles.paint(session, OutputTone::Accent)
        ),
        None => table,
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(super) fn format_micros(micros: u64) -> String {
    if micros < 1_000 {
        format!("{micros} µs")
    } else if micros < 1_000_000 {
        format!("{:.1} ms", micros as f64 / 1_000.0)
    } else {
        format!("{:.1} s", micros as f64 / 1_000_000.0)
    }
}

pub(super) fn metric_row(
    label: &str,
    metric: Option<&serde_json::Value>,
    detail: String,
) -> Vec<TableCell> {
    let bytes = |key| {
        metric
            .and_then(|value| value_u64(value, key))
            .map(format_bytes)
            .unwrap_or_else(|| "—".to_string())
    };
    vec![
        TableCell::new(label, OutputTone::Accent),
        TableCell::plain(bytes("current_bytes")),
        TableCell::plain(bytes("high_water_bytes")),
        TableCell::plain(bytes("capacity_bytes")),
        TableCell::plain(detail),
    ]
}

pub(super) fn count_detail(
    metric: Option<&serde_json::Value>,
    key: &str,
    singular: &str,
    plural: &str,
) -> String {
    let Some(count) = metric.and_then(|value| value_u64(value, key)) else {
        return "—".to_string();
    };
    format!("{count} {}", if count == 1 { singular } else { plural })
}

pub(super) fn format_metrics_text(
    data: Option<&serde_json::Value>,
    styles: OutputStyles,
) -> String {
    let Some(metrics) = data else {
        return format!(
            "{}\n",
            styles.paint("Metrics unavailable.", OutputTone::Warning)
        );
    };
    let inbound = metrics
        .get("client_inbound")
        .filter(|value| !value.is_null());
    let outbound = metrics
        .get("client_outbound")
        .filter(|value| !value.is_null());
    let pipe = metrics.get("piped_remote").filter(|value| !value.is_null());
    let orphan = metrics.get("orphan_output");
    let server = metrics.get("server").filter(|value| !value.is_null());
    let pty = server.and_then(|value| value.get("pty_ingress"));
    let server_out = server.and_then(|value| value.get("client_outboxes"));
    let resurrection = server.and_then(|value| value.get("resurrection"));
    let server_age = server
        .and_then(|value| value_u64(value, "age_ms"))
        .map(|age| {
            let stale = server
                .and_then(|value| value.get("stale"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            format!("{age} ms · {}", if stale { "stale" } else { "fresh" })
        })
        .unwrap_or_else(|| "—".to_string());

    let mut rows = vec![
        metric_row(
            "Client in",
            inbound,
            count_detail(inbound, "queued_items", "item", "items"),
        ),
        metric_row(
            "Client out",
            outbound,
            count_detail(outbound, "queued_items", "item", "items"),
        ),
        metric_row("Remote pipe", pipe, "—".to_string()),
        metric_row(
            "Orphan output",
            orphan,
            count_detail(orphan, "keys", "key", "keys"),
        ),
        metric_row(
            "PTY ingress",
            pty,
            if server.is_some() {
                server_age
            } else {
                "—".to_string()
            },
        ),
        metric_row(
            "Server out",
            server_out,
            count_detail(server_out, "clients", "client", "clients"),
        ),
    ];
    if let Some(snapshot) = resurrection {
        let attempts = value_u64(snapshot, "attempts").unwrap_or(0);
        let successes = value_u64(snapshot, "successes").unwrap_or(0);
        let failures = value_u64(snapshot, "failures").unwrap_or(0);
        rows.push(vec![
            TableCell::new("Snapshot total", OutputTone::Accent),
            TableCell::plain(format_micros(
                value_u64(snapshot, "last_duration_us").unwrap_or(0),
            )),
            TableCell::plain(format_micros(
                value_u64(snapshot, "max_duration_us").unwrap_or(0),
            )),
            TableCell::plain("—"),
            TableCell::new(
                format!("{attempts} runs · {successes} ok · {failures} failed"),
                if failures == 0 {
                    OutputTone::Success
                } else {
                    OutputTone::Error
                },
            ),
        ]);
        rows.push(vec![
            TableCell::new("Snapshot block", OutputTone::Accent),
            TableCell::plain(format_micros(
                value_u64(snapshot, "last_blocking_us").unwrap_or(0),
            )),
            TableCell::plain(format_micros(
                value_u64(snapshot, "max_blocking_us").unwrap_or(0),
            )),
            TableCell::plain("—"),
            TableCell::plain(format!(
                "{} exported · {} reused",
                value_u64(snapshot, "last_exported_panes").unwrap_or(0),
                value_u64(snapshot, "last_reused_panes").unwrap_or(0)
            )),
        ]);
    }
    format_table(
        &["RESOURCE", "CURRENT", "PEAK", "CAPACITY", "DETAIL"],
        &rows,
        styles,
    )
}

pub(super) fn format_capture_text(data: Option<&serde_json::Value>) -> String {
    let text = data
        .and_then(|value| value_string(value, "text"))
        .unwrap_or("");
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

pub(super) fn format_control_text(
    command: &control::ControlCommand,
    response: &serde_json::Value,
    styles: OutputStyles,
) -> String {
    let data = response.get("data");
    match command {
        control::ControlCommand::ListPanes => format_panes_text(data, styles),
        control::ControlCommand::Metrics => format_metrics_text(data, styles),
        control::ControlCommand::CapturePane { .. } => format_capture_text(data),
        control::ControlCommand::NewPane { .. } => {
            let id = data.and_then(|value| value_u64(value, "id"));
            let ready = data
                .and_then(|value| value.get("pty_ready"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            match id {
                Some(id) => format!(
                    "{}  {}\n",
                    styles.paint(&format!("Pane {id}"), OutputTone::Accent),
                    styles.paint(
                        if ready { "ready" } else { "starting" },
                        if ready {
                            OutputTone::Success
                        } else {
                            OutputTone::Warning
                        }
                    )
                ),
                None => format!("{}\n", styles.paint("OK", OutputTone::Success)),
            }
        }
        _ => format!("{}\n", styles.paint("OK", OutputTone::Success)),
    }
}

pub(super) fn style_first_line(text: String, tone: OutputTone, styles: OutputStyles) -> String {
    let Some((first, rest)) = text.split_once('\n') else {
        return styles.paint(&text, tone);
    };
    format!("{}\n{rest}", styles.paint(first, tone))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_reports_have_human_tables_text_and_acknowledgements() {
        let panes = serde_json::json!({
            "ok": true,
            "data": [
                {
                    "session": "dev",
                    "id": 3,
                    "title": "tests",
                    "workspace": 2,
                    "command": "cargo test",
                    "argv": null,
                    "foreground_program": "cargo",
                    "foreground_arguments": ["test"],
                    "cwd": "/repo",
                    "status": "80×24",
                    "reported_status": "working",
                    "status_reason": "suite",
                    "agent": "cursor",
                    "agent_state": "idle"
                }
            ]
        });
        assert_eq!(
            format_control_text(
                &control::ControlCommand::ListPanes,
                &panes,
                OutputStyles::plain()
            ),
            "Session  dev\n\n\
             ID  WORKSPACE  TITLE  STATE    AGENT   SIZE   COMMAND\n\
             3   2          tests  working  cursor  80×24  cargo\n"
        );

        let capture = serde_json::json!({
            "ok": true,
            "data": {"id": 3, "text": "one\ntwo", "title": "tests"}
        });
        assert_eq!(
            format_control_text(
                &control::ControlCommand::CapturePane {
                    target: Some(3),
                    scrollback: None
                },
                &capture,
                OutputStyles::plain()
            ),
            "one\ntwo\n"
        );

        let ack = serde_json::json!({"ok": true});
        assert_eq!(
            format_control_text(
                &control::ControlCommand::Focus { target: 3 },
                &ack,
                OutputStyles::plain()
            ),
            "OK\n"
        );
    }

    #[test]
    fn runtime_metrics_report_summarizes_resources_without_dumping_json() {
        let response = serde_json::json!({
            "ok": true,
            "data": {
                "sampled_at_unix_ms": 1000,
                "client_inbound": {
                    "current_bytes": 0,
                    "high_water_bytes": 4096,
                    "capacity_bytes": 8388608,
                    "queued_items": 0
                },
                "client_outbound": null,
                "piped_remote": null,
                "orphan_output": {
                    "current_bytes": 12,
                    "high_water_bytes": 2048,
                    "capacity_bytes": 4194304,
                    "keys": 1,
                    "capacity_keys": 4096
                },
                "server": null
            }
        });
        let rendered = format_control_text(
            &control::ControlCommand::Metrics,
            &response,
            OutputStyles::plain(),
        );
        assert!(rendered.starts_with("RESOURCE"));
        assert!(rendered.contains("Client in"));
        assert!(rendered.contains("4.0 KiB"));
        assert!(rendered.contains("Orphan output"));
        assert!(rendered.contains("1 key"));
        assert!(!rendered.contains("sampled_at_unix_ms"));
        assert!(rendered.lines().all(|line| !line.ends_with(' ')));
    }
}
