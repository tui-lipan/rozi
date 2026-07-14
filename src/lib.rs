//! Internal library surface shared by the hyprmux binary, integration tests, and benchmarks.
//!
//! This is not a stable public API.

mod actions;
mod anim;
pub mod app;
pub mod cli;
mod commands;
pub mod config;
mod control;
mod copy_mode;
pub mod events;
mod geometry;
mod hints;
mod input;
mod key_routing;
mod layout;
pub mod layout_tree_ser;
pub mod msg;
mod ops;
pub mod pane;
mod pane_lifecycle;
pub mod platform;
mod popup;
mod profiles;
mod pty_events;
mod rules;
mod scratchpad;
pub mod session;
pub mod shared_layout;
pub mod state;
mod tiling;
mod update;
mod view;

pub use app::HyprmuxApp;
pub use msg::Msg;

pub(crate) use app::{schedule_theme_tick, schedule_workbar_tick};
