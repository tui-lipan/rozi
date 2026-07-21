pub(crate) mod bootstrap;
pub mod client;
pub mod discovery;
pub mod protocol;
pub(crate) mod remote;
pub mod server;

fn last_session_path() -> Option<std::path::PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    (env.home.is_some() || env.xdg_state_home.is_some())
        .then(|| crate::platform::paths::state_dir(&env).join("last-session"))
}

pub(crate) fn record_last_named_session(name: &str) {
    if !discovery::valid_session_name(name) {
        return;
    }
    let Some(path) = last_session_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    let _ = std::fs::write(path, format!("{name}\n"));
}

pub(crate) fn read_last_named_session() -> Option<String> {
    let name = std::fs::read_to_string(last_session_path()?).ok()?;
    let name = name.trim();
    discovery::valid_session_name(name).then(|| name.to_string())
}
