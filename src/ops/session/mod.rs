pub(crate) mod attach;
pub(crate) mod control_lease;
pub(crate) mod discovery;
pub(crate) mod lifecycle;
pub(crate) mod remotes;

pub(crate) use attach::{
    apply_pending_background_closes, attach_session_by_name, clear_pending_session_action,
    disconnect_host, ensure_session_for_pty, enter_launcher, held_ephemeral_session,
    install_fresh_attachment, kill_current_session, land_on_surviving_session,
    may_shutdown_ephemeral, needs_session_for_pty, park_current_and_install,
    reconnect_current_session, release_background_for_exit, release_current_session,
    restart_current_session, run_pending_session_action, start_launcher_shell,
    swap_to_fresh_ephemeral, switch_to_parked,
};
pub(crate) use control_lease::{
    can_evict, decline_control, evict_client, flush_layout_commit, grant_control,
    grant_control_to_requester, nudge_if_follower, open_collaborators, prompt_follow_if_occupied,
    request_control, resolve_follow_prompt, schedule_layout_commit, toggle_control_takeover,
    toggle_input_lock,
};
pub(crate) use discovery::{
    HostProbeStatus, apply_discovered_sessions, attached_session_rows, discover_picker_sessions,
    discover_sidebar_sessions, local_picker_rows, seed_host_registry,
};
pub(crate) use lifecycle::{
    activate_discovered_session, activate_selected_session, apply_rename_session,
    clear_pending_session_arms, close_rename_session, close_session_picker,
    disconnect_selected_attachment, disconnect_selected_host, kill_discovered_session,
    kill_selected_session, open_connect_remote_host, open_create_session,
    open_create_session_on_host, open_ephemeral_session, open_leave_prompt, open_rename_session,
    open_session_picker, open_startup_session_picker, restart_selected_session,
    session_row_can_disconnect, session_row_can_disconnect_host, session_row_can_restart,
    session_row_is_current, session_row_is_restorable,
};
pub(crate) use remotes::{close_remote_picker, open_remote_hosts};

#[cfg(test)]
mod tests;
