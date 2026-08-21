use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::FloatRect;

use crate::layout_tree_ser::{
    SerializedLayoutKind, SerializedSplitAxis, SerializedTree, from_dwindle, to_dwindle,
};
use crate::state::{Pane, PaneId, State, WORKSPACE_COUNT, Workspace};
use crate::tiling::DwindleTree;
use crate::tiling::append_tiled_window;

pub fn load_profile(path: &Path) -> Result<Profile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("Could not read profile {}: {err}", path.display()))?;
    Profile::from_toml_str(&text)
        .map_err(|err| format!("Could not parse profile {}: {err}", path.display()))
}

pub fn save_profile(path: &Path, profile: &Profile) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Could not create profile directory {}: {err}",
                parent.display()
            )
        })?;
    }

    let text = profile
        .to_toml_string()
        .map_err(|err| format!("Could not serialize profile {}: {err}", path.display()))?;

    std::fs::write(path, text)
        .map_err(|err| format!("Could not write profile {}: {err}", path.display()))
}

/// Resolve the session-autosave file path: the configured override, else
/// `$XDG_STATE_HOME/rozi/session.toml` (falling back to `~/.local/state/...`).
pub fn session_path(config: &crate::config::Config) -> Option<PathBuf> {
    if let Some(path) = &config.session.path {
        return Some(path.clone());
    }
    let env = crate::platform::paths::PlatformEnv::from_process();
    if env.home.is_none() && env.xdg_state_home.is_none() {
        // No usable state-directory source at all: preserve the historical "autosave silently
        // does nothing" behavior rather than falling back to a cwd-relative `.local/state`.
        return None;
    }
    Some(crate::platform::paths::state_dir(&env).join("session.toml"))
}

/// Write the live layout to the session file when `[session] autosave` is enabled. Called
/// synchronously just before quit, so failures are reported to stderr (toasts won't render).
pub fn persist_session_if_enabled(state: &State) {
    if !state.config.session.autosave {
        return;
    }
    persist_session_to_disk(state);
}

/// Always write the live layout on detach from local mode so leaving the client is recoverable.
pub fn persist_session_on_detach(state: &State) {
    persist_session_to_disk(state);
}

fn persist_session_to_disk(state: &State) {
    let Some(path) = session_path(&state.config) else {
        return;
    };
    let profile = profile_from_state(state);
    if let Err(err) = save_profile(&path, &profile) {
        eprintln!("rozi: session autosave failed: {err}");
    }
}

pub fn profile_from_state(state: &State) -> Profile {
    let shells = shell_basenames(&state.config);
    Profile {
        version: 1,
        active_workspace: state.current().active_workspace,
        workspaces: state
            .current()
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, workspace)| workspace.panes.iter().any(|pane| !pane.closing))
            .map(|(index, workspace)| workspace_profile_from_state(index, workspace, &shells))
            .collect(),
    }
}

pub fn restore_state_from_profile(
    config: crate::config::Config,
    theme: tui_lipan::prelude::Theme,
    profile: Profile,
) -> State {
    // A profile only ever seeds the *attachment* (the window-manager layout); everything else on
    // `State` is the same as a fresh launch, so build one and drop the profile's attachment in.
    match attachment_from_profile(&config, profile) {
        Some(attachment) => {
            let mut state = State::new(config, theme);
            state.attachment = attachment;
            state
        }
        None => State::new(config, theme),
    }
}

/// The seed a session starts from when the user named no recipe for it, paired with the attach
/// intent that records where it came from: the configured `[profile] default` when one is set and
/// loads, otherwise a single shell.
///
/// Startup resolves the same default separately (see `AppRoot::create_state`). Every *other*
/// path that opens a blank session goes through here, which is what makes `[profile] default` mean
/// "every session I did not otherwise specify" rather than "only the first one this process
/// opened".
///
/// A default that is missing or unreadable falls back to a blank session instead of refusing to
/// open one: startup already reported the failure, and the session the user asked for should still
/// appear.
pub(crate) fn default_session_seed(
    config: &crate::config::Config,
) -> (crate::state::Attachment, crate::state::AttachIntent) {
    let blank = || {
        (
            crate::state::fresh_default_attachment(config),
            crate::state::AttachIntent::Plain,
        )
    };
    let Some(name) = config.profile.default.as_deref() else {
        return blank();
    };
    let path = crate::config::profile_path_for_name(name);
    let Ok(profile) = load_profile(&path) else {
        return blank();
    };
    match attachment_from_profile(config, profile) {
        Some(attachment) => (
            attachment,
            crate::state::AttachIntent::ProfileSeed {
                profile: name.to_string(),
                path,
            },
        ),
        None => blank(),
    }
}

