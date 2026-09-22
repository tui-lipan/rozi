use tui_lipan::prelude::{CapStyle, TextInput};

use crate::config::{
    Config, CopyOnSelect, ForegroundRestore, MiddleClickPaste, RightClickClipboardAction,
    SessionStartup, WhichKey,
};
use crate::layout::anim::PaneAnimationStyle;

use super::{
    AlertMode, PaneBorderMode, PaneBorderStyle, PaneTitlebarMode, badge_cap_styles, cap_style_label,
};

/// Browsing category. Search always spans every category.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Panes,
    Bars,
    Alerts,
    Sessions,
    All,
}

impl SettingsTab {
    pub const ALL: [Self; 6] = [
        Self::General,
        Self::Panes,
        Self::Bars,
        Self::Alerts,
        Self::Sessions,
        Self::All,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Panes => "Panes",
            Self::Bars => "Bars",
            Self::Alerts => "Alerts",
            Self::Sessions => "Sessions",
            Self::All => "All",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }

    pub fn stepped(self, reverse: bool) -> Self {
        let step = if reverse { Self::ALL.len() - 1 } else { 1 };
        Self::ALL[(self.index() + step) % Self::ALL.len()]
    }
}

#[derive(Default)]
pub struct SettingsNavigation {
    pub tab: SettingsTab,
    pub query: TextInput,
    pub browse_selected: Option<SettingsAction>,
    tab_selected: [Option<SettingsAction>; SettingsTab::ALL.len()],
}

impl SettingsNavigation {
    pub fn with_tab(tab: SettingsTab) -> Self {
        Self {
            tab,
            ..Default::default()
        }
    }

    pub fn remembered(&self, tab: SettingsTab) -> Option<SettingsAction> {
        self.tab_selected[tab.index()]
    }

    pub fn remember(&mut self, tab: SettingsTab, selected: Option<SettingsAction>) {
        self.tab_selected[tab.index()] = selected;
    }
}

