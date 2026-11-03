//! A pool of buffers that a node's thread and its passes swap, rather than copying what's in them: the thread fills
//! a buffer and a pass swaps it for one of the worker's own (taking the data, giving back the worker's old buffer to
//! fill again), or the other way round, a pass swaps the data it's given for an empty buffer from the pool and the
//! thread writes it out. Nothing is copied, and once the pool's made, nothing's allocated.
//!
//! For a source (a board, say), the pass takes whatever's arrived since the last: the buffer being filled, however
//! full (`take`). If passes fall behind, a full buffer waits for them (`filled`) and the thread fills the next, and
//! later passes take those first. If the pool runs out, the passes are a long way behind: what comes then is lost.
//!
//! For a writer, a pass swaps its data into an empty buffer and queues it (`send`); the thread writes it and gives
//! the buffer back (`give_back`). If the pool runs out, the disk is a long way behind: that pass's data is lost.
//!
//! For a window (see `crate::standard_components::latest`), passes fill empty buffers and queue them, and the GUI
//! takes the latest, giving back the rest.
//!
//! Passes never wait on it. The empty and full buffers are on lock-free queues (crossbeam's bounded channels), so
//! taking and giving back is a few atomic operations; only a writer's thread blocks, waiting for a buffer to write.
//! A source's buffer being filled is behind a lock, which its thread holds while it decodes into it: a pass that
//! finds it held doesn't wait, but leaves what's in it for the next pass.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::core::engine::PoolStatus;

/// Passes' worth of slack a pool of a pass's buffers gives, before data's lost: a second's.
pub const SLACK_PASSES: usize = 50;

/// One end of a pool (clone it for the other).
pub struct Pool<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for Pool<T> {
    fn clone(&self) -> Self {
        Pool { shared: self.shared.clone() }
    }
}

struct Shared<T> {
    size: usize,
    bytes: usize,
    empty: (Sender<T>, Receiver<T>),
    full: (Sender<T>, Receiver<T>),
    /// A source's buffer being filled, which the next pass takes however full it is.
    filling: Mutex<Option<T>>,
    swaps: AtomicU64,
    overflows: AtomicU64,
}

impl<T> Pool<T> {
    /// A pool of `buffers`, all empty to begin with, each taking `bytes` (to show).
    pub fn new(buffers: impl IntoIterator<Item = T>, bytes: usize) -> Self {
        let buffers: Vec<T> = buffers.into_iter().collect();
        let size = buffers.len();
        let (empty, full) = (crossbeam_channel::bounded(size), crossbeam_channel::bounded(size));
        for buffer in buffers {
            let _ = empty.0.try_send(buffer);
        }
        let shared = Shared {
            size,
            bytes,
            empty,
            full,
            filling: Mutex::new(None),
            swaps: AtomicU64::new(0),
            overflows: AtomicU64::new(0),
        };
        Pool { shared: Arc::new(shared) }
    }

    /// Where the buffers are, for the dev tools.
    pub fn status(&self) -> PoolStatus {
        let s = &self.shared;
        let filling = s.filling.try_lock().map_or(true, |f| f.is_some());
        PoolStatus {
            size: s.size,
            bytes: s.bytes,
            ports: u64::MAX,
            empty: s.empty.1.len(),
            filling,
            full: s.full.1.len(),
            swaps: s.swaps.load(Ordering::Relaxed),
            overflows: s.overflows.load(Ordering::Relaxed),
        }
    }

    /// Counts data lost for want of a buffer.
    pub fn overflowed(&self) {
        self.shared.overflows.fetch_add(1, Ordering::Relaxed);
    }

    // A source's thread.

    /// Runs `f` on the buffer being filled, starting an empty one (made ready with `start`) if there isn't one.
    /// Nothing if there are no empty buffers left (the passes a long way behind).
    pub fn fill<R>(&self, start: impl FnOnce(&mut T), f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut filling = self.shared.filling.lock().unwrap();
        if filling.is_none() {
            let mut buffer = self.shared.empty.1.try_recv().ok()?;
            start(&mut buffer);
            *filling = Some(buffer);
        }
        filling.as_mut().map(f)
    }

