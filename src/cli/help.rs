//! `rozi --help` and `rozi --version`.
//!
//! The help text is data ([`HELP_SECTIONS`]) rendered by [`help_text`], so the same rows can be
//! measured by a test, styled for a terminal, or printed plain into a pipe.

use crate::platform::paths::{self, PlatformEnv};

pub(crate) fn print_help(advanced: bool) {
    println!(
        "{}",
        help_text(&HelpStyles::detect(), &endpoint_help(), advanced)
    );
}

/// Rozi-palette SGR sequences for the help screen, all empty when the stream cannot render them.
#[derive(Clone)]
pub(super) struct HelpStyles {
    pub(super) title: String,
    pub(super) heading: String,
    /// The leading command or option name, in the terminal's foreground.
    command: String,
    /// Signature syntax, descriptions, notes, and supporting text.
    muted: String,
    pub(super) reset: String,
}

impl HelpStyles {
    pub(super) fn plain() -> Self {
        Self {
            title: String::new(),
            heading: String::new(),
            command: String::new(),
            muted: String::new(),
            reset: String::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn colored() -> Self {
        Self::palette(true)
    }

    fn palette(truecolor: bool) -> Self {
        use crate::platform::ansi::{self, palette};

        Self {
            title: format!("{}{}", ansi::BOLD, ansi::fg(palette::ROSE, truecolor)),
            heading: format!("{}{}", ansi::BOLD, ansi::fg(palette::ROSE, truecolor)),
            command: ansi::BOLD.to_string(),
            muted: ansi::fg(palette::LAVENDER, truecolor),
            reset: ansi::RESET.to_string(),
        }
    }

    pub(super) fn detect() -> Self {
        if crate::platform::ansi::stdout_supports_color() {
            Self::palette(crate::platform::ansi::supports_truecolor())
        } else {
            Self::plain()
        }
    }

    pub(super) fn title_line(&self, title: &str, description: &str) -> String {
        format!(
            "{}{title}{}{} - {description}{}\n",
            self.title, self.reset, self.muted, self.reset
        )
    }
}

/// One help row: the literal to type, and what it does.
///
/// An empty `name` continues the previous row's description on a new line. A `name` too wide for
/// the description column takes a line of its own, which is what keeps the long session and
/// capture signatures from pushing every description past 80 columns.
pub(super) struct HelpRow {
    name: &'static str,
    description: &'static str,
    advanced_only: bool,
}

pub(super) struct HelpSection {
    pub(super) heading: &'static str,
    /// Prose shown under the heading, before the rows. Empty for most sections.
    pub(super) note: &'static str,
    /// Shown only under `--help --advanced`, keeping plumbing out of the first help a new user
    /// reads without hiding it from someone who needs it.
    pub(super) advanced_only: bool,
    pub(super) rows: &'static [HelpRow],
}

/// Write a row's name with its command prefix bold and its remaining syntax muted.
///
/// The first `[` or `<` starts the supporting syntax. Everything before it is the command or
/// option the user is looking for, including multiword namespaces and comma-separated short/long
/// flags. Under [`HelpStyles::plain`] every sequence is empty, so this appends `name` unchanged.
pub(super) fn push_styled_name(out: &mut String, name: &str, styles: &HelpStyles) {
    let leading = name.len() - name.trim_start().len();
    out.push_str(&name[..leading]);
    let syntax_start = name[leading..]
        .find(['[', '<'])
        .map_or(name.len(), |offset| leading + offset);
    let command_end = leading + name[leading..syntax_start].trim_end().len();

    if command_end > leading {
        out.push_str(&styles.command);
        out.push_str(&name[leading..command_end]);
        out.push_str(&styles.reset);
    }
    if command_end < name.len() {
        out.push_str(&styles.muted);
        out.push_str(&name[command_end..]);
        out.push_str(&styles.reset);
    }
}

/// Width of the name column. Descriptions start at `HELP_INDENT + HELP_NAME_WIDTH`.
pub(super) const HELP_NAME_WIDTH: usize = 27;
pub(super) const HELP_INDENT: &str = "    ";

pub(super) const fn row(name: &'static str, description: &'static str) -> HelpRow {
    HelpRow {
        name,
        description,
        advanced_only: false,
    }
}

pub(super) const fn advanced_row(name: &'static str, description: &'static str) -> HelpRow {
    HelpRow {
        name,
        description,
        advanced_only: true,
    }
}

pub(super) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "rozi [TARGET] [OPTIONS]",
                "Attach to TARGET, or launch its profile",
            ),
            row("rozi <COMMAND> [ARGS]", ""),
        ],
    },
    HelpSection {
        heading: "SESSIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "sessions attach <NAME>",
                "Attach to a running session, never create",
            ),
            row(
                "sessions new <NAME> [--profile <PROFILE>]",
                "Create a session, optionally from a profile",
            ),
            row(
                "sessions list [--format text|json] [--remote <HOST>]",
                "List connectable sessions",
            ),
            row(
                "sessions kill <NAME> [--remote <HOST>]",
                "Stop a session and all of its panes",
            ),
        ],
    },
    HelpSection {
        heading: "PANES",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "list-panes [--format text|json]",
                "List live panes; JSON when piped",
            ),
            row("focus <PANE_ID>", "Focus a pane"),
            row(
                "send-text [--target <PANE_ID>] <TEXT>",
                "Send literal text to a pane",
            ),
            row(
                "send-keys [--target <PANE_ID>] [-l|--literal] [--] <KEY|TEXT>...",
                "Send tmux-style key names, text, or both",
            ),
            row(
                "split [OPTIONS] [COMMAND | --argv PROGRAM [ARG...]]",
                "Spawn a pane, optionally in another workspace",
            ),
            row("capture-pane [OPTIONS]", "Print pane text; JSON when piped"),
            row("switch-workspace <1-9>", "Switch the active workspace"),
            row(
                "move-to-workspace <1-9>",
                "Move the focused pane to a workspace",
            ),
        ],
    },
    HelpSection {
        heading: "SCRIPTING",
        advanced_only: false,
        note: "",
        rows: &[
            row("status <VALUE> [--reason <TEXT>]", ""),
            row("status --clear", "Set or clear this pane's reported status"),
            row(
                "run-action <ACTION_ID>",
                "Run a bindable action by its command id",
            ),
            row(
                "notify <MESSAGE> [--title T] [--level info|error]",
                "Raise a toast from a script",
            ),
            row("publish", "Publish activity rows over stdio"),
            row("subscribe [EVENT...]", "Stream application events as JSON"),
            row(
                "pick [--title T] [--placeholder P] [--json]",
                "Choose a line of stdin in a modal picker",
            ),
            row(
                "metrics [--format text|json]",
                "Show runtime resources; JSON when piped",
            ),
        ],
    },
    HelpSection {
        heading: "EXTENSIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "extensions list [--verbose] [--json]",
                "Show discovery status (--verbose, --json)",
            ),
            row("extensions new <ID>", "Create a valid extension scaffold"),
            row(
                "extensions check <PATH> [--json]",
                "Validate an unpacked extension (--json)",
            ),
        ],
    },
    HelpSection {
        heading: "AGENTS",
        advanced_only: false,
        note: "",
        rows: &[row("skill [COMMAND]", "Manage the Rozi agent skill")],
    },
    HelpSection {
        heading: "INSTALLATION",
        advanced_only: false,
        note: "",
        rows: &[
            row("install", "Install this binary as a managed `rozi`"),
            row(
                "update [--check|--rollback]",
                "Update in place, check, or roll back",
            ),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "-h, --help [--advanced]",
                "Print help; --advanced adds internals",
            ),
            row("-V, --version", "Print version and protocol range"),
            row(
                "    --session <NAME>",
                "Session target, same as a positional TARGET",
            ),
            row(
                "    --profile <NAME>",
                "Seed a `sessions new` session from this profile",
            ),
            row("    --read-only", "Attach as a viewer; cannot type or tile"),
            row("    --pick", "Force the startup session picker, whatever"),
            row("", "`[session] startup` selects"),
            row(
                "    --remote [HOST]",
                "Attach over SSH to a host alias or ssh://",
            ),
            row("", "URL; omit HOST for `[remote] default_host`"),
            row("    --config <PATH>", "Load an alternate config.toml"),
            advanced_row(
                "    --socket <PATH>",
                "Send the control command to this endpoint",
            ),
            advanced_row("    --skill", "Print agent control instructions"),
        ],
    },
    HelpSection {
        heading: "ADVANCED",
        advanced_only: true,
        note: "Server plumbing; a normal launch needs none of it.",
        rows: &[
            row("    --server", "Run --session <NAME>'s server in this"),
            row("", "process instead of attaching a UI"),
        ],
    },
];

