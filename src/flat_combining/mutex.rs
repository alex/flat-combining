use rustix::thread::futex;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};

/// A Linux-only mutex implementation using futex
pub struct Mutex<T> {
    futex: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

pub struct MutexGuard<'a, T> {
    pub(crate) mutex: &'a Mutex<T>,
}

impl<T> Mutex<T> {
    pub(crate) const UNLOCKED: u32 = 0;
    pub(crate) const LOCKED: u32 = 1;
    pub(crate) const CONTENDED: u32 = 2;

    pub(crate) fn futex(&self) -> &AtomicU32 {
        &self.futex
    }

    pub fn new(data: T) -> Self {
        Self {
            futex: AtomicU32::new(Self::UNLOCKED),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Fast path: try to acquire lock directly
        if self
            .futex
            .compare_exchange_weak(
                Self::UNLOCKED,
                Self::LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            return MutexGuard { mutex: self };
        }

        // Slow path: contention
        self.lock_contended()
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .futex
            .compare_exchange(
                Self::UNLOCKED,
                Self::LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }

    fn lock_contended(&self) -> MutexGuard<'_, T> {
        let mut state = self.futex.load(Ordering::Relaxed);

        loop {
            // If unlocked, try to acquire
            if state == Self::UNLOCKED {
                match self.futex.compare_exchange_weak(
                    Self::UNLOCKED,
                    Self::LOCKED,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return MutexGuard { mutex: self },
                    Err(new_state) => {
                        state = new_state;
                        continue;
                    }
                }
            }

            // Mark as contended if not already
            if state == Self::LOCKED {
                // Upgrade the mutex to contended.
                if self.futex.swap(Self::CONTENDED, Ordering::Acquire) == Self::UNLOCKED {
                    // We just swapped from UNLOCKED -> CONTENDED, which means we
                    // took the lock.
                    return MutexGuard { mutex: self };
                }
            }

            // Wait on futex
            let _ = futex::wait(&self.futex, futex::Flags::PRIVATE, Self::CONTENDED, None);

            state = self.futex.load(Ordering::Relaxed);
        }
    }

    fn unlock(&self) {
        let prev = self.futex.swap(Self::UNLOCKED, Ordering::Release);

        // If there were waiters, wake one
        if prev == Self::CONTENDED {
            let _ = futex::wake(&self.futex, futex::Flags::PRIVATE, 1);
        }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}
