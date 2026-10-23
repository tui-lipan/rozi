//! Every Settings row, described once: where it is listed, what it is called and found by, and how
//! its value is shown, changed, and saved. The Settings view, its search, and the apply and save
//! paths all read this table, so an ordinary toggle or choice needs no code anywhere else. Rows
//! with an editor of their own are [`SettingControl::Custom`] and are handled by name.

use std::path::PathBuf;

use tui_lipan::prelude::CapStyle;

use crate::config::{
    Config, CopyOnSelect, CursorBlink, ForegroundRestore, MiddleClickPaste,
    RightClickClipboardAction, SessionOnAttach, SessionPickerOpenOn, SessionStartup, WhichKey,
};
use crate::input::Action;
use crate::layout::anim::{PaneAnimationStyle, PickerAnimationStyle, SessionAnimationStyle};
use crate::state::{
    AlertMode, AlertPaint, PaneAlertPaint, PaneBorderMode, PaneBorderStyle, badge_cap_styles,
    cap_style_id, cap_style_label,
};

use super::{SettingsAction, SettingsChoiceRing, SettingsTab};

/// A titled run of Settings rows, browsed under one tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsGroup {
    General,
    Animations,
    Clipboard,
    Pickers,
    Background,
    Borders,
    Titlebar,
    Workbar,
    Sidebar,
    Alerts,
    Highlights,
    Marks,
    DesktopNotifications,
    Sounds,
    Sessions,
}

impl SettingsGroup {
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Animations => "Animations",
            Self::Clipboard => "Clipboard",
            Self::Pickers => "Pickers",
            Self::Background => "Background",
            Self::Borders => "Borders",
            Self::Titlebar => "Titlebar",
            Self::Workbar => "Workbar",
            Self::Sidebar => "Sidebar",
            Self::Alerts => "Alerts",
            Self::Highlights => "Highlights",
            Self::Marks => "Marks",
            Self::DesktopNotifications => "Desktop notifications",
            Self::Sounds => "Sounds",
            Self::Sessions => "Sessions",
        }
    }

    pub const fn tab(self) -> SettingsTab {
        match self {
            Self::General | Self::Animations | Self::Clipboard | Self::Pickers => {
                SettingsTab::General
            }
            Self::Background | Self::Borders | Self::Titlebar => SettingsTab::Panes,
            Self::Workbar | Self::Sidebar => SettingsTab::Bars,
            Self::Alerts
            | Self::Highlights
            | Self::Marks
            | Self::DesktopNotifications
            | Self::Sounds => SettingsTab::Alerts,
            Self::Sessions => SettingsTab::Sessions,
        }
    }
}

/// One Settings row.
pub struct SettingSpec {
    pub action: SettingsAction,
    pub group: SettingsGroup,
    pub label: &'static str,
    /// Search terms the label and headings do not already carry. The row's group and tab names are
    /// added, and a multi-word query matches term by term across the label and every alias, so
    /// "workbar gap" needs no alias of its own. List synonyms, option values, and jargon only.
    pub aliases: &'static [&'static str],
    pub control: SettingControl,
}

/// How a row's value is read, changed, and saved.
pub enum SettingControl {
    /// An on/off key: Enter flips it and saves it.
    Toggle {
        field: ConfigField<bool>,
        key: ConfigKey,
        effect: Option<SettingEffect>,
    },
    /// An on/off value flipped by a shared action, which also applies and saves it.
    ActionToggle {
        value: fn(&Config) -> bool,
        action: Action,
    },
    /// One value from a list: Enter opens a picker, Shift+Enter steps to the next value.
    Choice(&'static dyn ChoiceSetting),
    /// One value from a short list that Enter steps through without a picker.
    Cycle(&'static dyn ChoiceSetting),
    /// A row with an editor of its own, handled by name.
    Custom,
}

/// A config value, read and written in place.
pub struct ConfigField<T: 'static> {
    pub get: fn(&Config) -> T,
    pub slot: fn(&mut Config) -> &mut T,
}

macro_rules! config_field {
    ($($path:ident).+) => {
        ConfigField {
            get: |config| config.$($path).+,
            slot: |config| &mut config.$($path).+,
        }
    };
}

/// Where a value lives in `config.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigKey {
    pub section: &'static str,
    pub key: &'static str,
}

impl ConfigKey {
    pub const fn new(section: &'static str, key: &'static str) -> Self {
        Self { section, key }
    }

    pub fn save_flag(self, value: bool) -> Result<PathBuf, String> {
        crate::config::persist_section_flag(self.section, self.key, value)
    }

    pub fn save_string(self, value: &str) -> Result<PathBuf, String> {
        crate::config::persist_section_string(self.section, self.key, value)
    }
}

/// Runtime state that has to follow a changed value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingEffect {
    /// The framework's clipboard behavior.
    Clipboard,
    /// How long a prefix waits before showing its key hints.
    WhichKeyDelay,
    /// The session servers' sleep inhibition, which they read from the saved file.
    SleepPolicy,
}

impl SettingEffect {
    /// Whether the effect reads the saved file, so it follows a successful save rather than the
    /// in-memory change.
    pub const fn after_save(self) -> bool {
        matches!(self, Self::SleepPolicy)
    }
}

