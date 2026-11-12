//! User-selected opening policy shared by terminal clicks and hint mode.
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;
use tui_lipan::utils::hints::PathPosition;

use crate::AppRoot;
use crate::config::{LinkAction, LinkHandler, UserCommandAction};
use crate::pane::id::PaneId;
use crate::state::State;

/// The first handler that opens a `scheme` link, and for a `file` link, the file at `path`.
pub(crate) fn handler<'a>(
    handlers: &'a [LinkHandler],
    scheme: &str,
    path: Option<&str>,
) -> Option<&'a LinkAction> {
    handlers
        .iter()
        .find(|handler| handler.opens(scheme, path))
        .map(|handler| &handler.action)
}

pub(crate) fn open(ctx: &mut Context<AppRoot>, source: PaneId, text: &str) -> Update {
    if let Ok(url) = url::Url::parse(text) {
        let file = if url.scheme() == "file" {
            match url
                .to_file_path()
                .ok()
                .and_then(|path| path.into_os_string().into_string().ok())
            {
                Some(path) => Some(path),
                None => {
                    return failed(ctx, "File URL cannot be represented as a path on this host");
                }
            }
        } else {
            None
        };
        let destination = match handler(
            &ctx.state.config.link_handlers,
            url.scheme(),
            file.as_deref(),
        )
        .cloned()
        {
            Some(action) => crate::state::OpenDestination::Handler(action),
            None => crate::state::OpenDestination::System,
        };
        if let Some(request) =
            super::ports::forward_request_for_link(&ctx.state, source, &url, &destination)
        {
            return super::ports::open_forwarded_link(ctx, source, text, request, destination);
        }
        return open_destination_with_file(
            ctx,
            source,
            &destination,
            text,
            text,
            file.as_deref().unwrap_or_default(),
        );
    }
    match tui_lipan::utils::open_url(text) {
        Ok(()) => Update::none(),
        Err(error) => failed(ctx, error.to_string()),
    }
}

fn open_destination_with_file(
    ctx: &mut Context<AppRoot>,
    source: PaneId,
    destination: &crate::state::OpenDestination,
    hint_text: &str,
    url_to_open: &str,
    file: &str,
) -> Update {
    match destination {
        crate::state::OpenDestination::Handler(action) => {
            let target = LinkTarget {
                uri: url_to_open,
                file,
                ..LinkTarget::default()
            };
            execute(ctx, source, action, hint_text, target)
        }
        crate::state::OpenDestination::System => match tui_lipan::utils::open_url(url_to_open) {
            Ok(()) => Update::none(),
            Err(error) => failed(ctx, error.to_string()),
        },
    }
}

/// Open a link a pane activated, by what the terminal recognized it as.
pub(crate) fn activate(
    ctx: &mut Context<AppRoot>,
    source: PaneId,
    kind: TerminalLinkKind,
    text: &str,
) -> Update {
    match kind {
        TerminalLinkKind::Path => open_path(ctx, source, text),
        _ => open(ctx, source, text),
    }
}

pub(crate) fn open_path(ctx: &mut Context<AppRoot>, source: PaneId, text: &str) -> Update {
    let position = PathPosition::parse(text);
    let opener = match PathLinks::for_pane(&ctx.state, source).open(position.path) {
        Ok(opener) => opener,
        Err(error) => return failed(ctx, error),
    };
    let target = LinkTarget {
        file: &opener.file,
        line: position.line,
        column: position.column,
        ..LinkTarget::default()
    };
    execute(ctx, source, &opener.action, text, target)
}

/// Which machine a process runs on, or a pane's files are on.
///
/// Without a remote session, both are this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Machine {
    /// The machine running this client.
    Client,
    /// The remote host of the session on screen.
    SessionHost,
}

/// Where the link handler `action` runs when the session on screen is `remote`.
pub(crate) fn handler_machine(state: &State, action: &LinkAction) -> Machine {
    if state.current().remote_host.is_none() && state.current().remote_target.is_none() {
        return Machine::Client;
    }
    let action = match action {
        LinkAction::Inline(action) => action,
        LinkAction::Command(id) => {
            if super::placement::placed_command(state, id).is_some() {
                return Machine::SessionHost;
            }
            match state
                .config
                .commands
                .iter()
                .find(|command| &command.id == id)
            {
                Some(command) => &command.action,
                None => return Machine::Client,
            }
        }
    };
    // As `user_command` launches them: a popup on the session server, a pane there unless the
    // scratchpad is open, and anything else here.
    match action {
        UserCommandAction::Popup { .. } => Machine::SessionHost,
        UserCommandAction::Run { .. } if !state.scratch_visible => Machine::SessionHost,
        _ => Machine::Client,
    }
}

