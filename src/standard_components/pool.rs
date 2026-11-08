//! A pool of buffers that threads swap, rather than copying what's in them: one fills a buffer (or swaps its own in)
//! and queues it, another takes it, and empty buffers go back to be filled again. Nothing is copied, and once the
//! pool's made, nothing's allocated. It's for any two threads: a node's thread and its passes, or its passes and its
//! window. How it's used depends on who's at each end, as below.
//!
//! For a source (a board, say), the pass takes whatever's arrived since the last: the buffer being filled, however
//! full (`take`). If passes fall behind, a full buffer waits for them (`filled`) and the thread fills the next, and
//! later passes take those first. If the pool runs out, the passes are a long way behind: what comes then is lost.
//!
//! For a writer, a pass swaps its data into an empty buffer and queues it (`send`); the thread writes it and gives
//! the buffer back (`give_back`). If the pool runs out, the disk is a long way behind: that pass's data is lost.
//!
//! For a window, only the newest matters: passes fill a buffer (`reuse`: an empty one, or failing that the oldest
//! queued, so none are lost if the window's closed) and queue it as of their pass (`send_as_of`); the window reads
//! the newest (`newest`), giving back the one it was reading and any older, and holds on to it for as long as it
//! likes. A window that needs everything, not only the newest (each line of a log, say), takes every buffer, in order
//! (`send_pending`, `receive_all`). A pool shared with the GUI is (or is part of) the node's `Node::Shared`, which
//! the passes find with `Pass::shared`, and the window with `UiContext::shared`.
//!
//! Passes never wait on it. The empty and full buffers are on lock-free queues (crossbeam's bounded channels), so
//! taking and giving back is a few atomic operations; only a writer's thread blocks, waiting for a buffer to write.
//! A source's buffer being filled is behind a lock, which its thread holds while it decodes into it: a pass that
//! finds it held doesn't wait, but leaves what's in it for the next pass.

use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::core::engine::{PoolStatus, Shared};

/// Passes' worth of slack a pool of a pass's buffers gives, before data's lost: a second's.
pub const SLACK_PASSES: usize = 50;

/// Buffers in a pool shared with a window: one for each worker sending at once, one queued, and one being read.
pub const WINDOW_BUFFERS: usize = 6;

/// One end of a pool (clone it for the other).
pub struct Pool<T> {
    shared: Arc<Inner<T>>,
}

/// A node can share a pool with the GUI by itself: named by what its buffers hold, its type's name.
impl<T: Send + Sync + 'static> Shared for Pool<T> {
    fn pools(&self) -> Vec<(&'static str, PoolStatus)> {
        let name = std::any::type_name::<T>();
        vec![(name.rsplit("::").next().unwrap_or(name), self.status())]
    }
}

/// A pool for a window: `WINDOW_BUFFERS` buffers made by `T::default`.
impl<T: Default> Default for Pool<T> {
    fn default() -> Self {
        Pool::new((0..WINDOW_BUFFERS).map(|_| T::default()), 0)
    }
}

impl<T> Clone for Pool<T> {
    fn clone(&self) -> Self {
        Pool { shared: self.shared.clone() }
    }
}

/// A buffer, with the pass it's as of.
type AsOf<T> = (u64, T);