/// A row whose value is one of a list.
pub trait ChoiceSetting: Sync {
    fn ring(&self, config: &Config) -> SettingsChoiceRing;
    /// The row's value text.
    fn status(&self, config: &Config) -> String;
    /// Write option `index` into live config without saving.
    fn apply(&self, config: &mut Config, index: usize) -> bool;
    fn save(&self, config: &Config) -> Result<PathBuf, String>;
    /// Whether highlighting an option previews it before Enter confirms.
    fn previews(&self) -> bool;
    fn effect(&self) -> Option<SettingEffect>;
}

/// The options of an [`EnumChoice`].
pub enum Choices<T: 'static> {
    Fixed(fn() -> &'static [T]),
    /// Options that depend on other settings.
    ForConfig(fn(&Config) -> Vec<T>),
}

/// How an [`EnumChoice`] is written to `config.toml`.
pub enum ChoiceSave<T: 'static> {
    /// One string key, written as the value's id.
    Key(ConfigKey, fn(T) -> &'static str),
    /// A save that touches more than one key.
    With(fn(&Config) -> Result<PathBuf, String>),
}

/// A choice between the values of one enum-valued config field.
pub struct EnumChoice<T: 'static> {
    /// The picker title.
    title: &'static str,
    choices: Choices<T>,
    field: ConfigField<T>,
    label: fn(T) -> &'static str,
    save: ChoiceSave<T>,
    preview: bool,
    effect: Option<SettingEffect>,
    /// Row text for a value that says more than the chosen option's label.
    status: Option<fn(&Config) -> String>,
}

impl<T: Copy + 'static> EnumChoice<T> {
    pub const fn new(
        title: &'static str,
        choices: Choices<T>,
        field: ConfigField<T>,
        label: fn(T) -> &'static str,
        save: ChoiceSave<T>,
    ) -> Self {
        Self {
            title,
            choices,
            field,
            label,
            save,
            preview: false,
            effect: None,
            status: None,
        }
    }

    /// Preview each highlighted option. For static appearance only: behavior and event-driven
    /// effects need confirmation.
    pub const fn previewed(mut self) -> Self {
        self.preview = true;
        self
    }

    pub const fn with_effect(mut self, effect: SettingEffect) -> Self {
        self.effect = Some(effect);
        self
    }

    pub const fn with_status(mut self, status: fn(&Config) -> String) -> Self {
        self.status = Some(status);
        self
    }

    fn options(&self, config: &Config) -> std::borrow::Cow<'static, [T]> {
        match self.choices {
            Choices::Fixed(all) => std::borrow::Cow::Borrowed(all()),
            Choices::ForConfig(all) => std::borrow::Cow::Owned(all(config)),
        }
    }
}

impl<T: Copy + PartialEq + Sync + 'static> ChoiceSetting for EnumChoice<T> {
    fn ring(&self, config: &Config) -> SettingsChoiceRing {
        let current = (self.field.get)(config);
        let options = self.options(config);
        SettingsChoiceRing {
            title: self.title,
            index: options
                .iter()
                .position(|candidate| *candidate == current)
                .unwrap_or(0),
            options: options
                .iter()
                .map(|value| (self.label)(*value).to_string())
                .collect(),
        }
    }

    fn status(&self, config: &Config) -> String {
        match self.status {
            Some(status) => status(config),
            None => (self.label)((self.field.get)(config)).to_string(),
        }
    }

    fn apply(&self, config: &mut Config, index: usize) -> bool {
        let Some(&value) = self.options(config).get(index) else {
            return false;
        };
        *(self.field.slot)(config) = value;
        true
    }

    fn save(&self, config: &Config) -> Result<PathBuf, String> {
        match self.save {
            ChoiceSave::Key(key, id) => key.save_string(id((self.field.get)(config))),
            ChoiceSave::With(save) => save(config),
        }
    }

    fn previews(&self) -> bool {
        self.preview
    }

    fn effect(&self) -> Option<SettingEffect> {
        self.effect
    }
}

const fn toggle(
    field: ConfigField<bool>,
    section: &'static str,
    key: &'static str,
) -> SettingControl {
    SettingControl::Toggle {
        field,
        key: ConfigKey::new(section, key),
        effect: None,
    }
}

const fn row(
    action: SettingsAction,
    group: SettingsGroup,
    label: &'static str,
    aliases: &'static [&'static str],
    control: SettingControl,
) -> SettingSpec {
    SettingSpec {
        action,
        group,
        label,
        aliases,
        control,
    }
}

fn save_pane_animation_styles(config: &Config) -> Result<PathBuf, String> {
    crate::config::persist_pane_animation_styles(
        config.animations.pane_open_style.id(),
        config.animations.pane_close_style.id(),
    )
}

fn pane_alert_status(config: &Config) -> String {
    let animations = &config.animations;
    config
        .pane
        .alert_border
        .status_label(animations.enabled, animations.focus_chrome)
}

fn workbar_alert_status(config: &Config) -> String {
    let animations = &config.animations;
    config
        .workbar
        .alert
        .mode
        .status_label(animations.enabled, animations.focus_chrome)
}

fn startup_choices(config: &Config) -> Vec<SessionStartup> {
    SessionStartup::choices(config.profile.default.is_some())
}

