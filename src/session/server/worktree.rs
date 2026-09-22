//! Serialized Git worktree work, independent of the browse queue and session pump.

use super::*;
use std::path::{Path, PathBuf};

const QUEUE_CAPACITY: usize = 4;
const MAX_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub(super) struct WorktreeJob {
    pub client_id: ClientId,
    pub request_id: u64,
    pub request: protocol::WorktreeRequest,
}

impl WorktreeJob {
    pub fn size(&self) -> usize {
        use protocol::WorktreeRequest as Request;
        match &self.request {
            Request::List { cwd } => cwd.len(),
            Request::Preview { cwd, branch } => cwd.len() + branch.len(),
            Request::Create {
                cwd,
                branch,
                base,
                path,
            } => cwd.len() + branch.len() + base.len() + path.as_ref().map_or(0, String::len),
            Request::Remove { cwd, path, .. } => cwd.len() + path.len(),
        }
    }

    fn run(self) -> WorktreeDone {
        use protocol::{WorktreeRequest as Request, WorktreeResult as ResultValue};
        let result = match self.request {
            Request::List { cwd } => crate::git::worktrees::list(Path::new(&cwd))
                .map(|worktrees| ResultValue::Listed { worktrees }),
            Request::Preview { cwd, branch } =>
                crate::git::worktrees::default_path(Path::new(&cwd), &branch).map(|path| {
                    ResultValue::Previewed {
                        path: path.to_string_lossy().into_owned(),
                    }
                }),
            Request::Create {
                cwd,
                branch,
                base,
                path,
            } => {
                let cwd = Path::new(&cwd);
                let path = path
                    .map(PathBuf::from)
                    .map(Ok)
                    .unwrap_or_else(|| crate::git::worktrees::default_path(cwd, &branch));
                path.and_then(|path| {
                    if !path.is_absolute() {
                        return Err("worktree path must be absolute on the session host".to_string());
                    }
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
                    }
                    crate::git::worktrees::create(cwd, &branch, &base, &path)
                })
                .map(|worktree| ResultValue::Created { worktree })
            }
            Request::Remove { cwd, path, force } => {
                let requested = Path::new(&path);
                let result = if !requested.is_absolute() {
                    Err("worktree path must be absolute on the session host".to_string())
                } else {
                    let canonical = requested.canonicalize().unwrap_or_else(|_| requested.to_path_buf());
                    crate::session::discovery::sessions_using_worktree(&canonical.to_string_lossy())
                        .and_then(|users| {
                            if users.is_empty() {
                                crate::git::worktrees::remove(Path::new(&cwd), &canonical, force)
                            } else {
                                Err(format!(
                                    "worktree is used by session {}; stop or forget it before removal",
                                    users.join(", ")
                                ))
                            }
                        })
                };
                result.map(|()| ResultValue::Removed { path })
            }
        }
        .unwrap_or_else(|message| ResultValue::Failed { message });
        WorktreeDone {
            client_id: self.client_id,
            request_id: self.request_id,
            result,
        }
    }
}

pub(super) struct WorktreeDone {
    pub client_id: ClientId,
    pub request_id: u64,
    pub result: protocol::WorktreeResult,
}

pub(super) struct WorktreeWorker {
    jobs: Option<mpsc::SyncSender<WorktreeJob>>,
    done: mpsc::Receiver<WorktreeDone>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WorktreeWorker {
    pub fn new() -> Self {
        let (jobs, incoming) = mpsc::sync_channel::<WorktreeJob>(QUEUE_CAPACITY);
        let (completed, done) = mpsc::sync_channel::<WorktreeDone>(QUEUE_CAPACITY);
        let handle = std::thread::Builder::new()
            .name("rozi-worktrees".into())
            .spawn(move || {
                for job in incoming {
                    if completed.send(job.run()).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Self {
            jobs: handle.is_some().then_some(jobs),
            done,
            handle,
        }
    }

    pub fn try_submit(
        &self,
        job: WorktreeJob,
    ) -> std::result::Result<(), mpsc::TrySendError<WorktreeJob>> {
        let Some(jobs) = &self.jobs else {
            return Err(mpsc::TrySendError::Disconnected(job));
        };
        jobs.try_send(job)
    }

    pub fn drain(&self) -> Vec<WorktreeDone> {
        self.done.try_iter().collect()
    }

    pub fn finish(mut self) {
        self.jobs = None;
        if self
            .handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
    }
}

pub(super) fn request_too_large(job: &WorktreeJob) -> bool {
    job.size() > MAX_REQUEST_BYTES
}
