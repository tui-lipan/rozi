//! Client environment values that may be exported to newly created local panes.

use std::collections::HashSet;

/// Desktop/session variables that are safe and useful to inherit from the client creating a pane.
const DESKTOP_VARIABLES: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "DESKTOP_SESSION",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
];

/// Read the built-in desktop variables and explicitly configured additions from this client.
///
/// Non-Unicode values cannot cross the JSON session protocol and are omitted. Duplicate names are
/// read once, preserving the built-in-first order used by pane environment precedence.
pub fn forwarded_client_environment(configured: &[String]) -> Vec<(String, String)> {
    forwarded_client_environment_with(configured, |name| std::env::var(name).ok())
}

fn forwarded_client_environment_with(
    configured: &[String],
    mut read: impl FnMut(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    DESKTOP_VARIABLES
        .iter()
        .copied()
        .chain(configured.iter().map(String::as_str))
        .filter(|name| seen.insert((*name).to_string()))
        .filter_map(|name| read(name).map(|value| (name.to_string(), value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_variables_extend_desktop_allowlist_without_duplicates() {
        let configured = vec!["CURSOR_API_KEY".to_string(), "WAYLAND_DISPLAY".to_string()];
        let env = forwarded_client_environment_with(&configured, |name| match name {
            "WAYLAND_DISPLAY" => Some("wayland-1".to_string()),
            "CURSOR_API_KEY" => Some("secret".to_string()),
            _ => None,
        });

        assert_eq!(
            env,
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
                ("CURSOR_API_KEY".to_string(), "secret".to_string()),
            ]
        );
    }
}
