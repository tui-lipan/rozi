use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use tui_lipan::prelude::*;

use crate::anim::WindowAnimationConfig;
#[cfg(test)]
use crate::state::DEFAULT_SPLIT_WIDTH_MULTIPLIER;
use crate::state::{CapStyle, PaneBorderStyle};

use super::schema::*;

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: HyprmuxConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    shell: Option<String>,
    cwd: Option<String>,
    scrollback: Option<usize>,
    input: InputFileConfig,
    animations: AnimationFileConfig,
    theme: ThemeFileConfig,
    profile: ProfileFileConfig,
    session: SessionFileConfig,
    layout: LayoutFileConfig,
    pane: PaneFileConfig,
    clipboard: ClipboardFileConfig,
    notifications: NotificationsFileConfig,
    navigation: NavigationFileConfig,
    confirm: ConfirmFileConfig,
    scratchpad: ScratchpadFileConfig,
    workbar: WorkbarFileConfig,
    rules: Vec<RuleFileConfig>,
    hooks: HashMap<String, String>,
    logging: LoggingFileConfig,
    keys: HashMap<String, KeyBindingSpec>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct LoggingFileConfig {
    dir: Option<String>,
}

/// A `[keys]` value: replacement bindings, an additive binding table, or a user command table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyBindingSpec {
    One(String),
    Many(Vec<String>),
    Add(AddKeyBindingSpec),
    UserCommand(UserCommandTableSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddKeyBindingSpec {
    add: KeyBindingCandidates,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyBindingCandidates {
    One(String),
    Many(Vec<String>),
}

impl KeyBindingCandidates {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct UserCommandTableSpec {
    run: Option<String>,
    send: Option<String>,
    popup: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ConfirmFileConfig {
    close_pane: Option<bool>,
    kill_workspace: Option<bool>,
    kill_session: Option<bool>,
    quit_ephemeral: Option<bool>,
    new_temporary_session: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ScratchpadFileConfig {
    command: Option<String>,
    cwd: Option<String>,
    height: Option<f32>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct WorkbarFileConfig {
    left: Option<Vec<WorkbarSegmentSpec>>,
    right: Option<Vec<WorkbarSegmentSpec>>,
    clock_format: Option<String>,
}

/// A `[workbar]` list entry: either a bare segment name (`"clock"`, `"text:.."`, `"command:.."`)
/// or a table `{ segment = "..", color = "info" }` that overrides the badge color by theme role.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WorkbarSegmentSpec {
    Name(String),
    Table {
        segment: String,
        #[serde(default)]
        color: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProfileFileConfig {
    default: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SessionFileConfig {
    autosave: Option<bool>,
    path: Option<String>,
    startup: Option<String>,
    resurrect: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct LayoutFileConfig {
    split_width_multiplier: Option<f32>,
}

/// `[pane] padding` value: a single number applies to all four sides, or a CSS-style array of
/// `[vertical, horizontal]` (2 values) or `[top, right, bottom, left]` (4 values).
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
enum PaddingSpec {
    All(u16),
    Sides(Vec<u16>),
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct PaneFileConfig {
    hold_on_exit: Option<bool>,
    highlight_focused_background: Option<bool>,
    highlight_focused_border: Option<bool>,
    focus_on_hover: Option<bool>,
    show_workbar: Option<bool>,
    workbar_gap: Option<bool>,
    workbar_at_bottom: Option<bool>,
    show_titles: Option<bool>,
    merge_borders: Option<bool>,
    background_follows_terminal: Option<bool>,
    border_style: Option<String>,
    padding: Option<PaddingSpec>,
    title_style: Option<String>,
    workbar_badge_style: Option<String>,
    workbar_powerline: Option<bool>,
    workbar_tab_style: Option<String>,
    workbar_style: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RuleFileConfig {
    #[serde(rename = "match")]
    matches: String,
    float: bool,
    width: Option<f32>,
    height: Option<f32>,
    workspace: Option<usize>,
    focus: bool,
    fullscreen: bool,
}

impl Default for RuleFileConfig {
    fn default() -> Self {
        Self {
            matches: String::new(),
            float: false,
            width: None,
            height: None,
            workspace: None,
            focus: true,
            fullscreen: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_split_width_multiplier_is_configurable() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [layout]
            split_width_multiplier = 2.28
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.layout.split_width_multiplier, Some(2.28));
        assert_eq!(
            HyprmuxLayoutConfig::default().split_width_multiplier,
            DEFAULT_SPLIT_WIDTH_MULTIPLIER
        );
    }

    #[test]
    fn confirm_section_overrides_defaults() {
        let defaults = HyprmuxConfirmConfig::default();
        assert!(!defaults.close_pane);
        assert!(defaults.kill_workspace);
        assert!(defaults.kill_session);
        assert!(defaults.quit_ephemeral);
        assert!(defaults.new_temporary_session);

        let parsed: FileConfig = toml::from_str(
            r#"
            [confirm]
            close_pane = true
            kill_workspace = false
            quit_ephemeral = false
            new_temporary_session = false
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.confirm.close_pane, Some(true));
        assert_eq!(parsed.confirm.kill_workspace, Some(false));
        assert_eq!(parsed.confirm.kill_session, None);
        assert_eq!(parsed.confirm.quit_ephemeral, Some(false));
        assert_eq!(parsed.confirm.new_temporary_session, Some(false));
    }

    #[test]
    fn session_section_parses_startup() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [session]
            startup = "picker"
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.session.startup.as_deref(), Some("picker"));
    }

    #[test]
    fn key_overrides_parse_native_bindings_and_warn_on_unknown_action() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = ["alt-enter"]
            close = "ctrl-b q"
            notanaction = "x"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert_eq!(
            overrides.get("spawn"),
            Some(&vec![KeyBinding::from_str("alt-enter").unwrap()])
        );
        assert_eq!(
            overrides.get("close"),
            Some(&vec![KeyBinding::from_str("ctrl-b q").unwrap()])
        );

        // Unknown action id yields exactly one warning and is skipped.
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("notanaction"));
    }

    #[test]
    fn keys_table_value_registers_a_run_user_command() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            "ctrl-a g" = { run = "lazygit" }
            "#,
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(user_commands.len(), 1);
        assert_eq!(
            user_commands[0].action,
            UserCommandAction::Run("lazygit".to_string())
        );
        assert_eq!(
            user_commands[0].binding,
            KeyBinding::from_str("ctrl-a g").unwrap()
        );
    }

    #[test]
    fn keys_table_value_registers_a_send_user_command() {
        let parsed: FileConfig = toml::from_str(
            "
            [keys]
            alt-g = { send = \"echo hi\\n\" }
            ",
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(user_commands.len(), 1);
        assert_eq!(
            user_commands[0].action,
            UserCommandAction::Send("echo hi\n".to_string())
        );
    }

    #[test]
    fn keys_table_value_without_run_or_send_warns_and_is_skipped() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            "ctrl-a g" = {}
            "#,
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(user_commands.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("run"));
    }

    #[test]
    fn keys_table_value_with_both_run_and_send_warns_and_is_skipped() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            "ctrl-a g" = { run = "lazygit", send = "hi" }
            "#,
        )
        .expect("config parses");

        let mut user_commands = Vec::new();
        let mut warnings = Vec::new();
        build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut user_commands,
            &mut warnings,
        );

        assert!(user_commands.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("both"));
    }

    #[test]
    fn bare_key_override_expands_through_the_input_scheme() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            copy-mode = "b"
            close = "shift-w"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![
                KeyBinding::from_str("ctrl-a b").unwrap(),
                KeyBinding::from_str("alt-b").unwrap(),
            ])
        );
        // Shift-only steps count as bare keys too, matching the built-in default-key grammar.
        assert_eq!(
            overrides.get("close"),
            Some(&vec![
                KeyBinding::from_str("ctrl-a shift-w").unwrap(),
                KeyBinding::from_str("alt-shift-w").unwrap(),
            ])
        );
    }

    #[test]
    fn bare_key_override_follows_custom_prefix_modifier_and_mirror_toggle() {
        let keys = || {
            toml::from_str::<FileConfig>(
                r#"
                [keys]
                copy-mode = "b"
                "#,
            )
            .expect("config parses")
            .keys
        };

        let mut input = InputConfig {
            prefix: KeyBinding::from_str("ctrl-b").unwrap(),
            modifier: WmModifier::Super,
            modifier_shortcuts: true,
        };
        let mut warnings = Vec::new();
        let overrides = build_key_overrides(keys(), &input, &mut Vec::new(), &mut warnings);
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![
                KeyBinding::from_str("ctrl-b b").unwrap(),
                KeyBinding::from_str("super-b").unwrap(),
            ])
        );

        input.modifier_shortcuts = false;
        let overrides = build_key_overrides(keys(), &input, &mut Vec::new(), &mut warnings);
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![KeyBinding::from_str("ctrl-b b").unwrap()])
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn bare_keys_and_literal_bindings_mix_in_one_override() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = ["n", "super-enter"]
            focus-left = "ctrl-h"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides.get("spawn"),
            Some(&vec![
                KeyBinding::from_str("ctrl-a n").unwrap(),
                KeyBinding::from_str("alt-n").unwrap(),
                KeyBinding::from_str("super-enter").unwrap(),
            ])
        );
        // A step with a real modifier stays a literal binding rather than expanding.
        assert_eq!(
            overrides.get("focus-left"),
            Some(&vec![KeyBinding::from_str("ctrl-h").unwrap()])
        );
    }

    #[test]
    fn additive_key_override_keeps_defaults_and_accepts_bare_or_literal_bindings() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            spawn = { add = ["n", "super-enter", "enter"] }
            "#,
        )
        .expect("config parses");

        let input = InputConfig::default();
        let defaults = crate::commands::default_shortcuts_for_action(&input, "spawn").unwrap();
        let mut warnings = Vec::new();
        let overrides = build_key_overrides(parsed.keys, &input, &mut Vec::new(), &mut warnings);
        let bindings = overrides.get("spawn").unwrap();

        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(bindings.starts_with(&defaults));
        assert!(bindings.contains(&KeyBinding::from_str("ctrl-a n").unwrap()));
        assert!(bindings.contains(&KeyBinding::from_str("alt-n").unwrap()));
        assert!(bindings.contains(&KeyBinding::from_str("super-enter").unwrap()));
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| **binding == KeyBinding::from_str("ctrl-a enter").unwrap())
                .count(),
            1
        );
    }

    #[test]
    fn scheme_marker_expands_modified_keys_in_replacements_and_additions() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            copy-mode = "scheme:ctrl-t"
            spawn = { add = ["scheme:ctrl-n"] }
            "#,
        )
        .expect("config parses");
        let input = InputConfig {
            prefix: KeyBinding::from_str("ctrl-b").unwrap(),
            modifier: WmModifier::Super,
            modifier_shortcuts: true,
        };

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(parsed.keys, &input, &mut Vec::new(), &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            overrides.get("copy-mode"),
            Some(&vec![
                KeyBinding::from_str("ctrl-b ctrl-t").unwrap(),
                KeyBinding::from_str("super-ctrl-t").unwrap(),
            ])
        );
        let spawn = overrides.get("spawn").unwrap();
        assert!(spawn.contains(&KeyBinding::from_str("ctrl-b ctrl-n").unwrap()));
        assert!(spawn.contains(&KeyBinding::from_str("super-ctrl-n").unwrap()));
    }

    #[test]
    fn scheme_marker_rejects_multiple_key_steps() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            copy-mode = "scheme:ctrl-t x"
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();

        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert!(!overrides.contains_key("copy-mode"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("one key step"))
        );
    }

    #[test]
    fn top_level_input_aliases_are_rejected() {
        let error = toml::from_str::<FileConfig>(
            r#"
            prefix = "ctrl-b"
            modifier = "super"
            "#,
        )
        .expect_err("top-level input aliases should not parse");

        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn empty_key_specs_record_an_explicit_unbind() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            scratchpad = []
            spawn = ""
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert_eq!(overrides.get("scratchpad"), Some(&Vec::new()));
        assert_eq!(overrides.get("spawn"), Some(&Vec::new()));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn malformed_key_specs_keep_defaults_and_warn() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            scratchpad = "not-a-real-key"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        // A fully-unparseable spec is a config error, not an intentional unbind - no override
        // is recorded, so `resolve_shortcuts` falls back to `scratchpad`'s default shortcuts
        // instead of leaving it unreachable.
        assert_eq!(overrides.get("scratchpad"), None);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("not-a-real-key"));
        assert!(warnings[0].contains("scratchpad"));
        assert!(warnings[1].contains("scratchpad"));
        assert!(warnings[1].contains("default"));
    }

    #[test]
    fn no_default_actions_are_bindable() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [keys]
            toggle-pane-synchronization = "ctrl-a y"
            rename-session = "alt-y"
            "#,
        )
        .expect("config parses");

        let mut warnings = Vec::new();
        let overrides = build_key_overrides(
            parsed.keys,
            &InputConfig::default(),
            &mut Vec::new(),
            &mut warnings,
        );

        assert_eq!(
            overrides.get("toggle-pane-synchronization"),
            Some(&vec![KeyBinding::from_str("ctrl-a y").unwrap()])
        );
        assert_eq!(
            overrides.get("rename-session"),
            Some(&vec![KeyBinding::from_str("alt-y").unwrap()])
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn file_config_parses_profile_default() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [profile]
            default = "dev"
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.profile.default.as_deref(), Some("dev"));
    }

    #[test]
    fn file_config_parses_pane_options() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            hold_on_exit = true
            highlight_focused_background = true
            highlight_focused_border = true
            focus_on_hover = false
            show_workbar = false
            workbar_gap = false
            workbar_at_bottom = true
            show_titles = false
            padding = 2
            title_style = "round"
            workbar_badge_style = "arrow"
            workbar_tab_style = "round"
            workbar_style = "half"
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.pane.highlight_focused_background, Some(true));
        assert_eq!(parsed.pane.hold_on_exit, Some(true));
        assert_eq!(parsed.pane.highlight_focused_border, Some(true));
        assert_eq!(parsed.pane.focus_on_hover, Some(false));
        assert_eq!(parsed.pane.show_workbar, Some(false));
        assert_eq!(parsed.pane.workbar_gap, Some(false));
        assert_eq!(parsed.pane.workbar_at_bottom, Some(true));
        assert_eq!(parsed.pane.show_titles, Some(false));
        assert_eq!(parsed.pane.padding, Some(PaddingSpec::All(2)));
        assert_eq!(parsed.pane.title_style.as_deref(), Some("round"));
        assert_eq!(parsed.pane.workbar_badge_style.as_deref(), Some("arrow"));
        assert_eq!(parsed.pane.workbar_tab_style.as_deref(), Some("round"));
        assert_eq!(parsed.pane.workbar_style.as_deref(), Some("half"));
    }

    #[test]
    fn rules_parse_and_merge_with_clamps_and_workspace_remap() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [[rules]]
            match = "btop"
            float = true
            width = 2.0
            height = 0.05
            workspace = 9
            focus = false
            fullscreen = true
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let rules = build_rules(parsed.rules, &mut warnings);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].width, Some(1.0));
        assert_eq!(rules[0].height, Some(0.1));
        assert_eq!(rules[0].workspace, Some(8));
        assert!(!rules[0].focus);
        assert!(rules[0].fullscreen);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn invalid_rules_are_skipped_or_lose_invalid_workspace() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [[rules]]
            match = ""
            [[rules]]
            match = "cargo watch"
            workspace = 10
            "#,
        )
        .expect("config parses");
        let mut warnings = Vec::new();
        let rules = build_rules(parsed.rules, &mut warnings);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].workspace, None);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn pane_padding_accepts_scalar_and_array_forms() {
        let scalar: PaneFileConfig = toml::from_str("padding = 3").expect("scalar parses");
        assert_eq!(scalar.padding, Some(PaddingSpec::All(3)));

        let pair: PaneFileConfig = toml::from_str("padding = [0, 1]").expect("pair parses");
        assert_eq!(pair.padding, Some(PaddingSpec::Sides(vec![0, 1])));

        let quad: PaneFileConfig = toml::from_str("padding = [1, 2, 3, 4]").expect("quad parses");
        assert_eq!(quad.padding, Some(PaddingSpec::Sides(vec![1, 2, 3, 4])));
    }

    #[test]
    fn resolve_pane_padding_maps_css_shorthand() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::All(2), &mut warnings),
            Some((2, 2, 2, 2))
        );
        // Two values are [vertical, horizontal].
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![0, 1]), &mut warnings),
            Some((0, 1, 0, 1))
        );
        // Four values are [top, right, bottom, left].
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![1, 2, 3, 4]), &mut warnings),
            Some((1, 2, 3, 4))
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn resolve_pane_padding_clamps_and_rejects_bad_lengths() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::All(99), &mut warnings),
            Some((8, 8, 8, 8))
        );
        assert_eq!(warnings.len(), 1);

        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(vec![1, 2, 3]), &mut warnings),
            None
        );
        assert_eq!(warnings.len(), 1);

        let mut warnings = Vec::new();
        assert_eq!(
            resolve_pane_padding(PaddingSpec::Sides(Vec::new()), &mut warnings),
            None
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn workbar_segment_table_form_overrides_color() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [workbar]
            right = [{ segment = "clock", color = "info" }, "session"]
            "#,
        )
        .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, parsed.workbar, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            workbar.right,
            vec![
                WorkbarItem {
                    segment: WorkbarSegment::Clock,
                    color: Some(BadgeColor::Info),
                },
                WorkbarItem {
                    segment: WorkbarSegment::Session,
                    color: None,
                },
            ]
        );
    }

    #[test]
    fn workbar_unknown_color_warns_and_falls_back_to_default() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [workbar]
            right = [{ segment = "clock", color = "chartreuse" }]
            "#,
        )
        .expect("config parses");
        let mut workbar = WorkbarConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_config(&mut workbar, parsed.workbar, &mut warnings);
        assert_eq!(
            workbar.right,
            vec![WorkbarItem {
                segment: WorkbarSegment::Clock,
                color: None,
            }]
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn workbar_powerline_parses_and_applies() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            workbar_powerline = false
            "#,
        )
        .expect("config parses");
        assert_eq!(parsed.pane.workbar_powerline, Some(false));
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();
        apply_workbar_style_config(&mut pane, &parsed.pane, &mut warnings);
        assert!(warnings.is_empty());
        assert!(!pane.workbar_powerline);
    }

    #[test]
    fn workbar_badge_style_backfills_workbar_tabs_when_tabs_are_unset() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            workbar_badge_style = "arrow"
            "#,
        )
        .expect("config parses");
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed.pane, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, CapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, CapStyle::Arrow);
    }

    #[test]
    fn explicit_workbar_tab_style_overrides_only_tabs() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [pane]
            workbar_badge_style = "arrow"
            workbar_tab_style = "round"
            "#,
        )
        .expect("config parses");
        let mut pane = HyprmuxPaneConfig::default();
        let mut warnings = Vec::new();

        apply_workbar_style_config(&mut pane, &parsed.pane, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(pane.workbar_badge_style, CapStyle::Arrow);
        assert_eq!(pane.workbar_tab_style, CapStyle::Round);
    }

    #[test]
    fn file_config_parses_notifications() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [notifications]
            enabled = true
            pane_exit = false
            "#,
        )
        .expect("config parses");

        assert_eq!(parsed.notifications.enabled, Some(true));
        assert_eq!(parsed.notifications.pane_exit, Some(false));
    }

    #[test]
    fn file_config_parses_navigation_editors() {
        let parsed: FileConfig = toml::from_str(
            r#"
            [navigation]
            editors = ["nvim", "hx"]
            "#,
        )
        .expect("config parses");

        assert_eq!(
            parsed.navigation.editors,
            Some(vec!["nvim".to_string(), "hx".to_string()])
        );
    }

    #[test]
    fn default_navigation_recognizes_vim_family_case_insensitively() {
        let nav = HyprmuxNavigationConfig::default();
        assert!(nav.is_split_editor("nvim"));
        assert!(nav.is_split_editor("VIM"));
        assert!(nav.is_split_editor("vimdiff"));
        assert!(!nav.is_split_editor("bash"));
        assert!(!nav.is_split_editor("less"));
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct InputFileConfig {
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ThemeFileConfig {
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ClipboardFileConfig {
    enable_osc52: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct NotificationsFileConfig {
    enabled: Option<bool>,
    pane_exit: Option<bool>,
    bell: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct NavigationFileConfig {
    editors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct AnimationFileConfig {
    enabled: Option<bool>,
    spawn: Option<bool>,
    close: Option<bool>,
    fullscreen: Option<bool>,
    tile_float: Option<bool>,
    axis_change: Option<bool>,
    focus_chrome: Option<bool>,
    geometry_ms: Option<u64>,
    close_ms: Option<u64>,
    focus_chrome_ms: Option<u64>,
    open_delay_ms: Option<u64>,
}

/// The config text most recently read or written by this process. Lets the live-reload
/// watcher distinguish external edits from hyprmux's own persistence writes (theme selection,
/// appearance toggles, default profile) and skip event bursts that left the content unchanged.
static LAST_SEEN_CONFIG: Mutex<Option<String>> = Mutex::new(None);

pub(super) fn note_config_text(text: Option<String>) {
    *LAST_SEEN_CONFIG.lock().unwrap() = text;
}

/// True when the on-disk config no longer matches the text hyprmux last read or wrote.
pub fn config_text_changed_on_disk() -> bool {
    let current = std::fs::read_to_string(config_path()).ok();
    *LAST_SEEN_CONFIG.lock().unwrap() != current
}

pub fn load_config() -> LoadedConfig {
    let path = config_path();
    let mut warnings = Vec::new();
    let mut config = HyprmuxConfig::default();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            note_config_text(None);
            return LoadedConfig { config, warnings };
        }
        Err(err) => {
            note_config_text(None);
            warnings.push(format!("Config read failed for {}: {err}", path.display()));
            return LoadedConfig { config, warnings };
        }
    };
    note_config_text(Some(text.clone()));

    let parsed = match toml::from_str::<FileConfig>(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(format!("Config parse failed for {}: {err}", path.display()));
            return LoadedConfig { config, warnings };
        }
    };

    if let Some(shell) = non_empty(parsed.shell) {
        config.shell = Some(shell);
    }
    if let Some(cwd) = non_empty(parsed.cwd) {
        config.cwd = Some(expand_path(cwd).to_string_lossy().to_string());
    }
    if let Some(scrollback) = parsed.scrollback {
        config.scrollback = scrollback.max(1);
    }
    if let Some(dir) = non_empty(parsed.logging.dir) {
        config.logging.dir = Some(expand_path(dir));
    }

    let mut input = config.input.clone();
    apply_input_config(
        &mut input,
        parsed.input.modifier,
        parsed.input.prefix,
        parsed.input.modifier_shortcuts,
        &mut warnings,
    );
    config.input = input;
    apply_animations(&mut config.animations, parsed.animations);

    if let Some(name) = non_empty(parsed.theme.name) {
        config.theme.name = name;
    }
    if let Some(name) = non_empty(parsed.profile.default) {
        config.profile.default = Some(name);
    }
    if let Some(autosave) = parsed.session.autosave {
        config.session.autosave = autosave;
    }
    if let Some(resurrect) = parsed.session.resurrect {
        config.session.resurrect = resurrect;
    }
    if let Some(path) = non_empty(parsed.session.path) {
        config.session.path = Some(expand_path(path));
    }
    if let Some(startup) = non_empty(parsed.session.startup) {
        match SessionStartup::parse(&startup) {
            Some(value) => config.session.startup = value,
            None => warnings.push(format!(
                "Ignored unknown session.startup \"{startup}\" (expected `ephemeral` or `picker`)"
            )),
        }
    }
    if let Some(multiplier) = parsed.layout.split_width_multiplier {
        if multiplier.is_finite() && multiplier > 0.0 {
            config.layout.split_width_multiplier = multiplier;
        } else {
            warnings.push(format!(
                "Ignored layout.split_width_multiplier {multiplier} (expected a positive finite number)"
            ));
        }
    }
    if let Some(highlight_focused_background) = parsed.pane.highlight_focused_background {
        config.pane.highlight_focused_background = highlight_focused_background;
    }
    if let Some(hold_on_exit) = parsed.pane.hold_on_exit {
        config.pane.hold_on_exit = hold_on_exit;
    }
    if let Some(highlight_focused_border) = parsed.pane.highlight_focused_border {
        config.pane.highlight_focused_border = highlight_focused_border;
    }
    if let Some(focus_on_hover) = parsed.pane.focus_on_hover {
        config.pane.focus_on_hover = focus_on_hover;
    }
    if let Some(show_workbar) = parsed.pane.show_workbar {
        config.pane.show_workbar = show_workbar;
    }
    if let Some(workbar_gap) = parsed.pane.workbar_gap {
        config.pane.workbar_gap = workbar_gap;
    }
    if let Some(workbar_at_bottom) = parsed.pane.workbar_at_bottom {
        config.pane.workbar_at_bottom = workbar_at_bottom;
    }
    if let Some(show_titles) = parsed.pane.show_titles {
        config.pane.show_titles = show_titles;
    }
    if let Some(merge_borders) = parsed.pane.merge_borders {
        config.pane.merge_borders = merge_borders;
    }
    if let Some(background_follows_terminal) = parsed.pane.background_follows_terminal {
        config.pane.background_follows_terminal = background_follows_terminal;
    }
    if let Some(border_style) = parsed.pane.border_style.as_deref() {
        match PaneBorderStyle::parse(border_style) {
            Some(style) => config.pane.border_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.border_style \"{border_style}\" (expected one of: rounded, plain, double, thick)"
            )),
        }
    }
    if let Some(padding) = parsed.pane.padding.clone() {
        if let Some(resolved) = resolve_pane_padding(padding, &mut warnings) {
            config.pane.padding = resolved;
        }
    }
    if let Some(title_style) = parsed.pane.title_style.as_deref() {
        match CapStyle::parse(title_style) {
            Some(style) => config.pane.title_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.title_style \"{title_style}\" (expected one of: padded, half, round, arrow)"
            )),
        }
    }
    apply_workbar_style_config(&mut config.pane, &parsed.pane, &mut warnings);
    if let Some(enable_osc52) = parsed.clipboard.enable_osc52 {
        config.clipboard.enable_osc52 = enable_osc52;
    }
    if let Some(enabled) = parsed.notifications.enabled {
        config.notifications.enabled = enabled;
    }
    if let Some(pane_exit) = parsed.notifications.pane_exit {
        config.notifications.pane_exit = pane_exit;
    }
    if let Some(bell) = parsed.notifications.bell {
        config.notifications.bell = bell;
    }
    if let Some(editors) = parsed.navigation.editors {
        config.navigation.editors = editors
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
    }
    if let Some(close_pane) = parsed.confirm.close_pane {
        config.confirm.close_pane = close_pane;
    }
    if let Some(kill_workspace) = parsed.confirm.kill_workspace {
        config.confirm.kill_workspace = kill_workspace;
    }
    if let Some(kill_session) = parsed.confirm.kill_session {
        config.confirm.kill_session = kill_session;
    }
    if let Some(quit_ephemeral) = parsed.confirm.quit_ephemeral {
        config.confirm.quit_ephemeral = quit_ephemeral;
    }
    if let Some(new_temporary_session) = parsed.confirm.new_temporary_session {
        config.confirm.new_temporary_session = new_temporary_session;
    }

    config.scratchpad.command = non_empty(parsed.scratchpad.command);
    config.scratchpad.cwd =
        non_empty(parsed.scratchpad.cwd).map(|cwd| expand_path(cwd).to_string_lossy().to_string());
    if let Some(height) = parsed.scratchpad.height {
        let clamped = height.clamp(SCRATCHPAD_MIN_HEIGHT, SCRATCHPAD_MAX_HEIGHT);
        if (clamped - height).abs() > f32::EPSILON {
            warnings.push(format!(
                "Scratchpad height {height} out of range; clamped to {clamped}"
            ));
        }
        config.scratchpad.height = clamped;
    }

    apply_workbar_config(&mut config.workbar, parsed.workbar, &mut warnings);
    config.rules = build_rules(parsed.rules, &mut warnings);
    config.hooks = parsed
        .hooks
        .into_iter()
        .filter_map(|(kind, command)| {
            if crate::events::EventKind::parse(&kind).is_none() {
                warnings.push(format!("Ignored unknown hook event `{kind}`"));
                None
            } else if command.trim().is_empty() {
                warnings.push(format!("Ignored empty hook for `{kind}`"));
                None
            } else {
                Some((kind, command))
            }
        })
        .collect();
    let mut user_commands = Vec::new();
    config.key_overrides = build_key_overrides(
        parsed.keys,
        &config.input,
        &mut user_commands,
        &mut warnings,
    );
    config.user_commands = user_commands;

    LoadedConfig { config, warnings }
}

