use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::FloatRect;

use crate::layout_tree_ser::{
    SerializedLayoutKind, SerializedSplitAxis, SerializedTree, from_dwindle, to_dwindle,
};
use crate::state::{Mode, Pane, PaneId, State, WORKSPACE_COUNT, Workspace};
use crate::tiling::DwindleTree;
use crate::tiling::append_tiled_window;

pub fn load_profile(path: &Path) -> Result<HyprmuxProfile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("Could not read profile {}: {err}", path.display()))?;
    HyprmuxProfile::from_toml_str(&text)
        .map_err(|err| format!("Could not parse profile {}: {err}", path.display()))
}

pub fn save_profile(path: &Path, profile: &HyprmuxProfile) -> Result<(), String> {
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
/// `$XDG_STATE_HOME/hyprmux/session.toml` (falling back to `~/.local/state/...`).
pub fn session_path(config: &crate::config::HyprmuxConfig) -> Option<PathBuf> {
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
        eprintln!("hyprmux: session autosave failed: {err}");
    }
}

pub fn profile_from_state(state: &State) -> HyprmuxProfile {
    let shells = shell_basenames(&state.config);
    HyprmuxProfile {
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
    config: crate::config::HyprmuxConfig,
    theme: tui_lipan::prelude::Theme,
    profile: HyprmuxProfile,
) -> State {
    if profile
        .workspaces
        .iter()
        .all(|workspace| workspace.panes.is_empty())
    {
        return State::new(config, theme);
    }

    let scrollback = config.scrollback;
    let mut workspaces: Vec<Workspace> = (0..WORKSPACE_COUNT).map(Workspace::new).collect();
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
            pane.identity.command = pane_profile
                .command
                .clone()
                .filter(|command| !command.trim().is_empty());
            // Profile commands are replayed through the interactive shell (typed at the prompt)
            // rather than the command-runner shell: they were captured from what the user ran
            // interactively, so aliases, shell functions, and rc-file PATH entries must resolve.
            pane.identity.replay = pane.identity.command.is_some();
            pane.identity.keep_open = pane_profile.keep_open;
            pane.floating = pane_profile.floating;
            pane.fullscreen = pane_profile.fullscreen;
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
        return State::new(config, theme);
    }

    let sidebar_visible = config.sidebar.visible;
    let sidebar = crate::state::SidebarState::new(&config.sidebar);
    let mut attachment = crate::state::Attachment::new();
    attachment.workspaces = workspaces;
    attachment.active_workspace = active_workspace;
    attachment.focused_pane = focused_pane;
    attachment.next_pane_id = next_pane_id;
    State {
        config,
        attachment,
        background: std::collections::HashMap::new(),
        hosts: crate::state::HostRegistry::default(),
        runtime_epoch: 0,
        next_attachment_id: 1,
        command_link: None,
        mode: Mode::Normal,
        moving_pane: None,
        resizing_pane: None,
        split_drag: None,
        animation: crate::anim::GeometryAnimation::None,
        last_viewport: std::cell::Cell::new(None),
        last_content_viewport: std::cell::Cell::new(None),
        last_clock_text: std::cell::RefCell::new(None),
        sidebar_visible,
        sidebar,
        show_palette: false,
        show_help: false,
        show_appearance: false,
        pane_padding_editor: None,
        show_theme_picker: false,
        theme_picker_preview: None,
        theme,
        system_theme: None,
        theme_watcher: None,
        search: None,
        rename: None,
        rename_session: None,
        save_profile_prompt: None,
        show_profile_picker: false,
        profile_picker: None,
        show_session_picker: false,
        session_picker: None,
        client_list: None,
        last_blocked_input_toast: None,
        replaceable_toasts: std::collections::HashMap::new(),
        session_picker_epoch: 0,
        profile_picker_epoch: 0,
        copy_mode: None,
        hint_mode: None,
        copy_flash: None,
        next_copy_flash_id: 1,
        scratch: None,
        scratch_visible: false,
        scratch_return_focus: None,
        scratch_height: None,
        scratch_resize_start: None,
        popup: None,
        popup_return_focus: None,
        control_socket_path: None,
        event_hub: crate::events::EventHub::default(),
        pending_destructive: None,
        workbar_command_output: std::collections::HashMap::new(),
        workbar_commands_running: std::collections::HashSet::new(),
        commands_dirty: false,
    }
}

pub fn replace_layout_from_profile(
    state: &mut State,
    mut profile: HyprmuxProfile,
    first_pane_id: PaneId,
) {
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
            .map(|(index, pane)| PaneProfile {
                id: index as PaneId,
                pane_id: Some(pane.id),
                name: pane
                    .identity
                    .custom_title
                    .clone()
                    .or_else(|| pane.identity.profile_name.clone()),
                title: pane.identity.custom_title.clone(),
                cwd: pane.local_cwd().map(PathBuf::from),
                command: live_running_command(pane, shells)
                    .or_else(|| pane.identity.command.clone()),
                keep_open: pane.identity.keep_open,
                floating: pane.floating,
                fullscreen: pane.fullscreen,
                rect: pane.floating.then_some(pane.floating_rect.into()),
            })
            .collect(),
    }
}

