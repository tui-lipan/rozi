#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertsAction {
    ToggleDoNotDisturb,
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

impl AlertsAction {
    /// Alerts has no nested rows, so its toggles and cycles follow Appearance's Left/Right policy.
    pub const fn steps_horizontally(self) -> bool {
        true
    }

    pub fn disabled_reason(
        self,
        pane: &crate::config::HyprmuxPaneConfig,
        notifications: bool,
        sounds: bool,
    ) -> Option<&'static str> {
        match self {
            Self::CycleAlertBorder if pane.border_mode == crate::state::PaneBorderMode::None => {
                Some("Needs pane borders")
            }
            Self::CycleWorkbarAlert
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
                if !notifications =>
            {
                Some("Needs notifications")
            }
            Self::ToggleSoundBell
            | Self::ToggleSoundBlocked
            | Self::ToggleSoundDone
            | Self::ToggleSoundError
                if !sounds =>
            {
                Some("Needs sound")
            }
            _ => None,
        }
    }
}
