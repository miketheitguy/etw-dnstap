//! Bounded, eagerly allocated aligned storage with cross-thread lease recycling.

use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use crossbeam_queue::ArrayQueue;
use windows_sys::Win32::System::Diagnostics::Etw::EVENT_HEADER_EXTENDED_DATA_ITEM;

use super::MAX_EXTENDED_ITEMS;

pub(super) const REGULAR_BYTES: usize = 2048;
// Two u16-sized payload regions plus one alignment gap of at most seven bytes per
// extended item, rounded up to 8. copy_record admits at most 65_535 bytes of
// user data and 65_535 bytes of extended data, so its largest layout is
// 65_535 + 7 * MAX_EXTENDED_ITEMS + 65_535 <= JUMBO_BYTES.
pub(super) const JUMBO_BYTES: usize = (2 * 65_535 + 7 * MAX_EXTENDED_ITEMS).next_multiple_of(8);
const _: () = {
    assert!(JUMBO_BYTES.is_multiple_of(8));
    assert!(JUMBO_BYTES >= 2 * 65_535 + 7 * MAX_EXTENDED_ITEMS);
};
pub(crate) const MAX_CAPACITY: usize = 262_144;
/// Smallest capacity accepted on the command line (`PoolManager::new` accepts 1).
pub(crate) const MIN_CAPACITY: usize = 1_024;
/// Largest `--jumbo-capacity`; the command line and `PoolManager::new` both enforce it.
pub(crate) const MAX_JUMBO_CAPACITY: u16 = 256;

struct Slot {
    bytes: Box<[u64]>,
    items: [EVENT_HEADER_EXTENDED_DATA_ITEM; MAX_EXTENDED_ITEMS],
    offsets: [usize; MAX_EXTENDED_ITEMS],
    jumbo: bool,
}

/// Allocated before callbacks; slots move between exactly one free list and lease.
pub(crate) struct PoolManager {
    regular: ArrayQueue<Box<Slot>>,
    jumbo: Option<ArrayQueue<Box<Slot>>>,
}

impl PoolManager {
    pub(crate) fn new(capacity: usize, jumbo_capacity: usize) -> Result<Arc<Self>> {
        ensure!(
            (1..=MAX_CAPACITY).contains(&capacity),
            "invalid channel capacity"
        );
        ensure!(
            jumbo_capacity <= usize::from(MAX_JUMBO_CAPACITY),
            "invalid jumbo capacity"
        );
        let pool = Arc::new(Self {
            regular: ArrayQueue::new(capacity + 2),
            jumbo: (jumbo_capacity != 0).then(|| ArrayQueue::new(jumbo_capacity)),
        });
        for _ in 0..capacity + 2 {
            pool.add_slot(REGULAR_BYTES, false)?;
        }
        for _ in 0..jumbo_capacity {
            pool.add_slot(JUMBO_BYTES, true)?;
        }
        Ok(pool)
    }

    fn add_slot(&self, size: usize, jumbo: bool) -> Result<()> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size / 8)
            .context("reserve ETW pool storage")?;
        // Touch and initialize every page outside the native callback.
        bytes.resize(size / 8, 0u64);
        self.recycle(Box::new(Slot {
            bytes: bytes.into_boxed_slice(),
            items: [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS],
            offsets: [0; MAX_EXTENDED_ITEMS],
            jumbo,
        }));
        Ok(())
    }

    pub(super) fn acquire(self: &Arc<Self>, length: usize) -> Option<PooledBuffer> {
        // Regular events do not consume the jumbo reserve. Pool exhaustion is
        // explicit; neither branch allocates, waits, or grows a free list.
        let slot = if length <= REGULAR_BYTES {
            self.regular.pop()
        } else if length <= JUMBO_BYTES {
            self.jumbo.as_ref()?.pop()
        } else {
            None
        }?;
        Some(PooledBuffer {
            slot: Some(slot),
            pool: Arc::clone(self),
        })
    }

    fn recycle(&self, slot: Box<Slot>) {
        let queue = if slot.jumbo {
            self.jumbo.as_ref()
        } else {
            Some(&self.regular)
        };
        // Slot is private and never cloned. A class contains exactly N slots,
        // and its free queue has capacity N. While recycling one owned slot,
        // at most N-1 can be free, including concurrent returns. Full/missing
        // therefore indicates a programming invariant failure, not overload.
        if queue.is_none_or(|queue| queue.push(slot).is_err()) {
            std::process::abort();
        }
    }
}