/// The help body, with the platform-specific endpoint paragraph passed in so a test can measure
/// the template's own width without depending on how long this machine's runtime directory is.
pub(super) fn help_text(styles: &HelpStyles, endpoint_help: &str, advanced: bool) -> String {
    let title = &styles.title;
    let heading = &styles.heading;
    let muted = &styles.muted;
    let reset = &styles.reset;
    let version = env!("CARGO_PKG_VERSION");
    let mut out = format!(
        "{title}rozi {version}{reset}{muted} - dynamic tiling terminal multiplexer{reset}\n"
    );
    append_help_sections(&mut out, HELP_SECTIONS, styles, advanced);

    if advanced {
        out.push_str(&format!(
            "\n{heading}ENDPOINTS{reset}\n{muted}{HELP_INDENT}{endpoint_help}{reset}\n"
        ));
    }
    out.push_str(&format!(
        "\n{muted}Detach with prefix d, or use a configured quit binding.{reset}"
    ));
    out
}

pub(super) fn append_help_sections(
    out: &mut String,
    sections: &[HelpSection],
    styles: &HelpStyles,
    advanced: bool,
) {
    let heading = &styles.heading;
    let muted = &styles.muted;
    let reset = &styles.reset;
    for section in sections {
        if section.advanced_only && !advanced {
            continue;
        }
        out.push_str(&format!("\n{heading}{}{reset}\n", section.heading));
        if !section.note.is_empty() {
            out.push_str(&format!("{muted}{HELP_INDENT}{}{reset}\n\n", section.note));
        }
        for HelpRow {
            name,
            description,
            advanced_only,
        } in section.rows
        {
            if *advanced_only && !advanced {
                continue;
            }
            if name.is_empty() {
                out.push_str(&format!(
                    "{HELP_INDENT}{:width$}{muted}{description}{reset}\n",
                    "",
                    width = HELP_NAME_WIDTH
                ));
                continue;
            }
            out.push_str(HELP_INDENT);
            push_styled_name(out, name, styles);
            if description.is_empty() {
                out.push('\n');
                continue;
            }
            if name.chars().count() < HELP_NAME_WIDTH {
                out.push_str(&format!(
                    "{:width$}{muted}{description}{reset}\n",
                    "",
                    width = HELP_NAME_WIDTH - name.chars().count()
                ));
            } else {
                out.push_str(&format!(
                    "\n{HELP_INDENT}{:width$}{muted}{description}{reset}\n",
                    "",
                    width = HELP_NAME_WIDTH
                ));
            }
        }
    }
}

