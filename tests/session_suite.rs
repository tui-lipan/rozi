mod common;

#[path = "suites/session/pane_status_e2e.rs"]
mod pane_status_e2e;
#[path = "suites/session/published_rows_e2e.rs"]
mod published_rows_e2e;
#[path = "suites/session/session_file_tree.rs"]
mod session_file_tree;
#[path = "suites/session/session_multi_client.rs"]
mod session_multi_client;
#[path = "suites/session/session_protocol_errors.rs"]
mod session_protocol_errors;
#[path = "suites/session/session_protocol_skew.rs"]
mod session_protocol_skew;
#[path = "suites/session/session_protocol_smoke.rs"]
mod session_protocol_smoke;
#[cfg(unix)]
#[path = "suites/session/session_scrollback.rs"]
mod session_scrollback;
