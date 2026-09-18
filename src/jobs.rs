//! Bounded fire-and-forget jobs for hooks and detached `exec` commands.

use std::sync::atomic::{AtomicUsize, Ordering};

/// How many hook and detached `exec` jobs may run at once.
///
/// Matching hooks still overlap, and a slow command still occupies its slot until it exits. Past
/// this count, further launches are skipped rather than accumulating threads and processes.
pub const MAX_ONE_SHOT_JOBS: usize = 32;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn lock_for_tests() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `job` on a dedicated thread if a slot is free.
///
/// Returns `false` when [`MAX_ONE_SHOT_JOBS`] are already running, or when the thread itself could
/// not be created. The caller decides whether that skip is silent (hooks) or reported (`exec`).
pub fn try_spawn(job: impl FnOnce() + Send + 'static) -> bool {
    if !acquire_slot() {
        return false;
    }
    match std::thread::Builder::new()
        .name("rozi-oneshot".into())
        .spawn(move || {
            struct Slot;
            impl Drop for Slot {
                fn drop(&mut self) {
                    IN_FLIGHT.fetch_sub(1, Ordering::Release);
                }
            }
            let _slot = Slot;
            job();
        }) {
        Ok(_) => true,
        Err(_) => {
            IN_FLIGHT.fetch_sub(1, Ordering::Release);
            false
        }
    }
}

fn acquire_slot() -> bool {
    loop {
        let current = IN_FLIGHT.load(Ordering::Relaxed);
        if current >= MAX_ONE_SHOT_JOBS {
            return false;
        }
        if IN_FLIGHT
            .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn try_spawn_runs_the_job() {
        let _lock = lock_for_tests();
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(try_spawn(move || {
            let _ = tx.send(());
        }));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn try_spawn_rejects_work_past_the_cap() {
        let _lock = lock_for_tests();
        let start = Arc::new(Barrier::new(MAX_ONE_SHOT_JOBS + 1));
        let running = Arc::new(Mutex::new(0usize));
        for _ in 0..MAX_ONE_SHOT_JOBS {
            let start = Arc::clone(&start);
            let running = Arc::clone(&running);
            assert!(try_spawn(move || {
                *running.lock().unwrap() += 1;
                start.wait();
            }));
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while *running.lock().unwrap() < MAX_ONE_SHOT_JOBS && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*running.lock().unwrap(), MAX_ONE_SHOT_JOBS);
        assert!(!try_spawn(|| {}));
        start.wait();

        let deadline = Instant::now() + Duration::from_secs(2);
        while IN_FLIGHT.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(try_spawn(|| {}));
    }
}
