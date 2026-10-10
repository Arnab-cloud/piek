use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

pub struct Shared {
    pub queue: Mutex<VecDeque<usize>>,
    pub wake: Condvar,
}

impl Shared {
    pub fn next_job(&self) -> usize {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(page) = queue.pop_front() {
                return page;
            }
            queue = self.wake.wait(queue).unwrap();
        }
    }
}
