//! Human-facing report formatting, shared by every subcommand that prints a table.
//!
//! JSON forms, publish/subscribe streams, and the version preamble deliberately bypass this
//! module: those are protocols even when a person sometimes reads them.

use crate::control;
use crate::platform::ansi::{Role, RoleColors};

/// Write a finished report to stdout, treating a closed reader as a normal end.
///
/// Rust ignores `SIGPIPE`, so `println!` *panics* once the other side of a pipe is gone — piping any
/// of these reports into `head` would print a backtrace instead of the first lines. Unix convention
/// is to stop quietly, which is what exiting here does. Callers therefore build a whole report and
/// hand it over in one piece rather than printing line by line.
pub(super) fn print_or_stop(text: &str) {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if out.write_all(text.as_bytes()).is_err() || out.flush().is_err() {
        std::process::exit(0);
    }
}

/// ANSI palette for human-facing command output.
///
/// JSON forms, publish/subscribe streams, and the version/protocol preamble deliberately bypass
/// this type: those streams are protocols even when a person sometimes reads them. Reports meant
/// for a terminal share this palette and fall back to plain text when colour was disabled through
/// the standard environment variables. Inside a rozi pane the colours are theme-relative; see
/// [`RoleColors`].
#[derive(Clone, Copy)]
pub(super) struct OutputStyles {
    /// Whether to emit any styling at all.
    color: bool,
    colors: RoleColors,
}

impl OutputStyles {
    pub(super) const fn plain() -> Self {
        Self {
            color: false,
            colors: RoleColors::Brand { truecolor: false },
        }
    }

    /// A fully styled instance, for tests that assert the coloured form directly rather than
    /// depending on the ambient terminal.
    #[cfg(test)]
    pub(super) const fn colored() -> Self {
        Self {
            color: true,
            colors: RoleColors::Brand { truecolor: true },
        }
    }

    /// The styling a command run inside a rozi pane gets, for tests.
    #[cfg(test)]
    pub(super) const fn in_pane() -> Self {
        Self {
            color: true,
            colors: RoleColors::PaneTheme,
        }
    }

    pub(super) fn detect() -> Self {
        if crate::platform::ansi::stdout_supports_color() {
            Self {
                color: true,
                colors: RoleColors::detect(),
            }
        } else {
            Self::plain()
        }
    }