/// Which machine a pane's terminal output and relative context belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanePlace {
    /// On this machine (a local session pane, or a client-local scratch pane).
    Client,
    /// On the remote host of the session on screen.
    SessionHost,
    /// On another host the pane's shell reports, such as over `ssh`.
    Elsewhere(String),
}

impl PanePlace {
    pub(crate) fn for_pane(state: &State, source: PaneId) -> Self {
        let pane = crate::pane::lifecycle::find_pane(state, source);
        // The pane's own report wins: a shell that moved to another host took its context with
        // it, whether the session runs here or remotely.
        if let Some(host) = pane.and_then(|pane| pane.terminal.cwd_host.clone()) {
            return Self::Elsewhere(host);
        }
        // Scratch panes run on this machine even while a remote session is on screen.
        if (state.current().remote_target.is_some() || state.current().remote_host.is_some())
            && !crate::scratchpad::contains(state, source)
        {
            return Self::SessionHost;
        }
        Self::Client
    }
}

/// Where a pane's paths are, and how they read there.
#[derive(Clone, Debug)]
pub(crate) enum PathPlace {
    /// On this machine, so a path can be made absolute and checked.
    Local {
        cwd: Option<PathBuf>,
        home: Option<PathBuf>,
    },
    /// On the remote host of the session on screen, which this client cannot look at.
    SessionHost,
    /// On another host the pane's shell reports, such as over `ssh`, where no handler runs.
    Elsewhere(String),
}

impl PathPlace {
    fn for_pane(state: &State, source: PaneId) -> Self {
        match PanePlace::for_pane(state, source) {
            PanePlace::Elsewhere(host) => Self::Elsewhere(host),
            PanePlace::SessionHost => Self::SessionHost,
            PanePlace::Client => {
                let pane = crate::pane::lifecycle::find_pane(state, source);
                Self::Local {
                    cwd: pane
                        .and_then(|pane| pane.local_cwd_ref())
                        .map(PathBuf::from),
                    home: crate::platform::paths::home_directory().map(PathBuf::from),
                }
            }
        }
    }

    fn machine(&self) -> Option<Machine> {
        match self {
            Self::Local { .. } => Some(Machine::Client),
            Self::SessionHost => Some(Machine::SessionHost),
            Self::Elsewhere(_) => None,
        }
    }
}

/// The `file` handlers that can open a pane's paths, with the machine each one runs on.
#[derive(Clone, Debug)]
pub(crate) struct PathLinks {
    place: PathPlace,
    handlers: Vec<(LinkHandler, Machine)>,
    session_host: Option<String>,
}

/// A path ready for its handler.
pub(crate) struct PathOpener {
    pub action: LinkAction,
    /// The `ROZI_FILE` the handler receives.
    pub file: String,
}

impl PathLinks {
    pub(crate) fn for_pane(state: &State, source: PaneId) -> Self {
        Self {
            place: PathPlace::for_pane(state, source),
            handlers: state
                .config
                .link_handlers
                .iter()
                .filter(|handler| handler.schemes.iter().any(|scheme| scheme == "file"))
                .map(|handler| (handler.clone(), handler_machine(state, &handler.action)))
                .collect(),
            session_host: state.current().remote_host.clone(),
        }
    }

    /// The handler for `path`, without its position suffix, and the `ROZI_FILE` it receives, or
    /// why it cannot be opened.
    pub(crate) fn open(&self, path: &str) -> std::result::Result<PathOpener, String> {
        let (action, runs_on) = self
            .handler(path)
            .ok_or_else(|| format!("No link handler opens {path}"))?;
        let file = match &self.place {
            PathPlace::Elsewhere(host) => return Err(format!("{path} is on {host}")),
            PathPlace::SessionHost => path.to_string(),
            PathPlace::Local { cwd, home } => local_path(path, cwd.as_deref(), home.as_deref())
                .filter(|resolved| resolved.exists())
                .ok_or_else(|| format!("No such file or directory: {path}"))?
                .into_os_string()
                .into_string()
                .map_err(|_| format!("{path} cannot be passed to a handler on this host"))?,
        };
        if self.place.machine() != Some(runs_on) {
            let host = self.session_host.as_deref().unwrap_or("the session host");
            let (files, handler) = match runs_on {
                Machine::Client => (host, "this machine"),
                Machine::SessionHost => ("this machine", host),
            };
            return Err(format!(
                "{path} is on {files}, but its link handler runs on {handler}"
            ));
        }
        Ok(PathOpener {
            action: action.clone(),
            file,
        })
    }

