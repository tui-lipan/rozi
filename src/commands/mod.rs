//! Builds and registers rozi's `CommandEntry` set with tui-lipan's native command
//! registry/chord dispatch.
//!
//! Three families of commands are registered:
//! - [`BUILTIN_COMMANDS`]: the stable, individually rebindable actions (`Action::id()`).
//!   Each gets a leader-prefix chord (`<prefix> <key>`) and a WM-modifier chord
//!   (`<modifier>-<key>`) by default; a `[keys]` override replaces both with the user's exact
//!   bindings, except that a bare key step (e.g. `"b"`) re-enters the same prefix/modifier
//!   expansion with that key (resolved at config parse time in `build_key_overrides`).
//! - Workspace digit switch/move/relocate (27 commands, `workspace.<kind>.<1-9>`): not
//!   individually rebindable, generated straight from the configured prefix/modifier.
//! - User `[keys]` `{ run = .. }` / `{ send = .. }` commands (`user.<index>`), one literal
//!   binding each, exactly as configured.
//!
//! [`sync`] (re)registers everything from the current `State`, including a global
//! `enabled` gate ([`commands_active`]) that disables every command while a modal overlay or
//! `Resize`/`Copy` mode has focus, so leader/modifier chords never steal keys from a focused
//! text widget (e.g. `Ctrl+A` for select-all in a rename prompt) or fire mid-resize.

mod catalog;
mod registry;

pub(crate) use catalog::*;
pub(crate) use registry::*;