fn build_rules(raw: Vec<RuleFileConfig>, warnings: &mut Vec<String>) -> Vec<HyprmuxRuleConfig> {
    raw.into_iter()
        .filter_map(|rule| {
            let matches = rule.matches.trim().to_string();
            if matches.is_empty() {
                warnings.push("Ignored rule with an empty match".to_string());
                return None;
            }
            let clamp = |name: &str, value: Option<f32>, warnings: &mut Vec<String>| {
                value.map(|value| {
                    let clamped = value.clamp(0.1, 1.0);
                    if (clamped - value).abs() > f32::EPSILON {
                        warnings.push(format!(
                            "Rule `{matches}` {name} {value} out of range; clamped to {clamped}"
                        ));
                    }
                    clamped
                })
            };
            let workspace = rule.workspace.and_then(|workspace| {
                if (1..=crate::state::WORKSPACE_COUNT).contains(&workspace) {
                    Some(workspace - 1)
                } else {
                    warnings.push(format!(
                        "Ignored rule `{matches}` workspace {workspace} (expected 1..={})",
                        crate::state::WORKSPACE_COUNT
                    ));
                    None
                }
            });
            Some(HyprmuxRuleConfig {
                width: clamp("width", rule.width, warnings),
                height: clamp("height", rule.height, warnings),
                matches,
                float: rule.float,
                workspace,
                focus: rule.focus,
                fullscreen: rule.fullscreen,
            })
        })
        .collect()
}

pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HYPRMUX_CONFIG") {
        return expand_path(path);
    }
    config_home().join("hyprmux.toml")
}

fn apply_input_config(
    input: &mut InputConfig,
    modifier: Option<String>,
    prefix: Option<String>,
    modifier_shortcuts: Option<bool>,
    warnings: &mut Vec<String>,
) {
    if let Some(modifier_shortcuts) = modifier_shortcuts {
        input.modifier_shortcuts = modifier_shortcuts;
    }
    if let Some(modifier) = modifier {
        match parse_modifier(&modifier) {
            Some(parsed) => input.modifier = parsed,
            None => warnings.push(format!(
                "Unknown modifier `{modifier}`; expected `alt` or `super`"
            )),
        }
    }
    if let Some(prefix) = prefix {
        match KeyBinding::from_str(&prefix) {
            Ok(parsed) if parsed.step_count() == 1 => input.prefix = parsed,
            Ok(_) => warnings.push(format!(
                "Could not parse prefix `{prefix}`; prefix must be a single key"
            )),
            Err(_) => warnings.push(format!(
                "Could not parse prefix `{prefix}`; try e.g. `ctrl-a`"
            )),
        }
    }
}

/// True when a `[keys]` candidate is a bare key step: a single chord step carrying at most
/// `shift` (e.g. `"b"`, `"shift-w"`, `"tab"`). A literal global binding on such a step would
/// steal plain typing from the focused terminal, so it is treated as a default-key replacement
/// and expanded through the `[input]` prefix/modifier scheme instead. Steps with a real
/// modifier (`ctrl-h`, `alt-b`) and multi-step chords stay literal.
fn is_bare_key_step(candidate: &str) -> bool {
    let mut steps = candidate.split_whitespace();
    let (Some(step), None) = (steps.next(), steps.next()) else {
        return false;
    };
    // Non-shift modifier spellings accepted by tui-lipan's binding grammar.
    const MODIFIERS: &[&str] = &[
        "ctrl", "control", "alt", "option", "super", "cmd", "command", "meta", "win", "windows",
    ];
    !step
        .split(['-', '+'])
        .any(|token| MODIFIERS.contains(&token.to_ascii_lowercase().as_str()))
}

