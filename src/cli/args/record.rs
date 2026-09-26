//! `rozi record`: record a pane from its session server or a running UI as it paints, and export
//! or play a recording.

use std::path::PathBuf;

use crate::control::ControlCommand;
use crate::recording::export::CONCAT_LISTING;
use crate::recording::play::Seek;
use crate::recording::units::{parse_duration_ms, parse_size};

use super::{ControlCli, ListFormat, control_request, parse_list_format, require_value};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[
            row("rozi [--session <NAME>] record <COMMAND> [OPTIONS]", ""),
            row("rozi record export|play <FILE> [OPTIONS]", ""),
        ],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "A pane recording runs in the session server. Without --session, the running\n    \
               rozi passes start, stop, list, and mark to the session it is attached to;\n    \
               pane needs --session <NAME>. A UI recording runs in the rozi --socket\n    \
               names, or the one this shell runs in, and writes on its host. export and\n    \
               play read a file here.",
        rows: &[
            row(
                "start [pane] [--target <PANE>] [--output <FILE>]",
                "Start recording a pane",
            ),
            row(
                "start ui [--output <FILE>]",
                "Record the running UI, chrome included",
            ),
            row(
                "pane --target <PANE> [--output <FILE>]",
                "Record until Ctrl-C",
            ),
            row("list [--format text|json]", "List running recordings"),
            row(
                "mark <TEXT> [--id <ID> | --target <PANE>]",
                "Label this moment",
            ),
            row("stop [--id <ID> | --target <PANE>]", "Stop recordings"),
            row("stop --ui", "Stop the UI recording"),
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
            row("    --target <PANE>", "The pane to record, stop, or mark"),
            row(
                "    --output <FILE>",
                "The file, where it is recorded ([recording] dir)",
            ),
            row(
                "    --max-fps <N>",
                "Frame-rate cap, 1 to 120 ([recording] max_fps)",
            ),
            row(
                "    --duration <DUR>",
                "Stop after this long ([recording] duration)",
            ),
            row(
                "    --max-bytes <SIZE>",
                "Stop at this size ([recording] max_bytes)",
            ),
            row("    --force", "Replace an existing file"),
            row("    --id <ID>", "A recording, from record list"),
            row("    --ui", "Stop or mark the UI recording"),
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
    let mut out = styles.title_line("rozi record", "record a pane or the UI and replay it");
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
                rest.remove(0);
                return parse_start_ui(rest);
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
            let (mut label, mut id, mut target, mut ui) = (None, None, None, false);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--ui" => ui = true,
                    "--id" => {
                        id = Some(parse_id(&require_value(
                            &mut iter,
                            "--id requires a recording id",
                        )?)?)
                    }
                    "--target" => target = Some(parse_target(&mut iter)?),
                    "--" => label = iter.next(),
                    _ if label.is_none() && !arg.starts_with("--") => label = Some(arg),
                    other => {
                        return Err(format!("unexpected argument `{other}` after record mark"));
                    }
                }
            }
            let label = label.ok_or_else(|| "record mark requires a label".to_string())?;
            one_selector(id, target)?;
            if ui {
                no_selector_with_ui(id, target)?;
                return Ok(RecordArgs::Control {
                    command: ControlCommand::RecordUiMark { label },
                    output_format: None,
                    foreground: false,
                });
            }
            Ok(RecordArgs::Control {
                command: ControlCommand::RecordMark { label, id, target },
                output_format: None,
                foreground: false,
            })
        }
        "stop" => {
            let mut iter = rest.into_iter();
            let (mut id, mut target, mut ui) = (None, None, false);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--ui" => ui = true,
                    "--id" => {
                        id = Some(parse_id(&require_value(
                            &mut iter,
                            "--id requires a recording id",
                        )?)?)
                    }
                    "--target" => target = Some(parse_target(&mut iter)?),
                    other => {
                        return Err(format!("unexpected argument `{other}` after record stop"));
                    }
                }
            }
            one_selector(id, target)?;
            if ui {
                no_selector_with_ui(id, target)?;
                return Ok(RecordArgs::Control {
                    command: ControlCommand::RecordUiStop,
                    output_format: None,
                    foreground: false,
                });
            }
            Ok(RecordArgs::Control {
                command: ControlCommand::RecordStop { id, target },
                output_format: None,
                foreground: false,
            })
        }
        "export" => parse_export(rest).map(RecordArgs::Offline),
        "play" => parse_play(rest).map(RecordArgs::Offline),
        "ui" => Err(
            "record ui does not run in the foreground; use `rozi record start ui` and `rozi record stop --ui`"
                .to_string(),
        ),
        other => Err(format!(
            "unknown record subcommand `{other}`; expected start, pane, list, mark, stop, export, or play"
        )),
    }
}

