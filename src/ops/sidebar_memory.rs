//! Client-local dock memory. Read once at launch and written only when the client leaves.

use std::io;

use crate::platform::{paths, persist};

const FILE_NAME: &str = "sidebar-docks.json";

fn persist_io<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    {
        crate::test_support::with_persisted_state(f)
    }
    #[cfg(not(test))]
    {
        f()
    }
}

pub(crate) fn load() -> io::Result<Option<[bool; 2]>> {
    persist_io(|| {
        let env = paths::PlatformEnv::from_process();
        let Some(dir) = paths::state_dir_if_available(&env) else {
            return Ok(None);
        };
        let text = match std::fs::read_to_string(dir.join(FILE_NAME)) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let docks: [bool; 2] = serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        // An old or hand-edited empty value must not defeat first-run restoration.
        Ok(docks.iter().any(|shown| *shown).then_some(docks))
    })
}

pub(crate) fn save(docks: Option<[bool; 2]>) -> io::Result<()> {
    persist_io(|| {
        let Some(docks) = docks.filter(|docks| docks.iter().any(|shown| *shown)) else {
            return Ok(());
        };
        let env = paths::PlatformEnv::from_process();
        if paths::state_dir_if_available(&env).is_none() {
            return Ok(());
        }
        let dir = paths::private_state_dir(&env)?;
        persist::replace_file(&dir.join(FILE_NAME), serde_json::to_vec(&docks)?)
    })
}

pub(crate) fn save_for_exit(state: &crate::state::State) {
    // Like last-session memory, failure must not prevent detaching from running sessions.
    let _ = save(state.sidebar.remembered_docks());
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{AppRoot, Msg, input::Action};
    use tui_lipan::TestBackend;

    // Restore the file even after an assertion fails: other unit tests create real clients too.
    pub(crate) struct MemoryFile {
        pub(crate) path: std::path::PathBuf,
        original: Option<Vec<u8>>,
    }

    impl MemoryFile {
        pub(crate) fn new() -> Self {
            let dir = paths::private_state_dir(&paths::PlatformEnv::from_process()).unwrap();
            let path = dir.join(FILE_NAME);
            let original = std::fs::read(&path).ok();
            let _ = std::fs::remove_file(&path);
            Self { path, original }
        }
    }

    impl Drop for MemoryFile {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                std::fs::write(&self.path, original).unwrap();
            } else {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    #[test]
    fn memory_is_nonempty_atomic_and_last_writer_wins() {
        let _persist = crate::test_support::lock_persisted_state();
        let file = MemoryFile::new();
        assert_eq!(load().unwrap(), None);
        save(Some([true, false])).unwrap();
        assert_eq!(load().unwrap(), Some([true, false]));
        save(None).unwrap();
        save(Some([false, false])).unwrap();
        assert_eq!(load().unwrap(), Some([true, false]));
        save(Some([false, true])).unwrap();
        assert_eq!(load().unwrap(), Some([false, true]));
        std::fs::write(&file.path, "[false, false]").unwrap();
        assert_eq!(load().unwrap(), None);
        std::fs::write(&file.path, "broken").unwrap();
        assert_eq!(load().unwrap_err().kind(), io::ErrorKind::InvalidData);
        save(Some([true, true])).unwrap();
        assert_eq!(load().unwrap(), Some([true, true]));
        // A failed replacement leaves the destination intact and can be retried later.
        std::fs::remove_file(&file.path).unwrap();
        std::fs::create_dir(&file.path).unwrap();
        assert!(save(Some([true, false])).is_err());
        assert!(file.path.is_dir());
        std::fs::remove_dir(&file.path).unwrap();
        save(Some([true, false])).unwrap();
        assert_eq!(load().unwrap(), Some([true, false]));
    }

    #[test]
    fn quit_detach_and_hangup_save_visible_or_restored_docks_without_toggle_writes() {
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let _persist = crate::test_support::lock_persisted_state();
                let file = MemoryFile::new();
                for exit in [
                    Msg::RunAction(Action::Quit),
                    Msg::RunAction(Action::Detach),
                    Msg::Hangup,
                ] {
                    for hidden in [false, true] {
                        for remembered in [[true, false], [false, true], [true, true]] {
                            let mut backend = TestBackend::new(AppRoot::default());
                            backend.state_mut().config.session.autosave = false;
                            backend.state_mut().sidebar.shown = remembered;
                            backend.state_mut().sidebar.restore = [false, false];
                            std::fs::write(&file.path, "unchanged until exit").unwrap();
                            // Every visibility command can run without writing the memory file.
                            for action in [Action::ToggleLeftSidebar, Action::ToggleRightSidebar] {
                                backend.dispatch(Msg::RunAction(action)).unwrap();
                                backend.dispatch(Msg::RunAction(action)).unwrap();
                            }
                            if hidden {
                                backend
                                    .dispatch(Msg::RunAction(Action::ToggleSidebar))
                                    .unwrap();
                            }
                            assert_eq!(
                                std::fs::read_to_string(&file.path).unwrap(),
                                "unchanged until exit"
                            );
                            backend.dispatch(exit.clone()).unwrap();
                            assert_eq!(load().unwrap(), Some(remembered));
                        }
                    }
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
