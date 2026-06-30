use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::FloatRect;

use crate::state::{LayoutKind, Mode, Pane, PaneId, SplitAxis, State, WORKSPACE_COUNT, Workspace};
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
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state_home.join("hyprmux/session.toml"))
}

/// Write the live layout to the session file when `[session] autosave` is enabled. Called
/// synchronously just before quit, so failures are reported to stderr (toasts won't render).
pub fn persist_session_if_enabled(state: &State) {
    if !state.config.session.autosave {
        return;
    }
    let Some(path) = session_path(&state.config) else {
        return;
    };
    let profile = profile_from_state(state);
    if let Err(err) = save_profile(&path, &profile) {
        eprintln!("hyprmux: session autosave failed: {err}");
    }
}

pub fn profile_from_state(state: &State) -> HyprmuxProfile {
    HyprmuxProfile {
        version: 1,
        active_workspace: state.active_workspace,
        workspaces: state
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, workspace)| workspace.panes.iter().any(|pane| !pane.closing))
            .map(|(index, workspace)| workspace_profile_from_state(index, workspace))
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
        workspace.layout_kind = workspace_profile.layout.into();
        if !workspace_profile.split_ratios.is_empty() {
            workspace.split_ratios = workspace_profile.split_ratios.clone();
        }

        let mut profile_pane_ids = HashMap::new();
        for pane_profile in &workspace_profile.panes {
            let id = next_pane_id;
            next_pane_id += 1;
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
            pane.identity.command = pane_profile.command.clone();
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

    State {
        config,
        workspaces,
        active_workspace,
        focused_pane,
        next_pane_id,
        mode: Mode::Normal,
        moving_pane: None,
        resizing_pane: None,
        animation: crate::anim::GeometryAnimation::None,
        last_viewport: std::cell::Cell::new(None),
        show_palette: false,
        show_help: false,
        show_titles: true,
        show_theme_picker: false,
        theme_picker_preview: None,
        theme,
        system_theme: None,
        theme_watcher: None,
        search: None,
        rename: None,
        copy_mode: None,
        scratch: None,
        scratch_visible: false,
        scratch_return_focus: None,
    }
}

fn restore_dwindle_tree(
    tree: &ProfileTree,
    profile_pane_ids: &HashMap<PaneId, PaneId>,
) -> Option<DwindleTree> {
    match tree {
        ProfileTree::Leaf { pane } => profile_pane_ids.get(pane).copied().map(DwindleTree::Leaf),
        ProfileTree::Split {
            axis,
            ratio,
            first,
            second,
        } => Some(DwindleTree::Split {
            axis: (*axis).into(),
            ratio: *ratio,
            first: Box::new(restore_dwindle_tree(first, profile_pane_ids)?),
            second: Box::new(restore_dwindle_tree(second, profile_pane_ids)?),
        }),
    }
}

fn workspace_profile_from_state(index: usize, workspace: &Workspace) -> WorkspaceProfile {
    let pane_indices: HashMap<PaneId, usize> = workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .enumerate()
        .map(|(index, pane)| (pane.id, index))
        .collect();

    WorkspaceProfile {
        index,
        name: None,
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
                name: pane
                    .identity
                    .custom_title
                    .clone()
                    .or_else(|| pane.identity.profile_name.clone()),
                title: pane.identity.custom_title.clone(),
                // Prefer the shell's real live cwd; fall back to the launch identity cwd.
                cwd: pane
                    .live_cwd()
                    .or_else(|| pane.identity.cwd.clone())
                    .map(PathBuf::from),
                command: pane.identity.command.clone(),
                floating: pane.floating,
                fullscreen: pane.fullscreen,
                rect: pane.floating.then_some(pane.floating_rect.into()),
            })
            .collect(),
    }
}