    pub(super) fn paint(self, text: &str, tone: OutputTone) -> String {
        let role = match tone {
            OutputTone::Plain => None,
            OutputTone::Accent => Some(Role::Accent),
            OutputTone::Heading => Some(Role::Heading),
            OutputTone::Key => Some(Role::Key),
            OutputTone::Success => Some(Role::Success),
            OutputTone::Warning => Some(Role::Warning),
            OutputTone::Error => Some(Role::Error),
            OutputTone::Muted => Some(Role::Muted),
        };
        match role.filter(|_| self.color) {
            Some(role) => format!(
                "{}{text}{}",
                self.colors.sgr(role),
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
    /// A section title or table column header, styled like the headings in `--help`.
    Heading,
    /// The first column of a table row, which names the row.
    Key,
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
                    OutputTone::Key,
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

fn format_cell_rect(rect: Option<&serde_json::Value>) -> String {
    let Some(rect) = rect.filter(|rect| rect.is_object()) else {
        return "—".to_string();
    };
    let field = |key: &str| {
        rect.get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    format!(
        "{},{} {}×{}",
        field("x"),
        field("y"),
        field("width"),
        field("height")
    )
}

/// `layout get` as a header naming the session and canvas, then one row per placed pane.
///
/// Empty workspaces are left out of the table; the JSON keeps all of them, with their layouts,
/// for a script that wants to know what a workspace would tile as.
pub(super) fn format_layout_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let Some(report) = data else {
        return format!("{}\n", styles.paint("No layout.", OutputTone::Muted));
    };
    let mut out = String::new();
    let mut header = vec![
        styles.paint("Session", OutputTone::Muted),
        styles.paint(
            value_string(report, "session").unwrap_or("—"),
            OutputTone::Accent,
        ),
    ];
    match value_u64(report, "revision") {
        Some(revision) => {
            header.push(styles.paint("revision", OutputTone::Muted));
            header.push(revision.to_string());
        }
        None => header.push(styles.paint("no layout yet", OutputTone::Warning)),
    }
    if let Some(canvas) = report.get("canvas").filter(|canvas| canvas.is_object()) {
        header.push(styles.paint("canvas", OutputTone::Muted));
        header.push(format!(
            "{}×{}",
            value_u64(canvas, "cols").unwrap_or(0),
            value_u64(canvas, "rows").unwrap_or(0)
        ));
    }
    out.push_str(&header.join("  "));
    out.push('\n');

    let client = report.get("client").filter(|client| client.is_object());
    if let Some(client) = client {
        let mut line = vec![
            styles.paint("Client", OutputTone::Muted),
            format!(
                "workspace {}",
                value_u64(client, "active_workspace").unwrap_or(0)
            ),
        ];
        if let Some(focused) = value_u64(client, "focused_pane") {
            line.push(format!("focus {focused}"));
        }
        let flag = |key: &str| client.get(key).and_then(serde_json::Value::as_bool);
        line.push(if flag("controller") == Some(true) {
            "controller".to_string()
        } else {
            "follower".to_string()
        });
        if flag("committed") == Some(false) {
            line.push(styles.paint("uncommitted changes", OutputTone::Warning));
        }
        out.push_str(&line.join("  "));
        out.push('\n');
    }

    let unplaced: Vec<String> = report
        .get("unplaced_panes")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_u64)
                .map(|id| id.to_string())
                .collect()
        })
        .unwrap_or_default();
    if !unplaced.is_empty() {
        out.push_str(&format!(
            "{}  {}\n",
            styles.paint("Unplaced", OutputTone::Muted),
            styles.paint(&unplaced.join(", "), OutputTone::Warning)
        ));
    }

    let workspaces = report
        .get("workspaces")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    out.push('\n');
    out.push_str(&format_layout_table(workspaces, client.is_some(), styles));
    out
}

/// One row per pane across `workspaces`, with a `VIEW` column when a UI answered. Empty
/// workspaces are left out; the JSON keeps them.
fn format_layout_table(
    workspaces: &[serde_json::Value],
    with_view: bool,
    styles: OutputStyles,
) -> String {
    let mut rows = Vec::new();
    for workspace in workspaces {
        let index = value_u64(workspace, "index")
            .map(|index| index.to_string())
            .unwrap_or_else(|| "—".to_string());
        let layout = value_string(workspace, "layout").unwrap_or("—");
        for pane in workspace
            .get("panes")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let flag = |key: &str| pane.get(key).and_then(serde_json::Value::as_bool) == Some(true);
            let mut mode = if flag("floating") {
                "floating"
            } else {
                "tiled"
            }
            .to_string();
            if flag("fullscreen") {
                mode.push_str(", fullscreen");
            }
            let mut row = vec![
                TableCell::plain(index.clone()),
                TableCell::plain(layout),
                TableCell::new(
                    value_u64(pane, "id")
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                    OutputTone::Key,
                ),
                TableCell::plain(
                    value_u64(pane, "order")
                        .map(|order| order.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                ),
                TableCell::plain(mode),
                TableCell::plain(format_cell_rect(pane.get("rect"))),
            ];
            if with_view {
                row.push(TableCell::plain(format_cell_rect(pane.get("view_rect"))));
            }
            rows.push(row);
        }
    }
    if rows.is_empty() {
        return format!("{}\n", styles.paint("No panes placed.", OutputTone::Muted));
    }
    let mut headers = vec!["WS", "LAYOUT", "PANE", "ORDER", "MODE", "RECT"];
    if with_view {
        headers.push("VIEW");
    }
    format_table(&headers, &rows, styles)
}

/// `pane close`: which pane went, then the workspace it left, when a layout placed it.
fn format_pane_closed_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let Some(closed) = data else {
        return format!("{}\n", styles.paint("OK", OutputTone::Success));
    };
    let mut line = vec![styles.paint(
        &format!(
            "Closed pane {}",
            value_u64(closed, "id").map_or_else(|| "—".to_string(), |id| id.to_string())
        ),
        OutputTone::Success,
    )];
    if let Some(revision) = value_u64(closed, "revision") {
        line.push(styles.paint("revision", OutputTone::Muted));
        line.push(revision.to_string());
    }
    if closed.get("committed").and_then(serde_json::Value::as_bool) == Some(false) {
        line.push(styles.paint("not yet confirmed by the server", OutputTone::Warning));
    }
    let mut out = format!("{}\n", line.join("  "));
    if let Some(workspace) = closed
        .get("workspace")
        .filter(|workspace| workspace.is_object())
    {
        let with_view = workspace
            .get("panes")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|panes| panes.iter().any(|pane| pane.get("view_rect").is_some()));
        out.push('\n');
        out.push_str(&format_layout_table(
            std::slice::from_ref(workspace),
            with_view,
            styles,
        ));
    }
    out
}