    /// Whether `text`, a path as a pane shows it, is a link: a handler on the machine the path is
    /// on opens files of its name, and the file exists, or cannot be checked from here.
    ///
    /// The terminal asks on every pointer movement over a path, so the last answer is reused
    /// for a moment instead of touching the file system again.
    pub(crate) fn accepts(&self, text: &str, cache: &PathLinkCache) -> bool {
        let path = PathPosition::parse(text).path;
        let Some((_, runs_on)) = self.handler(path) else {
            return false;
        };
        if self.place.machine() != Some(runs_on) {
            return false;
        }
        match &self.place {
            // Unchecked, a bare name such as `example.com` or `e.g` is too often not a file.
            PathPlace::SessionHost => path.contains(['/', '\\']),
            PathPlace::Elsewhere(_) => false,
            PathPlace::Local { cwd, home } => local_path(path, cwd.as_deref(), home.as_deref())
                .is_some_and(|resolved| cache.exists(resolved)),
        }
    }

    /// Whether any handler opens files at all, which makes the pane look for paths.
    pub(crate) fn any(&self) -> bool {
        !self.handlers.is_empty()
    }

    fn handler(&self, path: &str) -> Option<(&LinkAction, Machine)> {
        self.handlers
            .iter()
            .find(|(handler, _)| handler.opens("file", Some(path)))
            .map(|(handler, machine)| (&handler.action, *machine))
    }
}

/// `path` as an absolute path on this machine, or `None` when it is relative to nothing known.
fn local_path(path: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let home_relative = path
        .strip_prefix('~')
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', std::path::MAIN_SEPARATOR]));
    if let Some(rest) = home_relative {
        let rest = rest.trim_start_matches(['/', std::path::MAIN_SEPARATOR]);
        return Some(home?.join(rest));
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    // Drop the `.` of `./src/main.rs` so the handler gets the path a user would write. `..` stays:
    // removing it without asking the file system would be wrong across a symbolic link.
    Some(
        cwd?.join(path)
            .components()
            .filter(|part| *part != std::path::Component::CurDir)
            .collect(),
    )
}

/// The last file-system answer given for a hovered path.
#[derive(Clone, Debug, Default)]
pub(crate) struct PathLinkCache(Rc<RefCell<Option<(PathBuf, bool, Instant)>>>);

impl PathLinkCache {
    /// How long an answer holds. Long enough to cover the pointer crossing one path, short enough
    /// that a file created or deleted since reads correctly when the pointer comes back.
    const FRESH: Duration = Duration::from_secs(1);

    fn exists(&self, path: PathBuf) -> bool {
        let now = Instant::now();
        if let Some((cached, exists, at)) = self.0.borrow().as_ref()
            && *cached == path
            && now.duration_since(*at) < Self::FRESH
        {
            return *exists;
        }
        let exists = path.exists();
        *self.0.borrow_mut() = Some((path, exists, now));
        exists
    }
}

/// The link a handler opens, as it reaches the handler's `ROZI_*` variables.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LinkTarget<'a> {
    pub uri: &'a str,
    pub file: &'a str,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

pub(crate) fn resolve_link_command(
    state: &State,
    id: &str,
) -> std::result::Result<crate::config::NamedCommand, String> {
    match state
        .config
        .commands
        .iter()
        .find(|command| command.id == id)
        .cloned()
    {
        Some(command) if !matches!(command.action, UserCommandAction::Send(_)) => Ok(command),
        _ => Err(format!(
            "Link command `{id}` is unavailable or does not launch a process"
        )),
    }
}

pub(crate) fn link_env(
    source: PaneId,
    source_session: Option<&crate::session::protocol::SessionInstanceId>,
    remote_host: Option<&str>,
    text: &str,
    target: LinkTarget<'_>,
    include_pane: bool,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("ROZI_HINT".into(), text.into()),
        ("ROZI_URL".into(), target.uri.into()),
        ("ROZI_FILE".into(), target.file.into()),
        (
            "ROZI_LINE".into(),
            target.line.map(|line| line.to_string()).unwrap_or_default(),
        ),
        (
            "ROZI_COLUMN".into(),
            target
                .column
                .map(|column| column.to_string())
                .unwrap_or_default(),
        ),
        ("ROZI_SOURCE_PANE".into(), source.to_string()),
        (
            "ROZI_SOURCE_SESSION".into(),
            source_session
                .map(|instance| instance.as_str().to_string())
                .unwrap_or_default(),
        ),
        (
            "ROZI_REMOTE_HOST".into(),
            remote_host.unwrap_or_default().to_string(),
        ),
    ];
    if include_pane {
        env.push(("ROZI_PANE".into(), source.to_string()));
    }
    env
}

