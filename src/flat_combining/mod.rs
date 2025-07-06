mod erased_fn;
mod mutex;
mod record_pool;

use self::erased_fn::ErasedFn;
use self::mutex::Mutex;
use self::record_pool::RecordPool;
use std::any::Any;
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

#[pinned_init::pin_data]
struct OpRecord {
    idx: UnsafeCell<u8>,
    f: UnsafeCell<Option<ErasedFn>>,
    panic: UnsafeCell<Option<Box<dyn Any + Send + 'static>>>,
    waiter: AtomicBool,

    next: AtomicPtr<Self>,
}

pub struct FlatCombining<T> {
    data: Mutex<T>,
    pool: RecordPool<OpRecord>,

    head: AtomicPtr<OpRecord>,
}

impl<T> FlatCombining<T> {
    unsafe fn push_head(&self, record: *mut OpRecord) {
        loop {
            let head = self.head.load(Ordering::Acquire);
            (*record).next.store(head, Ordering::Relaxed);

            if self
                .head
                .compare_exchange_weak(head, record, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
    }

    #[must_use]
    fn run_combiner(&self, value: &mut T) -> u32 {
        // Grab the head, replacing it with null.
        let mut head = self.head.swap(ptr::null_mut(), Ordering::AcqRel);

        let mut bitset = 0;
        while !head.is_null() {
            let record = unsafe { &*head };

            let f = unsafe { (*record.f.get()).take().unwrap() };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                unsafe { f.invoke(value) };
            }));
            unsafe {
                (*record.panic.get()) = result.err();
            }
            bitset |= idx_to_bitset(unsafe { *record.idx.get() });
            head = record.next.load(Ordering::Acquire);
            record.waiter.store(true, Ordering::Release);
        }

        bitset
    }
}

impl<T> crate::Mutator<T> for FlatCombining<T> {
    fn new(value: T) -> Self {
        FlatCombining {
            data: Mutex::new(value),
            pool: RecordPool::new(),

            head: AtomicPtr::new(ptr::null_mut()),
        }
    }

    fn mutate<F: FnOnce(&mut T) + Send>(&self, f: F) {
        if let Some(mut guard) = self.data.try_lock() {
            let bitset = self.run_combiner(&mut *guard);
            f(&mut *guard);
            guard.unlock_with_bitset_wake(bitset);
            return;
        }

        let cb = ErasedFn::new(&f);
        let init = pinned_init::init!(OpRecord {
            idx: UnsafeCell::new(0),
            f: UnsafeCell::new(Some(cb)),
            panic: UnsafeCell::new(None),
            waiter: AtomicBool::new(false),

            next: AtomicPtr::new(ptr::null_mut()),
        });
        if let Some((record, idx)) = self.pool.allocate(init) {
            // `f` is now owned by the `OpRecord`.
            std::mem::forget(f);
            // SAFETY: no one else has a reference to the `OpRecord`
            unsafe {
                *record.idx.get() = idx;
            }
            // SAFETY: pointer is valid, we just got it from the pool.
            unsafe {
                self.push_head(record as *const _ as _);
            }

            if let Some(mut guard) = self
                .data
                .bitset_wait_or_lock(idx_to_bitset(idx), || record.waiter.load(Ordering::Acquire))
            {
                // This means we got the lock.
                let bitset = self.run_combiner(&mut *guard);
                guard.unlock_with_bitset_wake(bitset);
            }

            // At this point the record should be ready, all we need to do is
            // check the panic state.
            assert!(record.waiter.load(Ordering::Relaxed));
            // SAFETY: we were marked as ready (or ran), therefore we're now
            // the only thread with access to the record.
            let panic_state = unsafe { (*record.panic.get()).take() };
            unsafe {
                self.pool.free(idx);
            }
            if let Some(p) = panic_state {
                std::panic::resume_unwind(p);
            }
            return;
        }
        // No records left in the pool, we just need to wait on the mutex.
        let mut guard = self.data.lock();
        let bitset = self.run_combiner(&mut *guard);
        f(&mut *guard);
        guard.unlock_with_bitset_wake(bitset);
    }
}

unsafe impl<T: Send> Send for FlatCombining<T> {}
unsafe impl<T: Send> Sync for FlatCombining<T> {}

fn idx_to_bitset(idx: u8) -> u32 {
    let compressed = idx % 32;
    1 << compressed
}

#[cfg(test)]
mod tests {
    use crate::{FlatCombining, Mutator};

    #[test]
    fn test_uncontended() {
        let m = FlatCombining::new(0);
        m.mutate(|v| {
            assert_eq!(*v, 0);
            *v += 1;
        });
        m.mutate(|v| {
            assert_eq!(*v, 1);
            *v += 1;
        });
        m.mutate(|v| {
            assert_eq!(*v, 2);
            *v += 1;
        });
    }

    #[test]
    fn test_contended() {
        const N_THREADS: usize = 32;
        const N_OPS: usize = 128;

        let m = FlatCombining::new(0);
        let b = std::sync::Barrier::new(N_THREADS);

        std::thread::scope(|s| {
            for _ in 0..N_THREADS {
                s.spawn(|| {
                    b.wait();

                    for _ in 0..N_OPS {
                        m.mutate(|v| *v += 1);
                    }
                });
            }
        });

        m.mutate(|v| {
            assert_eq!(*v, N_THREADS * N_OPS);
        });
    }
}
