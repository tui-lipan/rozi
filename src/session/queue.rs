use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueueStats {
    pub(crate) bytes: usize,
    pub(crate) high_water_bytes: usize,
    pub(crate) capacity: usize,
    pub(crate) len: usize,
    pub(crate) closed: bool,
    pub(crate) coalesced_frames: u64,
    pub(crate) coalesced_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PushError<T> {
    Full(T),
    Closed(T),
    TooLarge(T),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SegmentCoalesce {
    Merged,
    Continue,
    Barrier,
}

/// Parser-proven independence key. Ordered barriers advance the epoch before insertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReplacementKey {
    pub(crate) pane: u32,
    pub(crate) local: bool,
    pub(crate) generation: u64,
    pub(crate) epoch: u64,
    pub(crate) image: u32,
    pub(crate) incarnation: u64,
}

struct Entry<T> {
    value: T,
    bytes: usize,
    replacement: Option<ReplacementKey>,
}

struct State<T> {
    entries: VecDeque<Entry<T>>,
    base: u64,
    replacements: HashMap<ReplacementKey, u64>,
    coalesced_frames: u64,
    coalesced_bytes: u64,
    bytes: usize,
    high_water_bytes: usize,
    closed: bool,
    wakes: u64,
}

impl<T> State<T> {
    fn remove_front_index(&mut self, entry: &Entry<T>) {
        if let Some(key) = entry.replacement
            && self.replacements.get(&key) == Some(&self.base)
        {
            self.replacements.remove(&key);
        }
        self.base += 1;
    }
}

/// FIFO queue bounded by retained payload bytes rather than message count.
///
/// Zero-byte control entries remain ordered behind payload already in the queue. Producers may
/// either receive an explicit `Full` result or block until a consumer makes room; closing wakes all
/// blocked producers and consumers.
pub(crate) struct ByteQueue<T> {
    capacity: usize,
    state: Mutex<State<T>>,
    changed: Condvar,
}

impl<T> ByteQueue<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(State {
                entries: VecDeque::new(),
                base: 0,
                replacements: HashMap::new(),
                coalesced_frames: 0,
                coalesced_bytes: 0,
                bytes: 0,
                high_water_bytes: 0,
                closed: false,
                wakes: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// A replay/reset is a dependency barrier even when the server's producer epoch is unchanged.
    /// Invalidate the replacement index; queued original entries retain their position and bytes.
    pub(crate) fn invalidate_replacements(&self, pane: u32, local: bool) {
        let mut state = self.state.lock().expect("byte queue poisoned");
        state
            .replacements
            .retain(|key, _| key.pane != pane || key.local != local);
    }

    pub(crate) fn clear_replacements(&self) {
        self.state
            .lock()
            .expect("byte queue poisoned")
            .replacements
            .clear();
    }

    pub(crate) fn try_push(&self, value: T, bytes: usize) -> Result<(), PushError<T>> {
        self.try_push_with(value, bytes, |_, _| false)
    }

    pub(crate) fn try_push_with(
        &self,
        value: T,
        bytes: usize,
        coalesce: impl FnOnce(&mut T, &T) -> bool,
    ) -> Result<(), PushError<T>> {
        if bytes > self.capacity {
            return Err(PushError::TooLarge(value));
        }
        let mut state = self.state.lock().expect("byte queue poisoned");
        if state.closed {
            return Err(PushError::Closed(value));
        }
        if state.bytes.saturating_add(bytes) > self.capacity {
            return Err(PushError::Full(value));
        }
        if let Some(back) = state.entries.back_mut()
            && coalesce(&mut back.value, &value)
        {
            back.bytes += bytes;
            state.bytes += bytes;
            state.high_water_bytes = state.high_water_bytes.max(state.bytes);
            self.changed.notify_all();
            return Ok(());
        }
        state.entries.push_back(Entry {
            value,
            bytes,
            replacement: None,
        });
        state.bytes += bytes;
        state.high_water_bytes = state.high_water_bytes.max(state.bytes);
        self.changed.notify_all();
        Ok(())
    }

    /// Supersede pending independent state in O(1), accounting for reclaimed bytes before
    /// applying capacity. This never merges bytes or replaces an ordered entry.
    pub(crate) fn try_push_latest(
        &self,
        value: T,
        bytes: usize,
        key: Option<ReplacementKey>,
    ) -> Result<(), PushError<T>> {
        if bytes > self.capacity {
            return Err(PushError::TooLarge(value));
        }
        let mut state = self.state.lock().expect("byte queue poisoned");
        if state.closed {
            return Err(PushError::Closed(value));
        }
        let index = key
            .and_then(|key| state.replacements.get(&key).copied())
            .and_then(|index| index.checked_sub(state.base))
            .and_then(|index| usize::try_from(index).ok());
        let previous_bytes = index
            .and_then(|index| state.entries.get(index))
            .map_or(0, |entry| entry.bytes);
        let retained = state
            .bytes
            .saturating_sub(previous_bytes)
            .saturating_add(bytes);
        if retained > self.capacity {
            return Err(PushError::Full(value));
        }
        if let Some(entry) = index.and_then(|index| state.entries.get_mut(index)) {
            *entry = Entry {
                value,
                bytes,
                replacement: key,
            };
            state.coalesced_frames += 1;
            state.coalesced_bytes += previous_bytes as u64;
        } else {
            if let Some(key) = key {
                let position = state.base + state.entries.len() as u64;
                state.replacements.insert(key, position);
            }
            state.entries.push_back(Entry {
                value,
                bytes,
                replacement: key,
            });
        }
        state.bytes = retained;
        state.high_water_bytes = state.high_water_bytes.max(retained);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn push_blocking_latest(
        &self,
        mut value: T,
        bytes: usize,
        key: ReplacementKey,
    ) -> Result<(), PushError<T>> {
        loop {
            match self.try_push_latest(value, bytes, Some(key)) {
                Err(PushError::Full(returned)) => value = returned,
                result => return result,
            }
            let state = self.state.lock().expect("byte queue poisoned");
            let _state = self
                .changed
                .wait_while(state, |state| {
                    let previous = state
                        .replacements
                        .get(&key)
                        .and_then(|position| position.checked_sub(state.base))
                        .and_then(|position| state.entries.get(position as usize))
                        .map_or(0, |entry| entry.bytes);
                    !state.closed
                        && state.bytes.saturating_sub(previous).saturating_add(bytes)
                            > self.capacity
                })
                .expect("byte queue poisoned");
        }
    }

    pub(crate) fn push_blocking_with(
        &self,
        value: T,
        bytes: usize,
        coalesce: impl FnOnce(&mut T, &T) -> bool,
    ) -> Result<(), PushError<T>> {
        let mut coalesce = Some(coalesce);
        self.push_blocking_within_segment(value, bytes, move |queued, next, _adjacent| {
            if coalesce
                .take()
                .expect("adjacent coalescer called more than once")(queued, next)
            {
                SegmentCoalesce::Merged
            } else {
                SegmentCoalesce::Barrier
            }
        })
    }

    /// Scan the trailing queue segment backward until `coalesce` merges the value or finds a
    /// barrier. Byte accounting includes every original value even when several share one entry.
    ///
    /// Cross-pane output order is intentionally not preserved. Later bytes for pane A may join an
    /// earlier A entry that already sits ahead of pane B. Same-pane byte order and control
    /// barriers stay intact.
    pub(crate) fn push_blocking_within_segment(
        &self,
        value: T,
        bytes: usize,
        mut coalesce: impl FnMut(&mut T, &T, bool) -> SegmentCoalesce,
    ) -> Result<(), PushError<T>> {
        if bytes > self.capacity {
            return Err(PushError::TooLarge(value));
        }
        let mut state = self.state.lock().expect("byte queue poisoned");
        while !state.closed && state.bytes.saturating_add(bytes) > self.capacity {
            state = self.changed.wait(state).expect("byte queue poisoned");
        }
        if state.closed {
            return Err(PushError::Closed(value));
        }

        let mut merged = false;
        for (offset, entry) in state.entries.iter_mut().rev().enumerate() {
            match coalesce(&mut entry.value, &value, offset == 0) {
                SegmentCoalesce::Merged => {
                    entry.bytes += bytes;
                    merged = true;
                    break;
                }
                SegmentCoalesce::Continue => {}
                SegmentCoalesce::Barrier => break,
            }
        }
        if merged {
            state.bytes += bytes;
            state.high_water_bytes = state.high_water_bytes.max(state.bytes);
            self.changed.notify_all();
            return Ok(());
        }

        state.entries.push_back(Entry {
            value,
            bytes,
            replacement: None,
        });
        state.bytes += bytes;
        state.high_water_bytes = state.high_water_bytes.max(state.bytes);
        self.changed.notify_all();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn try_pop(&self) -> Option<T> {
        self.try_pop_with_bytes_and_more()
            .map(|(value, _, _)| value)
    }

    pub(crate) fn try_pop_with_bytes_and_more(&self) -> Option<(T, usize, bool)> {
        let mut state = self.state.lock().expect("byte queue poisoned");
        let entry = state.entries.pop_front()?;
        state.remove_front_index(&entry);
        state.bytes -= entry.bytes;
        let has_more = !state.entries.is_empty();
        self.changed.notify_all();
        Some((entry.value, entry.bytes, has_more))
    }

    /// Take the first ready entry while leaving blocked streams charged to the ingress budget.
    /// A split tail stays in its original position ahead of that stream's later events.
    pub(crate) fn try_pop_ready(
        &self,
        mut ready: impl FnMut(&T) -> bool,
        split: impl FnOnce(T) -> (T, Option<(T, usize)>),
    ) -> Option<T> {
        let mut state = self.state.lock().expect("byte queue poisoned");
        let index = state.entries.iter().position(|entry| ready(&entry.value))?;
        let entry = state.entries.remove(index)?;
        state.bytes -= entry.bytes;
        if index == 0 {
            state.base += 1;
        }
        let (value, tail) = split(entry.value);
        if let Some((value, bytes)) = tail {
            state.entries.insert(
                index,
                Entry {
                    value,
                    bytes,
                    replacement: None,
                },
            );
            state.bytes += bytes;
        }
        // Removing an entry from the middle shifts absolute coalescing indexes. Server ingress
        // has no replacement keys, but keep this operation correct for every ByteQueue caller.
        state.replacements = state
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                entry
                    .replacement
                    .map(|key| (key, state.base + index as u64))
            })
            .collect();
        self.changed.notify_all();
        Some(value)
    }

    /// Interrupt a consumer waiting for work that finishes outside this queue.
    pub(crate) fn wake(&self) {
        self.state.lock().expect("byte queue poisoned").wakes += 1;
        self.changed.notify_all();
    }

    /// Number of [`Self::wake`] calls so far. Read it before checking the outside work.
    pub(crate) fn wakes(&self) -> u64 {
        self.state.lock().expect("byte queue poisoned").wakes
    }

    /// Wait for activity even when every queued stream is blocked on deferred work: until the
    /// queue changes, a wake arrives, or `timeout` elapses. Returns immediately if a wake arrived
    /// after `seen` was read.
    pub(crate) fn wait_for_change(&self, seen: u64, timeout: Duration) {
        let state = self.state.lock().expect("byte queue poisoned");
        if state.wakes != seen {
            return;
        }
        let _ = self
            .changed
            .wait_timeout(state, timeout)
            .expect("byte queue poisoned");
    }

    pub(crate) fn pop_blocking(&self) -> Option<T> {
        let mut state = self.state.lock().expect("byte queue poisoned");
        loop {
            if let Some(entry) = state.entries.pop_front() {
                state.remove_front_index(&entry);
                state.bytes -= entry.bytes;
                self.changed.notify_all();
                return Some(entry.value);
            }
            if state.closed {
                return None;
            }
            state = self.changed.wait(state).expect("byte queue poisoned");
        }
    }

    /// Wait until an item is available, the queue closes, or `timeout` elapses.
    ///
    /// The item stays queued for the normal consumer. This is useful when a loop has other work to
    /// perform between wakeup and dequeueing, while still letting producers interrupt an idle
    /// wait without introducing another notification channel.
    pub(crate) fn wait_for_item(&self, timeout: Duration) -> bool {
        self.wait_for_item_after(timeout, || {})
    }

    fn wait_for_item_after(&self, timeout: Duration, before_wait: impl FnOnce()) -> bool {
        let state = self.state.lock().expect("byte queue poisoned");
        if !state.entries.is_empty() || state.closed {
            return !state.entries.is_empty();
        }
        // Tests use this hook to prove that producer notification interrupts the condvar wait. It
        // runs while the queue mutex is still held, immediately before `wait_timeout_while`
        // atomically releases it, so the producer cannot race ahead of the waiter.
        before_wait();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.entries.is_empty() && !state.closed
            })
            .expect("byte queue poisoned");
        !state.entries.is_empty()
    }

    pub(crate) fn close(&self) {
        let mut state = self.state.lock().expect("byte queue poisoned");
        state.closed = true;
        self.changed.notify_all();
    }

    pub(crate) fn stats(&self) -> QueueStats {
        let state = self.state.lock().expect("byte queue poisoned");
        QueueStats {
            bytes: state.bytes,
            high_water_bytes: state.high_water_bytes,
            capacity: self.capacity,
            len: state.entries.len(),
            closed: state.closed,
            coalesced_frames: state.coalesced_frames,
            coalesced_bytes: state.coalesced_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    const WAKE_FALLBACK: Duration = Duration::from_secs(5);
    const WAKE_DEADLINE: Duration = Duration::from_millis(500);

    fn replacement(image: u32, epoch: u64) -> ReplacementKey {
        ReplacementKey {
            pane: 1,
            local: false,
            generation: 2,
            epoch,
            image,
            incarnation: 1,
        }
    }

    #[test]
    fn reset_barriers_prevent_same_epoch_replacement_across_replay() {
        let queue = ByteQueue::new(8);
        let key = replacement(7, 1);
        queue.try_push_latest(1, 1, Some(key)).unwrap();
        queue.invalidate_replacements(1, false);
        queue.try_push(2, 1).unwrap();
        queue.try_push_latest(3, 1, Some(key)).unwrap();
        queue.try_push_latest(4, 1, Some(key)).unwrap();
        assert_eq!(queue.try_pop(), Some(1));
        assert_eq!(queue.try_pop(), Some(2));
        assert_eq!(queue.try_pop(), Some(4));
    }

    #[test]
    fn independent_frames_keep_only_latest_at_capacity() {
        let queue = ByteQueue::new(8);
        for frame in 0..100 {
            queue
                .try_push_latest(frame, 8, Some(replacement(7, 0)))
                .unwrap();
        }
        assert_eq!(queue.stats().len, 1);
        assert_eq!(queue.stats().bytes, 8);
        assert_eq!(queue.stats().coalesced_frames, 99);
        assert_eq!(queue.stats().coalesced_bytes, 792);
        assert_eq!(queue.try_pop(), Some(99));
        queue
            .try_push_latest(100, 1, Some(replacement(7, 0)))
            .unwrap();
        assert_eq!(queue.try_pop(), Some(100));
    }

    #[test]
    fn independent_streams_preserve_barriers_and_ordered_bytes() {
        let queue = ByteQueue::new(8);
        queue
            .try_push_latest(1, 1, Some(replacement(7, 0)))
            .unwrap();
        queue
            .try_push_latest(2, 1, Some(replacement(8, 0)))
            .unwrap();
        queue
            .try_push_latest(3, 1, Some(replacement(7, 0)))
            .unwrap();
        queue.try_push_latest(4, 1, None).unwrap();
        queue
            .try_push_latest(5, 1, Some(replacement(7, 1)))
            .unwrap();
        queue
            .try_push_latest(6, 1, Some(replacement(7, 1)))
            .unwrap();
        assert_eq!(queue.try_pop(), Some(3));
        assert_eq!(queue.pop_blocking(), Some(2));
        assert_eq!(queue.try_pop(), Some(4));
        assert_eq!(queue.try_pop(), Some(6));
        assert_eq!(queue.stats().bytes, 0);
    }

    #[test]
    fn preserves_order_and_exact_byte_accounting() {
        let queue = ByteQueue::new(5);
        queue.try_push(vec![1, 2], 2).unwrap();
        queue.try_push(vec![3, 4, 5], 3).unwrap();
        assert_eq!(queue.stats().bytes, 5);
        assert!(matches!(
            queue.try_push(vec![6], 1),
            Err(PushError::Full(_))
        ));
        assert_eq!(queue.try_pop(), Some(vec![1, 2]));
        assert_eq!(queue.try_pop(), Some(vec![3, 4, 5]));
        assert_eq!(queue.stats().high_water_bytes, 5);
    }

    #[test]
    fn adjacent_entries_can_coalesce_without_crossing_capacity() {
        let queue = ByteQueue::new(6);
        queue.try_push(vec![1, 2], 2).unwrap();
        queue
            .try_push_with(vec![3, 4], 2, |back, next| {
                back.extend_from_slice(next);
                true
            })
            .unwrap();
        assert_eq!(queue.stats().len, 1);
        assert_eq!(
            queue.try_pop_with_bytes_and_more(),
            Some((vec![1, 2, 3, 4], 4, false))
        );
    }

    #[test]
    fn segment_coalescing_scans_backward_until_a_barrier() {
        let queue = ByteQueue::new(8);
        let coalesce = |queued: &mut (u8, Vec<u8>), next: &(u8, Vec<u8>), _adjacent| {
            if queued.0 == 0 || next.0 == 0 {
                SegmentCoalesce::Barrier
            } else if queued.0 == next.0 {
                queued.1.extend_from_slice(&next.1);
                SegmentCoalesce::Merged
            } else {
                SegmentCoalesce::Continue
            }
        };

        queue
            .push_blocking_within_segment((1, vec![1]), 1, coalesce)
            .unwrap();
        queue
            .push_blocking_within_segment((2, vec![2]), 1, coalesce)
            .unwrap();
        queue
            .push_blocking_within_segment((1, vec![3]), 1, coalesce)
            .unwrap();
        queue
            .push_blocking_within_segment((0, Vec::new()), 0, coalesce)
            .unwrap();
        queue
            .push_blocking_within_segment((1, vec![4]), 1, coalesce)
            .unwrap();

        assert_eq!(queue.stats().bytes, 4);
        assert_eq!(queue.stats().len, 4);
        assert_eq!(queue.try_pop(), Some((1, vec![1, 3])));
        assert_eq!(queue.try_pop(), Some((2, vec![2])));
        assert_eq!(queue.try_pop(), Some((0, Vec::new())));
        assert_eq!(queue.try_pop(), Some((1, vec![4])));
    }

    #[test]
    fn weighted_pop_reports_whether_an_entry_remains() {
        let queue = ByteQueue::new(2);
        queue.try_push(1, 1).unwrap();
        queue.try_push(2, 1).unwrap();

        assert_eq!(queue.try_pop_with_bytes_and_more(), Some((1, 1, true)));
        assert_eq!(queue.try_pop_with_bytes_and_more(), Some((2, 1, false)));
    }

    #[test]
    fn blocked_producer_resumes_without_output_loss() {
        let queue = Arc::new(ByteQueue::new(3));
        queue.try_push(vec![1, 2, 3], 3).unwrap();
        let producer_queue = Arc::clone(&queue);
        let producer = std::thread::spawn(move || {
            producer_queue
                .push_blocking_with(vec![4, 5], 2, |_, _| false)
                .unwrap();
        });
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(queue.stats().bytes, 3);
        assert_eq!(queue.try_pop(), Some(vec![1, 2, 3]));
        producer.join().unwrap();
        assert_eq!(queue.try_pop(), Some(vec![4, 5]));
    }

    #[test]
    fn close_wakes_blocked_producers() {
        let queue = Arc::new(ByteQueue::new(1));
        queue.try_push(1, 1).unwrap();
        let producer_queue = Arc::clone(&queue);
        let producer =
            std::thread::spawn(move || producer_queue.push_blocking_with(2, 1, |_, _| false));
        std::thread::sleep(Duration::from_millis(20));
        queue.close();
        assert_eq!(producer.join().unwrap(), Err(PushError::Closed(2)));
    }

    #[test]
    fn timed_wait_observes_an_item_without_consuming_it() {
        let queue = Arc::new(ByteQueue::new(1));
        let waiting_queue = Arc::clone(&queue);
        let (waiting_tx, waiting_rx) = mpsc::sync_channel(0);
        let (result_tx, result_rx) = mpsc::sync_channel(0);
        let waiter = std::thread::spawn(move || {
            let result = waiting_queue.wait_for_item_after(WAKE_FALLBACK, || {
                waiting_tx.send(()).unwrap();
            });
            result_tx.send(result).unwrap();
        });

        waiting_rx.recv_timeout(WAKE_DEADLINE).unwrap();
        queue.try_push(7, 1).unwrap();

        assert!(result_rx.recv_timeout(WAKE_DEADLINE).unwrap());
        waiter.join().unwrap();
        assert_eq!(queue.try_pop(), Some(7));
    }

    #[test]
    fn timed_wait_returns_false_when_the_queue_closes_empty() {
        let queue = Arc::new(ByteQueue::<u8>::new(1));
        let waiting_queue = Arc::clone(&queue);
        let (waiting_tx, waiting_rx) = mpsc::sync_channel(0);
        let (result_tx, result_rx) = mpsc::sync_channel(0);
        let waiter = std::thread::spawn(move || {
            let result = waiting_queue.wait_for_item_after(WAKE_FALLBACK, || {
                waiting_tx.send(()).unwrap();
            });
            result_tx.send(result).unwrap();
        });

        waiting_rx.recv_timeout(WAKE_DEADLINE).unwrap();
        queue.close();

        assert!(!result_rx.recv_timeout(WAKE_DEADLINE).unwrap());
        waiter.join().unwrap();
    }

    #[test]
    fn zero_byte_control_stays_behind_queued_payload() {
        let queue = ByteQueue::new(1);
        queue.try_push("output", 1).unwrap();
        queue.try_push("exit", 0).unwrap();
        assert_eq!(queue.try_pop(), Some("output"));
        assert_eq!(queue.try_pop(), Some("exit"));
    }

    #[test]
    fn congested_flood_has_the_same_transcript_as_the_producer() {
        let queue = Arc::new(ByteQueue::new(32));
        let producer_queue = Arc::clone(&queue);
        let expected: Vec<u8> = (0..=255).cycle().take(16 * 1024).collect();
        let producer_bytes = expected.clone();
        let producer = std::thread::spawn(move || {
            for chunk in producer_bytes.chunks(13) {
                producer_queue
                    .push_blocking_with(chunk.to_vec(), chunk.len(), |_, _| false)
                    .unwrap();
            }
            producer_queue.close();
        });

        let mut actual = Vec::new();
        while let Some(chunk) = queue.pop_blocking() {
            actual.extend_from_slice(&chunk);
        }
        producer.join().unwrap();
        assert_eq!(actual, expected);
        assert!(queue.stats().high_water_bytes <= 32);
    }
}