/// Every Settings row, in the order Settings lists them. A group's rows are consecutive.
pub static SETTINGS: &[SettingSpec] = {
    use SettingControl::{ActionToggle, Choice, Custom, Cycle};
    use SettingsAction::*;
    use SettingsGroup as G;

    &[
        // General
        row(
            Theme,
            G::General,
            "Theme",
            &["color scheme", "colour scheme"],
            Custom,
        ),
        row(
            CycleCursorBlink,
            G::General,
            "Cursor blink",
            &["caret", "steady", "static", "blinking", "inherit"],
            Choice(&EnumChoice::new(
                "Cursor blink",
                Choices::Fixed(CursorBlink::all),
                config_field!(cursor.blink),
                CursorBlink::label,
                ChoiceSave::Key(ConfigKey::new("cursor", "blink"), CursorBlink::id),
            )),
        ),
        row(
            ToggleNerdIcons,
            G::General,
            "Nerd icons",
            &["nerd font", "patched font", "glyphs"],
            ActionToggle {
                value: |config| config.nerd_icons,
                action: Action::ToggleNerdIcons,
            },
        ),
        row(
            CycleWhichKey,
            G::General,
            "Which-key",
            &["prefix hints", "key hints", "chord panel"],
            Choice(
                &EnumChoice::new(
                    "Which-key",
                    Choices::Fixed(WhichKey::all),
                    config_field!(input.which_key),
                    WhichKey::label,
                    ChoiceSave::Key(ConfigKey::new("input", "which_key"), WhichKey::id),
                )
                .with_effect(SettingEffect::WhichKeyDelay),
            ),
        ),
        row(
            ToggleFocusOnHover,
            G::General,
            "Focus on hover",
            &["mouse"],
            ActionToggle {
                value: |config| config.pane.focus_on_hover,
                action: Action::ToggleFocusOnHover,
            },
        ),
        row(
            EditScrollMultiplier,
            G::General,
            "Scroll multiplier",
            &["mouse", "wheel", "scrollback", "scroll speed"],
            Custom,
        ),
        // Every motion control in one place, the master switch first: the rows below it grey out
        // when it is off, so they read as its dependents.
        row(
            ToggleAnimations,
            G::Animations,
            "Play animations",
            &["motion", "transitions", "effects"],
            ActionToggle {
                value: |config| config.animations.enabled,
                action: Action::ToggleAnimations,
            },
        ),
        row(
            ToggleWorkspaceAnimation,
            G::Animations,
            "Workspace switching",
            &["slide"],
            toggle(
                config_field!(animations.workspace),
                "animations",
                "workspace",
            ),
        ),
        row(
            CycleSessionAnimation,
            G::Animations,
            "Session switching",
            &["fade", "portal"],
            Choice(&EnumChoice::new(
                "Session switching animation",
                Choices::Fixed(SessionAnimationStyle::all),
                config_field!(animations.session),
                SessionAnimationStyle::label,
                ChoiceSave::Key(
                    ConfigKey::new("animations", "session"),
                    SessionAnimationStyle::id,
                ),
            )),
        ),
        row(
            CyclePickerAnimation,
            G::Animations,
            "Pickers",
            &["palette", "modal", "fade", "portal", "scan"],
            Choice(&EnumChoice::new(
                "Picker animation",
                Choices::Fixed(PickerAnimationStyle::all),
                config_field!(animations.picker),
                PickerAnimationStyle::label,
                ChoiceSave::Key(
                    ConfigKey::new("animations", "picker"),
                    PickerAnimationStyle::id,
                ),
            )),
        ),
        row(
            CyclePaneOpenAnimation,
            G::Animations,
            "Pane open",
            &["spawn", "scale", "slide", "portal", "scan", "particles"],
            Choice(&EnumChoice::new(
                "Pane open animation",
                Choices::Fixed(PaneAnimationStyle::all),
                config_field!(animations.pane_open_style),
                PaneAnimationStyle::label,
                ChoiceSave::With(save_pane_animation_styles),
            )),
        ),
        row(
            CyclePaneCloseAnimation,
            G::Animations,
            "Pane close",
            &["despawn", "scale", "slide", "portal", "scan", "particles"],
            Choice(&EnumChoice::new(
                "Pane close animation",
                Choices::Fixed(PaneAnimationStyle::all),
                config_field!(animations.pane_close_style),
                PaneAnimationStyle::label,
                ChoiceSave::With(save_pane_animation_styles),
            )),
        ),
        // Clipboard
        row(
            CycleCopyOnSelect,
            G::Clipboard,
            "Copy on selection",
            &["primary selection", "mouse"],
            Choice(
                &EnumChoice::new(
                    "Copy on selection",
                    Choices::Fixed(CopyOnSelect::all),
                    config_field!(clipboard.copy_on_select),
                    CopyOnSelect::label,
                    ChoiceSave::Key(
                        ConfigKey::new("clipboard", "copy_on_select"),
                        CopyOnSelect::id,
                    ),
                )
                .with_effect(SettingEffect::Clipboard),
            ),
        ),
        row(
            CycleMiddleClickPaste,
            G::Clipboard,
            "Middle-click paste",
            &["primary selection", "mouse"],
            Choice(
                &EnumChoice::new(
                    "Middle-click paste",
                    Choices::Fixed(MiddleClickPaste::all),
                    config_field!(clipboard.middle_click_paste),
                    MiddleClickPaste::label,
                    ChoiceSave::Key(
                        ConfigKey::new("clipboard", "middle_click_paste"),
                        MiddleClickPaste::id,
                    ),
                )
                .with_effect(SettingEffect::Clipboard),
            ),
        ),
        row(
            CycleRightClickClipboard,
            G::Clipboard,
            "Right-click action",
            &["copy", "paste", "mouse"],
            Choice(
                &EnumChoice::new(
                    "Right-click action",
                    Choices::Fixed(RightClickClipboardAction::all),
                    config_field!(clipboard.right_click),
                    RightClickClipboardAction::label,
                    ChoiceSave::Key(
                        ConfigKey::new("clipboard", "right_click"),
                        RightClickClipboardAction::id,
                    ),
                )
                .with_effect(SettingEffect::Clipboard),
            ),
        ),
        row(
            ToggleOsc52,
            G::Clipboard,
            "OSC 52",
            &["osc52", "ssh", "remote"],
            SettingControl::Toggle {
                field: config_field!(clipboard.enable_osc52),
                key: ConfigKey::new("clipboard", "enable_osc52"),
                effect: Some(SettingEffect::Clipboard),
            },
        ),
        // Pickers
        row(
            CyclePickerBorderStyle,
            G::Pickers,
            "Border",
            &["palette", "modal", "overlay"],
            Choice(
                &EnumChoice::new(
                    "Picker border",
                    Choices::Fixed(PaneBorderStyle::all),
                    config_field!(pane.picker_border_style),
                    PaneBorderStyle::label,
                    ChoiceSave::Key(
                        ConfigKey::new("pane", "picker_border_style"),
                        PaneBorderStyle::id,
                    ),
                )
                .previewed(),
            ),
        ),
        row(
            TogglePickerTabBackground,
            G::Pickers,
            "Tab strip",
            &["palette", "tab bar"],
            ActionToggle {
                value: |config| config.pane.picker_tab_background,
                action: Action::TogglePickerTabBackground,
            },
        ),
        row(
            CyclePickerTabStyle,
            G::Pickers,
            "Tab style",
            &["palette", "caps"],
            Choice(&cap_choice(
                "Picker tab style",
                Choices::Fixed(badge_cap_styles),
                config_field!(pane.picker_tab_style),
                ConfigKey::new("pane", "picker_tab_style"),
            )),
        ),
        row(
            CyclePickerSelectionStyle,
            G::Pickers,
            "Selection style",
            &["palette", "highlight", "caps", "pill"],
            Choice(&cap_choice(
                "Picker selection style",
                Choices::Fixed(badge_cap_styles),
                config_field!(pane.picker_selection_style),
                ConfigKey::new("pane", "picker_selection_style"),
            )),
        ),
        // Panes: background
        row(
            ToggleBackgroundFollowsTerminal,
            G::Background,
            "Follows terminal",
            &["match terminal"],
            ActionToggle {
                value: |config| config.pane.background_follows_terminal,
                action: Action::ToggleBackgroundFollowsTerminal,
            },
        ),
        row(
            ToggleHighlightFocusedBackground,
            G::Background,
            "Focused",
            &["active", "highlight"],
            ActionToggle {
                value: |config| config.pane.highlight_focused_background,
                action: Action::ToggleHighlightFocusedBackground,
            },
        ),
        row(
            EditPadding,
            G::Background,
            "Terminal padding",
            &["margins", "insets", "spacing"],
            Custom,
        ),
        // Panes: borders
        row(
            CycleBorderMode,
            G::Borders,
            "Mode",
            &["merge", "borderless", "dividers"],
            Choice(
                &EnumChoice::new(
                    "Border mode",
                    Choices::Fixed(PaneBorderMode::all),
                    config_field!(pane.border_mode),
                    PaneBorderMode::label,
                    ChoiceSave::Key(ConfigKey::new("pane", "border_mode"), PaneBorderMode::id),
                )
                .previewed(),
            ),
        ),
        row(
            CycleBorderStyle,
            G::Borders,
            "Style",
            &["tiled", "rounded", "square", "dashed"],
            Choice(&border_choice(
                "Border style",
                config_field!(pane.border_style),
                "border_style",
            )),
        ),
        row(
            ToggleHighlightFocusedBorder,
            G::Borders,
            "Focused",
            &["active", "highlight"],
            ActionToggle {
                value: |config| config.pane.highlight_focused_border,
                action: Action::ToggleHighlightFocusedBorder,
            },
        ),
        row(
            CycleFloatBorderStyle,
            G::Borders,
            "Floating",
            &["float", "popup"],
            Choice(&border_choice(
                "Floating border",
                config_field!(pane.float_border_style),
                "float_border_style",
            )),
        ),
        row(
            CycleScratchBorderStyle,
            G::Borders,
            "Scratchpad",
            &["scratch", "dropdown"],
            Choice(&border_choice(
                "Scratchpad border",
                config_field!(pane.scratch_border_style),
                "scratch_border_style",
            )),
        ),
        row(
            CycleFullscreenBorderStyle,
            G::Borders,
            "Fullscreen",
            &["full screen", "maximized", "maximised"],
            Choice(&border_choice(
                "Fullscreen border",
                config_field!(pane.fullscreen_border_style),
                "fullscreen_border_style",
            )),
        ),
        // Panes: titlebar
        row(
            ChooseTitlebar,
            G::Titlebar,
            "Layout",
            &["title bar", "show titles", "integrated", "inset"],
            Custom,
        ),
        row(
            CycleTitleStyle,
            G::Titlebar,
            "Style",
            &["caps", "pill"],
            Choice(&cap_choice(
                "Titlebar style",
                Choices::Fixed(CapStyle::all),
                config_field!(pane.title_style),
                ConfigKey::new("pane", "title_style"),
            )),
        ),
        row(
            ToggleHighlightFocusedTitlebar,
            G::Titlebar,
            "Focused",
            &["active", "highlight"],
            ActionToggle {
                value: |config| config.pane.highlight_focused_titlebar,
                action: Action::ToggleHighlightFocusedTitlebar,
            },
        ),
        // Workbar
        row(
            ChooseWorkbar,
            G::Workbar,
            "Position",
            &["show", "hide", "top", "bottom"],
            Custom,
        ),
        row(
            ToggleWorkbarGap,
            G::Workbar,
            "Gap",
            &["spacing"],
            ActionToggle {
                value: |config| config.pane.workbar_gap,
                action: Action::ToggleWorkbarGap,
            },
        ),
        row(
            ToggleWorkbarBackground,
            G::Workbar,
            "Background",
            &["strip", "fill"],
            ActionToggle {
                value: |config| config.pane.workbar_background,
                action: Action::ToggleWorkbarBackground,
            },
        ),
        row(
            CycleWorkbarStyle,
            G::Workbar,
            "Style",
            &["caps", "pill"],
            Choice(&cap_choice(
                "Workbar style",
                Choices::Fixed(CapStyle::all),
                config_field!(pane.workbar_style),
                ConfigKey::new("pane", "workbar_style"),
            )),
        ),
        row(
            CycleWorkbarBadgeStyle,
            G::Workbar,
            "Badge style",
            &["badges", "chips"],
            Choice(&cap_choice(
                "Workbar badge style",
                Choices::Fixed(badge_cap_styles),
                config_field!(pane.workbar_badge_style),
                ConfigKey::new("pane", "workbar_badge_style"),
            )),
        ),
        row(
            CycleWorkbarTabStyle,
            G::Workbar,
            "Tab style",
            &["workspace tabs"],
            Choice(&cap_choice(
                "Workbar tab style",
                Choices::Fixed(badge_cap_styles),
                config_field!(pane.workbar_tab_style),
                ConfigKey::new("pane", "workbar_tab_style"),
            )),
        ),
        row(
            ToggleWorkbarPowerline,
            G::Workbar,
            "Powerline",
            &["badge chain"],
            ActionToggle {
                value: |config| config.pane.workbar_powerline,
                action: Action::ToggleWorkbarPowerline,
            },
        ),
        // Sidebar
        row(
            LeftSidebarPanels,
            G::Sidebar,
            "Left panels",
            &["sidebar", "panels", "dock"],
            Custom,
        ),
        row(
            RightSidebarPanels,
            G::Sidebar,
            "Right panels",
            &["sidebar", "panels", "dock"],
            Custom,
        ),
        row(
            ToggleSidebarStartup,
            G::Sidebar,
            "Open sidebars at startup",
            &["sidebar", "startup", "launch", "visibility"],
            Custom,
        ),
        row(
            SidebarTabs,
            G::Sidebar,
            "Sidebar tabs",
            &["sidebar", "tabs", "visibility"],
            Custom,
        ),
        row(
            SidebarLayoutPreset,
            G::Sidebar,
            "Sidebar layout preset",
            &["sidebar", "layout", "preset"],
            Custom,
        ),
        row(
            ToggleSidebarBackgroundFollowsCanvas,
            G::Sidebar,
            "Background follows canvas",
            &["match canvas", "app background"],
            ActionToggle {
                value: |config| config.sidebar.background_follows_canvas,
                action: Action::ToggleSidebarBackgroundFollowsCanvas,
            },
        ),
        row(
            ToggleSidebarGap,
            G::Sidebar,
            "Gap",
            &["spacing"],
            ActionToggle {
                value: |config| config.sidebar.gap,
                action: Action::ToggleSidebarGap,
            },
        ),
        row(
            ToggleSidebarBackground,
            G::Sidebar,
            "Tab strip",
            &["background", "tab bar"],
            ActionToggle {
                value: |config| config.sidebar.background,
                action: Action::ToggleSidebarBackground,
            },
        ),
        row(
            CycleSidebarTabStyle,
            G::Sidebar,
            "Tab style",
            &["caps"],
            Choice(&cap_choice(
                "Sidebar tab style",
                Choices::Fixed(badge_cap_styles),
                config_field!(sidebar.tab_style),
                ConfigKey::new("sidebar", "tab_style"),
            )),
        ),
        // Alerts
        row(
            ToggleBellUrgency,
            G::Alerts,
            "Bell urgency",
            &["urgent", "terminal"],
            toggle(config_field!(notifications.bell), "notifications", "bell"),
        ),
        // Highlights
        row(
            CycleAlertBorder,
            G::Highlights,
            "Pane effect",
            &["attention", "pulse", "agent"],
            Choice(
                &EnumChoice::new(
                    "Pane alert effect",
                    Choices::Fixed(AlertMode::all),
                    config_field!(pane.alert_border),
                    AlertMode::label,
                    ChoiceSave::Key(ConfigKey::new("pane", "alert_border"), AlertMode::id),
                )
                .with_status(pane_alert_status),
            ),
        ),
        row(
            CycleAlertPaint,
            G::Highlights,
            "Pane paint",
            &["tint", "content", "attention", "breathe"],
            Choice(&EnumChoice::new(
                "Pane alert paint",
                Choices::Fixed(PaneAlertPaint::all),
                config_field!(pane.alert_paint),
                PaneAlertPaint::label,
                ChoiceSave::Key(ConfigKey::new("pane", "alert_paint"), PaneAlertPaint::id),
            )),
        ),
        row(
            CycleWorkbarAlert,
            G::Highlights,
            "Workspace tab",
            &["attention", "pulse", "marker"],
            Choice(
                &EnumChoice::new(
                    "Workspace tab effect",
                    Choices::Fixed(AlertMode::all),
                    config_field!(workbar.alert.mode),
                    AlertMode::label,
                    ChoiceSave::Key(ConfigKey::new("workbar.alert", "mode"), AlertMode::id),
                )
                .with_status(workbar_alert_status),
            ),
        ),
        row(
            CycleWorkbarAlertPaint,
            G::Highlights,
            "Workspace tab paint",
            &["fill", "color", "colour"],
            Cycle(&EnumChoice::new(
                "Workspace tab paint",
                Choices::Fixed(AlertPaint::all),
                config_field!(workbar.alert.paint),
                AlertPaint::label,
                ChoiceSave::Key(ConfigKey::new("workbar.alert", "paint"), AlertPaint::id),
            )),
        ),
        // Workspace tab markers. Working and Idle are status, not alerts, but they share the
        // marker surface and its `workbar.alert` keys.
        row(
            ToggleMarkBell,
            G::Marks,
            "Bell",
            &["marker"],
            toggle(config_field!(workbar.alert.bell), "workbar.alert", "bell"),
        ),
        row(
            ToggleMarkBlocked,
            G::Marks,
            "Blocked",
            &["marker", "waiting"],
            toggle(
                config_field!(workbar.alert.blocked),
                "workbar.alert",
                "blocked",
            ),
        ),
        row(
            ToggleMarkFinished,
            G::Marks,
            "Finished",
            &["marker", "done", "completed"],
            toggle(
                config_field!(workbar.alert.finished),
                "workbar.alert",
                "finished",
            ),
        ),
        row(
            ToggleMarkWorking,
            G::Marks,
            "Working",
            &["marker", "busy", "running"],
            toggle(
                config_field!(workbar.alert.working),
                "workbar.alert",
                "working",
            ),
        ),
        row(
            ToggleMarkIdle,
            G::Marks,
            "Idle",
            &["marker", "quiet"],
            toggle(config_field!(workbar.alert.idle), "workbar.alert", "idle"),
        ),
        // Desktop notifications
        row(
            ToggleDesktopEnabled,
            G::DesktopNotifications,
            "Show notifications",
            &["system", "notify-send", "popups"],
            toggle(
                config_field!(notifications.enabled),
                "notifications",
                "enabled",
            ),
        ),
        row(
            ToggleDesktopBlocked,
            G::DesktopNotifications,
            "Blocked",
            &["waiting", "agent prompt"],
            toggle(
                config_field!(notifications.pane_blocked),
                "notifications",
                "pane_blocked",
            ),
        ),
        row(
            ToggleDesktopDone,
            G::DesktopNotifications,
            "Finished",
            &["done"],
            toggle(
                config_field!(notifications.pane_done),
                "notifications",
                "pane_done",
            ),
        ),
        row(
            ToggleDesktopExit,
            G::DesktopNotifications,
            "Exit",
            &["quit"],
            toggle(
                config_field!(notifications.pane_exit),
                "notifications",
                "pane_exit",
            ),
        ),
        row(
            ToggleDesktopExitError,
            G::DesktopNotifications,
            "Exit with error",
            &["failure", "non-zero", "crash"],
            toggle(
                config_field!(notifications.pane_exit_error),
                "notifications",
                "pane_exit_error",
            ),
        ),
        // Sounds
        row(
            ToggleSoundEnabled,
            G::Sounds,
            "Play sounds",
            &["audio", "mute"],
            toggle(config_field!(sounds.enabled), "sounds", "enabled"),
        ),
        row(
            ToggleSoundBell,
            G::Sounds,
            "Bell",
            &["beep"],
            toggle(config_field!(sounds.bell), "sounds", "bell"),
        ),
        row(
            ToggleSoundBlocked,
            G::Sounds,
            "Blocked",
            &["waiting"],
            toggle(config_field!(sounds.blocked), "sounds", "blocked"),
        ),
        row(
            ToggleSoundDone,
            G::Sounds,
            "Finished",
            &["done"],
            toggle(config_field!(sounds.done), "sounds", "done"),
        ),
        row(
            ToggleSoundError,
            G::Sounds,
            "Exit with error",
            &["failure", "non-zero", "crash"],
            toggle(config_field!(sounds.error), "sounds", "error"),
        ),
        // Session startup, attach behavior, and server policies.
        row(
            CycleStartupMode,
            G::Sessions,
            "Startup mode",
            &["launch", "picker", "ephemeral", "last session"],
            Choice(&EnumChoice::new(
                "Startup mode",
                Choices::ForConfig(startup_choices),
                config_field!(session.startup),
                SessionStartup::label,
                ChoiceSave::Key(ConfigKey::new("session", "startup"), SessionStartup::id),
            )),
        ),
        row(
            CycleSessionPickerOpenOn,
            G::Sessions,
            "Open Sessions on",
            &["picker", "current host", "all sessions"],
            Choice(&EnumChoice::new(
                "Open Sessions on",
                Choices::Fixed(SessionPickerOpenOn::all),
                config_field!(session.picker_open_on),
                SessionPickerOpenOn::label,
                ChoiceSave::Key(
                    ConfigKey::new("session", "picker_open_on"),
                    SessionPickerOpenOn::id,
                ),
            )),
        ),
        row(
            CycleSessionOnAttach,
            G::Sessions,
            "On shared attach",
            &["follow", "take", "control", "attach"],
            Choice(&EnumChoice::new(
                "On shared attach",
                Choices::Fixed(SessionOnAttach::all),
                config_field!(session.on_attach),
                SessionOnAttach::label,
                ChoiceSave::Key(
                    ConfigKey::new("session", "on_attach"),
                    SessionOnAttach::as_str,
                ),
            )),
        ),
        row(
            ToggleKeepAwakeWhileAgentsWork,
            G::Sessions,
            "Keep awake while agents work",
            &["sleep", "power", "inhibit"],
            SettingControl::Toggle {
                field: config_field!(session.keep_awake_while_agents_work),
                key: ConfigKey::new("session", "keep_awake_while_agents_work"),
                effect: Some(SettingEffect::SleepPolicy),
            },
        ),
        row(
            ToggleSessionAutosave,
            G::Sessions,
            "Layout autosave",
            &["restore"],
            toggle(config_field!(session.autosave), "session", "autosave"),
        ),
        row(
            ToggleSessionResurrect,
            G::Sessions,
            "Resurrect named sessions",
            &["restore", "restart"],
            toggle(config_field!(session.resurrect), "session", "resurrect"),
        ),
        row(
            CycleResurrectForeground,
            G::Sessions,
            "Restored running commands",
            &["rerun", "agents", "foreground"],
            Choice(&EnumChoice::new(
                "Restored running commands",
                Choices::Fixed(ForegroundRestore::all),
                config_field!(session.resurrect_foreground),
                ForegroundRestore::label,
                ChoiceSave::Key(
                    ConfigKey::new("session", "resurrect_foreground"),
                    ForegroundRestore::as_str,
                ),
            )),
        ),
    ]
};

