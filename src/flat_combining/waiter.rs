use rustix::thread::futex;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct Waiter {
    futex_word: AtomicU32,
}

impl Waiter {
    pub fn new() -> Self {
        Self {
            futex_word: AtomicU32::new(0),
        }
    }

    pub(crate) fn futex_word(&self) -> &AtomicU32 {
        &self.futex_word
    }

    pub fn is_ready(&self) -> bool {
        self.futex_word.load(Ordering::Acquire) == 1
    }

    pub fn notify(&self) {
        // Signal that we're ready.
        self.futex_word.swap(1, Ordering::Release);

        // Wake up one waiting thread
        let _ = futex::wake(&self.futex_word, futex::Flags::PRIVATE, 1);
    }
}