    /// The buffer being filled is full: it waits for a pass, and the next starts.
    pub fn filled(&self) {
        if let Some(buffer) = self.shared.filling.lock().unwrap().take() {
            let _ = self.shared.full.0.try_send(buffer);
        }
    }

    // A source's passes.

    /// What's arrived since the last pass took any: a full buffer waiting, if there is one, or the one being
    /// filled, unless `is_empty` says there's nothing in it yet (or the thread's filling it just then: it's left for
    /// the next pass, rather than waited for). Give a buffer back for each taken.
    pub fn take(&self, is_empty: impl FnOnce(&T) -> bool) -> Option<T> {
        let taken = self.shared.full.1.try_recv().ok().or_else(|| {
            let mut filling = self.shared.filling.try_lock().ok()?;
            if filling.as_ref().is_some_and(|b| !is_empty(b)) { filling.take() } else { None }
        });
        if taken.is_some() {
            self.shared.swaps.fetch_add(1, Ordering::Relaxed);
        }
        taken
    }

    // Either: a source's passes give back the workers' buffers they swapped out, a writer's thread those written.

    pub fn give_back(&self, buffer: T) {
        let _ = self.shared.empty.0.try_send(buffer);
    }

    // A writer's passes (or a window's).

    /// An empty buffer to swap data into, if there's one left (otherwise the disk's a long way behind).
    pub fn empty(&self) -> Option<T> {
        let buffer = self.shared.empty.1.try_recv().ok();
        if buffer.is_none() {
            self.overflowed();
        }
        buffer
    }

    /// An empty buffer, if there's one, without counting it lost if there isn't.
    pub fn try_empty(&self) -> Option<T> {
        self.shared.empty.1.try_recv().ok()
    }

    /// Queues a buffer of data for the thread.
    pub fn send(&self, buffer: T) {
        self.shared.swaps.fetch_add(1, Ordering::Relaxed);
        let _ = self.shared.full.0.try_send(buffer);
    }

    // A writer's thread (or a window).

    /// The next buffer to write, waiting up to `timeout` for one.
    pub fn next_full(&self, timeout: Duration) -> Option<T> {
        self.shared.full.1.recv_timeout(timeout).ok()
    }

    /// The next buffer queued, without waiting.
    pub fn try_full(&self) -> Option<T> {
        self.shared.full.1.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_fills_and_passes_take_what_has_arrived() {
        let pool = Pool::new([Vec::<u32>::new(), Vec::new(), Vec::new()], 0);
        let pass = pool.clone();
        assert_eq!(pass.take(Vec::is_empty), None, "nothing's arrived");
        pool.fill(Vec::clear, |b| b.extend([1, 2]));
        assert_eq!(pool.status().empty, 2);
        // A pass takes what's there, however little, giving back a buffer of its own.
        assert_eq!(pass.take(Vec::is_empty), Some(vec![1, 2]));
        pass.give_back(vec![9]);
        // Falling behind: a full buffer waits, and the next fills; the passes take them in order.
        pool.fill(Vec::clear, |b| b.push(3));
        pool.filled();
        pool.fill(Vec::clear, |b| b.push(4));
        assert_eq!(
            pool.status(),
            PoolStatus { size: 3, bytes: 0, ports: u64::MAX, empty: 1, filling: true, full: 1, swaps: 1, overflows: 0 }
        );
        assert_eq!(pass.take(Vec::is_empty), Some(vec![3]));
        assert_eq!(pass.take(Vec::is_empty), Some(vec![4]));
        // Out of buffers: nothing to fill.
        let pool = Pool::new([Vec::<u32>::new()], 0);
        pool.fill(Vec::clear, |b| b.push(1));
        pool.filled();
        assert!(pool.fill(Vec::clear, |b| b.push(2)).is_none());
    }

    #[test]
    fn a_writer_swaps_into_empty_buffers_and_gets_them_back() {
        let pool = Pool::new([Vec::<u32>::new()], 0);
        let thread = pool.clone();
        let mut chunk = pool.empty().unwrap();
        chunk.push(5);
        pool.send(chunk);
        assert!(pool.empty().is_none(), "out until it's written");
        assert_eq!(pool.status().overflows, 1);
        let written = thread.next_full(Duration::ZERO).unwrap();
        assert_eq!(written, vec![5]);
        thread.give_back(written);
        assert!(pool.empty().is_some());
    }
}