/// A previewed end-cap style choice.
const fn cap_choice(
    title: &'static str,
    choices: Choices<CapStyle>,
    field: ConfigField<CapStyle>,
    key: ConfigKey,
) -> EnumChoice<CapStyle> {
    EnumChoice::new(
        title,
        choices,
        field,
        cap_style_label,
        ChoiceSave::Key(key, cap_style_id),
    )
    .previewed()
}

/// A previewed `[pane]` border style choice.
const fn border_choice(
    title: &'static str,
    field: ConfigField<PaneBorderStyle>,
    key: &'static str,
) -> EnumChoice<PaneBorderStyle> {
    EnumChoice::new(
        title,
        Choices::Fixed(PaneBorderStyle::all),
        field,
        PaneBorderStyle::label,
        ChoiceSave::Key(ConfigKey::new("pane", key), PaneBorderStyle::id),
    )
    .previewed()
}

impl SettingsAction {
    /// This row's entry in [`SETTINGS`].
    pub fn spec(self) -> &'static SettingSpec {
        SETTINGS
            .iter()
            .find(|spec| spec.action == self)
            .expect("every Settings action has a row in SETTINGS")
    }

    /// Every setting, in the order Settings lists them.
    pub fn all() -> impl Iterator<Item = Self> {
        SETTINGS.iter().map(|spec| spec.action)
    }

    /// The choice behind a list-valued row, for rows the table describes.
    pub fn choice(self) -> Option<&'static dyn ChoiceSetting> {
        match self.spec().control {
            SettingControl::Choice(choice) | SettingControl::Cycle(choice) => Some(choice),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lists every action. The match is exhaustive, so a new action fails to compile here until it
    /// is listed, and the test then fails until it has a row in [`SETTINGS`].
    fn listed(action: SettingsAction) -> SettingsAction {
        use SettingsAction::*;
        match action {
            Theme
            | EditPadding
            | ToggleAnimations
            | ToggleWorkspaceAnimation
            | CycleSessionAnimation
            | CyclePickerAnimation
            | ToggleNerdIcons
            | CycleWhichKey
            | CycleCursorBlink
            | CycleCopyOnSelect
            | CycleMiddleClickPaste
            | CycleRightClickClipboard
            | ToggleOsc52
            | ToggleFocusOnHover
            | EditScrollMultiplier
            | ToggleBackgroundFollowsTerminal
            | ChooseTitlebar
            | CycleTitleStyle
            | ChooseWorkbar
            | ToggleWorkbarGap
            | ToggleWorkbarBackground
            | CycleWorkbarStyle
            | CycleWorkbarBadgeStyle
            | CycleWorkbarTabStyle
            | ToggleWorkbarPowerline
            | ToggleHighlightFocusedBackground
            | ToggleHighlightFocusedBorder
            | ToggleHighlightFocusedTitlebar
            | CycleBorderMode
            | CycleBorderStyle
            | CycleFloatBorderStyle
            | CycleScratchBorderStyle
            | CycleFullscreenBorderStyle
            | CyclePickerBorderStyle
            | TogglePickerTabBackground
            | CyclePickerTabStyle
            | CyclePickerSelectionStyle
            | CyclePaneOpenAnimation
            | CyclePaneCloseAnimation
            | ToggleSidebarBackgroundFollowsCanvas
            | ToggleSidebarStartup
            | LeftSidebarPanels
            | RightSidebarPanels
            | SidebarTabs
            | SidebarLayoutPreset
            | ToggleSidebarGap
            | ToggleSidebarBackground
            | CycleSidebarTabStyle
            | ToggleBellUrgency
            | CycleAlertBorder
            | CycleAlertPaint
            | CycleWorkbarAlert
            | CycleWorkbarAlertPaint
            | ToggleMarkBell
            | ToggleMarkBlocked
            | ToggleMarkFinished
            | ToggleMarkWorking
            | ToggleMarkIdle
            | ToggleDesktopEnabled
            | ToggleDesktopBlocked
            | ToggleDesktopDone
            | ToggleDesktopExit
            | ToggleDesktopExitError
            | ToggleSoundEnabled
            | ToggleSoundBell
            | ToggleSoundBlocked
            | ToggleSoundDone
            | ToggleSoundError
            | CycleStartupMode
            | CycleSessionPickerOpenOn
            | ToggleSessionAutosave
            | ToggleKeepAwakeWhileAgentsWork
            | ToggleSessionResurrect
            | CycleResurrectForeground
            | CycleSessionOnAttach => action,
        }
    }

    #[test]
    fn every_action_has_exactly_one_row() {
        let actions: Vec<_> = SettingsAction::all().map(listed).collect();
        for (index, action) in actions.iter().enumerate() {
            assert!(
                !actions[index + 1..].contains(action),
                "{action:?} has two rows"
            );
        }
        assert_eq!(actions.len(), 75, "a listed action has no row in SETTINGS");
    }

    #[test]
    fn a_groups_rows_are_consecutive() {
        let mut seen = Vec::new();
        for spec in SETTINGS {
            if seen.last() != Some(&spec.group) {
                assert!(
                    !seen.contains(&spec.group),
                    "{:?} is split by another group",
                    spec.group
                );
                seen.push(spec.group);
            }
        }
    }

    /// The group name is always searchable, so a row with no aliases of its own is reachable by
    /// its group alone.
    #[test]
    fn every_row_has_search_aliases_of_its_own() {
        for spec in SETTINGS {
            assert!(
                !spec.aliases.is_empty(),
                "{:?} is searchable only by its group name; give it aliases of its own",
                spec.action
            );
        }
    }

    /// A choice row's value text is the label of the option its picker opens on.
    #[test]
    fn a_choice_rows_status_names_its_current_option() {
        let config = Config::default();
        for spec in SETTINGS {
            let SettingControl::Choice(choice) = spec.control else {
                continue;
            };
            if matches!(
                spec.action,
                SettingsAction::CycleAlertBorder | SettingsAction::CycleWorkbarAlert
            ) {
                continue;
            }
            let ring = choice.ring(&config);
            assert_eq!(
                ring.options.get(ring.index),
                Some(&choice.status(&config)),
                "{:?}",
                spec.action
            );
        }
    }

    /// Applying each option and reading the ring back lands on that option, so the field a row
    /// writes is the field it reads.
    #[test]
    fn a_choice_reads_back_every_option_it_writes() {
        for spec in SETTINGS {
            let Some(choice) = spec.action.choice() else {
                continue;
            };
            let mut config = Config::default();
            let count = choice.ring(&config).options.len();
            assert!(count > 1, "{:?} has nothing to choose", spec.action);
            for index in 0..count {
                assert!(choice.apply(&mut config, index), "{:?}", spec.action);
                assert_eq!(choice.ring(&config).index, index, "{:?}", spec.action);
            }
            assert!(!choice.apply(&mut config, count), "{:?}", spec.action);
        }
    }

    #[test]
    fn a_toggle_reads_back_the_value_it_writes() {
        for spec in SETTINGS {
            let SettingControl::Toggle { field, .. } = &spec.control else {
                continue;
            };
            let mut config = Config::default();
            let before = (field.get)(&config);
            let slot = (field.slot)(&mut config);
            *slot = !*slot;
            assert_eq!((field.get)(&config), !before, "{:?}", spec.action);
        }
    }
}
