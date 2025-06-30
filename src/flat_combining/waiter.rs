use rustix::thread::futex;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct Waiter {
    futex: AtomicU32,
}

impl Waiter {
    pub(crate) const UNREADY: u32 = 0;
    pub(crate) const READY: u32 = 1;

    pub fn new() -> Self {
        Self {
            futex: AtomicU32::new(Self::UNREADY),
        }
    }

    pub(crate) fn futex(&self) -> &AtomicU32 {
        &self.futex
    }

    pub fn is_ready(&self) -> bool {
        self.futex.load(Ordering::Acquire) == Self::READY
    }

    pub fn notify(&self) {
        // Signal that we're ready.
        self.futex.swap(Self::READY, Ordering::Release);

        // Wake up one waiting thread. Assumes that only one person is ever
        // waiting on this waiter.
        let _ = futex::wake(&self.futex, futex::Flags::PRIVATE, 1);
    }
}
