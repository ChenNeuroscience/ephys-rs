//! The latest of something, from a node's passes for its window (or the dev tools): a frame of traces to draw, say.
//!
//! It's a `Pool` of buffers, kept in the run's exchange (see `crate::core::exchange`), so the node's window can find
//! it. A pass takes an empty buffer, fills it (or swaps data into it), and queues it (`Sender::send`); the window
//! takes the newest queued, giving back the one it was reading and any older (`Latest::read`). Neither waits on the
//! other: a pass doesn't skip its send while the window's reading, and the window holds on to what it's reading for
//! as long as it likes. If the window isn't reading (it's closed), passes reuse the oldest buffer queued. Once the
//! buffers have grown to size (or if they're made to size: `Sender::with`), nothing's allocated.
//!
//! A node whose state is only working memory has a `Sender` on each worker (see `PassState::CARRIED_OVER`): they
//! share the pool, and the window reads the latest pass's.

use std::ops::Deref;
use std::sync::{Arc, RwLock, RwLockReadGuard};

use crate::core::engine::{Pass, PoolStatus};
use crate::core::exchange::Shared;
use crate::standard_components::pool::Pool;

/// Buffers in the pool: one for each worker sending at once, one queued, and one being read.
const BUFFERS: usize = 6;

/// The pool of a node's latest, in the run's exchange: find it from the window with `Context::shared`.
pub struct Latest<T> {
    /// Each buffer with the pass it's from.
    pool: Pool<(u64, T)>,
    /// What the window's reading (perhaps in more than one place at once: it's only replaced when it isn't).
    reading: RwLock<Option<(u64, T)>>,
}

impl<T: Send + Sync + 'static> Shared for Latest<T> {
    fn name(&self) -> &'static str {
        "latest"
    }

    fn pool(&self) -> Option<PoolStatus> {
        let mut status = self.pool.status();
        status.filling = false;
        Some(status)
    }
}

impl<T> Latest<T> {
    /// A pool of `buffers`, nothing sent yet.
    pub fn new(buffers: impl IntoIterator<Item = T>) -> Self {
        Latest { pool: Pool::new(buffers.into_iter().map(|b| (0, b)), 0), reading: RwLock::new(None) }
    }

    /// Already holding `value`, as if it had been sent (for tests).
    #[cfg(test)]
    pub fn holding(value: T) -> Self {
        Latest { pool: Pool::new([], 0), reading: RwLock::new(Some((0, value))) }
    }

    /// The latest sent, if anything has been: for the window, which should read it only from the GUI thread.
    pub fn read(&self) -> Option<Reading<'_, T>> {
        if let Ok(mut reading) = self.reading.try_write() {
            self.take_newest(&mut reading);
        }
        let reading = self.reading.read().unwrap();
        reading.is_some().then_some(Reading(reading))
    }

    /// Takes the newest queued to read, if there's one, giving back the one being read and any older.
    fn take_newest(&self, reading: &mut Option<(u64, T)>) {
        while let Some(newer) = self.pool.try_full() {
            // Passes on different workers can queue out of order: keep the latest pass's.
            match reading.take() {
                Some(old) if old.0 > newer.0 => {
                    self.pool.give_back(newer);
                    *reading = Some(old);
                }
                old => {
                    if let Some(old) = old {
                        self.pool.give_back(old);
                    }
                    *reading = Some(newer);
                }
            }
        }
    }
}

/// The latest sent, being read.
pub struct Reading<'a, T>(RwLockReadGuard<'a, Option<(u64, T)>>);

impl<T> Deref for Reading<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0.as_ref().expect("only made when there's something").1
    }
}

/// The sending end, kept in a node's state: `send` from its passes. Finds (or makes) the node's `Latest` in the
/// run's exchange on the first send.
pub struct Sender<T> {
    latest: Option<Arc<Latest<T>>>,
    /// Makes the pool's buffers.
    make: Box<dyn Fn() -> T + Send + Sync>,
}

impl<T: Default + 'static> Default for Sender<T> {
    fn default() -> Self {
        Sender { latest: None, make: Box::new(T::default) }
    }
}

impl<T: Send + Sync + 'static> Sender<T> {
    /// With the pool's buffers made by `make`: like a pass's, say, to swap them in.
    pub fn with(make: impl Fn() -> T + Send + Sync + 'static) -> Self {
        Sender { latest: None, make: Box::new(make) }
    }

    /// Fills a buffer with `fill` and queues it for the window. `fill` gets back an earlier send's buffer, to
    /// overwrite. Nothing, if every buffer's in use (passes on every worker sending at once).
    pub fn send<E: crate::core::engine::Edges>(&mut self, pass: &Pass<E>, fill: impl FnOnce(&mut T)) {
        if let Some(mut buffer) = self.empty(pass) {
            fill(&mut buffer);
            self.queue(pass.sequence, buffer);
        }
    }

    /// The same in two halves, for a pass to swap its own buffer in (one it takes: see `Node::takes`) while it can't
    /// lend itself to `send`: a buffer to fill (an earlier send's, to overwrite), unless every one's in use.
    pub fn empty<E: crate::core::engine::Edges>(&mut self, pass: &Pass<E>) -> Option<T> {
        let make = &self.make;
        let latest = self.latest.get_or_insert_with(|| pass.shared(|| Latest::new((0..BUFFERS).map(|_| make()))));
        // An empty buffer, or failing that the oldest queued (the window's not reading them).
        let buffer = latest.pool.try_empty().or_else(|| latest.pool.try_full());
        if buffer.is_none() {
            latest.pool.overflowed();
        }
        buffer.map(|(_, buffer)| buffer)
    }

    /// Then queues it for the window, as from pass `sequence`.
    pub fn queue(&self, sequence: u64, buffer: T) {
        if let Some(latest) = &self.latest {
            latest.pool.send((sequence, buffer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edges;
    use crate::core::engine::NodeId;

    #[test]
    fn the_window_reads_the_latest_sent() {
        let mut pass: Pass<Edges> = Pass::new(0, 0.02, &[], &mut []);
        pass.node = NodeId(7);
        let exchange = pass.exchange;
        let mut sender = Sender::<u32>::default();
        sender.send(&pass, |n| *n = 1);
        let latest = exchange.get::<Latest<u32>>(NodeId(7)).unwrap();
        pass.sequence = 1;
        sender.send(&pass, |n| *n = 2);
        assert_eq!(latest.read().as_deref(), Some(&2));
        // Sending while the window reads (in two places at once).
        let reading = latest.read().unwrap();
        pass.sequence = 2;
        sender.send(&pass, |n| *n = 3);
        assert_eq!(*reading, 2);
        assert_eq!(latest.read().as_deref(), Some(&2), "the same, until it's done with");
        drop(reading);
        assert_eq!(latest.read().as_deref(), Some(&3));
        // Another sender (another worker's) shares the pool; an earlier pass's doesn't replace a later one.
        let mut other = Sender::<u32>::default();
        pass.sequence = 5;
        other.send(&pass, |n| *n = 5);
        pass.sequence = 4;
        sender.send(&pass, |n| *n = 4);
        assert_eq!(latest.read().as_deref(), Some(&5));
        // Not read for a while: the oldest queued are reused, and nothing's lost for want of a buffer.
        for sequence in 6..30 {
            pass.sequence = sequence;
            sender.send(&pass, |n| *n = sequence as u32);
        }
        assert_eq!(latest.read().as_deref(), Some(&29));
        assert_eq!(latest.pool().unwrap().overflows, 0);
    }
}