/// The command a pane is running *right now*, if it is worth replaying on restore.
///
/// `foreground_program` keeps reporting the last executed command's executable while the shell
/// sits idle at a prompt (OSC 133 `hyprmux_exe=` is only replaced by the next command), so a pane
/// where the user merely changed directories would otherwise capture stale prompt machinery like
/// `__zoxide_hook` and replay it as a pane command. Only trust it while shell integration reports
/// a command mid-flight (`Executing`), or when there is no integration at all (`Unknown`) and the
/// value comes from the process inspector, which reads the live foreground process group.
fn live_running_command(pane: &Pane, shells: &HashSet<String>) -> Option<String> {
    use crate::session::protocol::PaneCommandPhase;

    match pane.terminal.command_phase {
        PaneCommandPhase::Executing | PaneCommandPhase::Unknown => pane
            .terminal
            .foreground_program
            .clone()
            .filter(|program| !shells.contains(&normalize_executable(program))),
        PaneCommandPhase::Prompt | PaneCommandPhase::Input | PaneCommandPhase::Completed { .. } => {
            None
        }
    }
}

fn shell_basenames(config: &crate::config::HyprmuxConfig) -> HashSet<String> {
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
    shells.insert(normalize_executable(&resolved.program));
    shells
}

fn normalize_executable(program: &str) -> String {
    let basename = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    let normalized = basename.to_ascii_lowercase();
    normalized
        .strip_suffix(".exe")
        .unwrap_or(&normalized)
        .to_string()
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
pub struct HyprmuxProfile {
    pub version: u32,
    pub active_workspace: usize,
    pub workspaces: Vec<WorkspaceProfile>,
}

impl Default for HyprmuxProfile {
    fn default() -> Self {
        Self {
            version: 1,
            active_workspace: 0,
            workspaces: Vec::new(),
        }
    }
}

impl HyprmuxProfile {
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(input)
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
    pub keep_open: bool,
    pub floating: bool,
    pub fullscreen: bool,
    pub rect: Option<ProfileRect>,
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

    use crate::config::HyprmuxConfig;
    use crate::state::{LayoutKind, Pane, SplitAxis, State};
    use tui_lipan::prelude::Theme;

    #[test]
    fn profile_tree_toml_shape_is_stable() {
        let profile = HyprmuxProfile {
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
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        let first = state.current_mut().workspaces[0]
            .panes
            .first_mut()
            .expect("initial pane");
        first.set_custom_title("editor");
        first.identity.profile_name = Some("profile-editor".to_string());
        first.identity.cwd = Some("/tmp/hyprmux-profile-test".to_string());
        first.identity.command = Some("nvim src/main.rs".to_string());
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
            Some(std::path::Path::new("/tmp/hyprmux-profile-test"))
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
        let profile = HyprmuxProfile {
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
            ..HyprmuxProfile::default()
        };

        let state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);
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
        let profile = HyprmuxProfile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![PaneProfile {
                    id: 0,
                    name: Some("server".to_string()),
                    ..PaneProfile::default()
                }],
                ..WorkspaceProfile::default()
            }],
            ..HyprmuxProfile::default()
        };
        let mut state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);
        let pane_id = state.current().workspaces[0].panes[0].id;

        crate::ops::identity::rename_pane_in_workspaces(
            &mut state.current_mut().workspaces,
            pane_id,
            "",
        );
        let snapshot = profile_from_state(&state);
        let restored = State::from_profile(HyprmuxConfig::default(), Theme::default(), snapshot);

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

    #[test]
    fn restore_expands_tilde_cwd_before_launch_identity_is_stored() {
        let _guard = cwd_lock().lock().expect("env lock");
        let original_home = std::env::var_os("HOME");
        let home = std::env::temp_dir().join(format!("hyprmux-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("home dir created");
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let profile = HyprmuxProfile {
            workspaces: vec![WorkspaceProfile {
                index: 0,
                panes: vec![PaneProfile {
                    id: 0,
                    cwd: Some(PathBuf::from("~/code/my-app")),
                    ..PaneProfile::default()
                }],
                ..WorkspaceProfile::default()
            }],
            ..HyprmuxProfile::default()
        };

        let state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);

        if let Some(original_home) = original_home {
            unsafe { std::env::set_var("HOME", original_home) };
        } else {
            unsafe { std::env::remove_var("HOME") };
        }
        std::fs::remove_dir_all(&home).expect("home dir removed");

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
    fn snapshot_excludes_closing_panes() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        let mut closing = Pane::new(
            2,
            state.config.scrollback,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        closing.closing = true;
        closing.identity.custom_title = Some("closing".to_string());
        state.current_mut().workspaces[0].panes.push(closing);

        let profile = profile_from_state(&state);

        assert_eq!(profile.workspaces[0].panes.len(), 1);
        assert_ne!(
            profile.workspaces[0].panes[0].name.as_deref(),
            Some("closing")
        );
    }

    #[test]
    fn profile_round_trips_named_pane_and_tree() {
        let profile = HyprmuxProfile {
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
                        cwd: Some(PathBuf::from("/tmp/hyprmux-profile-test")),
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
                        cwd: Some(PathBuf::from("/tmp/hyprmux-profile-test")),
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
        let decoded = HyprmuxProfile::from_toml_str(&encoded).expect("profile parses");

        assert_eq!(decoded, profile);
    }

    #[test]
    fn old_profile_without_synchronized_loads_false() {
        let profile = HyprmuxProfile::from_toml_str(
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
        let state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);
        assert!(!state.current().workspaces[0].synchronized);
    }

    #[test]
    fn synchronized_workspace_round_trips_from_state() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.current_mut().workspaces[0].synchronized = true;

        let profile = profile_from_state(&state);
        let restored = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);

        assert!(restored.current().workspaces[0].synchronized);
    }

    #[test]
    fn save_captures_local_runtime_identity_without_remote_paths_or_shells() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
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
        pane.identity.command = Some("cargo test".to_string());
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
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
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
    fn restored_profile_commands_are_marked_for_interactive_replay() {
        let profile = HyprmuxProfile {
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

        let state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);
        let panes = &state.current().workspaces[0].panes;
        assert_eq!(panes[0].identity.command.as_deref(), Some("n"));
        assert!(panes[0].identity.replay);
        assert_eq!(panes[1].identity.command, None);
        assert!(!panes[1].identity.replay);
        // A blank command must not become a stray injected carriage return.
        assert_eq!(panes[2].identity.command, None);
        assert!(!panes[2].identity.replay);
    }

    #[test]
    fn in_place_profile_replacement_remaps_ids_and_preserves_session_runtime() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.current_mut().session_name = Some("work".to_string());
        state.current_mut().session_attached = true;
        state.runtime_epoch = 9;
        state.current_mut().next_pty_generation = 42;
        state.current_mut().next_pane_id = 20;
        let profile = HyprmuxProfile {
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
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.current_mut().workspaces[0].name = Some("code".to_string());

        let profile = profile_from_state(&state);
        assert_eq!(profile.workspaces[0].name.as_deref(), Some("code"));

        let restored = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);
        assert_eq!(
            restored.current().workspaces[0].name.as_deref(),
            Some("code")
        );
    }

    #[test]
    fn blank_workspace_name_restores_as_unnamed() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.current_mut().workspaces[0].name = Some("code".to_string());
        let mut profile = profile_from_state(&state);
        profile.workspaces[0].name = Some("   ".to_string());

        let restored = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);

        assert_eq!(restored.current().workspaces[0].name, None);
    }

    #[test]
    fn save_profile_creates_parent_directory_and_file() {
        let root =
            std::env::temp_dir().join(format!("hyprmux-save-profile-test-{}", std::process::id()));
        let path = root.join("nested").join("project.toml");

        let result = save_profile(&path, &HyprmuxProfile::default());

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
            "hyprmux-save-profile-bare-relative-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("temporary profile directory created");

        std::env::set_current_dir(&root).expect("changed to temporary profile directory");
        let result = save_profile(
            Path::new("bare-relative-profile.toml"),
            &HyprmuxProfile::default(),
        );
        std::env::set_current_dir(original_cwd).expect("restored current dir");

        assert!(result.is_ok(), "save failed: {result:?}");
        let path = root.join("bare-relative-profile.toml");
        let contents = std::fs::read_to_string(&path).expect("profile file was written");
        assert!(contents.contains("version = 1"), "contents: {contents}");

        std::fs::remove_dir_all(root).expect("temporary profile directory removed");
    }

    #[test]
    fn restore_recreates_focus_identity_and_tree() {
        let profile = HyprmuxProfile {
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
                        cwd: Some(PathBuf::from("/tmp/hyprmux-docs")),
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
                            cwd: Some(PathBuf::from("/tmp/hyprmux-editor")),
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

        let state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);

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
            Some("/tmp/hyprmux-editor")
        );
        assert_eq!(
            state.current().workspaces[1].panes[0]
                .identity
                .command
                .as_deref(),
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
        let profile = HyprmuxProfile {
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

        let state = State::from_profile(HyprmuxConfig::default(), Theme::default(), profile);

        assert_eq!(state.current().workspaces[0].panes.len(), 1);
        assert_eq!(state.current().focused_pane, Some(1));
        assert_eq!(state.current().next_pane_id, 2);
    }
}
