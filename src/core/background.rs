//! Threads left to finish their work in the background (a writer closing its files, say), which the program
//! waits for before it quits.

use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;

/// The threads left to finish, and how many another `wait` has taken to join and hasn't yet.
static FINISHING: Mutex<(Vec<JoinHandle<()>>, usize)> = Mutex::new((Vec::new(), 0));
/// Told whenever a `wait` has joined what it took.
static JOINED: Condvar = Condvar::new();

/// Lets `thread` finish in its own time, as long as it does before the program quits.
pub fn finish_later(thread: JoinHandle<()>) {
    let mut finishing = FINISHING.lock().unwrap();
    finishing.0.retain(|t| !t.is_finished());
    finishing.0.push(thread);
}

/// Waits for every thread left to finish, including any they leave to finish in turn (a node's state, let go of
/// in the background, handing on its writer's thread, say), and any another `wait` is joining at the same time.
pub fn wait() {
    let mut finishing = FINISHING.lock().unwrap();
    loop {
        if finishing.0.is_empty() {
            if finishing.1 == 0 {
                return;
            }
            finishing = JOINED.wait(finishing).unwrap();
            continue;
        }
        let threads = std::mem::take(&mut finishing.0);
        finishing.1 += threads.len();
        drop(finishing);
        log::info!("Waiting for {} threads to finish", threads.len());
        let count = threads.len();
        for thread in threads {
            let _ = thread.join();
        }
        finishing = FINISHING.lock().unwrap();
        finishing.1 -= count;
        JOINED.notify_all();
    }
}
