//! `rozi record`: record a pane from its session server, and export or play a recording.

use std::path::PathBuf;

use crate::control::ControlCommand;
use crate::recording::export::CONCAT_LISTING;
use crate::recording::play::Seek;

use super::{ControlCli, ListFormat, control_request, parse_list_format, require_value};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[
            row("rozi --session <NAME> record <COMMAND> [OPTIONS]", ""),
            row("rozi record export|play <FILE> [OPTIONS]", ""),
        ],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "Recording runs in the session server and needs --session <NAME>;\n    \
               export and play read a file and need no session.",
        rows: &[
            row(
                "start [pane] --target <PANE> --output <FILE>",
                "Start recording a pane",
            ),
            row(
                "pane --target <PANE> --output <FILE>",
                "Record until Ctrl-C",
            ),
            row("list [--format text|json]", "List running recordings"),
            row("mark <TEXT> [--id <ID>]", "Label this moment"),
            row("stop [--id <ID>]", "Stop a recording"),
            row(
                "export <FILE> --to png-frames <DIR>",
                "PNG frames and an ffmpeg listing",
            ),
            row("export <FILE> --to cast <OUT>", "An asciinema cast"),
            row(
                "play <FILE> [--speed <N>] [--from <MARK|TIME>]",
                "Replay in this terminal",
            ),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("    --target <PANE>", "The pane to record"),
            row(
                "    --output <FILE>",
                "Where to write, on the session's host",
            ),
            row("    --max-fps <N>", "Frame-rate ceiling, 1 to 120 (30)"),
            row(
                "    --duration <DUR>",
                "Stop after this long, such as 8h (24h)",
            ),
            row(
                "    --max-bytes <SIZE>",
                "Stop at this size, such as 512MiB (1GiB)",
            ),
            row("    --force", "Replace an existing file"),
            row("    --id <ID>", "A recording, from record list"),
            row("    --scale <N>", "PNG frame scale, 1 to 3 (1)"),
            row("    --speed <N>", "Playback speed (1)"),
            row(
                "    --from <MARK|TIME>",
                "Start playing at a mark or a time",
            ),
            row("    --format text|json", "Output format"),
            row("-h, --help", "Print help"),
        ],
    },
];

pub(crate) fn print_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi record", "record a pane and replay it");
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

/// A parsed `rozi record` command.
#[derive(Debug, PartialEq)]
pub(crate) enum RecordCli {
    /// `start`, `stop`, `list`, `mark`, and the foreground `pane`, sent to a session.
    Control {
        control: Box<ControlCli>,
        /// The foreground `record pane`: say how to stop it before waiting.
        foreground: bool,
    },
    Export {
        input: PathBuf,
        to: ExportTarget,
        scale: u8,
        force: bool,
    },
    Play {
        input: PathBuf,
        speed: f64,
        from: Option<Seek>,
    },
}

#[derive(Debug, PartialEq)]
pub(crate) enum ExportTarget {
    PngFrames(PathBuf),
    Cast(PathBuf),
}

/// Whether `rozi record ...` asked for help.
pub(super) fn wants_help(args: &[String]) -> bool {
    args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help")
}

/// The part of a `record` command that decides where it goes: the offline commands need no
/// endpoint, the rest are control commands built by [`parse_control`].
pub(super) enum RecordArgs {
    Offline(RecordCli),
    Control {
        command: ControlCommand,
        output_format: Option<ListFormat>,
        foreground: bool,
    },
}