/// What `record start` and `record pane` share: where the file goes and the limits.
#[derive(Default)]
struct StartOptions {
    target: Option<u32>,
    output: Option<String>,
    max_fps: Option<u32>,
    duration_ms: Option<u64>,
    max_bytes: Option<u64>,
    force: bool,
}

fn parse_start(args: Vec<String>, foreground: bool) -> Result<RecordArgs, String> {
    let name = if foreground {
        "record pane"
    } else {
        "record start"
    };
    let StartOptions {
        target,
        output,
        max_fps,
        duration_ms,
        max_bytes,
        force,
    } = parse_start_options(args, name, true)?;
    if force && output.is_none() {
        return Err(format!(
            "{name} --force replaces the file --output names; a file the session names is always new"
        ));
    }
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

fn parse_start_ui(args: Vec<String>) -> Result<RecordArgs, String> {
    let StartOptions {
        output,
        max_fps,
        duration_ms,
        max_bytes,
        force,
        ..
    } = parse_start_options(args, "record start ui", false)?;
    if force && output.is_none() {
        return Err(
            "record start ui --force replaces the file --output names; a file the UI names is always new"
                .to_string(),
        );
    }
    Ok(RecordArgs::Control {
        command: ControlCommand::RecordUiStart {
            output,
            max_fps,
            duration_ms,
            max_bytes,
            force,
        },
        output_format: None,
        foreground: false,
    })
}

fn parse_start_options(
    args: Vec<String>,
    name: &str,
    takes_target: bool,
) -> Result<StartOptions, String> {
    let mut iter = args.into_iter();
    let mut options = StartOptions::default();
    let StartOptions {
        target,
        output,
        max_fps,
        duration_ms,
        max_bytes,
        force,
    } = &mut options;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" if takes_target => *target = Some(parse_target(&mut iter)?),
            "--output" | "-o" => {
                *output = Some(require_value(&mut iter, "--output requires a file")?)
            }
            "--max-fps" => {
                let value = require_value(&mut iter, "--max-fps requires a number")?;
                *max_fps = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|fps| (1..=crate::control::MAX_RECORDING_MAX_FPS).contains(fps))
                        .ok_or_else(|| "--max-fps requires a number from 1 to 120".to_string())?,
                );
            }
            "--duration" => {
                let value = require_value(&mut iter, "--duration requires a duration such as 8h")?;
                *duration_ms = Some(parse_duration_ms(&value)?);
            }
            "--max-bytes" => {
                let value = require_value(&mut iter, "--max-bytes requires a size such as 512MiB")?;
                *max_bytes = Some(parse_size(&value)?);
            }
            "--force" => *force = true,
            other => return Err(format!("unexpected argument `{other}` after {name}")),
        }
    }
    Ok(options)
}