pub(crate) fn execute(
    ctx: &mut Context<AppRoot>,
    source: PaneId,
    action: &LinkAction,
    text: &str,
    target: LinkTarget<'_>,
) -> Update {
    let named = match action {
        LinkAction::Inline(_) => None,
        LinkAction::Command(id) => match resolve_link_command(&ctx.state, id) {
            Ok(command) => Some(command),
            Err(error) => return failed(ctx, error),
        },
    };
    let action = match (&named, action) {
        (Some(command), _) => &command.action,
        (None, LinkAction::Inline(action)) => action,
        _ => unreachable!(),
    };
    if text.contains('\0') || target.file.contains('\0') {
        return failed(ctx, "Link contains a NUL character");
    }
    let env = link_env(
        source,
        ctx.state.current().session_instance.as_ref(),
        ctx.state.current().remote_host.as_deref(),
        text,
        target,
        matches!(
            action,
            UserCommandAction::Exec { .. } | UserCommandAction::ExecDirect { .. }
        ),
    );
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

pub(crate) fn failed(ctx: &mut Context<AppRoot>, error: impl Into<String>) -> Update {
    crate::pane::pty_events::notify_error(ctx, "Could not open link", error.into());
    Update::full()
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn file_handler(files: &[&str], action: UserCommandAction) -> LinkHandler {
        LinkHandler {
            schemes: vec!["file".into()],
            files: files.iter().map(|file| file.to_string()).collect(),
            action: LinkAction::Inline(action),
        }
    }

    #[test]
    fn paths_open_only_with_a_handler_on_their_machine() {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::write(scratch.path().join("notes.md"), "").unwrap();
        std::fs::create_dir(scratch.path().join("project")).unwrap();
        let cache = PathLinkCache::default();
        let links = |place, files: &[&str], machine| PathLinks {
            place,
            handlers: vec![(file_handler(files, UserCommandAction::run("open")), machine)],
            session_host: Some("buildbox".into()),
        };
        let local = || PathPlace::Local {
            cwd: Some(scratch.path().join("project")),
            home: Some(scratch.path().to_path_buf()),
        };

        let here = links(local(), &[], Machine::Client);
        let notes = scratch.path().join("notes.md").display().to_string();
        assert_eq!(
            here.open("~/notes.md").map(|opener| opener.file),
            Ok(notes.clone())
        );
        assert!(here.accepts("../notes.md:4:2", &cache));
        assert!(here.open("./missing.md").is_err());
        assert!(!here.accepts("./missing.md", &cache));
        // Routing by name decides too: no handler takes a Markdown file here.
        assert!(!links(local(), &["*.png"], Machine::Client).accepts("../notes.md", &cache));

        assert!(
            !links(PathPlace::Elsewhere("prod".into()), &[], Machine::Client)
                .accepts(&notes, &cache)
        );
        // A remote path opens, unchecked, on its host; a bare name there is too often not a file.
        let remote = links(PathPlace::SessionHost, &[], Machine::SessionHost);
        assert_eq!(
            remote.open("src/a.rs").map(|opener| opener.file),
            Ok("src/a.rs".into())
        );
        assert!(remote.accepts("src/a.rs:3", &cache));
        assert!(!remote.accepts("a.rs", &cache));
        // A handler on the other machine would open some other file, or none.
        assert!(!links(PathPlace::SessionHost, &[], Machine::Client).accepts("src/a.rs", &cache));
        assert!(
            links(PathPlace::SessionHost, &[], Machine::Client)
                .open("src/a.rs")
                .is_err()
        );
        assert!(!links(local(), &[], Machine::SessionHost).accepts("../notes.md", &cache));
    }

    #[test]
    fn panes_and_handlers_name_the_machine_they_run_on() {
        use crate::state::Pane;
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().remote_host = Some("buildbox".into());
        let mut ssh = Pane::new(7, 100, Default::default());
        ssh.terminal.cwd_host = Some("prod".into());
        state.current_mut().workspaces[0].panes.push(ssh);
        state.current_mut().workspaces[0]
            .panes
            .push(Pane::new(8, 100, Default::default()));
        state
            .scratch
            .panes
            .push(Pane::new(9, 100, Default::default()));
        assert!(
            matches!(PathPlace::for_pane(&state, 7), PathPlace::Elsewhere(host) if host == "prod")
        );
        assert!(matches!(
            PathPlace::for_pane(&state, 8),
            PathPlace::SessionHost
        ));
        assert!(matches!(
            PathPlace::for_pane(&state, 9),
            PathPlace::Local { .. }
        ));

        let machine = |state: &State, action| handler_machine(state, &LinkAction::Inline(action));
        assert_eq!(
            machine(&state, UserCommandAction::popup("open")),
            Machine::SessionHost
        );
        assert_eq!(
            machine(&state, UserCommandAction::run("open")),
            Machine::SessionHost
        );
        assert_eq!(
            machine(
                &state,
                UserCommandAction::Exec {
                    command: "open".into()
                }
            ),
            Machine::Client
        );
        state.scratch_visible = true;
        assert_eq!(
            machine(&state, UserCommandAction::run("open")),
            Machine::Client
        );
    }

    #[test]
    fn first_handler_matching_scheme_and_file_name_wins() {
        let handler_for = |schemes: &[&str], files: &[&str], run: &str| LinkHandler {
            schemes: schemes.iter().map(|scheme| scheme.to_string()).collect(),
            files: files.iter().map(|file| file.to_string()).collect(),
            action: LinkAction::Inline(UserCommandAction::run(run)),
        };
        let handlers = vec![
            handler_for(&["https"], &[], "browser"),
            handler_for(&["file"], &["*.png", "*.jp?g"], "images"),
            handler_for(&["https", "file"], &[], "editor"),
        ];
        let opened = |scheme, path| {
            handlers
                .iter()
                .position(|candidate| Some(&candidate.action) == handler(&handlers, scheme, path))
        };
        assert_eq!(opened("HTTPS", None), Some(0));
        assert_eq!(opened("file", Some("shots/Logo.PNG")), Some(1));
        assert_eq!(opened("file", Some(r"C:\shots\a.jpeg")), Some(1));
        assert_eq!(opened("file", Some("shots.png/notes.md")), Some(2));
        assert_eq!(opened("mailto", None), None);
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
        crate::test_support::on_large_stack(check_opening_routes);
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
            files: Vec::new(),
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
        std::fs::create_dir(scratch.path().join("src")).unwrap();
        std::fs::write(scratch.path().join("src/main.rs"), "").unwrap();
        let path = "./src/main.rs:12:3";
        for (text, hint, clicked) in [
            (dangerous, None, TerminalLinkKind::Url),
            (dangerous, Some(HintKind::Url), TerminalLinkKind::Url),
            (file_url.as_str(), None, TerminalLinkKind::Hyperlink),
            (path, Some(HintKind::Path), TerminalLinkKind::Path),
            (path, None, TerminalLinkKind::Path),
            (
                "ISSUE-123",
                Some(HintKind::Custom(0)),
                TerminalLinkKind::Url,
            ),
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
                    .dispatch(Msg::PaneLinkActivate(source, clicked, text.into()))
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
            let uri = if clicked != TerminalLinkKind::Path
                && matches!(hint, None | Some(HintKind::Url))
            {
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
            } else if clicked == TerminalLinkKind::Path {
                let file = scratch.path().join("src/main.rs").display().to_string();
                assert_eq!((lines[1], lines[5], lines[6]), (file.as_str(), "12", "3"));
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
            .dispatch(Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                dangerous.into(),
            ))
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
            .dispatch(Msg::PaneLinkActivate(
                source,
                TerminalLinkKind::Url,
                dangerous.into(),
            ))
            .unwrap();
        backend.pump().unwrap();
        assert!(!out.exists());
    }
}
