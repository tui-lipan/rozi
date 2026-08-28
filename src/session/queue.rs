use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueueStats {
    pub(crate) bytes: usize,
    pub(crate) high_water_bytes: usize,
    pub(crate) capacity: usize,
    pub(crate) len: usize,
    pub(crate) closed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PushError<T> {
    Full(T),
    Closed(T),
    TooLarge(T),
}

struct Entry<T> {
    value: T,
    bytes: usize,
}

struct State<T> {
    entries: VecDeque<Entry<T>>,
    bytes: usize,
    high_water_bytes: usize,
    closed: bool,
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
                bytes: 0,
                high_water_bytes: 0,
                closed: false,
            }),
            changed: Condvar::new(),
        }
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
        state.entries.push_back(Entry { value, bytes });
        state.bytes += bytes;
        state.high_water_bytes = state.high_water_bytes.max(state.bytes);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn push_blocking_with(
        &self,
        value: T,
        bytes: usize,
        coalesce: impl FnOnce(&mut T, &T) -> bool,
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
        if let Some(back) = state.entries.back_mut()
            && coalesce(&mut back.value, &value)
        {
            back.bytes += bytes;
            state.bytes += bytes;
            state.high_water_bytes = state.high_water_bytes.max(state.bytes);
            self.changed.notify_all();
            return Ok(());
        }
        state.entries.push_back(Entry { value, bytes });
        state.bytes += bytes;
        state.high_water_bytes = state.high_water_bytes.max(state.bytes);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn try_pop(&self) -> Option<T> {
        self.try_pop_with_bytes().map(|(value, _)| value)
    }

    pub(crate) fn try_pop_with_bytes(&self) -> Option<(T, usize)> {
        let mut state = self.state.lock().expect("byte queue poisoned");
        let entry = state.entries.pop_front()?;
        state.bytes -= entry.bytes;
        self.changed.notify_all();
        Some((entry.value, entry.bytes))
    }

    pub(crate) fn pop_blocking(&self) -> Option<T> {
        let mut state = self.state.lock().expect("byte queue poisoned");
        loop {
            if let Some(entry) = state.entries.pop_front() {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

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
        assert_eq!(queue.try_pop_with_bytes(), Some((vec![1, 2, 3, 4], 4)));
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