struct Inner<T> {
    size: usize,
    bytes: usize,
    empty: (Sender<T>, Receiver<T>),
    /// Each with the pass it's as of, for a window to take the newest (0 if no one's said).
    full: (Sender<AsOf<T>>, Receiver<AsOf<T>>),
    /// A source's buffer being filled, which the next pass takes however full it is.
    filling: Mutex<Option<T>>,
    /// What a window's reading (perhaps in more than one place at once: it's only replaced when it isn't).
    reading: RwLock<Option<(u64, T)>>,
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
        let shared = Inner {
            size,
            bytes,
            empty,
            full,
            filling: Mutex::new(None),
            reading: RwLock::new(None),
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
            let _ = self.shared.full.0.try_send((0, buffer));
        }
    }

    // A source's passes.

    /// What's arrived since the last pass took any: a full buffer waiting, if there is one, or the one being
    /// filled, unless `is_empty` says there's nothing in it yet (or the thread's filling it just then: it's left for
    /// the next pass, rather than waited for). Give a buffer back for each taken.
    pub fn take(&self, is_empty: impl FnOnce(&T) -> bool) -> Option<T> {
        let taken = self.shared.full.1.try_recv().ok().map(|(_, b)| b).or_else(|| {
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

    /// Queues a buffer of data for the thread.
    pub fn send(&self, buffer: T) {
        self.send_as_of(0, buffer);
    }

    /// Queues a buffer of data as of pass `sequence` (for a window, taking the newest).
    pub fn send_as_of(&self, sequence: u64, buffer: T) {
        self.shared.swaps.fetch_add(1, Ordering::Relaxed);
        let _ = self.shared.full.0.try_send((sequence, buffer));
    }

    /// A buffer to fill when only the newest matters (for a window): an empty one, or failing that the oldest queued
    /// (the window's not reading them). Nothing, and counted, if every one's in use.
    pub fn reuse(&self) -> Option<T> {
        let buffer = self.shared.empty.1.try_recv().ok().or_else(|| self.shared.full.1.try_recv().ok().map(|(_, b)| b));
        if buffer.is_none() {
            self.overflowed();
        }
        buffer
    }

    // A writer's thread.

    /// The next buffer to write, waiting up to `timeout` for one.
    pub fn next_full(&self, timeout: Duration) -> Option<T> {
        self.shared.full.1.recv_timeout(timeout).ok().map(|(_, b)| b)
    }

    /// The next buffer queued, without waiting.
    pub fn try_full(&self) -> Option<T> {
        self.shared.full.1.try_recv().ok().map(|(_, b)| b)
    }

    // A window (on the GUI thread).

    /// The newest queued, if anything has been, giving back the one being read and any older. Passes on different
    /// workers can queue out of order: it's the latest pass's.
    pub fn newest(&self) -> Option<Newest<'_, T>> {
        if let Ok(mut reading) = self.shared.reading.try_write() {
            while let Ok(newer) = self.shared.full.1.try_recv() {
                match reading.take() {
                    Some(old) if old.0 > newer.0 => {
                        self.give_back(newer.1);
                        *reading = Some(old);
                    }
                    old => {
                        if let Some((_, old)) = old {
                            self.give_back(old);
                        }
                        *reading = Some(newer);
                    }
                }
            }
        }
        let reading = self.shared.reading.read().unwrap();
        reading.is_some().then_some(Newest(reading))
    }

    /// For a window: fills a buffer with `fill` (an earlier send's, to overwrite: see `reuse`) and queues it as of pass
    /// `sequence`. Nothing, if every buffer's in use (passes on every worker sending at once).
    pub fn send_newest(&self, sequence: u64, fill: impl FnOnce(&mut T)) {
        if let Some(mut buffer) = self.reuse() {
            fill(&mut buffer);
            self.send_as_of(sequence, buffer);
        }
    }

    /// Already holding `value`, as if it had been sent and read (for tests).
    #[cfg(test)]
    pub fn holding(value: T) -> Self {
        let pool = Pool::new([], 0);
        *pool.shared.reading.write().unwrap() = Some((0, value));
        pool
    }
}

/// For a window that's sent everything, not only the newest (each line of a log, say): passes add what's new to
/// what's pending (in their state), and send it whenever there's an empty buffer; the window takes every buffer, in
/// order, keeping what it likes. If the window isn't taking them (it's closed, say), what's new waits in `pending`.
impl<T> Pool<Vec<T>> {
    /// Sends what's `pending`, if there is any and a buffer's empty: swapped, so `pending` is left with the empty
    /// buffer's memory.
    pub fn send_pending(&self, pending: &mut Vec<T>) {
        if pending.is_empty() {
            return;
        }
        if let Ok(mut buffer) = self.shared.empty.1.try_recv() {
            buffer.clear();
            std::mem::swap(&mut buffer, pending);
            self.send(buffer);
        }
    }

    /// For the window: everything sent, in order, giving the buffers back.
    pub fn receive_all(&self, mut receive: impl FnMut(std::vec::Drain<'_, T>)) {
        while let Some(mut buffer) = self.try_full() {
            receive(buffer.drain(..));
            self.give_back(buffer);
        }
    }
}

/// The newest in a pool, being read by a window.
pub struct Newest<'a, T>(RwLockReadGuard<'a, Option<(u64, T)>>);

impl<T> Deref for Newest<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0.as_ref().expect("only made when there's something").1
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

    #[test]
    fn a_window_reads_the_newest_sent() {
        let pool = Pool::<u32>::default();
        pool.send_newest(0, |n| *n = 1);
        pool.send_newest(1, |n| *n = 2);
        assert_eq!(pool.newest().as_deref(), Some(&2));
        // Sending while the window reads (in two places at once).
        let reading = pool.newest().unwrap();
        pool.send_newest(2, |n| *n = 3);
        assert_eq!(*reading, 2);
        assert_eq!(pool.newest().as_deref(), Some(&2), "the same, until it's done with");
        drop(reading);
        assert_eq!(pool.newest().as_deref(), Some(&3));
        // Passes on different workers send out of order: an earlier pass's doesn't replace a later one.
        pool.send_newest(5, |n| *n = 5);
        pool.send_newest(4, |n| *n = 4);
        assert_eq!(pool.newest().as_deref(), Some(&5));
        // Not read for a while: the oldest queued are reused, and nothing's lost for want of a buffer.
        for sequence in 6..30 {
            pool.send_newest(sequence, |n| *n = sequence as u32);
        }
        assert_eq!(pool.newest().as_deref(), Some(&29));
        assert_eq!(pool.status().overflows, 0);
    }
}
