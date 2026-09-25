//! The thread that writes a recording, fed through a bounded queue that never makes its producer
//! wait.
//!
//! The producer - the session server's loop - only captures a frame and hands it over. Turning it
//! into spans, diffing, hashing images, and writing all happen here. When the queue is full the
//! newest queued frame is replaced by the newer one: the recording keeps the latest state and counts
//! what it dropped, and the producer never blocks on the disk.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tui_lipan::CapturedFrame;
use tui_lipan::prelude::*;

use super::encode::{FrameEncoder, ImageStore};
use super::format::{
    EndReason, RecordingEnd, RecordingEvent, RecordingHeader, RecordingMeta, RecordingTotals,
};

/// Frames waiting for the writer before a newer one replaces the newest of them.
pub const QUEUE_FRAMES: usize = 8;
/// Marks and meta events waiting for the writer before more are dropped.
const QUEUE_EVENTS: usize = 1024;
/// How long written events may sit in the buffer before they reach the file.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Room kept under `max_bytes` for the `end` event.
const END_RESERVE: u64 = 1024;
/// The smallest `max_bytes` a recording accepts: room for a header, a keyframe, and the end.
pub const MIN_MAX_BYTES: u64 = 64 * 1024;

/// Where and how to write a recording.
#[derive(Clone, Debug)]
pub struct RecorderOptions {
    pub path: PathBuf,
    pub overwrite: bool,
    pub header: RecordingHeader,
    pub max_bytes: u64,
}

/// How a finished recording ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderOutcome {
    pub reason: EndReason,
    pub totals: RecordingTotals,
    /// Why writing failed, for [`EndReason::WriteFailed`].
    pub error: Option<String>,
}

enum Job {
    Frame {
        t: u64,
        frame: Box<CapturedFrame>,
        palette: TerminalColorPalette,
    },
    Event(RecordingEvent),
    End {
        t: u64,
        reason: EndReason,
    },
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    frames: usize,
    /// No job is accepted any more: the end is queued, or the writer stopped on its own.
    closed: bool,
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    keyframes: AtomicU64,
    deltas: AtomicU64,
    images: AtomicU64,
    marks: AtomicU64,
    dropped: AtomicU64,
    bytes: AtomicU64,
}

