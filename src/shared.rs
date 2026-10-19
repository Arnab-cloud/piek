use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

pub struct Shared {
    pub queue: Mutex<VecDeque<usize>>,
    pub wake: Condvar,
    /// Set when the document is closed so the worker thread can exit.
    pub shutdown: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Blocks until a page is queued. Returns `None` once shut down.
    pub fn next_job(&self) -> Option<usize> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            if let Some(page) = queue.pop_front() {
                return Some(page);
            }
            queue = self.wake.wait(queue).unwrap();
        }
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        // Take the lock so the worker can't miss the wakeup between its
        // flag check and its wait.
        let _guard = self.queue.lock().unwrap();
        self.wake.notify_all();
    }
}
