//! `rozi layout ...` and `rozi pane ...`: reading and changing the shared layout.

use crate::control::{CellRect, ControlCommand, ControlLayoutKind, FractionRect};
use crate::state::PaneId;

use super::{ListFormat, parse_list_format, require_value};

type Parsed = std::result::Result<(ControlCommand, Option<ListFormat>), String>;

/// `layout get [--workspace <1-9>]` and `layout set --workspace <1-9> <LAYOUT> [--if-revision <N>]`.
pub(super) fn parse_layout_args(iter: &mut impl Iterator<Item = String>) -> Parsed {
    let subcommand = iter
        .next()
        .ok_or_else(|| "layout requires a subcommand (get or set)".to_string())?;
    let command = match subcommand.as_str() {
        "get" => "layout get",
        "set" => "layout set",
        other => {
            return Err(format!(
                "unknown layout subcommand `{other}`; expected get or set"
            ));
        }
    };
    let mut flags = Flags::new(command);
    let mut layout = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" => flags.workspace(iter)?,
            "--if-revision" if command == "layout set" => flags.if_revision(iter)?,
            "--format" => flags.format(iter)?,
            value if command == "layout set" && !value.starts_with('-') && layout.is_none() => {
                layout = Some(ControlLayoutKind::parse(value).ok_or_else(|| {
                    format!(
                        "unknown layout `{value}`; expected dwindle, master, grid, columns, rows, scrollable, or monocle"
                    )
                })?);
            }
            other => return Err(format!("unexpected argument `{other}` after {command}")),
        }
    }
    let Flags {
        workspace,
        if_revision,
        format,
        ..
    } = flags;
    if command == "layout get" {
        return Ok((ControlCommand::LayoutGet { workspace }, format));
    }
    let workspace = workspace.ok_or_else(|| {
        "layout set requires --workspace; a script names the workspace it reshapes".to_string()
    })?;
    let layout = layout.ok_or_else(|| {
        "layout set requires a layout: dwindle, master, grid, columns, rows, scrollable, or monocle"
            .to_string()
    })?;
    Ok((
        ControlCommand::LayoutSet {
            workspace,
            layout,
            if_revision,
        },
        format,
    ))
}

/// `pane set|move|swap|close --target <ID> ...`.
///
/// `set` takes `[--floating B] [--fullscreen B] [--rect X,Y,W,H] [--rect-fraction X,Y,W,H]`,
/// `move` takes `--workspace <1-9>`, and `swap` takes `--with <ID>`. Every one takes
/// `[--if-revision N]`.
pub(super) fn parse_pane_args(iter: &mut impl Iterator<Item = String>) -> Parsed {
    let subcommand = iter
        .next()
        .ok_or_else(|| "pane requires a subcommand (set, move, swap, or close)".to_string())?;
    let command = match subcommand.as_str() {
        "set" => "pane set",
        "move" => "pane move",
        "swap" => "pane swap",
        "close" => "pane close",
        other => {
            return Err(format!(
                "unknown pane subcommand `{other}`; expected set, move, swap, or close"
            ));
        }
    };
    let mut flags = Flags::new(command);
    let mut target: Option<PaneId> = None;
    let mut with: Option<PaneId> = None;
    let mut floating = None;
    let mut fullscreen = None;
    let mut rect = None;
    let mut rect_fraction = None;
    let setting = command == "pane set";
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => once(&mut target, parse_pane_id(iter, "--target")?, "--target")?,
            "--with" if command == "pane swap" => {
                once(&mut with, parse_pane_id(iter, "--with")?, "--with")?
            }
            "--workspace" if command == "pane move" => flags.workspace(iter)?,
            "--floating" if setting => {
                once(&mut floating, parse_bool(iter, "--floating")?, "--floating")?
            }
            "--fullscreen" if setting => once(
                &mut fullscreen,
                parse_bool(iter, "--fullscreen")?,
                "--fullscreen",
            )?,
            "--rect" if setting => once(&mut rect, parse_cell_rect(iter)?, "--rect")?,
            "--rect-fraction" if setting => once(
                &mut rect_fraction,
                parse_fraction_rect(iter)?,
                "--rect-fraction",
            )?,
            "--if-revision" => flags.if_revision(iter)?,
            "--format" => flags.format(iter)?,
            other => return Err(format!("unexpected argument `{other}` after {command}")),
        }
    }
    // Always explicit, on every endpoint: a layout write aimed by an inherited `ROZI_PANE` or by
    // focus would reshape whichever pane happened to be nearest.
    let target = target.ok_or_else(|| format!("{command} requires --target <PANE_ID>"))?;
    let if_revision = flags.if_revision;
    let command = match command {
        "pane set" => ControlCommand::PaneSet {
            target,
            floating,
            fullscreen,
            rect,
            rect_fraction,
            if_revision,
        },
        "pane move" => ControlCommand::PaneMove {
            target,
            workspace: flags
                .workspace
                .ok_or_else(|| "pane move requires --workspace <1-9>".to_string())?,
            if_revision,
        },
        "pane swap" => ControlCommand::PaneSwap {
            target,
            with: with.ok_or_else(|| "pane swap requires --with <PANE_ID>".to_string())?,
            if_revision,
        },
        _ => ControlCommand::PaneClose {
            target,
            if_revision,
        },
    };
    Ok((command, flags.format))
}

