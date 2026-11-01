//! Threads left to finish their work in the background (a writer closing its files, say), which the program
//! waits for before it quits.

use std::sync::Mutex;
use std::thread::JoinHandle;

static FINISHING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Lets `thread` finish in its own time, as long as it does before the program quits.
pub fn finish_later(thread: JoinHandle<()>) {
    let mut finishing = FINISHING.lock().unwrap();
    finishing.retain(|t| !t.is_finished());
    finishing.push(thread);
}

/// Waits for every thread left to finish, including any they leave to finish in turn (a node's state, let go of
/// in the background, handing on its writer's thread, say).
pub fn wait() {
    loop {
        let threads = std::mem::take(&mut *FINISHING.lock().unwrap());
        if threads.is_empty() {
            return;
        }
        log::info!("Waiting for {} threads to finish", threads.len());
        for thread in threads {
            let _ = thread.join();
        }
    }
}