/// The `--socket`/`ROZI_SOCKET` explanation, which differs by platform: a Unix-domain socket
/// path on Linux/macOS, a named-pipe registry entry on Windows (see `platform::ipc::windows` for
/// why the *entry*, not the pipe name, is what a user points at).
pub(super) fn endpoint_help() -> String {
    // Resolved, never created: this only names the directory in help text, and creating it here
    // would establish the managed installation root on Windows as a side effect of `--help`.
    let runtime_dir = paths::runtime_dir_path(&PlatformEnv::from_process())
        .display()
        .to_string();
    if cfg!(windows) {
        format!(
            "Control endpoints live one per running rozi, named by pid, in\n        \
             {runtime_dir}\n    \
             Each entry stands for a named pipe (\\\\.\\pipe\\rozi.<sid>.control-<pid>);\n    \
             pass the entry, not the pipe name. Unless --socket is given, rozi uses\n    \
             ROZI_SOCKET; failing that, the only live endpoint there."
        )
    } else {
        format!(
            "Control sockets live one per running rozi, named by pid, in\n        \
             {runtime_dir}\n    \
             Unless --socket is given, rozi uses ROZI_SOCKET; failing that, the\n    \
             only live socket there."
        )
    }
}

pub(crate) fn print_version() {
    use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
    println!("rozi {}", env!("CARGO_PKG_VERSION"));
    println!("protocol_min={MIN_SUPPORTED_PROTOCOL}");
    println!("protocol_max={PROTOCOL_VERSION}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{
        EXTENSIONS_HELP_SECTIONS, ParsedCli, SESSIONS_HELP_SECTIONS, parse_cli_args,
    };
    use crate::cli::skill::SKILL_HELP_SECTIONS;

    #[test]
    fn cli_help_fits_an_eighty_column_terminal() {
        // The help is the only UI a first run gets; wrapping it destroys the aligned columns.
        let endpoint = "Control sockets live one per running rozi, named by pid, in\n    \
                        <dir>\n    \
                        With no --socket, ROZI_SOCKET is used, then the only live socket there.";
        for advanced in [false, true] {
            let mut widest = 0;
            for line in help_text(&HelpStyles::plain(), endpoint, advanced).lines() {
                assert!(
                    !line.ends_with(' '),
                    "trailing whitespace in help line: {line:?}"
                );
                widest = widest.max(line.chars().count());
            }
            assert!(
                widest <= 80,
                "help reaches {widest} columns (advanced: {advanced})"
            );
        }
        let mut skill_help = String::from("rozi skill - install the built-in Rozi agent skill\n");
        append_help_sections(
            &mut skill_help,
            SKILL_HELP_SECTIONS,
            &HelpStyles::plain(),
            true,
        );
        for line in skill_help.lines() {
            assert!(
                !line.ends_with(' '),
                "trailing whitespace in skill help line: {line:?}"
            );
            assert!(
                line.chars().count() <= 80,
                "skill help reaches {} columns: {line}",
                line.chars().count()
            );
        }
        for (name, sections) in [
            ("sessions", SESSIONS_HELP_SECTIONS),
            ("extensions", EXTENSIONS_HELP_SECTIONS),
        ] {
            let mut namespace_help = format!("rozi {name}\n");
            append_help_sections(&mut namespace_help, sections, &HelpStyles::plain(), true);
            for line in namespace_help.lines() {
                assert!(
                    !line.ends_with(' '),
                    "trailing whitespace in {name} help line: {line:?}"
                );
                assert!(
                    line.chars().count() <= 80,
                    "{name} help reaches {} columns: {line}",
                    line.chars().count()
                );
            }
        }
    }

    fn heading_order(text: &str, headings: &[&str]) {
        let mut pos = 0;
        for heading in headings {
            let Some(found) = text[pos..].find(heading) else {
                panic!("{heading} missing or out of order in help");
            };
            pos += found + heading.len();
        }
    }

    #[test]
    fn cli_advanced_help_gates_server_plumbing_without_hiding_it() {
        let render = |advanced| help_text(&HelpStyles::plain(), "<endpoints>", advanced);
        let normal = render(false);
        let advanced = render(true);
        heading_order(
            &normal,
            &[
                "USAGE",
                "SESSIONS",
                "PANES",
                "SCRIPTING",
                "EXTENSIONS",
                "AGENTS",
                "INSTALLATION",
                "OPTIONS",
            ],
        );
        heading_order(
            &advanced,
            &[
                "USAGE",
                "SESSIONS",
                "PANES",
                "SCRIPTING",
                "EXTENSIONS",
                "AGENTS",
                "INSTALLATION",
                "OPTIONS",
                "ADVANCED",
                "ENDPOINTS",
            ],
        );

        assert!(
            !normal.contains("--server"),
            "plumbing should stay out of the first help a new user reads"
        );
        assert!(!normal.contains("ADVANCED"));
        assert!(!normal.contains("ENDPOINTS"));
        assert!(!normal.contains("--socket"));
        assert!(!normal.contains("--skill"));
        assert!(!normal.contains("CONTROL"));
        assert!(advanced.contains("--server"));
        assert!(advanced.contains("--socket"));
        assert!(advanced.contains("--skill"));
        // Normal help still has to say where the rest went.
        assert!(normal.contains("--advanced"));

        // `--advanced` reads on either side of the help flag, and means nothing without it.
        for args in [
            vec!["--help", "--advanced"],
            vec!["--advanced", "--help"],
            vec!["-h", "--advanced"],
        ] {
            assert!(matches!(
                parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect())
                    .expect("parses"),
                ParsedCli::Help { advanced: true }
            ));
        }
        assert!(matches!(
            parse_cli_args(vec!["--help".into()]).expect("parses"),
            ParsedCli::Help { advanced: false }
        ));
        assert!(parse_cli_args(vec!["--advanced".into()]).is_err());
    }

    #[test]
    fn cli_help_names_split_commands_from_muted_syntax() {
        // Marker styles rather than real SGR, so a failure reads as the classification it is.
        let styles = HelpStyles {
            title: String::new(),
            heading: String::new(),
            command: "C<".to_string(),
            muted: "M<".to_string(),
            reset: ">".to_string(),
        };
        let styled = |name: &str| {
            let mut out = String::new();
            push_styled_name(&mut out, name, &styles);
            out
        };

        assert_eq!(styled("focus <PANE_ID>"), "C<focus>M< <PANE_ID>>");
        // Multiword namespaces are one command prefix; every argument and flag after it recedes.
        assert_eq!(
            styled("sessions new <NAME> [--profile <PROFILE>]"),
            "C<sessions new>M< <NAME> [--profile <PROFILE>]>"
        );
        assert_eq!(styled("split [COMMAND]"), "C<split>M< [COMMAND]>");
        assert_eq!(
            styled("sessions list [--format text|json]"),
            "C<sessions list>M< [--format text|json]>"
        );
        assert_eq!(
            styled("send-keys [-l|--literal] [--] <KEY|TEXT>..."),
            "C<send-keys>M< [-l|--literal] [--] <KEY|TEXT>...>"
        );
        assert_eq!(styled("-h, --help"), "C<-h, --help>");
        assert_eq!(styled("    --profile <NAME>"), "    C<--profile>M< <NAME>>");

        // Plain styling has to leave every name exactly as written.
        for section in HELP_SECTIONS {
            for row in section.rows {
                let mut plain = String::new();
                push_styled_name(&mut plain, row.name, &HelpStyles::plain());
                assert_eq!(plain, row.name);
            }
        }
    }

    #[test]
    fn cli_help_styling_only_adds_escapes() {
        // Colour must not move a single glyph: the styled help has to be the plain help with SGR
        // sequences woven in, or the aligned columns drift the moment a terminal supports colour.
        let endpoint = "Control sockets live in <dir>.";
        let plain = help_text(&HelpStyles::plain(), endpoint, true);
        let colored = help_text(&HelpStyles::colored(), endpoint, true);
        assert_ne!(plain, colored, "colored help should carry escapes");

        let mut stripped = String::with_capacity(colored.len());
        let mut rest = colored.as_str();
        while let Some(start) = rest.find('\x1b') {
            stripped.push_str(&rest[..start]);
            let end = rest[start..]
                .find('m')
                .expect("every SGR sequence this help emits ends in `m`");
            rest = &rest[start + end + 1..];
        }
        stripped.push_str(rest);
        assert_eq!(stripped, plain);
    }

    #[test]
    fn cli_help_uses_the_rozi_palette() {
        use crate::platform::ansi::{self, palette};

        let styles = HelpStyles::colored();
        assert_eq!(
            styles.title,
            format!("{}{}", ansi::BOLD, ansi::fg(palette::ROSE, true))
        );
        assert_eq!(
            styles.heading,
            format!("{}{}", ansi::BOLD, ansi::fg(palette::ROSE, true))
        );
        assert_eq!(styles.command, ansi::BOLD);
        assert_eq!(styles.muted, ansi::fg(palette::LAVENDER, true));
        assert_eq!(styles.reset, ansi::RESET);
    }
}
