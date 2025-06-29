use crate::flat_combining::{Mutex, MutexGuard, Waiter};
use rustix::thread::futex;
use std::sync::atomic::Ordering;

/// Result of waiting on either a mutex or waiter
pub enum WaitResult<'a, T> {
    /// The waiter was triggered/notified
    WaiterReady,
    /// The mutex was successfully locked
    MutexLocked(MutexGuard<'a, T>),
}

/// Wait for either the mutex to become unlocked or the waiter to be triggered.
/// Returns `WaitResult::MutexLocked` with the guard if the mutex was acquired,
/// or `WaitResult::WaiterReady` if the waiter was triggered.
pub fn wait_mutex_or_waiter<'a, T>(mutex: &'a Mutex<T>, waiter: &Waiter) -> WaitResult<'a, T> {
    // Create futex wait descriptors with expected values
    let mut wait1 = futex::Wait::new();
    wait1.val = Mutex::<T>::CONTENDED as u64;
    wait1.uaddr = futex::WaitPtr::new(mutex.futex() as *const _ as *mut _);
    wait1.flags = futex::WaitFlags::SIZE_U32 | futex::WaitFlags::PRIVATE;

    let mut wait2 = futex::Wait::new();
    wait2.val = Waiter::UNREADY as u64;
    wait2.uaddr = futex::WaitPtr::new(waiter.futex() as *const _ as *mut _);
    wait2.flags = futex::WaitFlags::SIZE_U32 | futex::WaitFlags::PRIVATE;

    let futexes = [wait1, wait2];

    loop {
        if waiter.is_ready() {
            return WaitResult::WaiterReady;
        }

        let mutex_value = mutex.futex().load(Ordering::Relaxed);
        if mutex_value == Mutex::<T>::UNLOCKED {
            if let Some(guard) = mutex.try_lock() {
                return WaitResult::MutexLocked(guard);
            } else {
                continue;
            }
        } else if mutex_value == Mutex::<T>::LOCKED {
            // Upgrade the mutex to contended.
            if mutex.futex().swap(Mutex::<T>::CONTENDED, Ordering::Acquire) == Mutex::<T>::UNLOCKED
            {
                // We just swapped from UNLOCKED -> CONTENDED, which means we
                // took the lock.
                let guard = MutexGuard { mutex };
                return WaitResult::MutexLocked(guard);
            }
        }

        match futex::waitv(
            &futexes,
            futex::WaitvFlags::empty(),
            None,
            futex::ClockId::Realtime,
        ) {
            Ok(index) => {
                if index == 0 {
                    // Mutex changed - try to lock it
                    if let Some(guard) = mutex.try_lock() {
                        return WaitResult::MutexLocked(guard);
                    }
                } else {
                    // Waiter was triggered. We need to verify its really ready
                    // because futex is allowed to cause spurious wakeups.
                    if waiter.is_ready() {
                        return WaitResult::WaiterReady;
                    }
                }
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat_combining::{Mutex, Waiter};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_wait_mutex_available() {
        let mutex = Mutex::new(42);
        let waiter = Waiter::new();

        // Mutex should be immediately available
        let result = wait_mutex_or_waiter(&mutex, &waiter);
        match result {
            WaitResult::MutexLocked(guard) => {
                assert_eq!(*guard, 42);
            }
            WaitResult::WaiterReady => panic!("Should have returned MutexLocked"),
        }
    }

    #[test]
    fn test_wait_waiter_triggered() {
        let mutex = Mutex::new(42);
        let waiter = Arc::new(Waiter::new());

        // Lock the mutex in another thread
        let _guard = mutex.lock();

        let waiter_clone = Arc::clone(&waiter);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            waiter_clone.notify();
        });

        // Should return WaiterReady when waiter is triggered
        let result = wait_mutex_or_waiter(&mutex, &waiter);
        match result {
            WaitResult::WaiterReady => {
                // Expected result
            }
            WaitResult::MutexLocked(_) => panic!("Should have returned WaiterReady"),
        }
    }
}