fn parse_pane_id(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<PaneId, String> {
    require_value(iter, &format!("{flag} requires a pane id"))?
        .parse()
        .map_err(|_| format!("{flag} requires a numeric pane id"))
}

/// Options shared by the layout commands, each accepted once.
struct Flags {
    command: &'static str,
    workspace: Option<usize>,
    if_revision: Option<u64>,
    format: Option<ListFormat>,
}

impl Flags {
    fn new(command: &'static str) -> Self {
        Self {
            command,
            workspace: None,
            if_revision: None,
            format: None,
        }
    }

    fn workspace(&mut self, iter: &mut impl Iterator<Item = String>) -> Result<(), String> {
        let value = require_value(iter, "--workspace requires a workspace number")?;
        let index = value
            .parse::<usize>()
            .ok()
            .filter(|index| (1..=crate::state::WORKSPACE_COUNT).contains(index))
            .ok_or_else(|| {
                format!(
                    "--workspace requires a workspace number from 1 to {}",
                    crate::state::WORKSPACE_COUNT
                )
            })?;
        once(&mut self.workspace, index, "--workspace")
            .map_err(|error| format!("{} {error}", self.command))
    }

    fn if_revision(&mut self, iter: &mut impl Iterator<Item = String>) -> Result<(), String> {
        let value = require_value(iter, "--if-revision requires a revision number")?;
        let revision = value
            .parse()
            .map_err(|_| "--if-revision requires a revision number".to_string())?;
        once(&mut self.if_revision, revision, "--if-revision")
            .map_err(|error| format!("{} {error}", self.command))
    }

    fn format(&mut self, iter: &mut impl Iterator<Item = String>) -> Result<(), String> {
        let value = require_value(iter, "--format requires text or json")?;
        let format = parse_list_format(&value, self.command)?;
        once(&mut self.format, format, "--format")
            .map_err(|error| format!("{} {error}", self.command))
    }
}

fn once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{flag} specified more than once"));
    }
    Ok(())
}

fn parse_bool(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<bool, String> {
    match require_value(iter, &format!("{flag} requires true or false"))?.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{flag} requires true or false, not `{other}`")),
    }
}

fn four_values(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<Vec<String>, String> {
    // Read directly rather than through `require_value`, which refuses anything starting with `-`:
    // a float may start left of the canvas, so `-2,3,40,12` is a rect, not a flag.
    let value = iter
        .next()
        .ok_or_else(|| format!("{flag} requires X,Y,W,H"))?;
    let parts: Vec<String> = value
        .split(',')
        .map(|part| part.trim().to_string())
        .collect();
    if parts.len() != 4 {
        return Err(format!(
            "{flag} requires four comma-separated values: X,Y,W,H"
        ));
    }
    Ok(parts)
}

fn parse_cell_rect(iter: &mut impl Iterator<Item = String>) -> Result<CellRect, String> {
    let parts = four_values(iter, "--rect")?;
    let invalid = || "--rect requires whole cells: X,Y,W,H with a positive W and H".to_string();
    Ok(CellRect {
        x: parts[0].parse().map_err(|_| invalid())?,
        y: parts[1].parse().map_err(|_| invalid())?,
        width: parts[2].parse().map_err(|_| invalid())?,
        height: parts[3].parse().map_err(|_| invalid())?,
    })
}

fn parse_fraction_rect(iter: &mut impl Iterator<Item = String>) -> Result<FractionRect, String> {
    let parts = four_values(iter, "--rect-fraction")?;
    let parse = |part: &str| {
        part.parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| "--rect-fraction requires numbers: X,Y,W,H".to_string())
    };
    Ok(FractionRect {
        x: parse(&parts[0])?,
        y: parse(&parts[1])?,
        width: parse(&parts[2])?,
        height: parse(&parts[3])?,
    })
}