fn profile_tree_from_dwindle(
    tree: &DwindleTree,
    pane_indices: &HashMap<PaneId, usize>,
) -> Option<ProfileTree> {
    match tree {
        DwindleTree::Leaf(id) => pane_indices.get(id).copied().map(|pane| ProfileTree::Leaf {
            pane: pane as PaneId,
        }),
        DwindleTree::Split {
            axis,
            ratio,
            first,
            second,
        } => Some(ProfileTree::Split {
            axis: (*axis).into(),
            ratio: *ratio,
            first: Box::new(profile_tree_from_dwindle(first, pane_indices)?),
            second: Box::new(profile_tree_from_dwindle(second, pane_indices)?),
        }),
    }
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
    pub name: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub command: Option<String>,
    pub floating: bool,
    pub fullscreen: bool,
    pub rect: Option<ProfileRect>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProfileTree {
    Leaf {
        pane: PaneId,
    },
    Split {
        axis: ProfileSplitAxis,
        ratio: f32,
        first: Box<ProfileTree>,
        second: Box<ProfileTree>,
    },
}

impl Default for ProfileTree {
    fn default() -> Self {
        Self::Leaf { pane: 0 }
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileLayoutKind {
    #[default]
    Dwindle,
    Master,
    Grid,
    Spiral,
    Monocle,
}

impl From<LayoutKind> for ProfileLayoutKind {
    fn from(layout: LayoutKind) -> Self {
        match layout {
            LayoutKind::Dwindle => Self::Dwindle,
            LayoutKind::Master => Self::Master,
            LayoutKind::Grid => Self::Grid,
            LayoutKind::Spiral => Self::Spiral,
            LayoutKind::Monocle => Self::Monocle,
        }
    }
}

impl From<ProfileLayoutKind> for LayoutKind {
    fn from(layout: ProfileLayoutKind) -> Self {
        match layout {
            ProfileLayoutKind::Dwindle => Self::Dwindle,
            ProfileLayoutKind::Master => Self::Master,
            ProfileLayoutKind::Grid => Self::Grid,
            ProfileLayoutKind::Spiral => Self::Spiral,
            ProfileLayoutKind::Monocle => Self::Monocle,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileSplitAxis {
    #[default]
    Horizontal,
    Vertical,
}

impl From<SplitAxis> for ProfileSplitAxis {
    fn from(axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::Horizontal => Self::Horizontal,
            SplitAxis::Vertical => Self::Vertical,
        }
    }
}

impl From<ProfileSplitAxis> for SplitAxis {
    fn from(axis: ProfileSplitAxis) -> Self {
        match axis {
            ProfileSplitAxis::Horizontal => Self::Horizontal,
            ProfileSplitAxis::Vertical => Self::Vertical,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    use crate::config::HyprmuxConfig;
    use crate::state::{Pane, State};
    use tui_lipan::prelude::Theme;

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
        let first = state.workspaces[0].panes.first_mut().expect("initial pane");
        first.set_custom_title("editor");
        first.identity.profile_name = Some("profile-editor".to_string());
        first.identity.cwd = Some("/tmp/hyprmux-profile-test".to_string());
        first.identity.command = Some("nvim src/main.rs".to_string());
        first.fullscreen = true;
        state.workspaces[0].split_ratios[0] = 0.63;

        let floating_rect = FloatRect {
            x: 11.0,
            y: 7.0,
            w: 91.0,
            h: 31.0,
        };
        let mut floating = Pane::new(2, state.config.scrollback, floating_rect);
        floating.floating = true;
        floating.identity.profile_name = Some("scratch".to_string());
        state.workspaces[0].panes.push(floating);
        state.workspaces[0].focused_pane = Some(2);
        state.focused_pane = Some(2);

        let profile = profile_from_state(&state);

        assert_eq!(profile.version, 1);
        assert_eq!(profile.active_workspace, 0);
        let workspace = &profile.workspaces[0];
        assert_eq!(workspace.focused_pane, Some(1));
        assert_eq!(workspace.split_ratios[0], 0.63);
        assert_eq!(workspace.panes.len(), 2);
        assert_eq!(workspace.panes[0].id, 0);
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
        let pane_id = state.workspaces[0].panes[0].id;

        crate::identity_ops::rename_pane_in_workspaces(&mut state.workspaces, pane_id, "");
        let snapshot = profile_from_state(&state);
        let restored = State::from_profile(HyprmuxConfig::default(), Theme::default(), snapshot);

        assert_eq!(restored.workspaces[0].panes[0].identity.custom_title, None);
        assert_eq!(restored.workspaces[0].panes[0].identity.profile_name, None);
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
            state.workspaces[0].panes[0].identity.cwd.as_deref(),
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
        state.workspaces[0].panes.push(closing);

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
                    },
                ],
            }],
        };

        let encoded = profile.to_toml_string().expect("profile serializes");
        let decoded = HyprmuxProfile::from_toml_str(&encoded).expect("profile parses");

        assert_eq!(decoded, profile);
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

        assert_eq!(state.active_workspace, 1);
        assert_eq!(state.workspaces[1].layout_kind, LayoutKind::Master);
        assert_eq!(state.focused_pane, Some(3));
        assert_eq!(state.workspaces[1].focused_pane, Some(3));
        assert_eq!(state.workspaces[1].split_ratios[0], 0.67);
        assert_eq!(
            state.workspaces[1].panes[0]
                .identity
                .custom_title
                .as_deref(),
            Some("editor")
        );
        assert_eq!(
            state.workspaces[1].panes[0]
                .identity
                .profile_name
                .as_deref(),
            Some("editor")
        );
        assert!(state.workspaces[1].panes[0].fullscreen);
        assert_eq!(
            state.workspaces[1].panes[0].identity.cwd.as_deref(),
            Some("/tmp/hyprmux-editor")
        );
        assert_eq!(
            state.workspaces[1].panes[0].identity.command.as_deref(),
            Some("nvim src/main.rs")
        );
        assert_eq!(
            state.workspaces[1].panes[1]
                .identity
                .custom_title
                .as_deref(),
            Some("scratch")
        );
        assert_eq!(
            state.workspaces[1].panes[1]
                .identity
                .profile_name
                .as_deref(),
            Some("scratch")
        );
        assert!(state.workspaces[1].panes[1].floating);
        assert!(!state.workspaces[1].panes[1].fullscreen);
        assert!(state.workspaces[1].tile_tree.is_some());
        assert_eq!(state.next_pane_id, 4);
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

        assert_eq!(state.workspaces[0].panes.len(), 1);
        assert_eq!(state.focused_pane, Some(1));
        assert_eq!(state.next_pane_id, 2);
    }
}
