use tui_lipan::prelude::TextInput;

use crate::config::Config;

use super::{PaneBorderMode, PaneTitlebarMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Theme,
    EditPadding,
    ToggleAnimations,
    ToggleFocusOnHover,
    ToggleBackgroundFollowsTerminal,
    ToggleTitles,
    CycleTitlebar,
    CycleTitleStyle,
    ToggleWorkbar,
    ToggleWorkbarPosition,
    ToggleWorkbarGap,
    CycleWorkbarStyle,
    CycleWorkbarBadgeStyle,
    CycleWorkbarTabStyle,
    ToggleWorkbarPowerline,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleHighlightFocusedTitlebar,
    CycleBorderMode,
    CycleBorderStyle,
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
}

impl SettingsAction {
    /// Rows whose value can be nudged with Left/Right. Theme and padding open nested UI, so arrows
    /// stay with the search caret there.
    pub const fn steps_horizontally(self) -> bool {
        !matches!(self, Self::Theme | Self::EditPadding)
    }

    /// Dependent settings stay visible but inert while their parent feature is unavailable.
    pub fn disabled_reason(self, config: &Config) -> Option<&'static str> {
        let pane = &config.pane;
        match self {
            Self::CycleTitlebar | Self::CycleTitleStyle if !pane.show_titles => {
                Some("Needs titlebar")
            }
            Self::CycleTitleStyle if pane.titlebar == PaneTitlebarMode::Border => {
                Some("Needs bar or integrated titlebar")
            }
            Self::ToggleHighlightFocusedBorder | Self::CycleAlertBorder
                if pane.border_mode == PaneBorderMode::None =>
            {
                Some("Needs pane borders")
            }
            Self::CycleBorderStyle if !pane.border_mode.draws_frames() => {
                Some("Unsupported in this mode")
            }
            Self::ToggleWorkbarPosition
            | Self::ToggleWorkbarGap
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

/// Temporary values for the Settings terminal-padding editor. Focus, rather than a second stage
/// flag, determines whether Enter advances or applies.
pub struct PanePaddingEditorState {
    pub vertical: TextInput,
    pub horizontal: TextInput,
    pub normalizes_asymmetric: bool,
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
    fn settings_dependencies_cover_appearance_and_alert_rows() {
        let mut config = Config::default();
        for mode in [PaneBorderMode::None, PaneBorderMode::Dividers] {
            config.pane.border_mode = mode;
            assert_eq!(
                SettingsAction::CycleBorderStyle.disabled_reason(&config),
                Some("Unsupported in this mode")
            );
        }
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
    }
}