/// Build `[keys]` overrides. Strings/lists replace an action's defaults, `{ add = ... }` extends
/// its generated defaults, and `{ run = ... }` / `{ send = ... }` defines a user command.
fn build_key_overrides(
    keys: HashMap<String, KeyBindingSpec>,
    input: &InputConfig,
    user_commands: &mut Vec<UserCommand>,
    warnings: &mut Vec<String>,
) -> HashMap<String, Vec<KeyBinding>> {
    let mut overrides = HashMap::new();
    for (key, spec) in keys {
        // A table value (`{ run = ".." }` / `{ send = ".." }`) defines a brand new user
        // command rather than rebinding a built-in action; here the map *key* is the literal
        // trigger binding (e.g. `"ctrl-a g"`), not a command id.
        if let KeyBindingSpec::UserCommand(table) = spec {
            bind_user_command(user_commands, &key, table, warnings);
            continue;
        }

        let Some(default_bindings) = crate::commands::default_shortcuts_for_action(input, &key)
        else {
            warnings.push(format!("Unknown key action `{key}`; skipped"));
            continue;
        };

        let (bindings, additive) = match spec {
            KeyBindingSpec::One(value) => (vec![value], false),
            KeyBindingSpec::Many(values) => (values, false),
            KeyBindingSpec::Add(table) => (table.add.into_vec(), true),
            KeyBindingSpec::UserCommand(_) => unreachable!(),
        };

        let mut parsed_bindings = if additive {
            default_bindings
        } else {
            Vec::new()
        };
        let mut candidate_count = 0;
        let initial_binding_count = parsed_bindings.len();
        for binding in bindings {
            for candidate in binding
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                candidate_count += 1;
                let scheme_key = if let Some(key_step) = candidate.strip_prefix("scheme:") {
                    match KeyBinding::from_str(key_step) {
                        Ok(binding) if binding.step_count() == 1 => Some(key_step),
                        _ => {
                            warnings.push(format!(
                                "Could not parse scheme binding `{candidate}` for `{key}`; expected one key step"
                            ));
                            continue;
                        }
                    }
                } else {
                    is_bare_key_step(candidate).then_some(candidate)
                };
                if let Some(key_step) = scheme_key {
                    let expanded = scheme_shortcuts(input, key_step);
                    if expanded.is_empty() {
                        warnings.push(format!(
                            "Could not parse binding `{candidate}` for `{key}`; skipped"
                        ));
                    } else {
                        for binding in expanded {
                            if !parsed_bindings.contains(&binding) {
                                parsed_bindings.push(binding);
                            }
                        }
                    }
                    continue;
                }
                match KeyBinding::from_str(candidate) {
                    Ok(parsed) => {
                        if !parsed_bindings.contains(&parsed) {
                            parsed_bindings.push(parsed);
                        }
                    }
                    Err(_) => warnings.push(format!(
                        "Could not parse binding `{candidate}` for `{key}`; skipped"
                    )),
                }
            }
        }
        // Only an explicit `= []` (no candidates at all) means "unbind". If every candidate
        // was present but failed to parse (e.g. the pre-tui-lipan `"prefix c"` grammar), that's
        // a config error, not intent to unbind - keep the default shortcuts rather than
        // silently making the action unreachable, and say so plainly.
        if candidate_count > 0 && parsed_bindings.len() == initial_binding_count && !additive {
            warnings.push(format!(
                "No valid bindings for `{key}`; keeping its default shortcuts"
            ));
            continue;
        }
        overrides.insert(key, parsed_bindings);
    }
    overrides
}

