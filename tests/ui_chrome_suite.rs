#[path = "suites/ui_chrome/pick_smoke.rs"]
mod pick_smoke;
#[path = "suites/ui_chrome/prefix_mouse_gestures.rs"]
mod prefix_mouse_gestures;
#[path = "suites/ui_chrome/session_picker_ephemeral.rs"]
mod session_picker_ephemeral;
#[path = "suites/ui_chrome/settings_rows_smoke.rs"]
mod settings_rows_smoke;
#[path = "suites/ui_chrome/which_key_smoke.rs"]
mod which_key_smoke;
#[path = "suites/ui_chrome/workbar_alert_smoke.rs"]
mod workbar_alert_smoke;
#[cfg(feature = "ui-snapshot")]
#[path = "suites/ui_chrome/workbar_alert_visual.rs"]
mod workbar_alert_visual;
#[path = "suites/ui_chrome/workbar_caps_smoke.rs"]
mod workbar_caps_smoke;
#[path = "suites/ui_chrome/workbar_tab_interaction_smoke.rs"]
mod workbar_tab_interaction_smoke;