pub(super) fn parse(args: Vec<String>) -> Result<RecordArgs, String> {
    let mut iter = args.into_iter();
    let subcommand = iter.next().ok_or_else(|| {
        "record requires a subcommand (start, pane, list, mark, stop, export, or play)".to_string()
    })?;
    let rest: Vec<String> = iter.collect();
    match subcommand.as_str() {
        "start" => {
            let mut rest = rest;
            if rest.first().map(String::as_str) == Some("pane") {
                rest.remove(0);
            } else if rest.first().map(String::as_str) == Some("ui") {
                return Err("record start ui is not available yet; record a pane".to_string());
            }
            parse_start(rest, false)
        }
        "pane" => parse_start(rest, true),
        "list" => {
            let mut iter = rest.into_iter();
            let mut format = None;
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--format" => {
                        let value = require_value(&mut iter, "--format requires text or json")?;
                        format = Some(parse_list_format(&value, "record list")?);
                    }
                    other => {
                        return Err(format!("unexpected argument `{other}` after record list"));
                    }
                }
            }
            Ok(RecordArgs::Control {
                command: ControlCommand::RecordList,
                output_format: format,
                foreground: false,
            })
        }
        "mark" => {
            let mut iter = rest.into_iter();
            let (mut label, mut id) = (None, None);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--id" => {
                        id = Some(parse_id(&require_value(
                            &mut iter,
                            "--id requires a recording id",
                        )?)?)
                    }
                    "--" => label = iter.next(),
                    _ if label.is_none() && !arg.starts_with("--") => label = Some(arg),
                    other => {
                        return Err(format!("unexpected argument `{other}` after record mark"));
                    }
                }
            }
            let label = label.ok_or_else(|| "record mark requires a label".to_string())?;
            Ok(RecordArgs::Control {
                command: ControlCommand::RecordMark { label, id },
                output_format: None,
                foreground: false,
            })
        }
        "stop" => {
            let mut iter = rest.into_iter();
            let mut id = None;
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--id" => {
                        id = Some(parse_id(&require_value(
                            &mut iter,
                            "--id requires a recording id",
                        )?)?)
                    }
                    other => {
                        return Err(format!("unexpected argument `{other}` after record stop"));
                    }
                }
            }
            Ok(RecordArgs::Control {
                command: ControlCommand::RecordStop { id },
                output_format: None,
                foreground: false,
            })
        }
        "export" => parse_export(rest).map(RecordArgs::Offline),
        "play" => parse_play(rest).map(RecordArgs::Offline),
        "ui" => Err("record ui is not available yet; record a pane".to_string()),
        other => Err(format!(
            "unknown record subcommand `{other}`; expected start, pane, list, mark, stop, export, or play"
        )),
    }
}

fn parse_start(args: Vec<String>, foreground: bool) -> Result<RecordArgs, String> {
    let name = if foreground {
        "record pane"
    } else {
        "record start"
    };
    let mut iter = args.into_iter();
    let (mut target, mut output, mut max_fps, mut duration_ms, mut max_bytes, mut force) =
        (None, None, None, None, None, false);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                let value = require_value(&mut iter, "--target requires a pane id")?;
                target = Some(
                    value
                        .parse()
                        .map_err(|_| "--target requires a numeric pane id".to_string())?,
                );
            }
            "--output" | "-o" => {
                output = Some(require_value(&mut iter, "--output requires a file")?)
            }
            "--max-fps" => {
                let value = require_value(&mut iter, "--max-fps requires a number")?;
                max_fps = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|fps| (1..=crate::control::MAX_RECORDING_MAX_FPS).contains(fps))
                        .ok_or_else(|| "--max-fps requires a number from 1 to 120".to_string())?,
                );
            }
            "--duration" => {
                let value = require_value(&mut iter, "--duration requires a duration such as 8h")?;
                duration_ms = Some(parse_long_duration(&value)?);
            }
            "--max-bytes" => {
                let value = require_value(&mut iter, "--max-bytes requires a size such as 512MiB")?;
                max_bytes = Some(parse_size(&value)?);
            }
            "--force" => force = true,
            other => return Err(format!("unexpected argument `{other}` after {name}")),
        }
    }
    let output = output.ok_or_else(|| format!("{name} requires --output <FILE>"))?;
    Ok(RecordArgs::Control {
        command: ControlCommand::RecordStart {
            target,
            output,
            max_fps,
            duration_ms,
            max_bytes,
            force,
            follow: foreground,
        },
        output_format: None,
        foreground,
    })
}