/// Build the window-manager attachment a profile describes: its workspaces, panes (as identities,
/// not yet PTY-backed), tile trees, focus, and active workspace. Returns `None` when the profile has
/// no panes, so the caller falls back to a fresh default attachment. The panes are spawned on the
/// session server once the attach completes (see `spawn_state_panes_on_session`).
pub(crate) fn attachment_from_profile(
    config: &crate::config::Config,
    profile: Profile,
) -> Option<crate::state::Attachment> {
    if profile
        .workspaces
        .iter()
        .all(|workspace| workspace.panes.is_empty())
    {
        return None;
    }

    let scrollback = config.scrollback;
    let mut workspaces: Vec<Workspace> = (0..WORKSPACE_COUNT).map(Workspace::new).collect();
    // Workspaces the profile does not name inherit the configured default; the ones it does name
    // overwrite `layout_kind` from the saved profile below.
    for workspace in &mut workspaces {
        workspace.layout_kind = config.layout.default;
    }
    let mut next_pane_id = 1;

    for workspace_profile in profile.workspaces {
        if workspace_profile.index >= WORKSPACE_COUNT {
            continue;
        }

        let workspace = &mut workspaces[workspace_profile.index];
        workspace.name = workspace_profile
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        workspace.synchronized = workspace_profile.synchronized;
        workspace.layout_kind = workspace_profile.layout.into();
        if !workspace_profile.split_ratios.is_empty() {
            workspace.split_ratios = workspace_profile.split_ratios.clone();
        }

        let mut profile_pane_ids = HashMap::new();
        for pane_profile in &workspace_profile.panes {
            let id = pane_profile.pane_id.unwrap_or(next_pane_id);
            next_pane_id = next_pane_id.max(id.saturating_add(1));
            profile_pane_ids.insert(pane_profile.id, id);

            let mut pane = Pane::new(id, scrollback, pane_profile.rect.unwrap_or_default().into());
            let profile_name = pane_profile
                .name
                .clone()
                .or_else(|| pane_profile.title.clone());
            pane.identity.custom_title = profile_name.clone();
            pane.identity.profile_name = profile_name;
            pane.identity.cwd = pane_profile.cwd.as_ref().map(|path| {
                crate::config::expand_path(path)
                    .to_string_lossy()
                    .to_string()
            });
            pane.identity.launch = pane_profile
                .argv
                .clone()
                .and_then(|argv| crate::pane_launch::PaneLaunch::direct(argv).ok())
                .or_else(|| {
                    pane_profile
                        .command
                        .clone()
                        .filter(|command| !command.trim().is_empty())
                        .map(crate::pane_launch::PaneLaunch::shell)
                });
            // Profile commands are replayed through the interactive shell (typed at the prompt)
            // rather than the command-runner shell: they were captured from what the user ran
            // interactively, so aliases, shell functions, and rc-file PATH entries must resolve.
            pane.identity.replay = matches!(
                pane.identity.launch.as_ref(),
                Some(crate::pane_launch::PaneLaunch::Shell { .. })
            );
            pane.identity.keep_open = pane_profile.keep_open;
            pane.floating = pane_profile.floating;
            pane.fullscreen = pane_profile.fullscreen;
            pane.scrollable_width = crate::tiling::sanitize_scrollable_width(
                pane_profile
                    .scrollable_width
                    .unwrap_or(crate::state::DEFAULT_SCROLLABLE_WIDTH),
            );
            workspace.panes.push(pane);
        }

        workspace.focused_pane = workspace_profile
            .focused_pane
            .and_then(|profile_id| profile_pane_ids.get(&profile_id).copied())
            .or_else(|| workspace.panes.first().map(|pane| pane.id));

        workspace.tile_tree = workspace_profile
            .tree
            .as_ref()
            .and_then(|tree| restore_dwindle_tree(tree, &profile_pane_ids));

        if workspace.tile_tree.is_none() {
            let tiled_ids: Vec<PaneId> = workspace
                .panes
                .iter()
                .filter(|pane| !pane.floating)
                .map(|pane| pane.id)
                .collect();
            for id in tiled_ids {
                append_tiled_window(workspace, id);
            }
        }
    }

    let active_workspace = if profile.active_workspace < WORKSPACE_COUNT {
        profile.active_workspace
    } else {
        0
    };
    let focused_pane = workspaces[active_workspace].focused_pane;

    if workspaces
        .iter()
        .all(|workspace| workspace.panes.is_empty())
    {
        return None;
    }

    let mut attachment = crate::state::Attachment::new();
    attachment.workspaces = workspaces;
    attachment.active_workspace = active_workspace;
    attachment.focused_pane = focused_pane;
    attachment.next_pane_id = next_pane_id;
    Some(attachment)
}

pub fn replace_layout_from_profile(state: &mut State, mut profile: Profile, first_pane_id: PaneId) {
    if first_pane_id > 1
        && profile
            .workspaces
            .iter()
            .all(|workspace| workspace.panes.is_empty())
    {
        profile.workspaces = vec![WorkspaceProfile {
            index: 0,
            panes: vec![PaneProfile {
                id: 0,
                pane_id: Some(first_pane_id),
                ..PaneProfile::default()
            }],
            ..WorkspaceProfile::default()
        }];
    }
    if first_pane_id > 1 {
        let mut next = first_pane_id;
        for workspace in &mut profile.workspaces {
            for pane in &mut workspace.panes {
                pane.pane_id = Some(next);
                next = next.saturating_add(1);
            }
        }
    }
    let mut restored =
        restore_state_from_profile(state.config.clone(), state.theme.clone(), profile);
    let restored = restored.current_mut();
    let next_pane_id = restored.next_pane_id.max(first_pane_id);
    let current = state.current_mut();
    current.workspaces = std::mem::take(&mut restored.workspaces);
    current.active_workspace = restored.active_workspace;
    current.focused_pane = restored.focused_pane;
    current.next_pane_id = next_pane_id;
}

fn restore_dwindle_tree(
    tree: &ProfileTree,
    profile_pane_ids: &HashMap<PaneId, PaneId>,
) -> Option<DwindleTree> {
    to_dwindle(tree, &|pane| profile_pane_ids.get(pane).copied(), false)
}

fn workspace_profile_from_state(
    index: usize,
    workspace: &Workspace,
    shells: &HashSet<String>,
) -> WorkspaceProfile {
    let pane_indices: HashMap<PaneId, usize> = workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .enumerate()
        .map(|(index, pane)| (pane.id, index))
        .collect();

    WorkspaceProfile {
        index,
        name: workspace.name.clone(),
        synchronized: workspace.synchronized,
        layout: workspace.layout_kind.into(),
        split_ratios: workspace.split_ratios.clone(),
        focused_pane: workspace
            .focused_pane
            .and_then(|id| pane_indices.get(&id).copied())
            .map(|index| index as PaneId),
        tree: workspace
            .tile_tree
            .as_ref()
            .and_then(|tree| profile_tree_from_dwindle(tree, &pane_indices)),
        panes: workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .enumerate()
            .map(|(index, pane)| {
                let (command, argv) = match pane.identity.launch.as_ref() {
                    Some(crate::pane_launch::PaneLaunch::Direct { argv }) => {
                        (None, Some(argv.clone()))
                    }
                    Some(crate::pane_launch::PaneLaunch::Shell { command }) => (
                        live_running_command(pane, shells).or_else(|| Some(command.clone())),
                        None,
                    ),
                    None => (live_running_command(pane, shells), None),
                };
                PaneProfile {
                    id: index as PaneId,
                    pane_id: Some(pane.id),
                    name: pane
                        .identity
                        .custom_title
                        .clone()
                        .or_else(|| pane.identity.profile_name.clone()),
                    title: pane.identity.custom_title.clone(),
                    cwd: pane.local_cwd().map(PathBuf::from),
                    command,
                    argv,
                    keep_open: pane.identity.keep_open,
                    floating: pane.floating,
                    fullscreen: pane.fullscreen,
                    rect: pane.floating.then_some(pane.floating_rect.into()),
                    scrollable_width: {
                        let width = crate::tiling::sanitize_scrollable_width(pane.scrollable_width);
                        (width != crate::state::DEFAULT_SCROLLABLE_WIDTH).then_some(width)
                    },
                }
            })
            .collect(),
    }
}