pub fn assign_settings_selection(state: &mut super::State, selected: Option<SettingsAction>) {
    state.settings_selected = selected;
    let navigation = &mut state.settings_navigation;
    if navigation.query.text().is_empty() {
        let tab = navigation.tab;
        navigation.remember(tab, selected);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Theme,
    EditPadding,
    ToggleAnimations,
    ToggleWorkspaceAnimation,
    CycleSessionAnimation,
    ToggleNerdIcons,
    CycleWhichKey,
    CycleCopyOnSelect,
    CycleMiddleClickPaste,
    CycleRightClickClipboard,
    ToggleOsc52,
    ToggleFocusOnHover,
    ToggleBackgroundFollowsTerminal,
    ChooseTitlebar,
    CycleTitleStyle,
    ChooseWorkbar,
    ToggleWorkbarGap,
    ToggleWorkbarBackground,
    CycleWorkbarStyle,
    CycleWorkbarBadgeStyle,
    CycleWorkbarTabStyle,
    ToggleWorkbarPowerline,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleHighlightFocusedTitlebar,
    CycleBorderMode,
    CycleBorderStyle,
    CycleFloatBorderStyle,
    CycleScratchBorderStyle,
    CycleFullscreenBorderStyle,
    CyclePickerBorderStyle,
    TogglePickerTabBackground,
    CyclePickerTabStyle,
    CyclePickerSelectionStyle,
    CyclePaneAnimation,
    ToggleSidebarBackgroundFollowsCanvas,
    ToggleSidebarPosition,
    ToggleSidebarGap,
    ToggleSidebarBackground,
    CycleSidebarTabStyle,
    ToggleBellUrgency,
    CycleAlertBorder,
    CycleWorkbarAlert,
    CycleWorkbarAlertPaint,
    ToggleMarkBell,
    ToggleMarkBlocked,
    ToggleMarkFinished,
    ToggleMarkWorking,
    ToggleMarkIdle,
    ToggleDesktopEnabled,
    ToggleDesktopBlocked,
    ToggleDesktopDone,
    ToggleDesktopExit,
    ToggleDesktopExitError,
    ToggleSoundEnabled,
    ToggleSoundBell,
    ToggleSoundBlocked,
    ToggleSoundDone,
    ToggleSoundError,
    CycleStartupMode,
    ToggleSessionAutosave,
    ToggleSessionResurrect,
    CycleResurrectForeground,
}

impl SettingsAction {
    /// Every setting; the view groups these into browsing categories. Anything that has to reason about
    /// the whole set - search aliases, coverage tests - iterates this instead of repeating a
    /// hand-maintained list that silently omits whatever was added last.
    pub const fn all() -> &'static [Self] {
        &[
            // General
            Self::Theme,
            Self::EditPadding,
            Self::ToggleAnimations,
            Self::ToggleWorkspaceAnimation,
            Self::CycleSessionAnimation,
            Self::ToggleNerdIcons,
            Self::CycleWhichKey,
            Self::CycleCopyOnSelect,
            Self::CycleMiddleClickPaste,
            Self::CycleRightClickClipboard,
            Self::ToggleOsc52,
            Self::ToggleFocusOnHover,
            Self::ToggleBackgroundFollowsTerminal,
            Self::CyclePickerBorderStyle,
            Self::TogglePickerTabBackground,
            Self::CyclePickerTabStyle,
            Self::CyclePickerSelectionStyle,
            // Titlebar
            Self::ChooseTitlebar,
            Self::CycleTitleStyle,
            // Workbar
            Self::ChooseWorkbar,
            Self::ToggleWorkbarGap,
            Self::ToggleWorkbarBackground,
            Self::CycleWorkbarStyle,
            Self::CycleWorkbarBadgeStyle,
            Self::CycleWorkbarTabStyle,
            Self::ToggleWorkbarPowerline,
            // Panes
            Self::ToggleHighlightFocusedBackground,
            Self::ToggleHighlightFocusedBorder,
            Self::ToggleHighlightFocusedTitlebar,
            Self::CycleBorderMode,
            Self::CycleBorderStyle,
            Self::CycleFloatBorderStyle,
            Self::CycleScratchBorderStyle,
            Self::CycleFullscreenBorderStyle,
            Self::CyclePaneAnimation,
            // Sidebar
            Self::ToggleSidebarPosition,
            Self::ToggleSidebarBackgroundFollowsCanvas,
            Self::ToggleSidebarGap,
            Self::ToggleSidebarBackground,
            Self::CycleSidebarTabStyle,
            // Alerts
            Self::ToggleBellUrgency,
            Self::CycleAlertBorder,
            Self::CycleWorkbarAlert,
            Self::CycleWorkbarAlertPaint,
            Self::ToggleMarkBell,
            Self::ToggleMarkBlocked,
            Self::ToggleMarkFinished,
            Self::ToggleMarkWorking,
            Self::ToggleMarkIdle,
            // Desktop notifications
            Self::ToggleDesktopEnabled,
            Self::ToggleDesktopBlocked,
            Self::ToggleDesktopDone,
            Self::ToggleDesktopExit,
            Self::ToggleDesktopExitError,
            // Sounds
            Self::ToggleSoundEnabled,
            Self::ToggleSoundBell,
            Self::ToggleSoundBlocked,
            Self::ToggleSoundDone,
            Self::ToggleSoundError,
            // Sessions
            Self::CycleStartupMode,
            Self::ToggleSessionAutosave,
            Self::ToggleSessionResurrect,
            Self::CycleResurrectForeground,
        ]
    }

    /// Multi-value rows. Enter opens a compact picker; Shift+Enter cycles the live value.
    pub fn choice_ring(self, config: &Config) -> Option<SettingsChoiceRing> {
        let pane = &config.pane;
        match self {
            Self::CycleWhichKey => Some(choice_ring(
                "Which-key",
                WhichKey::all(),
                config.input.which_key,
                WhichKey::label,
            )),
            Self::CycleCopyOnSelect => Some(choice_ring(
                "Copy on selection",
                CopyOnSelect::all(),
                config.clipboard.copy_on_select,
                CopyOnSelect::label,
            )),
            Self::CycleMiddleClickPaste => Some(choice_ring(
                "Middle-click paste",
                MiddleClickPaste::all(),
                config.clipboard.middle_click_paste,
                MiddleClickPaste::label,
            )),
            Self::CycleRightClickClipboard => Some(choice_ring(
                "Right-click action",
                RightClickClipboardAction::all(),
                config.clipboard.right_click,
                RightClickClipboardAction::label,
            )),
            Self::CyclePickerBorderStyle => Some(choice_ring(
                "Picker border",
                PaneBorderStyle::all(),
                pane.picker_border_style,
                PaneBorderStyle::label,
            )),
            Self::CyclePickerTabStyle => Some(choice_ring(
                "Picker tab style",
                badge_cap_styles(),
                pane.picker_tab_style,
                cap_style_label,
            )),
            Self::CyclePickerSelectionStyle => Some(choice_ring(
                "Picker selection style",
                badge_cap_styles(),
                pane.picker_selection_style,
                cap_style_label,
            )),
            Self::ChooseTitlebar => Some(titlebar_choice_ring(pane)),
            Self::ChooseWorkbar => Some(workbar_choice_ring(pane)),
            Self::CycleTitleStyle => Some(choice_ring(
                "Titlebar style",
                CapStyle::all(),
                pane.title_style,
                cap_style_label,
            )),
            Self::CycleWorkbarStyle => Some(choice_ring(
                "Workbar style",
                CapStyle::all(),
                pane.workbar_style,
                cap_style_label,
            )),
            Self::CycleWorkbarBadgeStyle => Some(choice_ring(
                "Workbar badge style",
                badge_cap_styles(),
                pane.workbar_badge_style,
                cap_style_label,
            )),
            Self::CycleWorkbarTabStyle => Some(choice_ring(
                "Workbar tab style",
                badge_cap_styles(),
                pane.workbar_tab_style,
                cap_style_label,
            )),
            Self::CycleBorderMode => Some(choice_ring(
                "Border mode",
                PaneBorderMode::all(),
                pane.border_mode,
                PaneBorderMode::label,
            )),
            Self::CycleBorderStyle => Some(choice_ring(
                "Border style",
                PaneBorderStyle::all(),
                pane.border_style,
                PaneBorderStyle::label,
            )),
            Self::CycleFloatBorderStyle => Some(choice_ring(
                "Floating border",
                PaneBorderStyle::all(),
                pane.float_border_style,
                PaneBorderStyle::label,
            )),
            Self::CycleScratchBorderStyle => Some(choice_ring(
                "Scratchpad border",
                PaneBorderStyle::all(),
                pane.scratch_border_style,
                PaneBorderStyle::label,
            )),
            Self::CycleFullscreenBorderStyle => Some(choice_ring(
                "Fullscreen border",
                PaneBorderStyle::all(),
                pane.fullscreen_border_style,
                PaneBorderStyle::label,
            )),
            Self::CycleSessionAnimation => Some(choice_ring(
                "Session switching animation",
                crate::layout::anim::SessionAnimationStyle::all(),
                config.animations.session,
                crate::layout::anim::SessionAnimationStyle::label,
            )),
            Self::CyclePaneAnimation => Some(choice_ring(
                "Pane open/close animation",
                PaneAnimationStyle::all(),
                config.animations.pane_style,
                PaneAnimationStyle::label,
            )),
            Self::CycleSidebarTabStyle => Some(choice_ring(
                "Sidebar tab style",
                badge_cap_styles(),
                config.sidebar.tab_style,
                cap_style_label,
            )),
            Self::CycleAlertBorder => Some(choice_ring(
                "Pane border effect",
                AlertMode::all(),
                pane.alert_border,
                AlertMode::label,
            )),
            Self::CycleWorkbarAlert => Some(choice_ring(
                "Workspace tab effect",
                AlertMode::all(),
                config.workbar.alert.mode,
                AlertMode::label,
            )),
            Self::CycleStartupMode => {
                let choices = SessionStartup::choices(config.profile.default.is_some());
                Some(choice_ring(
                    "Startup mode",
                    &choices,
                    config.session.startup,
                    SessionStartup::label,
                ))
            }
            Self::CycleResurrectForeground => Some(choice_ring(
                "Restored running commands",
                ForegroundRestore::all(),
                config.session.resurrect_foreground,
                ForegroundRestore::label,
            )),
            _ => None,
        }
    }

    /// Whether the Settings label should wear `…`. That mark is for rows whose compact picker
    /// lists more than two options, so Enter is visible on the row; two-option toggles stay
    /// unmarked.
    pub fn shows_choice_ellipsis(self, config: &Config) -> bool {
        self.choice_ring(config)
            .is_some_and(|ring| ring.options.len() > 2)
    }

    /// Write `index` into live config without persisting.
    pub fn apply_choice(self, config: &mut Config, index: usize) -> bool {
        match self {
            Self::CycleWhichKey => {
                assign_choice(WhichKey::all(), index, &mut config.input.which_key)
            }
            Self::CycleCopyOnSelect => assign_choice(
                CopyOnSelect::all(),
                index,
                &mut config.clipboard.copy_on_select,
            ),
            Self::CycleMiddleClickPaste => assign_choice(
                MiddleClickPaste::all(),
                index,
                &mut config.clipboard.middle_click_paste,
            ),
            Self::CycleRightClickClipboard => assign_choice(
                RightClickClipboardAction::all(),
                index,
                &mut config.clipboard.right_click,
            ),
            Self::CyclePickerBorderStyle => assign_choice(
                PaneBorderStyle::all(),
                index,
                &mut config.pane.picker_border_style,
            ),
            Self::CyclePickerTabStyle => {
                assign_choice(badge_cap_styles(), index, &mut config.pane.picker_tab_style)
            }
            Self::CyclePickerSelectionStyle => assign_choice(
                badge_cap_styles(),
                index,
                &mut config.pane.picker_selection_style,
            ),
            Self::ChooseTitlebar => apply_titlebar_choice(config, index),
            Self::ChooseWorkbar => apply_workbar_choice(config, index),
            Self::CycleTitleStyle => {
                assign_choice(CapStyle::all(), index, &mut config.pane.title_style)
            }
            Self::CycleWorkbarStyle => {
                assign_choice(CapStyle::all(), index, &mut config.pane.workbar_style)
            }
            Self::CycleWorkbarBadgeStyle => assign_choice(
                badge_cap_styles(),
                index,
                &mut config.pane.workbar_badge_style,
            ),
            Self::CycleWorkbarTabStyle => assign_choice(
                badge_cap_styles(),
                index,
                &mut config.pane.workbar_tab_style,
            ),
            Self::CycleBorderMode => {
                assign_choice(PaneBorderMode::all(), index, &mut config.pane.border_mode)
            }
            Self::CycleBorderStyle => {
                assign_choice(PaneBorderStyle::all(), index, &mut config.pane.border_style)
            }
            Self::CycleFloatBorderStyle => assign_choice(
                PaneBorderStyle::all(),
                index,
                &mut config.pane.float_border_style,
            ),
            Self::CycleScratchBorderStyle => assign_choice(
                PaneBorderStyle::all(),
                index,
                &mut config.pane.scratch_border_style,
            ),
            Self::CycleFullscreenBorderStyle => assign_choice(
                PaneBorderStyle::all(),
                index,
                &mut config.pane.fullscreen_border_style,
            ),
            Self::CycleSessionAnimation => assign_choice(
                crate::layout::anim::SessionAnimationStyle::all(),
                index,
                &mut config.animations.session,
            ),
            Self::CyclePaneAnimation => assign_choice(
                PaneAnimationStyle::all(),
                index,
                &mut config.animations.pane_style,
            ),
            Self::CycleSidebarTabStyle => {
                assign_choice(badge_cap_styles(), index, &mut config.sidebar.tab_style)
            }
            Self::CycleAlertBorder => {
                assign_choice(AlertMode::all(), index, &mut config.pane.alert_border)
            }
            Self::CycleWorkbarAlert => {
                assign_choice(AlertMode::all(), index, &mut config.workbar.alert.mode)
            }
            Self::CycleStartupMode => {
                let choices = SessionStartup::choices(config.profile.default.is_some());
                assign_choice(&choices, index, &mut config.session.startup)
            }
            Self::CycleResurrectForeground => assign_choice(
                ForegroundRestore::all(),
                index,
                &mut config.session.resurrect_foreground,
            ),
            _ => false,
        }
    }

    pub fn disabled_reason(self, config: &Config) -> Option<&'static str> {
        let pane = &config.pane;
        match self {
            Self::CycleTitleStyle if !pane.show_titles => Some("Needs titlebar"),
            Self::CycleTitleStyle if !pane.titlebar.fills_strip() => {
                Some("Unsupported in this layout")
            }
            Self::ToggleHighlightFocusedBorder | Self::CycleAlertBorder
                if pane.border_mode == PaneBorderMode::None =>
            {
                Some("Needs pane borders")
            }
            Self::CycleBorderStyle | Self::CycleFullscreenBorderStyle
                if !pane.border_mode.draws_frames() =>
            {
                Some("Unsupported in this mode")
            }
            Self::CycleFloatBorderStyle | Self::CycleScratchBorderStyle
                if !pane.border_mode.draws_frames() && !pane.keep_special_borders =>
            {
                Some("Unsupported in this mode")
            }
            Self::CyclePaneAnimation
            | Self::ToggleWorkspaceAnimation
            | Self::CycleSessionAnimation
                if !config.animations.enabled =>
            {
                Some("Needs animations")
            }
            Self::ToggleWorkbarGap
            | Self::ToggleWorkbarBackground
            | Self::CycleWorkbarStyle
            | Self::CycleWorkbarBadgeStyle
            | Self::CycleWorkbarTabStyle
            | Self::ToggleWorkbarPowerline
            | Self::CycleWorkbarAlert
            | Self::CycleWorkbarAlertPaint
            | Self::ToggleMarkBell
            | Self::ToggleMarkBlocked
            | Self::ToggleMarkFinished
            | Self::ToggleMarkWorking
            | Self::ToggleMarkIdle
                if !pane.show_workbar =>
            {
                Some("Needs workbar")
            }
            Self::ToggleDesktopBlocked
            | Self::ToggleDesktopDone
            | Self::ToggleDesktopExit
            | Self::ToggleDesktopExitError
                if !config.notifications.enabled =>
            {
                Some("Needs notifications")
            }
            Self::ToggleSoundBell
            | Self::ToggleSoundBlocked
            | Self::ToggleSoundDone
            | Self::ToggleSoundError
                if !config.sounds.enabled =>
            {
                Some("Needs sound")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SettingsChoiceRing {
    pub title: &'static str,
    pub options: Vec<&'static str>,
    pub index: usize,
}

fn assign_choice<T: Copy>(all: &[T], index: usize, slot: &mut T) -> bool {
    let Some(&value) = all.get(index) else {
        return false;
    };
    *slot = value;
    true
}

fn titlebar_choice_ring(pane: &crate::config::PaneConfig) -> SettingsChoiceRing {
    let index = if pane.show_titles {
        PaneTitlebarMode::all()
            .iter()
            .position(|mode| *mode == pane.titlebar)
            .map(|index| index + 1)
            .unwrap_or(1)
    } else {
        0
    };
    SettingsChoiceRing {
        title: "Layout",
        options: ["Hidden", "Bar", "Border", "Integrated", "Inset"]
            .into_iter()
            .collect(),
        index,
    }
}

fn workbar_choice_ring(pane: &crate::config::PaneConfig) -> SettingsChoiceRing {
    let index = if !pane.show_workbar {
        0
    } else if pane.workbar_at_bottom {
        2
    } else {
        1
    };
    SettingsChoiceRing {
        title: "Position",
        options: vec!["Hidden", "Top", "Bottom"],
        index,
    }
}

fn apply_titlebar_choice(config: &mut Config, index: usize) -> bool {
    match index {
        0 => {
            config.pane.show_titles = false;
            true
        }
        n => {
            let Some(&mode) = PaneTitlebarMode::all().get(n - 1) else {
                return false;
            };
            config.pane.show_titles = true;
            config.pane.titlebar = mode;
            true
        }
    }
}

fn apply_workbar_choice(config: &mut Config, index: usize) -> bool {
    match index {
        0 => {
            config.pane.show_workbar = false;
            true
        }
        1 => {
            config.pane.show_workbar = true;
            config.pane.workbar_at_bottom = false;
            true
        }
        2 => {
            config.pane.show_workbar = true;
            config.pane.workbar_at_bottom = true;
            true
        }
        _ => false,
    }
}

fn choice_ring<T: Copy + PartialEq>(
    title: &'static str,
    all: &[T],
    current: T,
    label: fn(T) -> &'static str,
) -> SettingsChoiceRing {
    let index = all
        .iter()
        .position(|candidate| *candidate == current)
        .unwrap_or(0);
    SettingsChoiceRing {
        title,
        options: all.iter().copied().map(label).collect(),
        index,
    }
}

/// Backing config captured when a choice picker opens. Esc restores this exactly.
///
/// A single-field row stores its original index. A composite row stores every key it can change,
/// because Hidden is one visible choice that must not overwrite the remembered layout or position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsChoiceSnapshot {
    Index(usize),
    Titlebar {
        show_titles: bool,
        titlebar: PaneTitlebarMode,
    },
    Workbar {
        show_workbar: bool,
        workbar_at_bottom: bool,
    },
}

impl SettingsChoiceSnapshot {
    fn capture(action: SettingsAction, config: &Config) -> Self {
        match action {
            SettingsAction::ChooseTitlebar => Self::Titlebar {
                show_titles: config.pane.show_titles,
                titlebar: config.pane.titlebar,
            },
            SettingsAction::ChooseWorkbar => Self::Workbar {
                show_workbar: config.pane.show_workbar,
                workbar_at_bottom: config.pane.workbar_at_bottom,
            },
            _ => Self::Index(
                action
                    .choice_ring(config)
                    .map(|ring| ring.index)
                    .unwrap_or(0),
            ),
        }
    }

    fn unchanged(self, editor_index: usize, config: &Config) -> bool {
        match self {
            Self::Index(original) => editor_index == original,
            Self::Titlebar {
                show_titles,
                titlebar,
            } => config.pane.show_titles == show_titles && config.pane.titlebar == titlebar,
            Self::Workbar {
                show_workbar,
                workbar_at_bottom,
            } => {
                config.pane.show_workbar == show_workbar
                    && config.pane.workbar_at_bottom == workbar_at_bottom
            }
        }
    }

    fn restore(self, action: SettingsAction, config: &mut Config) {
        match self {
            Self::Index(index) => {
                action.apply_choice(config, index);
            }
            Self::Titlebar {
                show_titles,
                titlebar,
            } => {
                config.pane.show_titles = show_titles;
                config.pane.titlebar = titlebar;
            }
            Self::Workbar {
                show_workbar,
                workbar_at_bottom,
            } => {
                config.pane.show_workbar = show_workbar;
                config.pane.workbar_at_bottom = workbar_at_bottom;
            }
        }
    }
}

/// Pending multi-value choice. Highlight previews; Enter persists; Esc restores [`SettingsChoiceSnapshot`].
pub struct SettingsChoiceEditor {
    pub action: SettingsAction,
    pub title: &'static str,
    pub options: Vec<&'static str>,
    pub index: usize,
    pub original_index: usize,
    pub snapshot: SettingsChoiceSnapshot,
}

impl SettingsChoiceEditor {
    pub fn from_ring(action: SettingsAction, ring: SettingsChoiceRing, config: &Config) -> Self {
        Self {
            action,
            title: ring.title,
            options: ring.options,
            index: ring.index,
            original_index: ring.index,
            snapshot: SettingsChoiceSnapshot::capture(action, config),
        }
    }
}

/// Drop a live-preview picker and restore the value from before it opened.
pub fn abandon_settings_choice(state: &mut super::State) -> Option<std::time::Duration> {
    let editor = state.settings_choice.take()?;
    if editor.snapshot.unchanged(editor.index, &state.config) {
        return None;
    }
    editor.snapshot.restore(editor.action, &mut state.config);
    matches!(editor.action, SettingsAction::CycleWhichKey)
        .then(|| state.config.input.which_key.reveal_delay())
}

/// Which of the terminal-padding editor's two fields the cursor is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanePaddingField {
    #[default]
    Vertical,
    Horizontal,
}

/// Temporary values for the Settings terminal-padding editor. Focus, rather than a second stage
/// flag, determines whether Enter advances or applies.
pub struct PanePaddingEditorState {
    pub vertical: TextInput,
    pub horizontal: TextInput,
    pub normalizes_asymmetric: bool,
    /// Mirrors where the runtime put focus, so the dialog can mark the active field the way the
    /// host editor does. Tracked rather than derived: the two fields sit side by side, and nothing
    /// else on this state says which one the next keystroke reaches.
    pub focus: PanePaddingField,
}

impl PanePaddingEditorState {
    pub fn new(padding: (u16, u16, u16, u16)) -> Self {
        let symmetric = padding.0 == padding.2 && padding.1 == padding.3;
        let mut vertical = TextInput::new(if symmetric {
            padding.0.to_string()
        } else {
            String::new()
        });
        let mut horizontal = TextInput::new(if symmetric {
            padding.1.to_string()
        } else {
            String::new()
        });
        if symmetric {
            vertical.set_anchor(Some(0));
            horizontal.set_anchor(Some(0));
        }
        Self {
            vertical,
            horizontal,
            normalizes_asymmetric: !symmetric,
            focus: PanePaddingField::Vertical,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_padding_prefills_and_asymmetric_padding_requires_explicit_normalization() {
        let symmetric = PanePaddingEditorState::new((2, 1, 2, 1));
        assert_eq!(symmetric.vertical.text(), "2");
        assert_eq!(symmetric.horizontal.text(), "1");
        assert!(!symmetric.normalizes_asymmetric);

        let asymmetric = PanePaddingEditorState::new((1, 2, 3, 4));
        assert!(asymmetric.vertical.text().is_empty());
        assert!(asymmetric.horizontal.text().is_empty());
        assert!(asymmetric.normalizes_asymmetric);
    }

    #[test]
    fn multi_value_rows_advertise_a_choice_picker() {
        let config = Config::default();
        assert!(SettingsAction::CycleWhichKey.shows_choice_ellipsis(&config));
        assert!(SettingsAction::CycleCopyOnSelect.shows_choice_ellipsis(&config));
        assert!(SettingsAction::CycleMiddleClickPaste.shows_choice_ellipsis(&config));
        assert!(SettingsAction::CycleRightClickClipboard.shows_choice_ellipsis(&config));
        assert!(SettingsAction::CyclePaneAnimation.shows_choice_ellipsis(&config));
        assert!(SettingsAction::ChooseTitlebar.shows_choice_ellipsis(&config));
        assert!(SettingsAction::ChooseWorkbar.shows_choice_ellipsis(&config));
        assert!(SettingsAction::CycleStartupMode.shows_choice_ellipsis(&config));
        assert!(!SettingsAction::ToggleAnimations.shows_choice_ellipsis(&config));
        assert!(!SettingsAction::CycleWorkbarAlertPaint.shows_choice_ellipsis(&config));
        assert!(!SettingsAction::Theme.shows_choice_ellipsis(&config));
    }

    #[test]
    fn settings_dependencies_cover_appearance_and_alert_rows() {
        let mut config = Config::default();
        for mode in [PaneBorderMode::None, PaneBorderMode::Dividers] {
            config.pane.border_mode = mode;
            assert_eq!(
                SettingsAction::CycleBorderStyle.disabled_reason(&config),
                Some("Unsupported in this mode")
            );
            assert_eq!(
                SettingsAction::CycleFullscreenBorderStyle.disabled_reason(&config),
                Some("Unsupported in this mode")
            );
            assert_eq!(
                SettingsAction::CycleFloatBorderStyle.disabled_reason(&config),
                None
            );
            assert_eq!(
                SettingsAction::CycleScratchBorderStyle.disabled_reason(&config),
                None
            );
        }
        config.pane.keep_special_borders = false;
        assert_eq!(
            SettingsAction::CycleFloatBorderStyle.disabled_reason(&config),
            Some("Unsupported in this mode")
        );
        assert_eq!(
            SettingsAction::CycleScratchBorderStyle.disabled_reason(&config),
            Some("Unsupported in this mode")
        );
        config.pane.keep_special_borders = true;
        config.pane.border_mode = PaneBorderMode::None;
        assert_eq!(
            SettingsAction::ToggleHighlightFocusedBorder.disabled_reason(&config),
            Some("Needs pane borders")
        );
        assert_eq!(
            SettingsAction::CycleAlertBorder.disabled_reason(&config),
            Some("Needs pane borders")
        );
        config.pane.border_mode = PaneBorderMode::Dividers;
        assert_eq!(
            SettingsAction::ToggleHighlightFocusedBorder.disabled_reason(&config),
            None
        );
        assert_eq!(
            SettingsAction::CycleAlertBorder.disabled_reason(&config),
            None
        );
        config.pane.show_workbar = false;
        assert_eq!(
            SettingsAction::ToggleWorkbarBackground.disabled_reason(&config),
            Some("Needs workbar")
        );
        config.pane.show_workbar = true;
        assert_eq!(
            SettingsAction::ToggleWorkbarBackground.disabled_reason(&config),
            None
        );
    }

    #[test]
    fn hidden_titlebar_and_workbar_choices_keep_the_remembered_value() {
        let mut config = Config::default();
        config.pane.show_titles = true;
        config.pane.titlebar = PaneTitlebarMode::Inset;
        assert!(SettingsAction::ChooseTitlebar.apply_choice(&mut config, 0));
        assert!(!config.pane.show_titles);
        assert_eq!(config.pane.titlebar, PaneTitlebarMode::Inset);
        let hidden = SettingsAction::ChooseTitlebar
            .choice_ring(&config)
            .expect("titlebar choices");
        assert_eq!(
            hidden.options,
            ["Hidden", "Bar", "Border", "Integrated", "Inset"]
        );
        assert_eq!(hidden.index, 0);

        assert!(SettingsAction::ChooseTitlebar.apply_choice(&mut config, 3));
        assert!(config.pane.show_titles);
        assert_eq!(config.pane.titlebar, PaneTitlebarMode::Integrated);
        assert!(SettingsAction::ChooseTitlebar.apply_choice(&mut config, 0));
        assert!(!config.pane.show_titles);
        assert_eq!(config.pane.titlebar, PaneTitlebarMode::Integrated);

        config.pane.show_workbar = true;
        config.pane.workbar_at_bottom = true;
        assert!(SettingsAction::ChooseWorkbar.apply_choice(&mut config, 0));
        assert!(!config.pane.show_workbar);
        assert!(config.pane.workbar_at_bottom);
        let workbar = SettingsAction::ChooseWorkbar
            .choice_ring(&config)
            .expect("workbar choices");
        assert_eq!(workbar.options, ["Hidden", "Top", "Bottom"]);
        assert_eq!(workbar.index, 0);

        assert!(SettingsAction::ChooseWorkbar.apply_choice(&mut config, 1));
        assert!(config.pane.show_workbar);
        assert!(!config.pane.workbar_at_bottom);
        assert!(SettingsAction::ChooseWorkbar.apply_choice(&mut config, 2));
        assert!(config.pane.show_workbar);
        assert!(config.pane.workbar_at_bottom);
    }

    #[test]
    fn cancelling_a_hidden_choice_preview_restores_both_keys() {
        let mut config = Config::default();
        config.pane.show_titles = false;
        config.pane.titlebar = PaneTitlebarMode::Inset;
        config.pane.show_workbar = false;
        config.pane.workbar_at_bottom = true;
        let mut state = crate::state::State::new(config, Default::default());

        for (action, preview) in [
            (SettingsAction::ChooseTitlebar, 1),
            (SettingsAction::ChooseWorkbar, 1),
        ] {
            let ring = action.choice_ring(&state.config).expect("choice ring");
            state.settings_choice =
                Some(SettingsChoiceEditor::from_ring(action, ring, &state.config));
            assert!(action.apply_choice(&mut state.config, preview));
            state.settings_choice.as_mut().expect("editor").index = preview;
            abandon_settings_choice(&mut state);
        }

        assert!(!state.config.pane.show_titles);
        assert_eq!(state.config.pane.titlebar, PaneTitlebarMode::Inset);
        assert!(!state.config.pane.show_workbar);
        assert!(state.config.pane.workbar_at_bottom);
    }
}
