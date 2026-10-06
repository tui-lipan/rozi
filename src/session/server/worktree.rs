//! Serialized Git worktree work, independent of the browse queue and session pump.

use super::*;

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
            Request::List { cwd } | Request::Status { cwd, .. } => cwd.len(),
            Request::Preview { cwd, branch } => cwd.len() + branch.len(),
            Request::Create {
                cwd,
                branch,
                base,
                path,
            } => cwd.len() + branch.len() + base.len() + path.as_ref().map_or(0, String::len),
            Request::Remove { cwd, path, .. } | Request::Unlock { cwd, path } => {
                cwd.len() + path.len()
            }
            Request::Exclude { cwd, directory } => cwd.len() + directory.len(),
        }
    }

    fn run(self, directory: Option<&std::path::Path>) -> WorktreeDone {
        WorktreeDone {
            client_id: self.client_id,
            request_id: self.request_id,
            result: crate::session::worktrees::execute(self.request, directory),
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
    status_jobs: Option<mpsc::SyncSender<WorktreeJob>>,
    done: mpsc::Receiver<WorktreeDone>,
    handle: Option<std::thread::JoinHandle<()>>,
    status_handle: Option<std::thread::JoinHandle<()>>,
}

impl WorktreeWorker {
    /// `directory` is `[worktrees] directory` as this server started with it.
    pub fn new(directory: Option<std::path::PathBuf>) -> Self {
        let (jobs, incoming) = mpsc::sync_channel::<WorktreeJob>(QUEUE_CAPACITY);
        let (completed, done) = mpsc::sync_channel::<WorktreeDone>(QUEUE_CAPACITY);
        let status_completed = completed.clone();
        let (status_jobs, status_incoming) = mpsc::sync_channel::<WorktreeJob>(QUEUE_CAPACITY);
        let status_handle = std::thread::Builder::new()
            .name("rozi-worktree-status".into())
            .spawn(move || {
                let mut cache = crate::git::pull_requests::StatusCache::default();
                for job in status_incoming {
                    let protocol::WorktreeRequest::Status { cwd, refresh } = job.request else {
                        continue;
                    };
                    let reply = WorktreeDone {
                        client_id: job.client_id,
                        request_id: job.request_id,
                        result: protocol::WorktreeResult::Statuses {
                            statuses: cache.get(std::path::Path::new(&cwd), refresh),
                        },
                    };
                    if status_completed.send(reply).is_err() {
                        break;
                    }
                }
            })
            .ok();
        let handle = std::thread::Builder::new()
            .name("rozi-worktrees".into())
            .spawn(move || {
                for job in incoming {
                    if completed.send(job.run(directory.as_deref())).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Self {
            jobs: handle.is_some().then_some(jobs),
            status_jobs: status_handle.is_some().then_some(status_jobs),
            done,
            handle,
            status_handle,
        }
    }

    pub fn try_submit(
        &self,
        job: WorktreeJob,
    ) -> std::result::Result<(), mpsc::TrySendError<WorktreeJob>> {
        let queue = if matches!(job.request, protocol::WorktreeRequest::Status { .. }) {
            &self.status_jobs
        } else {
            &self.jobs
        };
        let Some(jobs) = queue else {
            return Err(mpsc::TrySendError::Disconnected(job));
        };
        jobs.try_send(job)
    }

    pub fn drain(&self) -> Vec<WorktreeDone> {
        self.done.try_iter().collect()
    }

    pub fn finish(mut self) {
        self.jobs = None;
        self.status_jobs = None;
        if self
            .status_handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
            && let Some(handle) = self.status_handle.take()
        {
            let _ = handle.join();
        }
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