/// The command a pane is running *right now*, if it is worth replaying on restore.
///
/// `foreground_program` keeps reporting the last executed command's executable while the shell
/// sits idle at a prompt (OSC 133 `rozi_exe=` is only replaced by the next command), so a pane
/// where the user merely changed directories would otherwise capture stale prompt machinery like
/// `__zoxide_hook` and replay it as a pane command. Only trust it while shell integration reports
/// a command mid-flight (`Executing`), or when there is no integration at all (`Unknown`) and the
/// value comes from the process inspector, which reads the live foreground process group.
///
/// What is captured is the whole invocation, not just the program: an agent started with
/// `--dangerously-skip-permissions` is a different pane from the same agent without it. The
/// program is named where a name is enough to find it again and given as a path where it is not -
/// one started through an alias, or straight out of a build tree, restores as `command not found`
/// if only its name is written down.
///
/// Neither the path nor the arguments belong to a pane attached over `--remote`: both describe a
/// process on the far host, which the machine doing the restoring is not. Those panes keep the
/// bare program name, as they did before either was captured.
fn live_running_command(pane: &Pane, shells: &HashSet<String>) -> Option<String> {
    use crate::session::protocol::PaneCommandPhase;

    match pane.terminal.command_phase {
        PaneCommandPhase::Executing | PaneCommandPhase::Unknown => {}
        PaneCommandPhase::Prompt | PaneCommandPhase::Input | PaneCommandPhase::Completed { .. } => {
            return None;
        }
    }
    let program = pane
        .terminal
        .foreground_program
        .as_deref()
        .filter(|program| {
            !shells.contains(&crate::platform::command::normalized_program_name(program))
        })?;
    if pane.terminal.cwd_host.is_some() {
        return Some(program.to_string());
    }
    let program = match pane.terminal.foreground_executable.as_deref() {
        Some(path) => shell_quote(path),
        None => program.to_string(),
    };
    Some(
        std::iter::once(program)
            .chain(
                pane.terminal
                    .foreground_arguments
                    .iter()
                    .map(|argument| shell_quote(argument)),
            )
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Quote `word` so an interactive shell reads it as one literal argument, since the captured
/// command is replayed by typing it at a prompt. Only reached for inspector-reported paths and
/// arguments, which exist on Unix alone - a Windows pane has neither.
fn shell_quote(word: &str) -> String {
    let plain = |ch: char| ch.is_ascii_alphanumeric() || "_-./:@%+=".contains(ch);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn shell_basenames(config: &crate::config::Config) -> HashSet<String> {
    let mut shells: HashSet<String> = [
        "bash",
        "zsh",
        "fish",
        "sh",
        "dash",
        "ksh",
        "tcsh",
        "csh",
        "nu",
        "pwsh",
        "powershell",
        "cmd",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let resolved = crate::platform::command::resolve_interactive_shell(
        config.shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    shells.insert(crate::platform::command::normalized_program_name(
        &resolved.program,
    ));
    shells
}

fn profile_tree_from_dwindle(
    tree: &DwindleTree,
    pane_indices: &HashMap<PaneId, usize>,
) -> Option<ProfileTree> {
    from_dwindle(tree, &|id| {
        pane_indices.get(&id).copied().map(|pane| pane as PaneId)
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub version: u32,
    pub active_workspace: usize,
    pub workspaces: Vec<WorkspaceProfile>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: 1,
            active_workspace: 0,
            workspaces: Vec::new(),
        }
    }
}

impl Profile {
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, String> {
        let profile: Self = toml::from_str(input).map_err(|error| error.to_string())?;
        for workspace in &profile.workspaces {
            for pane in &workspace.panes {
                if pane.command.is_some() && pane.argv.is_some() {
                    return Err(format!(
                        "workspace {} pane {} declares both `command` and `argv`",
                        workspace.index, pane.id
                    ));
                }
                if let Some(argv) = &pane.argv {
                    crate::pane_launch::PaneLaunch::direct(argv.clone()).map_err(|error| {
                        format!("workspace {} pane {}: {error}", workspace.index, pane.id)
                    })?;
                }
            }
        }
        Ok(profile)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceProfile {
    pub index: usize,
    pub name: Option<String>,
    pub synchronized: bool,
    pub layout: ProfileLayoutKind,
    pub split_ratios: Vec<f32>,
    pub focused_pane: Option<PaneId>,
    pub tree: Option<ProfileTree>,
    pub panes: Vec<PaneProfile>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneProfile {
    pub id: PaneId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<PaneId>,
    pub name: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    pub keep_open: bool,
    pub floating: bool,
    pub fullscreen: bool,
    pub rect: Option<ProfileRect>,
    /// Scrollable column width fraction; absent restores as [`crate::state::DEFAULT_SCROLLABLE_WIDTH`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollable_width: Option<f32>,
}

pub type ProfileTree = SerializedTree<PaneId>;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for ProfileRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }
}

impl From<FloatRect> for ProfileRect {
    fn from(rect: FloatRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

impl From<ProfileRect> for FloatRect {
    fn from(rect: ProfileRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

pub type ProfileLayoutKind = SerializedLayoutKind;
#[allow(dead_code)]
pub type ProfileSplitAxis = SerializedSplitAxis;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    use crate::config::Config;
    use crate::state::{LayoutKind, Pane, SplitAxis, State};
    use tui_lipan::prelude::Theme;

    /// `docs/profiles.md` points readers at this file as the profile format, so a schema change
    /// that leaves it behind hands them a broken starting point.
    #[test]
    fn documented_profile_example_still_loads() {
        let profile = Profile::from_toml_str(include_str!("../examples/profiles/dev.toml"))
            .expect("profile example parses");

        assert_eq!(profile.workspaces.len(), 1);
        let panes = &profile.workspaces[0].panes;
        assert_eq!(
            panes
                .iter()
                .map(|pane| pane.command.as_deref())
                .collect::<Vec<_>>(),
            [Some("lazygit"), Some("nvim")]
        );
    }

    #[test]
    fn client_scratchpad_is_excluded_from_profiles() {
        let mut state = State::new(Config::default(), Theme::default());
        let scratch_id = 1 << 31;
        state.scratch.panes.push(Pane::new(
            scratch_id,
            100,
            tui_lipan::prelude::FloatRect::default(),
        ));
        crate::tiling::append_tiled_window(&mut state.scratch, scratch_id);

        let profile = profile_from_state(&state);
        assert!(
            profile
                .workspaces
                .iter()
                .all(|workspace| { workspace.panes.iter().all(|pane| pane.id != scratch_id) })
        );
    }

    #[test]
    fn profile_tree_toml_shape_is_stable() {
        let profile = Profile {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceProfile {
                index: 0,
                name: Some("dev".to_string()),
                synchronized: true,
                layout: ProfileLayoutKind::Master,
                split_ratios: vec![0.4],
                focused_pane: Some(2),
                tree: Some(ProfileTree::Split {
                    axis: ProfileSplitAxis::Vertical,
                    ratio: 0.375,
                    first: Box::new(ProfileTree::Leaf { pane: 2 }),
                    second: Box::new(ProfileTree::Leaf { pane: 9 }),
                }),
                panes: Vec::new(),
            }],
        };

        assert_eq!(
            profile.to_toml_string().unwrap(),
            concat!(
                "version = 1\n",
                "active_workspace = 0\n",
                "\n",
                "[[workspaces]]\n",
                "index = 0\n",
                "name = \"dev\"\n",
                "synchronized = true\n",
                "layout = \"master\"\n",
                "split_ratios = [0.4]\n",
                "focused_pane = 2\n",
                "panes = []\n",
                "\n",
                "[workspaces.tree]\n",
                "kind = \"split\"\n",
                "axis = \"vertical\"\n",
                "ratio = 0.375\n",
                "\n",
                "[workspaces.tree.first]\n",
                "kind = \"leaf\"\n",
                "pane = 2\n",
                "\n",
                "[workspaces.tree.second]\n",
                "kind = \"leaf\"\n",
                "pane = 9\n",
            )
        );
    }

    fn assert_rect_eq(actual: ProfileRect, expected: ProfileRect) {
        assert_eq!(actual, expected);
    }

    fn cwd_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn snapshot_preserves_custom_name_and_floating_rect() {
        let mut state = State::new(Config::default(), Theme::default());
        let first = state.current_mut().workspaces[0]
            .panes
            .first_mut()
            .expect("initial pane");
        first.set_custom_title("editor");
        first.identity.profile_name = Some("profile-editor".to_string());
        first.identity.cwd = Some("/tmp/rozi-profile-test".to_string());
        first.identity.launch = Some(crate::pane_launch::PaneLaunch::shell("nvim src/main.rs"));
        first.fullscreen = true;
        state.current_mut().workspaces[0].split_ratios[0] = 0.63;

        let floating_rect = FloatRect {
            x: 11.0,
            y: 7.0,
            w: 91.0,
            h: 31.0,
        };
        let mut floating = Pane::new(2, state.config.scrollback, floating_rect);
        floating.floating = true;
        floating.identity.profile_name = Some("scratch".to_string());
        state.current_mut().workspaces[0].panes.push(floating);
        state.current_mut().workspaces[0].focused_pane = Some(2);
        state.current_mut().focused_pane = Some(2);

        let profile = profile_from_state(&state);

        assert_eq!(profile.version, 1);
        assert_eq!(profile.active_workspace, 0);
        let workspace = &profile.workspaces[0];
        assert!(!workspace.synchronized);
        assert_eq!(workspace.focused_pane, Some(1));
        assert_eq!(workspace.split_ratios[0], 0.63);
        assert_eq!(workspace.panes.len(), 2);
        assert_eq!(workspace.panes[0].id, 0);
        assert_eq!(workspace.panes[0].pane_id, Some(1));
        assert_eq!(workspace.panes[0].name.as_deref(), Some("editor"));
        assert_eq!(workspace.panes[0].title.as_deref(), Some("editor"));
        assert_eq!(
            workspace.panes[0].cwd.as_deref(),
            Some(std::path::Path::new("/tmp/rozi-profile-test"))
        );
        assert_eq!(
            workspace.panes[0].command.as_deref(),
            Some("nvim src/main.rs")
        );
        assert!(!workspace.panes[0].floating);
        assert!(workspace.panes[0].fullscreen);
        assert_eq!(workspace.panes[1].id, 1);
        assert_eq!(workspace.panes[1].pane_id, Some(2));
        assert_eq!(workspace.panes[1].name.as_deref(), Some("scratch"));
        assert_eq!(workspace.panes[1].title.as_deref(), None);
        assert!(workspace.panes[1].floating);
        assert_rect_eq(
            workspace.panes[1].rect.expect("floating rect"),
            floating_rect.into(),
        );
        assert_eq!(workspace.tree, Some(ProfileTree::Leaf { pane: 0 }));
    }

    #[test]
    fn restore_preserves_explicit_session_pane_ids_and_tree() {
        let profile = Profile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                focused_pane: Some(1),
                tree: Some(ProfileTree::Split {
                    axis: ProfileSplitAxis::Horizontal,
                    ratio: 0.42,
                    first: Box::new(ProfileTree::Leaf { pane: 0 }),
                    second: Box::new(ProfileTree::Leaf { pane: 1 }),
                }),
                panes: vec![
                    PaneProfile {
                        id: 0,
                        pane_id: Some(2),
                        ..PaneProfile::default()
                    },
                    PaneProfile {
                        id: 1,
                        pane_id: Some(3),
                        ..PaneProfile::default()
                    },
                ],
                ..WorkspaceProfile::default()
            }],
            ..Profile::default()
        };

        let state = State::from_profile(Config::default(), Theme::default(), profile);
        let workspace = &state.current().workspaces[0];

        assert_eq!(
            workspace
                .panes
                .iter()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(workspace.focused_pane, Some(3));
        assert_eq!(state.current().next_pane_id, 4);
        assert_eq!(
            workspace.tile_tree,
            Some(DwindleTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.42,
                first: Box::new(DwindleTree::Leaf(2)),
                second: Box::new(DwindleTree::Leaf(3)),
            })
        );
    }

    #[test]
    fn cleared_restored_name_does_not_round_trip_old_profile_name() {
        let profile = Profile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![PaneProfile {
                    id: 0,
                    name: Some("server".to_string()),
                    ..PaneProfile::default()
                }],
                ..WorkspaceProfile::default()
            }],
            ..Profile::default()
        };
        let mut state = State::from_profile(Config::default(), Theme::default(), profile);
        let pane_id = state.current().workspaces[0].panes[0].id;

        crate::ops::identity::rename_pane_in_workspaces(
            &mut state.current_mut().workspaces,
            pane_id,
            "",
        );
        let snapshot = profile_from_state(&state);
        let restored = State::from_profile(Config::default(), Theme::default(), snapshot);

        assert_eq!(
            restored.current().workspaces[0].panes[0]
                .identity
                .custom_title,
            None
        );
        assert_eq!(
            restored.current().workspaces[0].panes[0]
                .identity
                .profile_name,
            None
        );
    }

    /// A default that is not configured, or cannot be read, must still yield a working session
    /// rather than blocking one from opening.
    #[test]
    fn default_seed_falls_back_to_a_blank_session() {
        let config = Config::default();
        assert!(config.profile.default.is_none());
        let (attachment, intent) = default_session_seed(&config);
        assert_eq!(attachment.workspaces[0].panes.len(), 1);
        assert!(matches!(intent, crate::state::AttachIntent::Plain));

        let mut config = Config::default();
        config.profile.default = Some("rozi-no-such-profile-xyzzy".to_string());
        let (attachment, intent) = default_session_seed(&config);
        assert_eq!(attachment.workspaces[0].panes.len(), 1);
        assert!(
            matches!(intent, crate::state::AttachIntent::Plain),
            "an unreadable default must not claim the session came from it"
        );
    }

    /// `[profile] default` has to seed *every* session opened without a recipe, not only the launch
    /// that started rozi - creating a session later used to silently start blank.
    #[test]
    fn default_seed_restores_the_configured_profile_and_records_its_origin() {
        // Resolved rather than chosen: `default_session_seed` looks the profile up by name under
        // the config directory, which `test_support` has already pointed at a scratch root.
        let profiles = crate::config::profiles_dir();
        std::fs::create_dir_all(&profiles).expect("profiles dir");

        let profile = Profile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![
                    PaneProfile {
                        id: 0,
                        ..PaneProfile::default()
                    },
                    PaneProfile {
                        id: 1,
                        ..PaneProfile::default()
                    },
                ],
                ..WorkspaceProfile::default()
            }],
            ..Profile::default()
        };
        save_profile(&profiles.join("work.toml"), &profile.clone()).expect("write default profile");

        let mut config = Config::default();
        config.profile.default = Some("work".to_string());
        let (attachment, intent) = default_session_seed(&config);

        let _ = std::fs::remove_file(profiles.join("work.toml"));

        assert_eq!(
            attachment.workspaces[0].panes.len(),
            2,
            "the default profile's layout should seed the session"
        );
        match intent {
            crate::state::AttachIntent::ProfileSeed { profile, .. } => assert_eq!(profile, "work"),
            other => panic!("expected the seed to record its origin profile, got {other:?}"),
        }
    }

    #[test]
    fn restore_expands_tilde_cwd_before_launch_identity_is_stored() {
        // `test_support` already points `$HOME` at this process's scratch root, so the expansion
        // has a home to resolve against without any test touching the real one.
        let home = crate::test_support::isolate_user_dirs();
        let profile = Profile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![PaneProfile {
                    id: 0,
                    cwd: Some(PathBuf::from("~/code/my-app")),
                    ..PaneProfile::default()
                }],
                ..WorkspaceProfile::default()
            }],
            ..Profile::default()
        };

        let state = State::from_profile(Config::default(), Theme::default(), profile);

        let expected = home.join("code/my-app").to_string_lossy().to_string();
        assert_eq!(
            state.current().workspaces[0].panes[0]
                .identity
                .cwd
                .as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn snapshot_includes_only_live_panes() {
        let mut state = State::new(Config::default(), Theme::default());
        let mut exiting = Pane::new(
            2,
            state.config.scrollback,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        exiting.identity.custom_title = Some("exiting".to_string());
        state.current_mut().workspaces[0].panes.push(exiting);
        let profile = profile_from_state(&state);

        assert_eq!(profile.workspaces[0].panes.len(), 2);
        assert_eq!(
            profile.workspaces[0].panes[1].name.as_deref(),
            Some("exiting")
        );
    }

    #[test]
    fn profile_round_trips_named_pane_and_tree() {
        let profile = Profile {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceProfile {
                index: 0,
                name: Some("main".to_string()),
                synchronized: true,
                layout: ProfileLayoutKind::Dwindle,
                split_ratios: vec![0.61, 0.39],
                focused_pane: Some(1),
                tree: Some(ProfileTree::Split {
                    axis: ProfileSplitAxis::Horizontal,
                    ratio: 0.58,
                    first: Box::new(ProfileTree::Leaf { pane: 0 }),
                    second: Box::new(ProfileTree::Leaf { pane: 1 }),
                }),
                panes: vec![
                    PaneProfile {
                        id: 0,
                        name: Some("editor".to_string()),
                        title: Some("editor".to_string()),
                        cwd: Some(PathBuf::from("/tmp/rozi-profile-test")),
                        command: Some("nvim src/main.rs".to_string()),
                        floating: false,
                        fullscreen: true,
                        rect: Some(ProfileRect {
                            x: 1.0,
                            y: 2.0,
                            w: 80.0,
                            h: 24.0,
                        }),
                        ..PaneProfile::default()
                    },
                    PaneProfile {
                        id: 1,
                        name: Some("shell".to_string()),
                        title: Some("shell".to_string()),
                        cwd: Some(PathBuf::from("/tmp/rozi-profile-test")),
                        command: Some("bash -l".to_string()),
                        floating: false,
                        fullscreen: false,
                        rect: Some(ProfileRect {
                            x: 81.0,
                            y: 2.0,
                            w: 80.0,
                            h: 24.0,
                        }),
                        ..PaneProfile::default()
                    },
                ],
            }],
        };

        let encoded = profile.to_toml_string().expect("profile serializes");
        let decoded = Profile::from_toml_str(&encoded).expect("profile parses");

        assert_eq!(decoded, profile);
    }

    #[test]
    fn old_profile_without_synchronized_loads_false() {
        let profile = Profile::from_toml_str(
            r#"
            version = 1
            active_workspace = 0

            [[workspaces]]
            index = 0

            [[workspaces.panes]]
            id = 0
            "#,
        )
        .expect("old profile parses");

        assert!(!profile.workspaces[0].synchronized);
        let state = State::from_profile(Config::default(), Theme::default(), profile);
        assert!(!state.current().workspaces[0].synchronized);
    }

    #[test]
    fn synchronized_workspace_round_trips_from_state() {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().workspaces[0].synchronized = true;

        let profile = profile_from_state(&state);
        let restored = State::from_profile(Config::default(), Theme::default(), profile);

        assert!(restored.current().workspaces[0].synchronized);
    }

    #[test]
    fn save_captures_local_runtime_identity_without_remote_paths_or_shells() {
        let mut state = State::new(Config::default(), Theme::default());
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.identity.cwd = Some("/local/fallback".to_string());
        pane.terminal.cwd = Some("/remote/project".to_string());
        pane.terminal.cwd_host = Some("server.example".to_string());
        pane.terminal.foreground_program = Some("nvim".to_string());

        let profile = profile_from_state(&state);
        let saved = &profile.workspaces[0].panes[0];
        assert_eq!(saved.cwd.as_deref(), Some(Path::new("/local/fallback")));
        assert_eq!(saved.command.as_deref(), Some("nvim"));

        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.cwd = Some("/local/project".to_string());
        pane.terminal.cwd_host = None;
        pane.terminal.foreground_program = Some("ZSH.EXE".to_string());
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(saved.cwd.as_deref(), Some(Path::new("/local/project")));
        assert_eq!(saved.command, None);

        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.identity.launch = Some(crate::pane_launch::PaneLaunch::shell("cargo test"));
        pane.terminal.foreground_program = Some("bash".to_string());
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(saved.command.as_deref(), Some("cargo test"));
    }

    #[test]
    fn save_ignores_stale_executable_reported_by_an_idle_prompt() {
        use crate::session::protocol::PaneCommandPhase;

        // `foreground_program` still holds the last command's executable (here a shell hook that
        // ran during `cd`) while shell integration reports the pane idle at a prompt; capturing
        // it would replay prompt machinery as a pane command on restore.
        let mut state = State::new(Config::default(), Theme::default());
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.foreground_program = Some("__zoxide_hook".to_string());
        for phase in [
            PaneCommandPhase::Prompt,
            PaneCommandPhase::Input,
            PaneCommandPhase::Completed { exit_status: None },
        ] {
            state.current_mut().workspaces[0].panes[0]
                .terminal
                .command_phase = phase;
            let saved = &profile_from_state(&state).workspaces[0].panes[0];
            assert_eq!(
                saved.command, None,
                "stale executable captured at {phase:?}"
            );
        }

        // A command genuinely mid-flight is worth replaying.
        state.current_mut().workspaces[0].panes[0]
            .terminal
            .command_phase = PaneCommandPhase::Executing;
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(saved.command.as_deref(), Some("__zoxide_hook"));
    }

    #[test]
    fn save_replays_an_off_path_program_by_the_path_the_server_reported() {
        // A pane started through a shell alias runs a binary whose *name* nothing resolves, so
        // capturing the name replays as `command not found`. The server sends the path exactly
        // when the name is unusable; that is what the profile has to store.
        let mut state = State::new(Config::default(), Theme::default());
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.foreground_program = Some("opencode-tui".to_string());
        pane.terminal.foreground_executable =
            Some("/home/dev/opencode/target/release/opencode-tui".to_string());

        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(
            saved.command.as_deref(),
            Some("/home/dev/opencode/target/release/opencode-tui")
        );

        // A path with a space is still one command word at the prompt.
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.foreground_executable = Some("/opt/My Apps/opencode-tui".to_string());
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(
            saved.command.as_deref(),
            Some("'/opt/My Apps/opencode-tui'")
        );

        // Under `--remote` the path names a file on the server's filesystem, which the local
        // restore cannot run: fall back to the name.
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.cwd = Some("/remote/project".to_string());
        pane.terminal.cwd_host = Some("server.example".to_string());
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(saved.command.as_deref(), Some("opencode-tui"));
    }

    #[test]
    fn save_captures_the_arguments_the_program_is_running_with() {
        // The flags are the pane: an agent restored without `--dangerously-skip-permissions` is
        // not the agent that was captured.
        let mut state = State::new(Config::default(), Theme::default());
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.foreground_program = Some("claude".to_string());
        pane.terminal.foreground_arguments = vec![
            "--dangerously-skip-permissions".to_string(),
            "--model".to_string(),
            "opus".to_string(),
        ];

        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(
            saved.command.as_deref(),
            Some("claude --dangerously-skip-permissions --model opus")
        );

        // Arguments are quoted per word, so a path with a space stays one argument.
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.foreground_arguments = vec!["/tmp/my notes.md".to_string()];
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(saved.command.as_deref(), Some("claude '/tmp/my notes.md'"));

        // The path replaces only the program word; the arguments follow it.
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.foreground_executable = Some("/opt/agents/claude".to_string());
        pane.terminal.foreground_arguments = vec!["--resume".to_string()];
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(
            saved.command.as_deref(),
            Some("/opt/agents/claude --resume")
        );

        // A remote pane describes a process on another host: neither its path nor its arguments
        // are ours to replay locally.
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.cwd = Some("/remote/project".to_string());
        pane.terminal.cwd_host = Some("server.example".to_string());
        let saved = &profile_from_state(&state).workspaces[0].panes[0];
        assert_eq!(saved.command.as_deref(), Some("claude"));
    }

    #[test]
    fn restored_profile_commands_are_marked_for_interactive_replay() {
        let profile = Profile {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![
                    PaneProfile {
                        id: 0,
                        command: Some("n".to_string()),
                        ..PaneProfile::default()
                    },
                    PaneProfile {
                        id: 1,
                        ..PaneProfile::default()
                    },
                    PaneProfile {
                        id: 2,
                        command: Some("   ".to_string()),
                        ..PaneProfile::default()
                    },
                ],
                ..WorkspaceProfile::default()
            }],
        };

        let state = State::from_profile(Config::default(), Theme::default(), profile);
        let panes = &state.current().workspaces[0].panes;
        assert_eq!(
            panes[0]
                .identity
                .launch
                .as_ref()
                .and_then(crate::pane_launch::PaneLaunch::shell_command),
            Some("n")
        );
        assert!(panes[0].identity.replay);
        assert_eq!(panes[1].identity.launch, None);
        assert!(!panes[1].identity.replay);
        // A blank command must not become a stray injected carriage return.
        assert_eq!(panes[2].identity.launch, None);
        assert!(!panes[2].identity.replay);
    }

    #[test]
    fn direct_argv_profiles_round_trip_without_shell_replay() {
        let argv = vec![
            "ssh".to_string(),
            "--".to_string(),
            "host with spaces; $literal".to_string(),
        ];
        let profile = Profile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![PaneProfile {
                    id: 0,
                    argv: Some(argv.clone()),
                    ..PaneProfile::default()
                }],
                ..WorkspaceProfile::default()
            }],
            ..Profile::default()
        };

        let state = State::from_profile(Config::default(), Theme::default(), profile);
        let pane = &state.current().workspaces[0].panes[0];
        assert_eq!(
            pane.identity.launch,
            Some(crate::pane_launch::PaneLaunch::Direct { argv: argv.clone() })
        );
        assert!(!pane.identity.replay);
        assert_eq!(
            profile_from_state(&state).workspaces[0].panes[0].argv,
            Some(argv)
        );
        assert!(
            Profile::from_toml_str(
                r#"
                [[workspaces]]
                index = 0

                [[workspaces.panes]]
                id = 0
                command = "ssh host"
                argv = ["ssh", "host"]
                "#
            )
            .unwrap_err()
            .contains("both `command` and `argv`")
        );
    }

    #[test]
    fn in_place_profile_replacement_remaps_ids_and_preserves_session_runtime() {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().session_name = Some("work".to_string());
        state.current_mut().session_attached = true;
        state.runtime_epoch = 9;
        state.current_mut().next_pty_generation = 42;
        state.current_mut().next_pane_id = 20;
        let profile = Profile {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![
                    PaneProfile {
                        id: 0,
                        pane_id: Some(2),
                        ..PaneProfile::default()
                    },
                    PaneProfile {
                        id: 1,
                        pane_id: Some(3),
                        ..PaneProfile::default()
                    },
                ],
                ..WorkspaceProfile::default()
            }],
        };

        replace_layout_from_profile(&mut state, profile, 20);

        assert_eq!(
            state.current().workspaces[0]
                .panes
                .iter()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
            vec![20, 21]
        );
        assert_eq!(state.current().next_pane_id, 22);
        assert_eq!(state.current().next_pty_generation, 42);
        assert_eq!(state.runtime_epoch, 9);
        assert_eq!(state.current().session_name.as_deref(), Some("work"));
        assert!(state.current().session_attached);
    }

    #[test]
    fn named_workspace_round_trips_from_state() {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().workspaces[0].name = Some("code".to_string());

        let profile = profile_from_state(&state);
        assert_eq!(profile.workspaces[0].name.as_deref(), Some("code"));

        let restored = State::from_profile(Config::default(), Theme::default(), profile);
        assert_eq!(
            restored.current().workspaces[0].name.as_deref(),
            Some("code")
        );
    }

    #[test]
    fn blank_workspace_name_restores_as_unnamed() {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().workspaces[0].name = Some("code".to_string());
        let mut profile = profile_from_state(&state);
        profile.workspaces[0].name = Some("   ".to_string());

        let restored = State::from_profile(Config::default(), Theme::default(), profile);

        assert_eq!(restored.current().workspaces[0].name, None);
    }

    #[test]
    fn save_profile_creates_parent_directory_and_file() {
        let root =
            std::env::temp_dir().join(format!("rozi-save-profile-test-{}", std::process::id()));
        let path = root.join("nested").join("project.toml");

        let result = save_profile(&path, &Profile::default());

        assert!(result.is_ok(), "save failed: {result:?}");
        let contents = std::fs::read_to_string(&path).expect("profile file was written");
        assert!(contents.contains("version = 1"), "contents: {contents}");

        std::fs::remove_dir_all(root).expect("temporary profile directory removed");
    }

    #[test]
    fn save_profile_writes_bare_relative_path_in_current_directory() {
        let _guard = cwd_lock().lock().expect("cwd lock");
        let original_cwd = std::env::current_dir().expect("current dir");
        let root = std::env::temp_dir().join(format!(
            "rozi-save-profile-bare-relative-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("temporary profile directory created");

        std::env::set_current_dir(&root).expect("changed to temporary profile directory");
        let result = save_profile(Path::new("bare-relative-profile.toml"), &Profile::default());
        std::env::set_current_dir(original_cwd).expect("restored current dir");

        assert!(result.is_ok(), "save failed: {result:?}");
        let path = root.join("bare-relative-profile.toml");
        let contents = std::fs::read_to_string(&path).expect("profile file was written");
        assert!(contents.contains("version = 1"), "contents: {contents}");

        std::fs::remove_dir_all(root).expect("temporary profile directory removed");
    }

    #[test]
    fn profile_scrollable_width_defaults_and_round_trips() {
        let absent = r#"
version = 1
active_workspace = 0
[[workspaces]]
index = 0
layout = "scrollable"
[[workspaces.panes]]
id = 0
"#;
        let profile = Profile::from_toml_str(absent).expect("parse");
        assert_eq!(profile.workspaces[0].panes[0].scrollable_width, None);
        let state = State::from_profile(Config::default(), Theme::default(), profile);
        assert_eq!(
            state.current().workspaces[0].panes[0].scrollable_width,
            crate::state::DEFAULT_SCROLLABLE_WIDTH
        );

        let mut live = State::new(Config::default(), Theme::default());
        live.current_mut().workspaces[0].layout_kind = LayoutKind::Scrollable;
        live.current_mut().workspaces[0].panes[0].scrollable_width =
            crate::state::DEFAULT_SCROLLABLE_WIDTH;
        let quiet = profile_from_state(&live);
        assert_eq!(
            quiet.workspaces[0].panes[0].scrollable_width, None,
            "default width must omit from saved profiles"
        );
        let toml = quiet.to_toml_string().expect("encode");
        assert!(
            !toml.contains("scrollable_width"),
            "default width must stay out of TOML: {toml}"
        );

        live.current_mut().workspaces[0].panes[0].scrollable_width = 0.62;
        let captured = profile_from_state(&live);
        assert_eq!(captured.workspaces[0].panes[0].scrollable_width, Some(0.62));
        let restored = State::from_profile(Config::default(), Theme::default(), captured);
        assert!((restored.current().workspaces[0].panes[0].scrollable_width - 0.62).abs() < 1e-6);

        let invalid = r#"
version = 1
active_workspace = 0
[[workspaces]]
index = 0
layout = "scrollable"
[[workspaces.panes]]
id = 0
scrollable_width = 9.0
"#;
        let profile = Profile::from_toml_str(invalid).expect("parse");
        let state = State::from_profile(Config::default(), Theme::default(), profile);
        assert_eq!(
            state.current().workspaces[0].panes[0].scrollable_width,
            crate::state::MAX_SPLIT_RATIO
        );
    }

    #[test]
    fn restore_recreates_focus_identity_and_tree() {
        let profile = Profile {
            version: 1,
            active_workspace: 1,
            workspaces: vec![
                WorkspaceProfile {
                    index: 0,
                    layout: ProfileLayoutKind::Dwindle,
                    panes: vec![PaneProfile {
                        id: 0,
                        name: Some("docs".to_string()),
                        title: Some("docs".to_string()),
                        cwd: Some(PathBuf::from("/tmp/rozi-docs")),
                        command: Some("bash".to_string()),
                        floating: false,
                        fullscreen: false,
                        rect: None,
                        ..PaneProfile::default()
                    }],
                    ..WorkspaceProfile::default()
                },
                WorkspaceProfile {
                    index: 1,
                    layout: ProfileLayoutKind::Master,
                    split_ratios: vec![0.67, 0.33],
                    focused_pane: Some(1),
                    tree: Some(ProfileTree::Leaf { pane: 0 }),
                    panes: vec![
                        PaneProfile {
                            id: 0,
                            name: Some("editor".to_string()),
                            cwd: Some(PathBuf::from("/tmp/rozi-editor")),
                            command: Some("nvim src/main.rs".to_string()),
                            floating: false,
                            fullscreen: true,
                            rect: None,
                            ..PaneProfile::default()
                        },
                        PaneProfile {
                            id: 1,
                            name: Some("scratch".to_string()),
                            floating: true,
                            fullscreen: false,
                            rect: Some(ProfileRect {
                                x: 12.0,
                                y: 6.0,
                                w: 90.0,
                                h: 28.0,
                            }),
                            ..PaneProfile::default()
                        },
                    ],
                    ..WorkspaceProfile::default()
                },
            ],
        };

        let state = State::from_profile(Config::default(), Theme::default(), profile);

        assert_eq!(state.current().active_workspace, 1);
        assert_eq!(
            state.current().workspaces[1].layout_kind,
            LayoutKind::Master
        );
        assert_eq!(state.current().focused_pane, Some(3));
        assert_eq!(state.current().workspaces[1].focused_pane, Some(3));
        assert_eq!(state.current().workspaces[1].split_ratios[0], 0.67);
        assert_eq!(
            state.current().workspaces[1].panes[0]
                .identity
                .custom_title
                .as_deref(),
            Some("editor")
        );
        assert_eq!(
            state.current().workspaces[1].panes[0]
                .identity
                .profile_name
                .as_deref(),
            Some("editor")
        );
        assert!(state.current().workspaces[1].panes[0].fullscreen);
        assert_eq!(
            state.current().workspaces[1].panes[0]
                .identity
                .cwd
                .as_deref(),
            Some("/tmp/rozi-editor")
        );
        assert_eq!(
            state.current().workspaces[1].panes[0]
                .identity
                .launch
                .as_ref()
                .and_then(crate::pane_launch::PaneLaunch::shell_command),
            Some("nvim src/main.rs")
        );
        assert_eq!(
            state.current().workspaces[1].panes[1]
                .identity
                .custom_title
                .as_deref(),
            Some("scratch")
        );
        assert_eq!(
            state.current().workspaces[1].panes[1]
                .identity
                .profile_name
                .as_deref(),
            Some("scratch")
        );
        assert!(state.current().workspaces[1].panes[1].floating);
        assert!(!state.current().workspaces[1].panes[1].fullscreen);
        assert!(state.current().workspaces[1].tile_tree.is_some());
        assert_eq!(state.current().next_pane_id, 4);
    }

    #[test]
    fn restore_falls_back_when_no_valid_panes_restore() {
        let profile = Profile {
            version: 1,
            active_workspace: 0,
            workspaces: vec![WorkspaceProfile {
                index: WORKSPACE_COUNT + 1,
                panes: vec![PaneProfile {
                    id: 0,
                    name: Some("invalid".to_string()),
                    ..PaneProfile::default()
                }],
                ..WorkspaceProfile::default()
            }],
        };

        let state = State::from_profile(Config::default(), Theme::default(), profile);

        assert_eq!(state.current().workspaces[0].panes.len(), 1);
        assert_eq!(state.current().focused_pane, Some(1));
        assert_eq!(state.current().next_pane_id, 2);
    }
}
