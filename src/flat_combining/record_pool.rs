use crossbeam_utils::CachePadded;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, Ordering};

/// A concurrent-safe pool of records with fixed capacity of 64 items.
/// Uses CPU-specific atomic operations and bit manipulation for high-performance allocation/deallocation.
/// All slots are pre-initialized with T::default() values.
pub struct RecordPool<T> {
    /// Storage for 64 pre-initialized items, cache-padded to reduce false sharing.
    items: [CachePadded<UnsafeCell<MaybeUninit<T>>>; 64],
    /// CPU-specific bitmasks tracking available slots. Each AtomicU8 tracks 8 slots.
    /// Array index corresponds to CPU affinity to reduce contention.
    available: [CachePadded<AtomicU8>; 8],
}

impl<T> RecordPool<T> {
    /// Creates a new RecordPool with all slots pre-initialized and available.
    pub fn new() -> Self {
        // Create array of 64 pre-initialized items wrapped in UnsafeCell and cache-padded
        let items =
            core::array::from_fn(|_| CachePadded::new(UnsafeCell::new(MaybeUninit::uninit())));

        // Initialize all atomic masks with all bits set (all slots available)
        let available = core::array::from_fn(|_| CachePadded::new(AtomicU8::new(u8::MAX)));

        Self { items, available }
    }

    /// Attempts to allocate a slot from the pool.
    /// Returns a reference to the initialized slot and its index if successful, None if pool is full.
    pub fn allocate(&self, init: impl pinned_init::Init<T>) -> Option<(&T, u8)> {
        // Get current CPU to determine which atomic to prefer
        let cpu_id = rustix::thread::sched_getcpu() % 8;

        // Use only the CPU-specific atomic
        let atomic_index = cpu_id;
        let atomic = &self.available[atomic_index];

        loop {
            let current = atomic.load(Ordering::Acquire);

            // If no bits are set in this atomic, no slots available
            if current == 0 {
                return None;
            }

            // Find the lowest set bit (first available slot within this atomic)
            let bit_position = current.trailing_zeros() as usize;
            let bit_mask = 1u8 << bit_position;

            // Calculate the real index in the items array
            let slot_index = atomic_index * 8 + bit_position;

            // Try to atomically claim this slot by clearing its bit
            let new_value = current & !bit_mask;

            match atomic.compare_exchange_weak(
                current,
                new_value,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Successfully claimed the slot
                    let item_ref = unsafe {
                        let unsafe_cell = self.items.get_unchecked(slot_index);
                        init.__init((&mut *(**unsafe_cell).get()).as_mut_ptr())
                            .unwrap();
                        (&*(**unsafe_cell).get()).assume_init_ref()
                    };
                    return Some((item_ref, slot_index as u8));
                }
                Err(_) => {
                    // Another thread modified this atomic, retry this atomic
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
        debug_assert!(index < 64, "Index {index} is out of bounds");

        let cell = &self.items[index as usize];
        unsafe {
            (&mut *(**cell).get()).assume_init_drop();
        }

        // Determine which atomic and bit position this index corresponds to
        let atomic_index = (index as usize) / 8;
        let bit_position = (index as usize) % 8;
        let bit_mask = 1u8 << bit_position;

        let atomic = &self.available[atomic_index];
        let previous = atomic.fetch_or(bit_mask, Ordering::AcqRel);

        // Check for double-free
        debug_assert!(
            (previous & bit_mask) == 0,
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
