//! User-selected opening policy shared by terminal clicks and hint mode.
use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{LinkAction, LinkHandler, UserCommandAction};
use crate::state::PaneId;

pub(crate) fn handler<'a>(handlers: &'a [LinkHandler], scheme: &str) -> Option<&'a LinkAction> {
    handlers
        .iter()
        .find(|handler| {
            handler
                .schemes
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(scheme))
        })
        .map(|handler| &handler.action)
}

pub(crate) fn open(ctx: &mut Context<AppRoot>, source: PaneId, text: &str) -> Update {
    let parsed = url::Url::parse(text);
    let action = parsed
        .as_ref()
        .ok()
        .and_then(|url| handler(&ctx.state.config.link_handlers, url.scheme()))
        .cloned();
    if let Some(action) = action {
        let file = if let Ok(url) = &parsed {
            if url.scheme() == "file" {
                match url
                    .to_file_path()
                    .ok()
                    .and_then(|path| path.into_os_string().into_string().ok())
                {
                    Some(path) => Some(path),
                    None => {
                        return failed(
                            ctx,
                            "File URL cannot be represented as a path on this host",
                        );
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        return execute(ctx, source, &action, text, Some(text), file.as_deref());
    }
    match tui_lipan::utils::open_url(text) {
        Ok(()) => Update::none(),
        Err(error) => failed(ctx, error.to_string()),
    }
}

pub(crate) fn open_path(ctx: &mut Context<AppRoot>, source: PaneId, text: &str) -> Update {
    let Some(action) = handler(&ctx.state.config.link_handlers, "file").cloned() else {
        return Update::none();
    };
    execute(ctx, source, &action, text, None, Some(text))
}

pub(crate) fn execute(
    ctx: &mut Context<AppRoot>,
    source: PaneId,
    action: &LinkAction,
    text: &str,
    uri: Option<&str>,
    file: Option<&str>,
) -> Update {
    let named = match action {
        LinkAction::Inline(_) => None,
        LinkAction::Command(id) => match ctx
            .state
            .config
            .commands
            .iter()
            .find(|command| &command.id == id)
            .cloned()
        {
            Some(command) if !matches!(command.action, UserCommandAction::Send(_)) => Some(command),
            _ => {
                return failed(
                    ctx,
                    format!("Link command `{id}` is unavailable or does not launch a process"),
                );
            }
        },
    };
    let action = match (&named, action) {
        (Some(command), _) => &command.action,
        (None, LinkAction::Inline(action)) => action,
        _ => unreachable!(),
    };
    if text.contains('\0') || file.is_some_and(|path| path.contains('\0')) {
        return failed(ctx, "Link contains a NUL character");
    }
    let (file, line, column) = if uri.is_none() {
        file.map(path_position).unwrap_or(("", "", ""))
    } else {
        (file.unwrap_or_default(), "", "")
    };
    let mut env = vec![
        ("ROZI_HINT".into(), text.into()),
        ("ROZI_URL".into(), uri.unwrap_or_default().into()),
        ("ROZI_FILE".into(), file.into()),
        ("ROZI_LINE".into(), line.into()),
        ("ROZI_COLUMN".into(), column.into()),
        ("ROZI_SOURCE_PANE".into(), source.to_string()),
        (
            "ROZI_SOURCE_SESSION".into(),
            ctx.state
                .current()
                .session_instance
                .as_ref()
                .map(|instance| instance.as_str().to_string())
                .unwrap_or_default(),
        ),
        (
            "ROZI_REMOTE_HOST".into(),
            ctx.state.current().remote_host.clone().unwrap_or_default(),
        ),
    ];
    if matches!(
        action,
        UserCommandAction::Exec { .. } | UserCommandAction::ExecDirect { .. }
    ) {
        env.push(("ROZI_PANE".into(), source.to_string()));
    }
    if let Some(command) = named {
        if let Some((placements, placed)) =
            super::placement::placed_command(&ctx.state, &command.id)
        {
            return super::placement::run_command_from_pane(
                ctx,
                &command,
                placements,
                &placed,
                env,
                Some(source),
            );
        }
        let mut command_env = command.env;
        command_env.extend(env);
        return super::user_command::execute_with_env_from_pane(
            ctx,
            &command.action,
            command_env,
            Some(source),
        );
    }
    super::user_command::execute_with_env_from_pane(ctx, action, env, Some(source))
}

fn path_position(text: &str) -> (&str, &str, &str) {
    fn numeric_suffix(text: &str) -> Option<(&str, &str)> {
        text.rsplit_once(':').filter(|(path, number)| {
            !path.is_empty() && !number.is_empty() && number.bytes().all(|ch| ch.is_ascii_digit())
        })
    }
    // Work directly with slices so paths (including Windows drive letters) remain untouched.
    if let Some((path, last)) = numeric_suffix(text) {
        if let Some((path, line)) = numeric_suffix(path) {
            return (path, line, last);
        }
        return (path, last, "");
    }
    (text, "", "")
}

fn failed(ctx: &mut Context<AppRoot>, error: impl Into<String>) -> Update {
    crate::pane::pty_events::notify_error(ctx, "Could not open link", error.into());
    Update::full()
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn first_matching_scheme_wins() {
        let handlers = vec![
            LinkHandler {
                schemes: vec!["https".into()],
                action: LinkAction::Inline(UserCommandAction::run("first")),
            },
            LinkHandler {
                schemes: vec!["https".into(), "file".into()],
                action: LinkAction::Inline(UserCommandAction::run("second")),
            },
        ];
        assert_eq!(handler(&handlers, "HTTPS"), Some(&handlers[0].action));
        assert_eq!(handler(&handlers, "file"), Some(&handlers[1].action));
        assert_eq!(handler(&handlers, "mailto"), None);
    }

    #[test]
    fn path_positions_preserve_drive_letters_and_nonnumeric_colons() {
        assert_eq!(
            path_position("./src/main.rs:12:3"),
            ("./src/main.rs", "12", "3")
        );
        assert_eq!(
            path_position("C:\\src\\main.rs:12"),
            ("C:\\src\\main.rs", "12", "")
        );
        assert_eq!(
            path_position("./file:with:colon"),
            ("./file:with:colon", "", "")
        );
        assert_eq!(path_position("./file:"), ("./file:", "", ""));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        Msg,
        config::HintConfig,
        state::{HintModeState, Mode},
    };
    use tui_lipan::{
        TestBackend,
        utils::hints::{HintKind, HintScan},
    };

    #[test]
    fn clicks_and_shift_hints_pass_literal_values_and_source_directory() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(check_opening_routes)
            .unwrap()
            .join()
            .unwrap();
    }

    fn check_opening_routes() {
        crate::test_support::isolate_user_dirs();
        let scratch = tempfile::tempdir().unwrap();
        let out = scratch.path().join("opened");
        let mut backend = TestBackend::new(AppRoot::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while backend.state().command_link.is_none() {
            assert!(std::time::Instant::now() < deadline);
            backend.pump().unwrap();
            std::thread::yield_now();
        }
        let source = backend.state().current().workspaces[0].panes[0].id;
        let action = UserCommandAction::Exec {
            command: format!(
                "printf '%s\\n' \"$ROZI_URL\" \"$ROZI_FILE\" \"$ROZI_HINT\" \"$ROZI_PANE\" \"$PWD\" \"$ROZI_LINE\" \"$ROZI_COLUMN\" > '{}'",
                out.display()
            ),
        };
        backend.state_mut().config.link_handlers = vec![LinkHandler {
            schemes: vec!["https".into(), "file".into()],
            action: LinkAction::Inline(action.clone()),
        }];
        backend.state_mut().config.hints = vec![HintConfig {
            pattern: regex_lite::Regex::new("ISSUE-[0-9]+").unwrap(),
            open: false,
            on_open: Some(LinkAction::Inline(action)),
        }];
        backend.state_mut().current_mut().workspaces[0].panes[0]
            .terminal
            .cwd = Some(scratch.path().display().to_string());
        let dangerous = "https://example.test/$(touch should-not-exist);quoted'";
        let file_url = url::Url::from_file_path(scratch.path().join("file with space"))
            .unwrap()
            .to_string();
        for (text, hint) in [
            (dangerous, None),
            (dangerous, Some(HintKind::Url)),
            (file_url.as_str(), None),
            ("./src/main.rs:12:3", Some(HintKind::Path)),
            ("ISSUE-123", Some(HintKind::Custom(0))),
        ] {
            if let Some(kind) = hint {
                let mut matches = HintScan::new().scan("https://example.test");
                matches[0].text = text.to_string();
                matches[0].kind = kind;
                backend.state_mut().hint_mode = Some(HintModeState {
                    target: source,
                    matches,
                    labels: vec!["a".into()],
                    input: String::new(),
                    offset: 0,
                });
                backend.state_mut().mode = Mode::Hint;
                backend
                    .dispatch(Msg::PaneKey(
                        source,
                        KeyEvent {
                            code: KeyCode::Char('a'),
                            mods: KeyMods::SHIFT,
                        },
                    ))
                    .unwrap();
                assert!(backend.state().hint_mode.is_none());
            } else {
                backend
                    .dispatch(Msg::PaneLinkActivate(source, text.into()))
                    .unwrap();
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let output = loop {
                backend.pump().unwrap();
                if let Ok(output) = std::fs::read_to_string(&out)
                    && output.lines().count() == 7
                {
                    break output;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "handler did not finish"
                );
                std::thread::yield_now();
            };
            let lines: Vec<_> = output.lines().collect();
            let uri = if hint.is_none() || hint == Some(HintKind::Url) {
                text
            } else {
                ""
            };
            assert_eq!(lines[0], uri);
            assert_eq!(lines[2], text);
            assert_eq!(lines[3], source.to_string());
            assert_eq!(
                std::fs::canonicalize(lines[4]).unwrap(),
                scratch.path().canonicalize().unwrap()
            );
            if text.starts_with("file:") {
                assert_eq!(
                    lines[1],
                    scratch.path().join("file with space").display().to_string()
                );
            } else if hint == Some(HintKind::Path) {
                assert_eq!((lines[1], lines[5], lines[6]), ("./src/main.rs", "12", "3"));
            } else {
                assert_eq!(lines[1], "");
            }
            assert!(!scratch.path().join("should-not-exist").exists());
            std::fs::remove_file(&out).unwrap();
        }

        backend.state_mut().config.commands.push(crate::config::NamedCommand {
            id: "browser.open".into(),
            label: None,
            action: UserCommandAction::ExecDirect { argv: vec!["/bin/sh".into(), "-c".into(), format!(
                "printf '%s\\n' \"$ROZI_URL\" \"$ROZI_EXTENSION_GENERATION\" \"$ROZI_EXTENSION_CONFIG\" > '{}'", out.display())] },
            category: "Browser".into(),
            env: vec![
                ("ROZI_EXTENSION".into(), "browser".into()),
                ("ROZI_EXTENSION_GENERATION".into(), "live-generation".into()),
                ("ROZI_EXTENSION_CONFIG".into(), "{\"reuse\":true}".into()),
            ],
            default_key: None,
            hidden: false,
        });
        backend.state_mut().config.link_handlers[0].action =
            LinkAction::Command("browser.open".into());
        backend
            .dispatch(Msg::PaneLinkActivate(source, dangerous.into()))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            backend.pump().unwrap();
            if let Ok(output) = std::fs::read_to_string(&out)
                && output.lines().count() == 3
            {
                assert_eq!(
                    output.lines().collect::<Vec<_>>(),
                    [dangerous, "live-generation", "{\"reuse\":true}"]
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        std::fs::remove_file(&out).unwrap();
        // A configured reference must resolve again after removal rather than cache an old launch.
        backend.state_mut().config.commands.clear();
        backend
            .dispatch(Msg::PaneLinkActivate(source, dangerous.into()))
            .unwrap();
        backend.pump().unwrap();
        assert!(!out.exists());
    }
}