fn bind_user_command(
    user_commands: &mut Vec<UserCommand>,
    key: &str,
    table: UserCommandTableSpec,
    warnings: &mut Vec<String>,
) {
    // Trim `run` (a shell command) but keep `send` byte-for-byte: trailing `\n`/whitespace is
    // often exactly the point (e.g. `send = "ls -la\n"` to submit the command).
    let run = table.run.map(|value| value.trim().to_string());
    let send = table.send.filter(|value| !value.is_empty());
    let popup = table
        .popup
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let choices = usize::from(run.as_ref().is_some_and(|v| !v.is_empty()))
        + usize::from(send.is_some())
        + usize::from(popup.is_some());
    let action = match choices {
        1 if run.as_ref().is_some_and(|v| !v.is_empty()) => UserCommandAction::Run(run.unwrap()),
        1 if send.is_some() => UserCommandAction::Send(send.unwrap()),
        1 => UserCommandAction::Popup(popup.unwrap()),
        0 => {
            warnings.push(format!(
                "User command `{key}` needs a `run`, `send`, or `popup` value; skipped"
            ));
            return;
        }
        _ => {
            warnings.push(format!(
                "User command `{key}` has both/conflicting `run`, `send`, or `popup` values; skipped"
            ));
            return;
        }
    };
    let Ok(binding) = KeyBinding::from_str(key) else {
        warnings.push(format!(
            "Could not parse binding `{key}` for a user command; skipped"
        ));
        return;
    };
    user_commands.push(UserCommand { action, binding });
}