/// `layout set` and the `pane` writes: what happened, then the workspace as it now stands.
pub(super) fn format_layout_change_text(
    data: Option<&serde_json::Value>,
    styles: OutputStyles,
) -> String {
    let Some(change) = data else {
        return format!("{}\n", styles.paint("OK", OutputTone::Success));
    };
    let flag = |key: &str| change.get(key).and_then(serde_json::Value::as_bool);
    let mut line = vec![if flag("changed") == Some(true) {
        styles.paint("Changed", OutputTone::Success)
    } else {
        styles.paint("No change", OutputTone::Muted)
    }];
    if let Some(revision) = value_u64(change, "revision") {
        line.push(styles.paint("revision", OutputTone::Muted));
        line.push(revision.to_string());
    }
    if flag("committed") == Some(false) {
        line.push(styles.paint("not yet confirmed by the server", OutputTone::Warning));
    }
    let workspace = change.get("workspace").cloned().unwrap_or_default();
    let with_view = workspace
        .get("panes")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|panes| panes.iter().any(|pane| pane.get("view_rect").is_some()));
    format!(
        "{}\n\n{}",
        line.join("  "),
        format_layout_table(std::slice::from_ref(&workspace), with_view, styles)
    )
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
        TableCell::new(label, OutputTone::Key),
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
    let attach_seed = server.and_then(|value| value.get("attach_seed"));
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
    if let Some(seed) = attach_seed {
        rows.push(vec![
            TableCell::new("Attach seed", OutputTone::Key),
            TableCell::plain(format_bytes(value_u64(seed, "queued_bytes").unwrap_or(0))),
            TableCell::plain(format_bytes(
                value_u64(seed, "peak_queued_bytes").unwrap_or(0),
            )),
            TableCell::plain(format_bytes(
                value_u64(seed, "send_window_bytes").unwrap_or(0),
            )),
            TableCell::plain(format!(
                "{} active · {} panes",
                value_u64(seed, "active_clients").unwrap_or(0),
                value_u64(seed, "panes_remaining").unwrap_or(0),
            )),
        ]);
        rows.push(vec![
            TableCell::new("Attach catch-up", OutputTone::Key),
            TableCell::plain(format_bytes(
                value_u64(seed, "live_catch_up_bytes").unwrap_or(0),
            )),
            TableCell::plain(format_bytes(
                value_u64(seed, "peak_live_catch_up_bytes").unwrap_or(0),
            )),
            TableCell::plain(format_bytes(
                value_u64(seed, "live_catch_up_limit_bytes").unwrap_or(0),
            )),
            TableCell::plain(format!(
                "{} complete · {} disconnected",
                value_u64(seed, "completed").unwrap_or(0),
                value_u64(seed, "disconnected").unwrap_or(0),
            )),
        ]);
        rows.push(vec![
            TableCell::new("Attach duration", OutputTone::Key),
            TableCell::plain(format_micros(
                value_u64(seed, "last_duration_us").unwrap_or(0),
            )),
            TableCell::plain(format_micros(
                value_u64(seed, "max_duration_us").unwrap_or(0),
            )),
            TableCell::plain("—"),
            TableCell::plain(format!(
                "{} replayed · {}",
                format_bytes(value_u64(seed, "replay_bytes_total").unwrap_or(0)),
                value_string(seed, "last_disconnect_reason").unwrap_or("no disconnects"),
            )),
        ]);
    }
    if let Some(snapshot) = resurrection {
        let attempts = value_u64(snapshot, "attempts").unwrap_or(0);
        let successes = value_u64(snapshot, "successes").unwrap_or(0);
        let failures = value_u64(snapshot, "failures").unwrap_or(0);
        rows.push(vec![
            TableCell::new("Snapshot total", OutputTone::Key),
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
            TableCell::new("Snapshot block", OutputTone::Key),
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

/// A `spans` capture's frame as one line of JSON, copied as it arrived so that fields this binary
/// does not know survive; `None` for any other render.
pub(super) fn span_frame(data: &serde_json::Value) -> Option<String> {
    if data.get("render").and_then(serde_json::Value::as_str) != Some("spans") {
        return None;
    }
    let frame = data.get("frame")?;
    Some(format!(
        "{}\n",
        serde_json::to_string(frame).unwrap_or_default()
    ))
}

pub(super) fn format_capture_text(data: Option<&serde_json::Value>) -> String {
    if let Some(frame) = data.and_then(span_frame) {
        return frame;
    }
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
        control::ControlCommand::LayoutGet { .. } => format_layout_text(data, styles),
        control::ControlCommand::LayoutSet { .. }
        | control::ControlCommand::PaneSet { .. }
        | control::ControlCommand::PaneMove { .. }
        | control::ControlCommand::PaneSwap { .. } => format_layout_change_text(data, styles),
        control::ControlCommand::PaneClose { .. } => format_pane_closed_text(data, styles),
        control::ControlCommand::AgentsList | control::ControlCommand::AgentGet { .. } => {
            format_agents_text(data, styles)
        }
        control::ControlCommand::AgentRead { .. } => format_capture_text(data),
        control::ControlCommand::AgentWait { .. } => format_agent_wait_text(data, styles),
        control::ControlCommand::AgentPrompt { .. } => {
            if data.is_some_and(|data| data.get("condition").is_some()) {
                format_agent_wait_text(data, styles)
            } else {
                format!(
                    "{}\n",
                    styles.paint("Prompt submitted", OutputTone::Success)
                )
            }
        }
        control::ControlCommand::Metrics => format_metrics_text(data, styles),
        control::ControlCommand::CapturePane { .. }
        | control::ControlCommand::CaptureUi { .. }
        | control::ControlCommand::SendText {
            capture: Some(_), ..
        }
        | control::ControlCommand::SendKeys {
            capture: Some(_), ..
        } => format_capture_text(data),
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
        control::ControlCommand::RecordStart { follow: false, .. } => {
            format_recording_started_text(data, styles)
        }
        control::ControlCommand::RecordStart { follow: true, .. }
        | control::ControlCommand::RecordStop { .. } => format_recording_stopped_text(data, styles),
        control::ControlCommand::RecordList => format_recordings_text(data, styles),
        control::ControlCommand::RecordMark { .. } => {
            let ids = data
                .and_then(|data| data.get("ids"))
                .and_then(serde_json::Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(serde_json::Value::as_u64)
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!(
                "{} {ids}\n",
                styles.paint("Marked recording", OutputTone::Success)
            )
        }
        _ => format!("{}\n", styles.paint("OK", OutputTone::Success)),
    }
}

fn format_recording_started_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let Some(data) = data else {
        return format!("{}\n", styles.paint("OK", OutputTone::Success));
    };
    format!(
        "{}  pane {}  {}\n",
        styles.paint(
            &format!("Recording {}", value_u64(data, "id").unwrap_or_default()),
            OutputTone::Accent
        ),
        value_u64(data, "pane").unwrap_or_default(),
        value_string(data, "path").unwrap_or_default(),
    )
}

fn format_recording_stopped_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let Some(data) = data else {
        return format!("{}\n", styles.paint("OK", OutputTone::Success));
    };
    let reason = value_string(data, "reason").unwrap_or("stopped");
    let dropped = value_u64(data, "dropped").unwrap_or_default();
    let mut out = format!(
        "{}  {}  {} frames  {}  {}{}\n",
        styles.paint(
            &format!("Recording {}", value_u64(data, "id").unwrap_or_default()),
            OutputTone::Accent
        ),
        styles.paint(
            reason,
            if reason == "write-failed" {
                OutputTone::Error
            } else {
                OutputTone::Success
            }
        ),
        value_u64(data, "frames").unwrap_or_default(),
        format_bytes(value_u64(data, "bytes").unwrap_or_default()),
        value_string(data, "path").unwrap_or_default(),
        if dropped > 0 {
            styles.paint(&format!("  {dropped} dropped"), OutputTone::Warning)
        } else {
            String::new()
        },
    );
    if let Some(error) = value_string(data, "error") {
        out.push_str(&format!("{}\n", styles.paint(error, OutputTone::Error)));
    }
    out
}

fn format_recordings_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let rows = data
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        return "No recordings found.\n".to_string();
    }
    let mut out = String::new();
    for row in rows {
        let seconds = value_u64(&row, "elapsed_ms").unwrap_or_default() / 1000;
        out.push_str(&format!(
            "{}  pane {}  {}  {} frames  {}  {}\n",
            styles.paint(
                &value_u64(&row, "id").unwrap_or_default().to_string(),
                OutputTone::Key
            ),
            value_u64(&row, "pane").unwrap_or_default(),
            styles.paint(
                &format!(
                    "{}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                ),
                OutputTone::Muted
            ),
            value_u64(&row, "frames").unwrap_or_default(),
            format_bytes(value_u64(&row, "bytes").unwrap_or_default()),
            value_string(&row, "path").unwrap_or_default(),
        ));
    }
    out
}