/// Owns a unique slot; no ETW pointer or lifetime extension is stored here.
pub(super) struct PooledBuffer {
    slot: Option<Box<Slot>>,
    pool: Arc<PoolManager>,
}

impl PooledBuffer {
    fn slot(&self) -> &Slot {
        match self.slot.as_deref() {
            Some(slot) => slot,
            None => std::process::abort(),
        }
    }

    fn slot_mut(&mut self) -> &mut Slot {
        match self.slot.as_deref_mut() {
            Some(slot) => slot,
            None => std::process::abort(),
        }
    }

    pub(super) fn bytes(&self) -> *const u8 {
        self.slot().bytes.as_ptr().cast()
    }
    pub(super) fn bytes_mut(&mut self) -> *mut u8 {
        self.slot_mut().bytes.as_mut_ptr().cast()
    }
    pub(super) fn items(&self) -> &[EVENT_HEADER_EXTENDED_DATA_ITEM; MAX_EXTENDED_ITEMS] {
        &self.slot().items
    }
    pub(super) fn set_item(
        &mut self,
        index: usize,
        item: EVENT_HEADER_EXTENDED_DATA_ITEM,
        offset: usize,
    ) {
        self.slot_mut().items[index] = item;
        self.slot_mut().offsets[index] = offset;
    }
    pub(super) fn offset(&self, index: usize) -> usize {
        self.slot().offsets[index]
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            self.pool.recycle(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Mutex, thread};

    use super::*;

    #[test]
    fn concurrent_leases_are_exclusive_and_all_slots_return() {
        let pool = PoolManager::new(4, 1).expect("bounded test pool");
        let initial: Vec<_> = (0..6)
            .map(|_| pool.acquire(1).expect("regular slot"))
            .collect();
        let addresses: HashSet<_> = initial.iter().map(|lease| lease.bytes() as usize).collect();
        assert_eq!(addresses.len(), 6);
        drop(initial);
        let active = Arc::new(Mutex::new(HashSet::new()));
        let workers: Vec<_> = (0..4u8)
            .map(|worker| {
                let pool = Arc::clone(&pool);
                let active = Arc::clone(&active);
                thread::spawn(move || {
                    let mut acquired = 0;
                    for iteration in 0..300u16 {
                        let Some(mut lease) = pool.acquire(REGULAR_BYTES) else {
                            thread::yield_now();
                            continue;
                        };
                        let address = lease.bytes() as usize;
                        assert!(
                            active.lock().expect("active set").insert(address),
                            "aliased lease"
                        );
                        let sentinel = worker.wrapping_add(iteration.to_le_bytes()[0]);
                        // SAFETY: This exclusive lease owns REGULAR_BYTES initialized
                        // bytes. Other threads can only obtain different slots.
                        unsafe {
                            std::ptr::write_bytes(lease.bytes_mut(), sentinel, REGULAR_BYTES);
                        }
                        thread::yield_now();
                        // SAFETY: The slot remains exclusively leased through this read.
                        let bytes =
                            unsafe { std::slice::from_raw_parts(lease.bytes(), REGULAR_BYTES) };
                        assert!(bytes.iter().all(|byte| *byte == sentinel));
                        assert!(active.lock().expect("active set").remove(&address));
                        drop(lease);
                        acquired += 1;
                    }
                    acquired
                })
            })
            .collect();
        for worker in workers {
            assert!(worker.join().expect("pool worker completes") > 0);
        }
        assert!(active.lock().expect("active set").is_empty());
        let recovered: Vec<_> = (0..6)
            .map(|_| pool.acquire(1).expect("every slot returned"))
            .collect();
        assert!(pool.acquire(1).is_none());
        let recovered_addresses: HashSet<_> = recovered
            .iter()
            .map(|lease| lease.bytes() as usize)
            .collect();
        assert_eq!(addresses, recovered_addresses);
        assert!(pool.acquire(JUMBO_BYTES).is_some());
    }

    #[test]
    fn x64_storage_sizes_match_documented_budget() {
        assert_eq!(std::mem::size_of::<Slot>(), 408);
        assert_eq!(std::mem::size_of::<super::super::RawEtwEvent>(), 104);
    }
}
