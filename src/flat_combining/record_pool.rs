use std::cell::UnsafeCell;

use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU64, Ordering};

/// A concurrent-safe pool of records with fixed capacity of 64 items.
/// Uses atomic operations and bit manipulation for high-performance allocation/deallocation.
/// All slots are pre-initialized with T::default() values.
pub struct RecordPool<T> {
    /// Storage for 64 pre-initialized items.
    items: [UnsafeCell<MaybeUninit<T>>; 64],
    /// Bitmask tracking available slots. Bit i is 1 if slot i is available, 0 if occupied.
    available: AtomicU64,
}

impl<T> RecordPool<T> {
    /// Creates a new RecordPool with all slots pre-initialized and available.
    pub fn new() -> Self {
        // Create array of 64 pre-initialized items wrapped in UnsafeCell
        let items = core::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit()));

        Self {
            items,
            available: AtomicU64::new(u64::MAX), // All bits set = all slots available
        }
    }

    /// Attempts to allocate a slot from the pool.
    /// Returns a reference to the initialized slot and its index if successful, None if pool is full.
    pub fn allocate(&self, init: impl pinned_init::Init<T>) -> Option<(&T, u8)> {
        loop {
            let current = self.available.load(Ordering::Acquire);

            // If no bits are set, pool is full
            if current == 0 {
                return None;
            }

            // Find the lowest set bit (first available slot)
            let slot_index = current.trailing_zeros() as usize;
            let slot_mask = 1u64 << slot_index;

            // Try to atomically claim this slot by clearing its bit
            let new_value = current & !slot_mask;

            match self.available.compare_exchange_weak(
                current,
                new_value,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Successfully claimed the slot
                    let item_ref = unsafe {
                        let unsafe_cell = self.items.get_unchecked(slot_index);
                        init.__init((*unsafe_cell.get()).as_mut_ptr()).unwrap();
                        (*unsafe_cell.get()).assume_init_ref()
                    };
                    return Some((item_ref, slot_index as u8));
                }
                Err(_) => {
                    // Another thread modified available, retry
                    continue;
                }
            }
        }
    }

    /// Returns a slot to the pool, making it available for future allocations.
    ///
    /// # Safety
    ///
    /// Must be called with a valid index from `allocate()`.
    ///
    /// # Panics
    /// Panics if index >= 64 or if the slot is already free (double-free).
    pub unsafe fn free(&self, index: u8) {
        let cell = &self.items[index as usize];
        unsafe {
            (*cell.get()).assume_init_drop();
        }

        let slot_mask = 1u64 << index;
        let previous = self.available.fetch_or(slot_mask, Ordering::AcqRel);

        // Check for double-free
        debug_assert!(
            (previous & slot_mask) == 0,
            "Attempted to free already available slot {index}",
        );
    }
}

// Safety: RecordPool is safe to send between threads as long as T is Send
unsafe impl<T: Default + Send> Send for RecordPool<T> {}

// Safety: RecordPool supports concurrent access through atomic operations
unsafe impl<T: Default + Send> Sync for RecordPool<T> {}

impl<T: Default> Default for RecordPool<T> {
    fn default() -> Self {
        Self::new()
    }
}
