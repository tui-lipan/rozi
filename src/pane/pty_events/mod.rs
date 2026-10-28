pub(crate) mod input;
pub(crate) mod notifications;
pub(crate) mod pointer_flow;
pub(crate) mod resize;

pub(crate) use input::{
    forward_key_to_pane, handle_pane_input, handle_pane_mouse, handle_pane_scroll,
    paste_pane_bytes, prepare_pane_paste, send_pane_bytes, terminal_key_event_bytes,
};
pub(crate) use notifications::{
    PaneStatusNotification, ToastKey, TrackedToast, confirm_toast, failed_rows_notice,
    finished_rows_notice, maybe_notify_pane_exit, maybe_notify_pane_failure,
    maybe_notify_pane_status, maybe_notify_program_status, maybe_notify_rows_done,
    maybe_notify_rows_failed, maybe_notify_session_agent, notify_error, notify_info, notify_on,
    notify_path_error, notify_path_info, notify_screenshot_saved, notify_update, notify_warning,
    push_toast,
};
pub(crate) use resize::{flush_background_resizes, flush_pending_resizes, handle_pane_resize};

#[cfg(test)]
mod tests;
