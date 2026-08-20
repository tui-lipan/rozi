//! Internal library surface shared by the rozi binary, integration tests, and benchmarks.
//!
//! This is not a stable public API.

mod actions;
pub mod agent_detection;
pub mod anim;
pub mod app;
pub mod cli;
mod commands;
pub mod config;
mod control;
mod copy_mode;
pub mod events;
mod exit_view;
mod geometry;
mod hints;
pub mod input;
mod key_routing;
mod keys_display;
mod layout;
pub mod layout_tree_ser;
pub mod msg;
mod ops;
pub mod pane;
pub mod pane_launch;
mod pane_lifecycle;
pub mod platform;
mod popup;
mod profiles;
mod pty_events;
mod release_app;
mod rules;
pub mod runtime_metrics;
mod scratchpad;
mod send_keys;
pub mod session;
pub mod shared_layout;
mod skill;
pub mod state;
#[doc(hidden)]
pub mod test_support;
pub mod tiling;
mod update;
mod view;

pub use app::AppRoot;
pub use msg::Msg;

#[doc(hidden)]
pub use ops::search::{
    SearchScanAdvance as BenchmarkSearchScanAdvance,
    advance_search_scan as benchmark_advance_search_scan,
};

pub(crate) use app::{
    schedule_agent_tick, schedule_alert_pulse_tick, schedule_theme_tick, schedule_workbar_tick,
};