impl Counters {
    fn totals(&self) -> RecordingTotals {
        RecordingTotals {
            frames: self.frames.load(Ordering::Relaxed),
            keyframes: self.keyframes.load(Ordering::Relaxed),
            deltas: self.deltas.load(Ordering::Relaxed),
            images: self.images.load(Ordering::Relaxed),
            marks: self.marks.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    counters: Counters,
    outcome: Mutex<Option<RecorderOutcome>>,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn finish(&self, outcome: RecorderOutcome) {
        {
            let mut queue = self.queue();
            queue.closed = true;
            queue.jobs.clear();
            queue.frames = 0;
        }
        *self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
    }
}

/// A recording being written.
pub struct Recorder {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// A writer not started yet, so a test can hold the queue full.
    #[cfg(test)]
    paused: Option<Writer>,
}

impl Recorder {
    /// Create the file, write its header, and start the writer.
    ///
    /// The file is created here rather than on the writer so a path that cannot be written is
    /// refused to whoever asked, before anything is recorded.
    pub fn start(options: RecorderOptions) -> io::Result<Self> {
        let (shared, writer) = Self::prepare(options)?;
        let thread = Self::spawn(writer)?;
        Ok(Self {
            shared,
            thread: Some(thread),
            #[cfg(test)]
            paused: None,
        })
    }

    fn prepare(options: RecorderOptions) -> io::Result<(Arc<Shared>, Writer)> {
        let file = crate::platform::persist::create_private_file(&options.path, options.overwrite)?;
        let mut header = serde_json::to_vec(&options.header).map_err(io::Error::other)?;
        header.push(b'\n');
        let mut out = BufWriter::with_capacity(256 * 1024, file);
        out.write_all(&header)?;
        out.flush()?;
        let shared = Arc::new(Shared::default());
        shared
            .counters
            .bytes
            .store(header.len() as u64, Ordering::Relaxed);
        let writer = Writer {
            shared: shared.clone(),
            out,
            max_bytes: options.max_bytes,
            encoder: FrameEncoder::default(),
            images: ImageStore::default(),
            last_t: 0,
        };
        Ok((shared, writer))
    }

    fn spawn(writer: Writer) -> io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("rozi-record".to_string())
            .spawn(move || writer.run())
    }

    /// A recorder whose writer does not run until [`Self::resume`], as though the disk stalled.
    #[cfg(test)]
    pub(crate) fn start_paused(options: RecorderOptions) -> io::Result<Self> {
        let (shared, writer) = Self::prepare(options)?;
        Ok(Self {
            shared,
            thread: None,
            paused: Some(writer),
        })
    }

    #[cfg(test)]
    pub(crate) fn resume(&mut self) {
        if let Some(writer) = self.paused.take() {
            self.thread = Some(Self::spawn(writer).expect("spawn the writer"));
        }
    }

    /// Hand the writer a frame shown at `t`. Never waits on the writer: when it has fallen
    /// [`QUEUE_FRAMES`] behind, the newest queued frame is replaced and counted as dropped.
    pub fn push_frame(&self, t: u64, frame: CapturedFrame, palette: TerminalColorPalette) {
        let mut queue = self.shared.queue();
        if queue.closed {
            return;
        }
        let job = Job::Frame {
            t,
            frame: Box::new(frame),
            palette,
        };
        if queue.frames >= QUEUE_FRAMES
            && let Some(newest) = queue
                .jobs
                .iter_mut()
                .rev()
                .find(|job| matches!(job, Job::Frame { .. }))
        {
            *newest = job;
            self.shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
        } else {
            queue.jobs.push_back(job);
            queue.frames += 1;
        }
        drop(queue);
        self.shared.ready.notify_one();
    }

    /// Add a mark at `t`.
    pub fn mark(&self, t: u64, label: String) {
        self.push_event(RecordingEvent::Mark { t, label });
    }

    /// Add a meta event at `t`.
    pub fn meta(&self, t: u64, meta: RecordingMeta) {
        self.push_event(RecordingEvent::Meta { t, meta });
    }

    fn push_event(&self, event: RecordingEvent) {
        let mut queue = self.shared.queue();
        if queue.closed {
            return;
        }
        if queue.jobs.len() - queue.frames >= QUEUE_EVENTS {
            self.shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        queue.jobs.push_back(Job::Event(event));
        drop(queue);
        self.shared.ready.notify_one();
    }

    /// End the recording at `t`: everything already queued is written, then the `end` event. Does
    /// nothing if it has already ended.
    pub fn finish(&self, t: u64, reason: EndReason) {
        let mut queue = self.shared.queue();
        if queue.closed {
            return;
        }
        queue.jobs.push_back(Job::End { t, reason });
        queue.closed = true;
        drop(queue);
        self.shared.ready.notify_one();
    }

    /// How the recording ended, once its writer has finished.
    pub fn outcome(&self) -> Option<RecorderOutcome> {
        self.shared
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether no more frames will be accepted: it is ending or has ended.
    pub fn closed(&self) -> bool {
        self.shared.queue().closed
    }

    pub fn totals(&self) -> RecordingTotals {
        self.shared.counters.totals()
    }

    /// Wait up to `timeout` for the writer to finish, for a server that is about to exit.
    pub fn join(mut self, timeout: Duration) -> Option<RecorderOutcome> {
        let deadline = Instant::now() + timeout;
        while self.outcome().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.outcome().is_some()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
        self.outcome()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // A recording dropped without an end still gets one; the writer exits on its own.
        self.finish(0, EndReason::Stopped);
    }
}

struct Writer {
    shared: Arc<Shared>,
    out: BufWriter<File>,
    max_bytes: u64,
    encoder: FrameEncoder,
    images: ImageStore,
    last_t: u64,
}

impl Writer {
    fn run(mut self) {
        let mut last_flush = Instant::now();
        let mut unflushed = false;
        loop {
            let job = {
                let mut queue = self.shared.queue();
                loop {
                    if let Some(job) = queue.jobs.pop_front() {
                        if matches!(job, Job::Frame { .. }) {
                            queue.frames -= 1;
                        }
                        break Some(job);
                    }
                    if unflushed && last_flush.elapsed() >= FLUSH_INTERVAL {
                        break None;
                    }
                    let (next, _) = self
                        .shared
                        .ready
                        .wait_timeout(queue, FLUSH_INTERVAL)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    queue = next;
                }
            };
            let result = match job {
                None => Ok(()),
                Some(Job::Frame { t, frame, palette }) => self.frame(t, &frame, palette),
                Some(Job::Event(event)) => {
                    let is_mark = matches!(event, RecordingEvent::Mark { .. });
                    self.last_t = self.last_t.max(event.time().unwrap_or(0));
                    self.write(&[event]).map(|()| {
                        if is_mark {
                            self.shared.counters.marks.fetch_add(1, Ordering::Relaxed);
                        }
                    })
                }
                Some(Job::End { t, reason }) => {
                    let t = t.max(self.last_t);
                    let outcome = self.end(t, reason, None);
                    self.shared.finish(outcome);
                    return;
                }
            };
            unflushed = true;
            match result {
                Ok(()) => {}
                Err(Stop::MaxBytes) => {
                    let outcome = self.end(self.last_t, EndReason::MaxBytes, None);
                    self.shared.finish(outcome);
                    return;
                }
                Err(Stop::Failed(error)) => {
                    let outcome = self.end(self.last_t, EndReason::WriteFailed, Some(error));
                    self.shared.finish(outcome);
                    return;
                }
            }
            if last_flush.elapsed() >= FLUSH_INTERVAL {
                if let Err(error) = self.out.flush() {
                    let outcome =
                        self.end(self.last_t, EndReason::WriteFailed, Some(error.to_string()));
                    self.shared.finish(outcome);
                    return;
                }
                last_flush = Instant::now();
                unflushed = false;
            }
        }
    }

    fn frame(
        &mut self,
        t: u64,
        frame: &CapturedFrame,
        palette: TerminalColorPalette,
    ) -> std::result::Result<(), Stop> {
        let t = t.max(self.last_t);
        let (mut events, span) = self.images.frame(t, frame, palette).map_err(Stop::Failed)?;
        let encoded = self.encoder.encode(t, span);
        if encoded.is_empty() {
            return Ok(());
        }
        events.extend(encoded);
        self.last_t = t;
        self.write(&events)?;
        let counters = &self.shared.counters;
        for event in &events {
            match event {
                RecordingEvent::Keyframe { .. } => {
                    counters.frames.fetch_add(1, Ordering::Relaxed);
                    counters.keyframes.fetch_add(1, Ordering::Relaxed);
                }
                RecordingEvent::Delta(_) => {
                    counters.frames.fetch_add(1, Ordering::Relaxed);
                    counters.deltas.fetch_add(1, Ordering::Relaxed);
                }
                RecordingEvent::Image(_) => {
                    counters.images.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Write `events` whole or not at all, keeping room for the end.
    fn write(&mut self, events: &[RecordingEvent]) -> std::result::Result<(), Stop> {
        let mut lines = Vec::new();
        for event in events {
            serde_json::to_writer(&mut lines, event)
                .map_err(|error| Stop::Failed(error.to_string()))?;
            lines.push(b'\n');
        }
        let written = self.shared.counters.bytes.load(Ordering::Relaxed);
        if written + lines.len() as u64 + END_RESERVE > self.max_bytes {
            return Err(Stop::MaxBytes);
        }
        self.out
            .write_all(&lines)
            .map_err(|error| Stop::Failed(error.to_string()))?;
        self.shared
            .counters
            .bytes
            .fetch_add(lines.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Write the `end` event and make the file durable. A recording whose writes already failed
    /// still tries, since whatever reaches the file helps.
    fn end(&mut self, t: u64, reason: EndReason, error: Option<String>) -> RecorderOutcome {
        let totals = self.shared.counters.totals();
        let end = RecordingEvent::End(RecordingEnd { t, reason, totals });
        let mut line = serde_json::to_vec(&end).unwrap_or_default();
        line.push(b'\n');
        let written = self
            .out
            .write_all(&line)
            .and_then(|()| self.out.flush())
            .and_then(|()| self.out.get_ref().sync_data());
        let error = error.or_else(|| written.err().map(|error| error.to_string()));
        RecorderOutcome {
            reason: if error.is_some() {
                EndReason::WriteFailed
            } else {
                reason
            },
            totals,
            error,
        }
    }
}

enum Stop {
    MaxBytes,
    Failed(String),
}