/// `--ui` names the UI's one recording, so it takes no recording id or pane.
fn no_selector_with_ui(id: Option<u64>, target: Option<u32>) -> Result<(), String> {
    if id.is_some() || target.is_some() {
        return Err(
            "--ui names the UI recording; --id and --target name pane recordings".to_string(),
        );
    }
    Ok(())
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
                from = Some(match parse_duration_ms(&value) {
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

fn parse_target(iter: &mut impl Iterator<Item = String>) -> Result<u32, String> {
    require_value(iter, "--target requires a pane id")?
        .parse()
        .map_err(|_| "--target requires a numeric pane id".to_string())
}

fn one_selector(id: Option<u64>, target: Option<u32>) -> Result<(), String> {
    if id.is_some() && target.is_some() {
        return Err("name a recording with --id or a pane with --target, not both".to_string());
    }
    Ok(())
}

fn parse_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| "--id requires a numeric recording id".to_string())
}

/// Whether a `record` command can go to a UI, which forwards it to the session it is attached to.
///
/// `record pane` cannot: it holds its caller until the recording ends, and a UI answers each
/// request once.
///
/// `--output` is not checked here. The file is written on the session's host, which may run
/// another OS than this one (`C:\rec.rozirec` is relative to a Unix `Path`), so the session server
/// alone decides whether the path is absolute.
pub(super) fn check_ui_endpoint(foreground: bool) -> Result<(), String> {
    if foreground {
        return Err("record pane runs in the foreground and needs --session <NAME>".to_string());
    }
    Ok(())
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
        let absolute_output = if cfg!(windows) {
            r"C:\tmp\a.rozirec"
        } else {
            "/tmp/a.rozirec"
        };
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
                output: Some("a.rozirec".into()),
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

        let Ok(super::super::ParsedCli::Record(RecordCli::Control { control, .. })) =
            parse(&["record", "list"])
        else {
            panic!("expected a record list for the running rozi");
        };
        assert_eq!(control.endpoint, super::super::ControlEndpoint::Ui(None));
        assert!(parse(&["record", "stop", "--id", "2"]).is_ok());
        assert!(parse(&["record", "mark", "here"]).is_ok());
        // The session's host judges the path, whatever OS this side runs: a Unix path for a Linux
        // session from a Windows UI, a drive path for the reverse, and a relative one to refuse.
        for output in ["/tmp/a.rozirec", r"C:\recordings\a.rozirec", "x"] {
            let Ok(super::super::ParsedCli::Record(RecordCli::Control { control, .. })) =
                parse(&["record", "start", "--output", output])
            else {
                panic!("expected {output} to go to the session unjudged");
            };
            assert!(
                matches!(&control.request.command, ControlCommand::RecordStart { output: sent, .. } if sent.as_deref() == Some(output)),
                "{output} is sent as written"
            );
        }
        let foreground = parse(&["record", "pane", "--output", absolute_output]).unwrap_err();
        assert!(foreground.contains("--session"), "{foreground}");
        let Ok(super::super::ParsedCli::Record(RecordCli::Control { control, .. })) =
            parse(&["--session", "dev", "record", "start", "--target", "3"])
        else {
            panic!("expected a start the session names a file for");
        };
        assert!(matches!(
            control.request.command,
            ControlCommand::RecordStart { output: None, .. }
        ));
        assert!(parse(&["--session", "dev", "record", "start", "--force"]).is_err());

        for (args, command) in [
            (
                &["record", "stop", "--target", "3"][..],
                ControlCommand::RecordStop {
                    id: None,
                    target: Some(3),
                },
            ),
            (
                &["record", "mark", "built", "--target", "3"][..],
                ControlCommand::RecordMark {
                    label: "built".into(),
                    id: None,
                    target: Some(3),
                },
            ),
        ] {
            let Ok(super::super::ParsedCli::Record(RecordCli::Control { control, .. })) =
                parse(args)
            else {
                panic!("expected {args:?} to parse");
            };
            assert_eq!(control.request.command, command);
        }
        for args in [
            &["record", "stop", "--id", "1", "--target", "3"][..],
            &["record", "mark", "x", "--target", "3", "--id", "1"][..],
        ] {
            let both = parse(args).unwrap_err();
            assert!(both.contains("not both"), "{both}");
        }

        let Ok(super::super::ParsedCli::Record(RecordCli::Control { control, .. })) = parse(&[
            "record",
            "start",
            "ui",
            "--output",
            "relative.rozirec",
            "--max-fps",
            "12",
        ]) else {
            panic!("expected a UI recording start");
        };
        assert_eq!(control.endpoint, super::super::ControlEndpoint::Ui(None));
        assert_eq!(
            control.request.command,
            ControlCommand::RecordUiStart {
                output: Some("relative.rozirec".into()),
                max_fps: Some(12),
                duration_ms: None,
                max_bytes: None,
                force: false,
            },
            "a UI records on this machine, so a relative path is resolved here"
        );
        for (args, command) in [
            (
                &["record", "stop", "--ui"][..],
                ControlCommand::RecordUiStop,
            ),
            (
                &["record", "mark", "--ui", "built"][..],
                ControlCommand::RecordUiMark {
                    label: "built".into(),
                },
            ),
        ] {
            let Ok(super::super::ParsedCli::Record(RecordCli::Control { control, .. })) =
                parse(args)
            else {
                panic!("expected {args:?} to parse");
            };
            assert_eq!(control.request.command, command);
        }
        for (args, error) in [
            (
                &["--session", "dev", "record", "start", "ui"][..],
                "draws nothing",
            ),
            (&["record", "start", "ui", "--target", "3"][..], "--target"),
            (&["record", "start", "ui", "--force"][..], "--force"),
            (&["record", "stop", "--ui", "--id", "1"][..], "--ui"),
            (
                &["record", "mark", "x", "--ui", "--target", "3"][..],
                "--ui",
            ),
            (&["record", "ui"][..], "record start ui"),
        ] {
            let refused = parse(args).unwrap_err();
            assert!(refused.contains(error), "{args:?}: {refused}");
        }

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
}
