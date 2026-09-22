//! The colours human-facing command output paints with, handed from the running app to the CLI.
//!
//! Only the app can resolve the active theme: a custom theme file overlays another, and the
//! `system` theme is built from colours the host terminal reports to a live query. So the app
//! writes the handful of roles the CLI paints with to a state file whenever its resolved theme
//! changes, and a command run inside a rozi pane reads them back. That pane's background is the
//! theme's, so the output matches it. Anywhere else the terminal's background belongs to its own
//! theme, and the CLI keeps the rozi brand palette.
//!
//! The file is a hint, never an authority: a missing, stale, or malformed one falls back to the
//! brand palette rather than failing the command.

use std::io;
use std::path::{Path, PathBuf};

use super::ansi::{Rgb, palette};
use super::paths::{self, PlatformEnv};

/// Bumped when a role changes meaning, so an old file is ignored rather than misread.
const FORMAT_VERSION: u32 = 1;

const FILE_NAME: &str = "cli-palette.toml";

/// The theme roles command output uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CliPalette {
    pub accent: Rgb,
    pub muted: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub error: Rgb,
}

impl CliPalette {
    /// The rozi brand palette, which the default rozi theme also resolves to.
    pub const BRAND: Self = Self {
        accent: palette::ROSE,
        muted: palette::LAVENDER,
        success: palette::SUCCESS,
        warning: palette::WARNING,
        error: palette::ERROR,
    };

    /// The palette for output from this process: the running app's theme inside a rozi pane, and
    /// the brand palette anywhere else.
    pub fn current() -> Self {
        if std::env::var_os("ROZI_PANE").is_none() {
            return Self::BRAND;
        }
        palette_path(&PlatformEnv::from_process())
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| Self::parse(&text))
            .unwrap_or(Self::BRAND)
    }

    /// Record this palette for commands run inside rozi panes.
    pub fn save(&self) -> io::Result<()> {
        let env = PlatformEnv::from_process();
        let path = palette_path(&env)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state directory"))?;
        paths::private_state_dir(&env)?;
        save_to(&path, self)
    }

    /// Accent softened halfway toward muted: the column that names each row of a table, kept in
    /// the heading's family without merging into the header above it.
    pub fn key(&self) -> Rgb {
        self.accent.mix(self.muted, 1, 2)
    }

    fn format(&self) -> String {
        let hex = |Rgb(r, g, b): Rgb| format!("\"#{r:02x}{g:02x}{b:02x}\"");
        format!(
            "# Written by rozi from the active theme. Edits are overwritten.\n\
             version = {FORMAT_VERSION}\n\
             accent = {}\n\
             muted = {}\n\
             success = {}\n\
             warning = {}\n\
             error = {}\n",
            hex(self.accent),
            hex(self.muted),
            hex(self.success),
            hex(self.warning),
            hex(self.error),
        )
    }

    fn parse(text: &str) -> Option<Self> {
        let table: toml::Table = text.parse().ok()?;
        if table.get("version")?.as_integer()? != i64::from(FORMAT_VERSION) {
            return None;
        }
        let color = |key: &str| parse_hex(table.get(key)?.as_str()?);
        Some(Self {
            accent: color("accent")?,
            muted: color("muted")?,
            success: color("success")?,
            warning: color("warning")?,
            error: color("error")?,
        })
    }
}

fn palette_path(env: &PlatformEnv) -> Option<PathBuf> {
    let has_base = env.home.is_some()
        || env.xdg_state_home.is_some()
        || (cfg!(windows) && env.local_appdata.is_some());
    has_base.then(|| paths::state_dir(env).join(FILE_NAME))
}

fn save_to(path: &Path, palette: &CliPalette) -> io::Result<()> {
    super::persist::replace_file(path, palette.format())
}

fn parse_hex(value: &str) -> Option<Rgb> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some(Rgb(channel(0)?, channel(2)?, channel(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEMED: CliPalette = CliPalette {
        accent: Rgb(0x7A, 0xA2, 0xF7),
        muted: Rgb(0x56, 0x5F, 0x89),
        success: Rgb(0x9E, 0xCE, 0x6A),
        warning: Rgb(0xE0, 0xAF, 0x68),
        error: Rgb(0xF7, 0x76, 0x8E),
    };

    #[test]
    fn a_saved_palette_reads_back_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(FILE_NAME);
        save_to(&path, &THEMED).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(CliPalette::parse(&text), Some(THEMED));
    }

    #[test]
    fn an_unreadable_palette_is_ignored_rather_than_half_applied() {
        let text = THEMED.format();
        assert_eq!(
            CliPalette::parse(&text.replace("version = 1", "version = 2")),
            None
        );
        assert_eq!(
            CliPalette::parse(&text.replace("\"#9ece6a\"", "\"green\"")),
            None
        );
        assert_eq!(CliPalette::parse(&text.replace("error = ", "err = ")), None);
        assert_eq!(CliPalette::parse("not toml ["), None);
    }

    #[test]
    fn the_key_colour_is_accent_halfway_to_muted() {
        assert_eq!(CliPalette::BRAND.key(), Rgb(0xC5, 0x6F, 0x9A));
    }
}