fn apply_workbar_config(
    workbar: &mut WorkbarConfig,
    raw: WorkbarFileConfig,
    warnings: &mut Vec<String>,
) {
    fn parse_segments(
        raw: Vec<WorkbarSegmentSpec>,
        region: &str,
        warnings: &mut Vec<String>,
    ) -> Vec<WorkbarItem> {
        raw.into_iter()
            .filter_map(|spec| {
                let (name, color_name) = match spec {
                    WorkbarSegmentSpec::Name(name) => (name, None),
                    WorkbarSegmentSpec::Table { segment, color } => (segment, color),
                };
                let segment = match WorkbarSegment::parse(&name) {
                    Some(segment) => segment,
                    None => {
                        warnings.push(format!(
                            "Unknown {region} workbar segment `{name}`; skipped"
                        ));
                        return None;
                    }
                };
                // An unknown color role name falls back to the segment's curated default rather than
                // dropping the whole segment.
                let color = match color_name {
                    Some(color_name) => match BadgeColor::parse(&color_name) {
                        Some(color) => Some(color),
                        None => {
                            warnings.push(format!(
                                "Unknown {region} workbar color `{color_name}` for `{name}` (expected one of: {}); using default",
                                BadgeColor::NAMES
                            ));
                            None
                        }
                    },
                    None => None,
                };
                Some(WorkbarItem { segment, color })
            })
            .collect()
    }

    if let Some(left) = raw.left {
        workbar.left = parse_segments(left, "left", warnings);
    }
    if let Some(right) = raw.right {
        workbar.right = parse_segments(right, "right", warnings);
    }
    if let Some(format) = non_empty(raw.clock_format) {
        // Reject invalid strftime so a clock segment can't panic at render time.
        if chrono::format::StrftimeItems::new(&format).parse().is_ok() {
            workbar.clock_format = format;
        } else {
            warnings.push(format!(
                "Invalid clock_format `{format}`; keeping `{}`",
                workbar.clock_format
            ));
        }
    }
}