fn parse_export(args: Vec<String>) -> Result<RecordCli, String> {
    let mut iter = args.into_iter();
    let (mut input, mut to, mut scale, mut force) = (None, None, None, false);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--to" => {
                let kind = require_value(&mut iter, "--to requires png-frames or cast")?;
                let path = PathBuf::from(require_value(
                    &mut iter,
                    "--to requires a destination after the format",
                )?);
                to = Some(match kind.as_str() {
                    "png-frames" => ExportTarget::PngFrames(path),
                    "cast" => ExportTarget::Cast(path),
                    other => return Err(format!("--to accepts png-frames or cast, not `{other}`")),
                });
            }
            "--scale" => {
                let value = require_value(&mut iter, "--scale requires 1, 2, or 3")?;
                scale = Some(
                    value
                        .parse::<u8>()
                        .ok()
                        .filter(|scale| (1..=crate::control::MAX_CAPTURE_SCALE).contains(scale))
                        .ok_or_else(|| "--scale requires 1, 2, or 3".to_string())?,
                );
            }
            "--force" => force = true,
            _ if input.is_none() && !arg.starts_with('-') => input = Some(PathBuf::from(arg)),
            other => return Err(format!("unexpected argument `{other}` after record export")),
        }
    }
    let input = input.ok_or_else(|| "record export requires a recording file".to_string())?;
    let to = to.ok_or_else(|| {
        "record export requires --to png-frames <DIR> or --to cast <FILE>".to_string()
    })?;
    if scale.is_some() && !matches!(to, ExportTarget::PngFrames(_)) {
        return Err(format!(
            "--scale applies to png-frames, whose listing is {CONCAT_LISTING}"
        ));
    }
    Ok(RecordCli::Export {
        input,
        to,
        scale: scale.unwrap_or(1),
        force,
    })
}

fn parse_play(args: Vec<String>) -> Result<RecordCli, String> {
    let mut iter = args.into_iter();
    let (mut input, mut speed, mut from) = (None, 1.0, None);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--speed" => {
                let value = require_value(&mut iter, "--speed requires a number such as 2")?;
                speed = value
                    .parse::<f64>()
                    .ok()
                    .filter(|speed| speed.is_finite() && (0.1..=100.0).contains(speed))
                    .ok_or_else(|| "--speed requires a number from 0.1 to 100".to_string())?;
            }
            "--from" => {
                let value =
                    require_value(&mut iter, "--from requires a mark or a time such as 1m30s")?;
                from = Some(match parse_long_duration(&value) {
                    Ok(ms) => Seek::Time(ms),
                    Err(_) => Seek::Mark(value),
                });
            }
            _ if input.is_none() && !arg.starts_with('-') => input = Some(PathBuf::from(arg)),
            other => return Err(format!("unexpected argument `{other}` after record play")),
        }
    }
    Ok(RecordCli::Play {
        input: input.ok_or_else(|| "record play requires a recording file".to_string())?,
        speed,
        from,
    })
}

fn parse_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| "--id requires a numeric recording id".to_string())
}

/// A duration such as `90s`, `30m`, `8h`, `1h30m`, or `2d`, in milliseconds. A bare number is
/// seconds.
pub(super) fn parse_long_duration(value: &str) -> Result<u64, String> {
    let invalid = || format!("`{value}` is not a duration such as 30s, 8h, or 1h30m");
    if let Ok(seconds) = value.parse::<u64>() {
        return seconds.checked_mul(1_000).ok_or_else(invalid);
    }
    let mut total: u64 = 0;
    let mut rest = value;
    if rest.is_empty() {
        return Err(invalid());
    }
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(invalid)?;
        if digits == 0 {
            return Err(invalid());
        }
        let number: u64 = rest[..digits].parse().map_err(|_| invalid())?;
        rest = &rest[digits..];
        let (unit, multiplier) = [
            ("ms", 1),
            ("s", 1_000),
            ("m", 60_000),
            ("h", 3_600_000),
            ("d", 86_400_000),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit) && !(*unit == "m" && rest.starts_with("ms")))
        .ok_or_else(invalid)?;
        rest = &rest[unit.len()..];
        total = number
            .checked_mul(multiplier)
            .and_then(|ms| total.checked_add(ms))
            .ok_or_else(invalid)?;
    }
    Ok(total)
}

/// A size such as `512MiB`, `1GiB`, `100MB`, or a byte count. `K`, `M`, and `G` alone are binary.
pub(super) fn parse_size(value: &str) -> Result<u64, String> {
    let invalid = || format!("`{value}` is not a size such as 512MiB or 1GiB");
    let digits = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let number: u64 = value[..digits].parse().map_err(|_| invalid())?;
    let multiplier: u64 = match value[digits..].trim() {
        "" | "B" => 1,
        "K" | "KiB" => 1 << 10,
        "M" | "MiB" => 1 << 20,
        "G" | "GiB" => 1 << 30,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        _ => return Err(invalid()),
    };
    number.checked_mul(multiplier).ok_or_else(invalid)
}

