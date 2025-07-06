use rustix::thread::futex;
use std::cell::UnsafeCell;
use std::num::NonZero;
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
    // The lock is available and may be taken.
    pub(crate) const UNLOCKED: u32 = 0;
    // The lock is taken, but there are no other waiters. If you attempt to
    // take the lock and its in this state, you _must_ upgrade it to
    // `CONTENDED` or you won't get woken up.
    pub(crate) const LOCKED: u32 = 1;
    // The lock is taken and there are waiters. When the lock is released,
    // exactly one of those waiters will be woken up.
    pub(crate) const CONTENDED: u32 = 2;

    pub fn new(data: T) -> Self {
        Self {
            futex: AtomicU32::new(Self::UNLOCKED),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Fast path: try to acquire lock directly
        if let Some(guard) = self.try_lock() {
            return guard;
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

    pub fn bitset_wait_or_lock(
        &self,
        bitset: u32,
        f: impl Fn() -> bool,
    ) -> Option<MutexGuard<'_, T>> {
        // Fast path if we're ready.
        if f() {
            return None;
        }
        // Fast path if the lock is free
        if let Some(guard) = self.try_lock() {
            return Some(guard);
        }

        // Slow path: contention
        self.lock_contended_bitset(bitset, f)
    }

    fn lock_contended(&self) -> MutexGuard<'_, T> {
        loop {
            let state = self.spin();

            // Upgrade the mutex to contended if it's not already.
            if state != Self::CONTENDED
                && self.futex.swap(Self::CONTENDED, Ordering::Acquire) == Self::UNLOCKED
            {
                // We just swapped from UNLOCKED -> CONTENDED, which means we
                // took the lock.
                return MutexGuard { mutex: self };
            }

            // Wait on futex
            let _ = futex::wait(&self.futex, futex::Flags::PRIVATE, Self::CONTENDED, None);
        }
    }

    fn lock_contended_bitset(
        &self,
        bitset: u32,
        f: impl Fn() -> bool,
    ) -> Option<MutexGuard<'_, T>> {
        let bitset = NonZero::new(bitset).unwrap();
        loop {
            let state = self.spin_bitset(&f);

            // Upgrade the mutex to contended if it's not already.
            if state != Self::CONTENDED
                && self.futex.swap(Self::CONTENDED, Ordering::Acquire) == Self::UNLOCKED
            {
                // We just swapped from UNLOCKED -> CONTENDED, which means we
                // took the lock.
                return Some(MutexGuard { mutex: self });
            }

            if f() {
                return None;
            }

            // Wait on futex
            let _ = futex::wait_bitset(
                &self.futex,
                futex::Flags::PRIVATE,
                Self::CONTENDED,
                None,
                bitset,
            );
        }
    }

    pub(crate) fn spin(&self) -> u32 {
        let mut spin = 64;
        loop {
            let state = self.futex.load(Ordering::Relaxed);

            // We stop spinning if the mutex is either UNLOCKED or CONTENDED,
            // or if we've exhausted ourselves.
            if state != Self::LOCKED || spin == 0 {
                return state;
            }

            std::hint::spin_loop();
            spin -= 1;
        }
    }

    pub(crate) fn spin_bitset(&self, f: impl Fn() -> bool) -> u32 {
        let mut spin = 64;
        loop {
            let state = self.futex.load(Ordering::Relaxed);

            // We stop spinning if the condition in f() is true, mutex is
            // either UNLOCKED or CONTENDED, or if we've exhausted ourselves.
            if f() || state != Self::LOCKED || spin == 0 {
                return state;
            }

            std::hint::spin_loop();
            spin -= 1;
        }
    }

    fn unlock(&self) {
        let prev = self.futex.swap(Self::UNLOCKED, Ordering::Release);

        // If there were waiters, wake one
        if prev == Self::CONTENDED {
            let _ = futex::wake(&self.futex, futex::Flags::PRIVATE, 1);
        }
    }

    fn unlock_bitset(&self, bitset: u32) {
        let prev = self.futex.swap(Self::UNLOCKED, Ordering::Release);

        if let Some(bitset) = NonZero::new(bitset) {
            let _ = futex::wake_bitset(&self.futex, futex::Flags::PRIVATE, u32::MAX, bitset);
        }

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

impl<T> MutexGuard<'_, T> {
    pub fn unlock_with_bitset_wake(self, bitset: u32) {
        self.mutex.unlock_bitset(bitset);
        std::mem::forget(self);
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
