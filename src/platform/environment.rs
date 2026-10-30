//! Desktop environment for newly created panes: a local client's own values, or else the values of
//! the desktop session running on the host right now.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

/// The variables that describe one running desktop session.
///
/// A session server can outlive the desktop it was started from, so panes never inherit its copy of
/// these. `XDG_RUNTIME_DIR` and the bus address belong to the user rather than the desktop, so they
/// stay usable on a host with no desktop at all.
pub const DESKTOP_SESSION_VARIABLES: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "DESKTOP_SESSION",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
];

/// How long a pane spawn waits for the host's service manager before going without a desktop.
#[cfg(target_os = "linux")]
const USER_MANAGER_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Give a pane the host's current desktop, unless its spawn request already names a display.
///
/// A request that names one came from a client running on a desktop, or from a caller that chose,
/// and mixing in the host's would point its programs at two different sessions.
pub fn fill_host_desktop_environment(env: &mut Vec<(String, String)>) {
    if !names_a_display(env) {
        env.extend(host_desktop_environment());
    }
}

fn names_a_display(env: &[(String, String)]) -> bool {
    env.iter()
        .any(|(key, _)| key == "DISPLAY" || key == "WAYLAND_DISPLAY")
}

fn host_desktop_environment() -> Vec<(String, String)> {
    read_user_manager_environment()
        .map(|output| {
            live_desktop_environment(
                parse_manager_environment(&output),
                std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
                |path| path.exists(),
            )
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn read_user_manager_environment() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).ok().map(|_| output)
    });
    let deadline = std::time::Instant::now() + USER_MANAGER_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader.join().ok().flatten();
                return output.filter(|_| status.success());
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Only systemd keeps a queryable record of the desktop session. Other hosts report no desktop
/// rather than guess, and a local client on a desktop still forwards its own.
#[cfg(not(target_os = "linux"))]
fn read_user_manager_environment() -> Option<String> {
    None
}

/// The desktop variables from `systemctl --user show-environment` output. A value systemd had to
/// shell-quote is not one a display name or desktop identifier takes, so it is left out.
fn parse_manager_environment(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, value)| DESKTOP_VARIABLES.contains(name) && !value.starts_with("$'"))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// Drop displays whose socket is gone, since a manager's record can outlive the compositor that
/// wrote it. With no display left there is no desktop, and nothing else in the record applies.
fn live_desktop_environment(
    mut env: Vec<(String, String)>,
    process_runtime_dir: Option<PathBuf>,
    socket_exists: impl Fn(&Path) -> bool,
) -> Vec<(String, String)> {
    let runtime_dir = env
        .iter()
        .find(|(key, _)| key == "XDG_RUNTIME_DIR")
        .map(|(_, value)| PathBuf::from(value))
        .or(process_runtime_dir);
    env.retain(|(key, value)| match key.as_str() {
        "DISPLAY" => x11_socket_path(value).is_some_and(|path| socket_exists(&path)),
        "WAYLAND_DISPLAY" => runtime_dir
            .as_deref()
            .is_some_and(|dir| socket_exists(&dir.join(value))),
        _ => true,
    });
    if names_a_display(&env) {
        env
    } else {
        Vec::new()
    }
}

/// The socket of a display on this machine, `:0` or `:0.1`. Anything else names a display elsewhere,
/// which is no part of this host's desktop.
fn x11_socket_path(display: &str) -> Option<PathBuf> {
    let number = display.strip_prefix(':')?.split('.').next()?;
    let valid = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
    valid.then(|| PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
}

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

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn manager_output_yields_only_desktop_variables_with_plain_values() {
        let output = "HOME=/home/you\nDISPLAY=:0\nWAYLAND_DISPLAY=wayland-1\n\
                      DESKTOP_SESSION=$'odd value'\nXDG_RUNTIME_DIR=/run/user/1000\nnot a line\n";
        assert_eq!(
            parse_manager_environment(output),
            vars(&[
                ("DISPLAY", ":0"),
                ("WAYLAND_DISPLAY", "wayland-1"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ])
        );
    }

    #[test]
    fn a_display_counts_only_while_its_socket_exists() {
        let record = vars(&[
            ("DISPLAY", ":0"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]);
        let with_sockets = |live: &'static [&'static str]| {
            live_desktop_environment(record.clone(), None, |path| {
                live.contains(&path.to_str().unwrap())
            })
        };

        assert_eq!(
            with_sockets(&["/tmp/.X11-unix/X0", "/run/user/1000/wayland-1"]),
            record
        );
        // One display surviving keeps the desktop; the dead one is not offered to programs.
        assert_eq!(
            with_sockets(&["/run/user/1000/wayland-1"]),
            vars(&[
                ("WAYLAND_DISPLAY", "wayland-1"),
                ("XDG_CURRENT_DESKTOP", "Hyprland"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ])
        );
        assert!(with_sockets(&[]).is_empty(), "no display means no desktop");
    }

    #[test]
    fn wayland_sockets_resolve_against_the_process_runtime_dir_when_the_manager_omits_one() {
        let env = live_desktop_environment(
            vars(&[("WAYLAND_DISPLAY", "wayland-1")]),
            Some(PathBuf::from("/run/user/1000")),
            |path| path == Path::new("/run/user/1000/wayland-1"),
        );
        assert_eq!(env, vars(&[("WAYLAND_DISPLAY", "wayland-1")]));
    }

    #[test]
    fn only_a_local_x11_display_has_a_socket_here() {
        assert_eq!(
            x11_socket_path(":0"),
            Some(PathBuf::from("/tmp/.X11-unix/X0"))
        );
        assert_eq!(
            x11_socket_path(":10.1"),
            Some(PathBuf::from("/tmp/.X11-unix/X10"))
        );
        for elsewhere in ["localhost:10.0", "", ":", ":x", "host:0"] {
            assert_eq!(x11_socket_path(elsewhere), None, "{elsewhere:?}");
        }
    }

    #[test]
    fn a_spawn_that_names_a_display_keeps_it_untouched() {
        let mut env = vars(&[("DISPLAY", "localhost:10.0"), ("ROZI", "1")]);
        let before = env.clone();
        fill_host_desktop_environment(&mut env);
        assert_eq!(env, before);
    }
}
