use std::str::FromStr;

/// Cursor blink policy shared by editable controls and terminal panes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorBlink {
    #[default]
    Inherit,
    Static,
    Blinking,
}

impl CursorBlink {
    pub fn all() -> &'static [Self] {
        &[Self::Inherit, Self::Static, Self::Blinking]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Static => "static",
            Self::Blinking => "blinking",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Inherit => "Inherit",
            Self::Static => "Static",
            Self::Blinking => "Blinking",
        }
    }

    pub fn override_blinking(self) -> Option<bool> {
        match self {
            Self::Inherit => None,
            Self::Static => Some(false),
            Self::Blinking => Some(true),
        }
    }
}

impl FromStr for CursorBlink {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "inherit" => Ok(Self::Inherit),
            "static" => Ok(Self::Static),
            "blinking" => Ok(Self::Blinking),
            _ => Err(format!(
                "Ignored cursor.blink {value:?}: expected inherit, static, or blinking"
            )),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CursorConfig {
    pub blink: CursorBlink,
}
