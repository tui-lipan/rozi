//! Internal library surface shared by the rozi binary, integration tests, and benchmarks.
//!
//! This is not a stable public API.

mod actions;
pub mod agent_detection;
pub mod app;
pub mod cli;
mod commands;
pub mod config;
mod control;
pub mod events;
pub mod input;
pub mod layout;
pub mod msg;
mod ops;
pub mod pane;
pub mod platform;
mod profiles;
mod release_app;
pub mod runtime_metrics;
mod scratchpad;
pub mod session;
mod skill;
pub mod state;
#[doc(hidden)]
pub mod test_support;
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