/// The `hyprmux` config directory (already includes the `hyprmux` segment - callers should join
/// filenames directly, e.g. `config_home().join("hyprmux.toml")`).
///
/// Delegates to [`crate::platform::paths::config_dir`]; kept as a thin wrapper here (rather than
/// switching every call site to the platform module directly) so `config_path()`/`profiles_dir()`/
/// `themes_dir()` in this module family don't need to change beyond the path they join onto it.
pub(super) fn config_home() -> PathBuf {
    crate::platform::paths::config_dir(&crate::platform::paths::PlatformEnv::from_process())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn expand_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let text = path.to_string_lossy();
    if text == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(text.as_ref()));
    }
    PathBuf::from(text.as_ref())
}

fn apply_workbar_style_config(
    config: &mut HyprmuxPaneConfig,
    parsed: &PaneFileConfig,
    warnings: &mut Vec<String>,
) {
    if let Some(workbar_badge_style) = parsed.workbar_badge_style.as_deref() {
        match CapStyle::parse(workbar_badge_style) {
            Some(CapStyle::Half) => warnings.push(format!(
                "Ignored pane.workbar_badge_style \"{workbar_badge_style}\" (half block is not available for workbar badges)"
            )),
            Some(style) => {
                config.workbar_badge_style = style;
                if parsed.workbar_tab_style.is_none() {
                    config.workbar_tab_style = style;
                }
            }
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_badge_style \"{workbar_badge_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(workbar_tab_style) = parsed.workbar_tab_style.as_deref() {
        match CapStyle::parse(workbar_tab_style) {
            Some(CapStyle::Half) => warnings.push(format!(
                "Ignored pane.workbar_tab_style \"{workbar_tab_style}\" (half block is not available for workspace tabs)"
            )),
            Some(style) => config.workbar_tab_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_tab_style \"{workbar_tab_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(workbar_style) = parsed.workbar_style.as_deref() {
        match CapStyle::parse(workbar_style) {
            Some(style) => config.workbar_style = style,
            None => warnings.push(format!(
                "Ignored unknown pane.workbar_style \"{workbar_style}\" (expected one of: padded, half, round, arrow)"
            )),
        }
    }
    if let Some(workbar_powerline) = parsed.workbar_powerline {
        config.workbar_powerline = workbar_powerline;
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Cap defensively: padding eats terminal grid on every side, so a large value would leave no
/// usable pane. 8 cells is already generous for a cosmetic inset.
pub const MAX_PANE_PADDING: u16 = 8;

fn clamp_pane_padding(value: u16, warnings: &mut Vec<String>) -> u16 {
    if value > MAX_PANE_PADDING {
        warnings.push(format!(
            "Clamped pane.padding {value} to the maximum of {MAX_PANE_PADDING}"
        ));
        MAX_PANE_PADDING
    } else {
        value
    }
}

/// Resolve a `[pane] padding` spec into `(top, right, bottom, left)` cells using CSS shorthand: one
/// value applies to all sides, two are `[vertical, horizontal]`, four are `[top, right, bottom,
/// left]`. Other array lengths are rejected with a warning and leave the default untouched.
fn resolve_pane_padding(
    spec: PaddingSpec,
    warnings: &mut Vec<String>,
) -> Option<(u16, u16, u16, u16)> {
    let sides = match spec {
        PaddingSpec::All(value) => vec![value],
        PaddingSpec::Sides(values) => values,
    };
    match sides.as_slice() {
        [all] => {
            let all = clamp_pane_padding(*all, warnings);
            Some((all, all, all, all))
        }
        [vertical, horizontal] => {
            let vertical = clamp_pane_padding(*vertical, warnings);
            let horizontal = clamp_pane_padding(*horizontal, warnings);
            Some((vertical, horizontal, vertical, horizontal))
        }
        [top, right, bottom, left] => Some((
            clamp_pane_padding(*top, warnings),
            clamp_pane_padding(*right, warnings),
            clamp_pane_padding(*bottom, warnings),
            clamp_pane_padding(*left, warnings),
        )),
        other => {
            warnings.push(format!(
                "Ignored pane.padding with {} value(s) (expected 1, 2, or 4)",
                other.len()
            ));
            None
        }
    }
}

fn parse_modifier(value: &str) -> Option<WmModifier> {
    match value.trim().to_ascii_lowercase().as_str() {
        "alt" | "mod" => Some(WmModifier::Alt),
        "super" | "meta" | "logo" | "win" | "windows" => Some(WmModifier::Super),
        _ => None,
    }
}

fn apply_animations(target: &mut WindowAnimationConfig, raw: AnimationFileConfig) {
    if let Some(value) = raw.enabled {
        target.enabled = value;
    }
    if let Some(value) = raw.spawn {
        target.spawn = value;
    }
    if let Some(value) = raw.close {
        target.close = value;
    }
    if let Some(value) = raw.fullscreen {
        target.fullscreen = value;
    }
    if let Some(value) = raw.tile_float {
        target.tile_float = value;
    }
    if let Some(value) = raw.axis_change {
        target.axis_change = value;
    }
    if let Some(value) = raw.focus_chrome {
        target.focus_chrome = value;
    }
    if let Some(value) = raw.geometry_ms {
        target.geometry_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.close_ms {
        target.close_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.focus_chrome_ms {
        target.focus_chrome_duration = Duration::from_millis(value);
    }
    if let Some(value) = raw.open_delay_ms {
        target.open_delay = Duration::from_millis(value);
    }
}
