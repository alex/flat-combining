mod erased_fn;
mod mutex;
mod record_pool;
mod waiter;
mod waitv;

use self::erased_fn::ErasedFn;
use self::mutex::{Mutex, MutexGuard};
use self::record_pool::RecordPool;
use self::waiter::Waiter;
use std::any::Any;
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

#[pinned_init::pin_data]
struct OpRecord<T> {
    f: UnsafeCell<Option<ErasedFn<T>>>,
    panic: UnsafeCell<Option<Box<dyn Any + Send + 'static>>>,
    waiter: Waiter,

    next: AtomicPtr<Self>,
}

pub struct FlatCombining<T> {
    data: Mutex<T>,
    pool: RecordPool<OpRecord<T>>,

    head: AtomicPtr<OpRecord<T>>,
}

impl<T> FlatCombining<T> {
    unsafe fn push_head(&self, record: *mut OpRecord<T>) {
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

    fn run_combiner(&self, value: &mut T) {
        // Grab the head, replacing it with null.
        let mut head = loop {
            let p = self.head.load(Ordering::Acquire);
            match self.head.compare_exchange_weak(
                p,
                ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(p) => break p,
                Err(_) => continue,
            };
        };

        while !head.is_null() {
            let record = unsafe { &*head };

            let f = unsafe { (*record.f.get()).take().unwrap() };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                f.invoke(value);
            }));
            unsafe {
                (*record.panic.get()) = result.err();
            }
            head = record.next.load(Ordering::Acquire);
            record.waiter.notify();
        }
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
            self.run_combiner(&mut *guard);
            f(&mut *guard);
            return;
        }

        let cb = ErasedFn::new(&f);
        let init = pinned_init::init!(OpRecord {
            f: UnsafeCell::new(Some(cb)),
            panic: UnsafeCell::new(None),
            waiter: Waiter::new(),

            next: AtomicPtr::new(ptr::null_mut()),
        });
        if let Some((record, idx)) = self.pool.allocate(init) {
            // SAFETY: pointer is valid, we just got it from the pool.
            unsafe {
                self.push_head(record as *const _ as _);
            }
            match waitv::wait_mutex_or_waiter(&self.data, &record.waiter) {
                // Waiter triggered, therefore someone else ran us.
                waitv::WaitResult::WaiterReady => {}
                // We got the mutex before our waiter triggered, we are the
                // combiner.
                waitv::WaitResult::MutexLocked(mut guard) => {
                    self.run_combiner(&mut *guard);
                }
            }
            // At this point the record should be ready, all we need to do is
            // check the panic state.
            assert!(record.waiter.is_ready());
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
        self.run_combiner(&mut *guard);
        f(&mut *guard);
    }
}

unsafe impl<T: Send> Send for FlatCombining<T> {}
unsafe impl<T: Send> Sync for FlatCombining<T> {}

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
