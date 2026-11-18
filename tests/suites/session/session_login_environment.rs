//! Reading the user's login environment for a server or extension runtime started over SSH.

use std::ffi::{OsStr, OsString};
use std::time::{Duration, Instant};

use rozi::platform::login_environment::{RESOLVED_ENV, RESOLVING_ENV, capture};

use crate::common::private_temp_dir;

/// The login files' variables come back, around whatever else those files print, without waiting
/// for a daemon they start that keeps the output pipe open.
#[test]
fn the_login_shells_environment_is_read_past_its_output_and_daemons() {
    let home = private_temp_dir();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join(".profile"),
        "echo 'Welcome back'\n\
         export PATH=\"$HOME/tools:$PATH\"\n\
         export FROM_LOGIN='two\nlines'\n\
         sleep 30 &\n",
    )
    .unwrap();
    let base: Vec<(OsString, OsString)> = [
        ("HOME", home.as_os_str()),
        ("PATH", OsStr::new("/usr/bin:/bin")),
        ("PWD", home.as_os_str()),
        ("FROM_SSH", OsStr::new("kept")),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.to_os_string()))
    .collect();

    let started = Instant::now();
    let env = capture(
        OsStr::new("/bin/sh"),
        std::path::Path::new(env!("CARGO_BIN_EXE_rozi")),
        &base,
        Duration::from_secs(20),
    )
    .expect("login environment");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "waited for the daemon"
    );

    let get = |key: &str| {
        env.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.to_string_lossy().into_owned())
    };
    // The host's `/etc/profile` may add its own directories after the user's.
    let path = get("PATH").expect("PATH");
    assert!(
        path.starts_with(&format!("{}/tools:", home.display())),
        "PATH from the login files: {path}"
    );
    assert_eq!(get("FROM_LOGIN").as_deref(), Some("two\nlines"));
    assert_eq!(get("FROM_SSH").as_deref(), Some("kept"));
    assert_eq!(get("PWD"), Some(home.display().to_string()));
    assert_eq!(get(RESOLVED_ENV).as_deref(), Some("1"));
    assert_eq!(get(RESOLVING_ENV), None);
    assert_eq!(get("ROZI_ENVIRONMENT_PROBE"), None);
    let _ = std::fs::remove_dir_all(&home);
}