fn format_agents_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let agents: Vec<&serde_json::Value> = match data {
        Some(serde_json::Value::Array(agents)) => agents.iter().collect(),
        Some(agent @ serde_json::Value::Object(_)) => vec![agent],
        _ => Vec::new(),
    };
    if agents.is_empty() {
        return "No agents found.\n".to_string();
    }
    let rows = agents
        .into_iter()
        .map(|agent| {
            let text = |field: &str| {
                agent
                    .get(field)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("—")
                    .to_string()
            };
            vec![
                TableCell::plain(text("label")),
                TableCell::plain(
                    agent
                        .get("ref")
                        .and_then(|reference| reference.get("slot"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("—"),
                ),
                TableCell::plain(text("state")),
                TableCell::plain(
                    agent
                        .get("pane")
                        .and_then(serde_json::Value::as_u64)
                        .map_or_else(|| "—".to_string(), |value| value.to_string()),
                ),
                TableCell::plain(
                    agent
                        .get("workspace")
                        .and_then(serde_json::Value::as_u64)
                        .map_or_else(|| "—".to_string(), |value| value.to_string()),
                ),
                TableCell::plain(text("cwd")),
            ]
        })
        .collect::<Vec<_>>();
    format_table(
        &["AGENT", "ACTIVITY", "STATE", "PANE", "WORKSPACE", "CWD"],
        &rows,
        styles,
    )
}

fn format_agent_wait_text(data: Option<&serde_json::Value>, styles: OutputStyles) -> String {
    let condition = data
        .and_then(|data| data.get("condition"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ready");
    format!("{}\n", styles.paint(condition, OutputTone::Success))
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
    fn layout_change_text_says_what_happened_and_shows_the_workspace() {
        let change = serde_json::json!({"ok": true, "data": {
            "changed": true, "revision": 8, "committed": false,
            "workspace": {"index": 2, "name": null, "layout": "grid", "synchronized": false,
                "panes": [{"id": 4, "order": 0, "floating": false, "fullscreen": false,
                           "rect": {"x": 0, "y": 0, "width": 80, "height": 24}}]}
        }});
        let text = format_control_text(
            &control::ControlCommand::LayoutSet {
                workspace: 2,
                layout: Some(control::ControlLayoutKind::Grid),
                master_ratio: None,
                if_revision: None,
            },
            &change,
            OutputStyles::plain(),
        );
        assert!(text.starts_with("Changed  revision  8  not yet confirmed by the server\n"));
        assert!(text.contains("2   grid    4     0      tiled  0,0 80×24"));
        assert!(!text.contains("VIEW"), "no view column without view rects");

        let unchanged = serde_json::json!({"ok": true, "data": {
            "changed": false, "revision": 8, "committed": true,
            "workspace": {"index": 2, "name": null, "layout": "grid", "synchronized": false,
                "panes": []}
        }});
        let text = format_control_text(
            &control::ControlCommand::LayoutSet {
                workspace: 2,
                layout: Some(control::ControlLayoutKind::Grid),
                master_ratio: None,
                if_revision: None,
            },
            &unchanged,
            OutputStyles::plain(),
        );
        assert!(text.starts_with("No change  revision  8\n"));
    }

    #[test]
    fn layout_text_lists_placed_panes_and_the_client_view_only_when_a_ui_answered() {
        let rect = |x: i32, y: i32, width: u32, height: u32| serde_json::json!({"x": x, "y": y, "width": width, "height": height});
        let workspaces = serde_json::json!([
            {"index": 1, "name": null, "layout": "dwindle", "synchronized": false, "panes": [
                {"id": 7, "order": 0, "floating": false, "fullscreen": false,
                 "rect": rect(0, 0, 48, 24), "view_rect": rect(0, 1, 47, 23)},
                {"id": 9, "order": null, "floating": true, "fullscreen": true,
                 "rect": rect(20, 6, 40, 12), "view_rect": rect(0, 0, 80, 25)}
            ]},
            {"index": 2, "name": null, "layout": "grid", "synchronized": false, "panes": []}
        ]);
        let ui = serde_json::json!({"ok": true, "data": {
            "session": "dev", "revision": 12, "canvas": {"cols": 80, "rows": 24},
            "workspaces": workspaces,
            "client": {"active_workspace": 1, "focused_pane": 7, "controller": true,
                       "committed": false, "viewport": {"cols": 80, "rows": 25}}
        }});
        let text = format_control_text(
            &control::ControlCommand::LayoutGet { workspace: None },
            &ui,
            OutputStyles::plain(),
        );
        assert!(text.starts_with("Session  dev  revision  12  canvas  80×24\n"));
        assert!(text.contains("workspace 1  focus 7  controller  uncommitted changes"));
        assert!(text.contains("VIEW"));
        assert!(text.contains("7     0      tiled                 0,0 48×24   0,1 47×23"));
        assert!(text.contains("floating, fullscreen"));
        assert!(
            !text.contains("grid"),
            "empty workspaces stay out of the table"
        );

        let session = serde_json::json!({"ok": true, "data": {
            "session": "dev", "revision": null, "canvas": null,
            "workspaces": [], "unplaced_panes": [2, 5]
        }});
        let text = format_control_text(
            &control::ControlCommand::LayoutGet { workspace: None },
            &session,
            OutputStyles::plain(),
        );
        assert!(text.contains("no layout yet"));
        assert!(text.contains("Unplaced  2, 5"));
        assert!(text.contains("No panes placed."));
        assert!(!text.contains("Client"));
    }

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
                    scrollback: None,
                    render: control::CaptureRender::Text,
                    scale: None,
                    wait: None,
                    image_pixels: false,
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

    #[test]
    fn metrics_text_reports_attach_windows_and_disconnect_reason() {
        let metrics = serde_json::json!({
            "orphan_output": null,
            "server": {
                "age_ms": 2,
                "stale": false,
                "attach_seed": {
                    "active_clients": 1,
                    "queued_bytes": 1048576,
                    "peak_queued_bytes": 4194304,
                    "send_window_bytes": 4194304,
                    "live_catch_up_bytes": 2048,
                    "peak_live_catch_up_bytes": 4096,
                    "live_catch_up_limit_bytes": 8388608,
                    "panes_remaining": 3,
                    "replay_bytes_total": 10485760,
                    "completed": 4,
                    "disconnected": 1,
                    "last_duration_us": 1250,
                    "max_duration_us": 2500,
                    "last_disconnect_reason": "attach-catch-up-overflow"
                }
            }
        });

        let rendered = format_metrics_text(Some(&metrics), OutputStyles::plain());

        assert!(rendered.contains("Attach seed"));
        assert!(rendered.contains("1 active · 3 panes"));
        assert!(rendered.contains("Attach catch-up"));
        assert!(rendered.contains("8.0 MiB"));
        assert!(rendered.contains("Attach duration"));
        assert!(rendered.contains("attach-catch-up-overflow"));
    }

    /// Theme is client-local, but a pane's output is stored once and shared by every client that
    /// shows it. A report printed inside a pane must therefore resolve through whichever theme
    /// renders it: the bytes below are fed once, then drawn under two themes with nothing
    /// rewritten in between.
    #[test]
    fn pane_output_resolves_through_each_clients_theme_without_being_rewritten() {
        use crate::state::ThemePreset;
        use tui_lipan::prelude::{Style, TerminalColorPalette, TerminalScreen, Theme};

        let report = format_table(
            &["NAME", "STATUS"],
            &[vec![
                TableCell::new("dev", OutputTone::Key),
                TableCell::new("running", OutputTone::Success),
            ]],
            OutputStyles::in_pane(),
        );
        assert!(
            !report.contains("38;2;") && !report.contains("38;5;"),
            "pane output must not carry a resolved colour: {report:?}"
        );

        let mut screen = TerminalScreen::new(4, 40, 0);
        screen.process_bytes(report.replace('\n', "\r\n").as_bytes());

        let render = |screen: &mut TerminalScreen, theme: &Theme| {
            screen.set_palette(TerminalColorPalette::from_theme(
                theme,
                theme.surface.backdrop,
            ));
            let snapshot = screen.render_snapshot();
            let style = |text: &str| -> Style {
                snapshot
                    .color_lines
                    .iter()
                    .flatten()
                    .find(|span| span.content.contains(text))
                    .map(|span| span.style)
                    .unwrap_or_else(|| panic!("`{text}` was not rendered"))
            };
            let (header, key, status) = (style("NAME"), style("dev"), style("running"));
            assert_eq!(header.bold, Some(true), "headings stay bold");
            assert_eq!(key.dim, Some(true), "the key column stays faint");
            (
                header.resolved_fg(),
                key.resolved_fg(),
                status.resolved_fg(),
            )
        };

        let dark = ThemePreset::TokyoNight.theme();
        let light = ThemePreset::SolarizedLight.theme();
        let (dark_header, dark_key, dark_status) = render(&mut screen, &dark);
        let (light_header, light_key, light_status) = render(&mut screen, &light);

        assert_ne!(dark_header, light_header);
        assert_ne!(dark_key, light_key);
        assert_eq!(dark_status, Some(dark.status.success));
        assert_eq!(light_status, Some(light.status.success));
    }
}
