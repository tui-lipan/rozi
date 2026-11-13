//! The emulator engine behind this process's terminal screens.
//!
//! `[experimental] terminal_engine` picks it. The session server and each UI client resolve it
//! from their own config once, before they build a screen, and keep it for the life of the
//! process, so one process never mixes engines and a config reload does not switch it. The two
//! processes need not agree: what crosses between them - live output, attach replays, resurrection
//! seeds, an upgrade handoff - is VT bytes, which either engine parses. A client on one engine
//! attaching to a server on the other, or an upgraded server on a different engine, rebuilds the
//! same screen from them.

use std::sync::OnceLock;

use tui_lipan::prelude::TerminalScreen;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalEngine {
    /// `alacritty_terminal`, the default.
    #[default]
    Alacritty,
    /// libghostty-vt, with the `ghostty-engine` feature. Experimental.
    Ghostty,
}

impl TerminalEngine {
    /// The engines this build can run, in `--version` order.
    pub(crate) const COMPILED: &'static [Self] = if cfg!(feature = "ghostty-engine") {
        &[Self::Alacritty, Self::Ghostty]
    } else {
        &[Self::Alacritty]
    };

    /// The config value naming this engine.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Alacritty => "alacritty",
            Self::Ghostty => "ghostty",
        }
    }
}

static ACTIVE: OnceLock<TerminalEngine> = OnceLock::new();

/// Fix this process's engine from its config. Call before the first screen is built; the first
/// call wins. Unit tests ignore it and run on [`default_engine`], so one test's config cannot
/// switch the engine under the rest of the binary.
pub(crate) fn install(engine: TerminalEngine) {
    if cfg!(not(test)) {
        let _ = ACTIVE.set(engine);
    }
}

/// The engine this process's screens run on.
pub(crate) fn active() -> TerminalEngine {
    *ACTIVE.get_or_init(default_engine)
}

/// Alacritty, except that the crate's own tests run on Ghostty with
/// `ROZI_TEST_TERMINAL_ENGINE=ghostty` (see `dev/terminal-engines.md`).
fn default_engine() -> TerminalEngine {
    #[cfg(all(test, feature = "ghostty-engine"))]
    if std::env::var_os("ROZI_TEST_TERMINAL_ENGINE").is_some_and(|engine| engine == "ghostty") {
        return TerminalEngine::Ghostty;
    }
    TerminalEngine::Alacritty
}

/// A bare screen on this process's engine. Panes use [`super::new_terminal_screen`]; this is for
/// tests and tools that need a screen without pane setup.
pub(crate) fn screen(rows: u16, cols: u16, scrollback: usize) -> TerminalScreen {
    #[cfg(feature = "ghostty-engine")]
    if active() == TerminalEngine::Ghostty {
        return TerminalScreen::with_engine(
            tui_lipan::TerminalEngineKind::Ghostty,
            rows,
            cols,
            scrollback,
        );
    }
    TerminalScreen::new(rows, cols, scrollback)
}