/// Build the control half of a `record` command once its endpoint is known.
pub(super) fn control_cli(
    endpoint: super::ControlEndpoint,
    command: ControlCommand,
    output_format: Option<ListFormat>,
    foreground: bool,
) -> RecordCli {
    RecordCli::Control {
        control: Box::new(ControlCli {
            endpoint,
            request: control_request(command),
            output_format,
            output: None,
        }),
        foreground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<super::super::ParsedCli, String> {
        super::super::parse_cli_args(args.iter().copied().map(str::to_string).collect())
    }

    #[test]
    fn recording_is_sent_to_a_session_and_files_are_read_here() {
        let Ok(super::super::ParsedCli::Record(RecordCli::Control {
            control,
            foreground,
        })) = parse(&[
            "--session",
            "dev",
            "record",
            "start",
            "pane",
            "--target",
            "3",
            "--output",
            "a.rozirec",
            "--max-fps",
            "10",
            "--duration",
            "8h",
            "--max-bytes",
            "512MiB",
            "--force",
        ])
        else {
            panic!("expected a record start");
        };
        assert!(!foreground);
        assert_eq!(
            control.endpoint,
            super::super::ControlEndpoint::Session("dev".into())
        );
        assert_eq!(
            control.request.command,
            ControlCommand::RecordStart {
                target: Some(3),
                output: "a.rozirec".into(),
                max_fps: Some(10),
                duration_ms: Some(8 * 3_600_000),
                max_bytes: Some(512 << 20),
                force: true,
                follow: false,
            }
        );
        let Ok(super::super::ParsedCli::Record(RecordCli::Control {
            control,
            foreground,
        })) = parse(&[
            "--session",
            "dev",
            "record",
            "pane",
            "--target",
            "3",
            "-o",
            "b",
        ])
        else {
            panic!("expected a foreground record");
        };
        assert!(foreground);
        assert!(matches!(
            control.request.command,
            ControlCommand::RecordStart { follow: true, .. }
        ));

        let refused = parse(&["record", "list"]).unwrap_err();
        assert!(refused.contains("--session"), "{refused}");
        assert!(parse(&["record", "start", "--output", "x"]).is_err());
        assert!(parse(&["--session", "dev", "record", "start", "--target", "3"]).is_err());

        let Ok(super::super::ParsedCli::Record(export)) = parse(&[
            "record",
            "export",
            "a.rozirec",
            "--to",
            "png-frames",
            "out",
            "--scale",
            "2",
        ]) else {
            panic!("expected an export");
        };
        assert_eq!(
            export,
            RecordCli::Export {
                input: "a.rozirec".into(),
                to: ExportTarget::PngFrames("out".into()),
                scale: 2,
                force: false,
            }
        );
        assert!(
            parse(&[
                "--session",
                "dev",
                "record",
                "export",
                "a",
                "--to",
                "cast",
                "b"
            ])
            .is_err()
        );
        assert!(parse(&["record", "export", "a", "--to", "cast", "b", "--scale", "2"]).is_err());
        let Ok(super::super::ParsedCli::Record(play)) = parse(&[
            "record",
            "play",
            "a",
            "--from",
            "tests started",
            "--speed",
            "2",
        ]) else {
            panic!("expected a play");
        };
        assert_eq!(
            play,
            RecordCli::Play {
                input: "a".into(),
                speed: 2.0,
                from: Some(Seek::Mark("tests started".into())),
            }
        );
    }

    #[test]
    fn durations_and_sizes_read_the_way_people_write_them() {
        assert_eq!(parse_long_duration("8h"), Ok(8 * 3_600_000));
        assert_eq!(parse_long_duration("1h30m"), Ok(90 * 60_000));
        assert_eq!(parse_long_duration("500ms"), Ok(500));
        assert_eq!(parse_long_duration("2d"), Ok(2 * 86_400_000));
        assert_eq!(parse_long_duration("90"), Ok(90_000));
        for bad in ["", "h", "8x", "1.5h", "-1s"] {
            assert!(parse_long_duration(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_size("512MiB"), Ok(512 << 20));
        assert_eq!(parse_size("1GiB"), Ok(1 << 30));
        assert_eq!(parse_size("100MB"), Ok(100_000_000));
        assert_eq!(parse_size("4096"), Ok(4096));
        assert!(parse_size("1TB").is_err() && parse_size("MiB").is_err());
    }
}
