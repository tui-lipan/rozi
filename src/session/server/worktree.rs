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
            Request::List { cwd } => cwd.len(),
            Request::Preview { cwd, branch } => cwd.len() + branch.len(),
            Request::Create {
                cwd,
                branch,
                base,
                path,
            } => cwd.len() + branch.len() + base.len() + path.as_ref().map_or(0, String::len),
            Request::Remove { cwd, path, .. } => cwd.len() + path.len(),
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
    done: mpsc::Receiver<WorktreeDone>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WorktreeWorker {
    /// `directory` is `[worktrees] directory` as this server started with it.
    pub fn new(directory: Option<std::path::PathBuf>) -> Self {
        let (jobs, incoming) = mpsc::sync_channel::<WorktreeJob>(QUEUE_CAPACITY);
        let (completed, done) = mpsc::sync_channel::<WorktreeDone>(QUEUE_CAPACITY);
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
