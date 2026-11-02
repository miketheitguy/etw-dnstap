//! Bounded, eagerly allocated aligned storage with cross-thread lease recycling.

use anyhow::{Context, Result, ensure};
use crossbeam_queue::ArrayQueue;
use windows_sys::Win32::System::Diagnostics::Etw::EVENT_HEADER_EXTENDED_DATA_ITEM;

use super::MAX_EXTENDED_ITEMS;

pub(super) const REGULAR_BYTES: usize = 2048;
/// Bytes of one extended-data item in a slot's table: a header word and an
/// offset word.
pub(super) const ITEM_TABLE_BYTES: usize = 16;
// Two u16-sized payload regions plus one alignment gap of at most seven bytes per
// extended item and one more before the item table, then the table itself,
// rounded up to 8. copy_record admits at most 65_535 bytes of user data and
// 65_535 bytes of extended data, so its largest layout is
// 65_535 + 7 * MAX_EXTENDED_ITEMS + 7 + 65_535 + ITEM_TABLE_BYTES * MAX_EXTENDED_ITEMS
// <= JUMBO_BYTES.
pub(super) const JUMBO_BYTES: usize =
    (2 * 65_535 + (7 + ITEM_TABLE_BYTES) * MAX_EXTENDED_ITEMS + 7).next_multiple_of(8);
const _: () = {
    assert!(JUMBO_BYTES.is_multiple_of(8));
    assert!(JUMBO_BYTES >= 2 * 65_535 + (7 + ITEM_TABLE_BYTES) * MAX_EXTENDED_ITEMS + 7);
    assert!(size_of::<EVENT_HEADER_EXTENDED_DATA_ITEM>() == 16);
    assert!(REGULAR_BYTES >= ITEM_TABLE_BYTES * MAX_EXTENDED_ITEMS);
};
pub(crate) const MAX_CAPACITY: usize = 262_144;
/// Smallest capacity accepted on the command line (`PoolManager::new` accepts 1).
pub(crate) const MIN_CAPACITY: usize = 1_024;
/// Largest `--jumbo-capacity`; the command line and `PoolManager::new` both enforce it.
pub(crate) const MAX_JUMBO_CAPACITY: u16 = 256;

/// A slot's header. The bytes hold the payload at the front and, for a record
/// with extended data, one `ITEM_TABLE_BYTES` cell per item at the very end:
/// item `i` occupies the `i`th 16-byte cell counted from the end, so the cells
/// need no count to find. This keeps the 384 bytes of per-item tables out of
/// every slot header, where this provider (which sends no extended data)
/// never used them. `copy_record` asks `acquire` for room for the payload, its
/// alignment padding, and the cells, so the two regions cannot overlap.
struct Slot {
    bytes: Box<[u64]>,
    jumbo: bool,
}

/// The four `u16` fields of an item that precede `DataPtr`, packed little-endian.
/// `DataPtr` is not stored: `copy_record` zeroes it, and `with_record` repairs it
/// from the offset.
fn pack_item(item: &EVENT_HEADER_EXTENDED_DATA_ITEM) -> u64 {
    let [r0, r1] = item.Reserved1.to_le_bytes();
    let [t0, t1] = item.ExtType.to_le_bytes();
    let [b0, b1] = item.Anonymous._bitfield.to_le_bytes();
    let [s0, s1] = item.DataSize.to_le_bytes();
    u64::from_le_bytes([r0, r1, t0, t1, b0, b1, s0, s1])
}

/// The inverse of [`pack_item`], with `DataPtr` zero.
fn unpack_item(word: u64) -> EVENT_HEADER_EXTENDED_DATA_ITEM {
    let [r0, r1, t0, t1, b0, b1, s0, s1] = word.to_le_bytes();
    let mut item = EVENT_HEADER_EXTENDED_DATA_ITEM {
        Reserved1: u16::from_le_bytes([r0, r1]),
        ExtType: u16::from_le_bytes([t0, t1]),
        DataSize: u16::from_le_bytes([s0, s1]),
        ..Default::default()
    };
    item.Anonymous._bitfield = u16::from_le_bytes([b0, b1]);
    item
}

/// Allocated before callbacks; slots move between exactly one free list and lease.
///
/// A pool is never freed. [`PoolManager::with_progress`] leaks it once, at
/// startup, and returns a `&'static` reference: the pool lives for the process
/// (the service exits after the pipeline ends), and a lease then needs no
/// reference count, which would cost two atomic read-modify-writes per event on
/// a cache line shared by the ETW and parser threads. A test that builds a pool
/// leaks it; the test pools are a few slots each.
pub(crate) struct PoolManager {
    regular: ArrayQueue<Box<Slot>>,
    jumbo: Option<ArrayQueue<Box<Slot>>>,
}

impl PoolManager {
    /// Slots allocated between two progress reports. The startup budget in
    /// `pipeline::startup_budget` allows one second per interval, so a report
    /// arrives at least once per budgeted second of allocation.
    pub(crate) const PROGRESS_INTERVAL: usize = 16_384;

    /// Allocates a pool without progress reports. The pipeline always reports
    /// progress, so this constructor serves the tests.
    #[cfg(test)]
    pub(crate) fn new(capacity: usize, jumbo_capacity: usize) -> Result<&'static Self> {
        Self::with_progress(capacity, jumbo_capacity, || Ok(()))
    }

    /// Allocates and touches every slot before any callback can run, calling
    /// `progress` after each [`Self::PROGRESS_INTERVAL`] slots (regular and jumbo
    /// counted together). An error from `progress` stops the allocation and frees
    /// the slots allocated so far. On success the pool is leaked and lives for the
    /// rest of the process; call this once per pipeline run.
    pub(crate) fn with_progress(
        capacity: usize,
        jumbo_capacity: usize,
        progress: impl FnMut() -> Result<()>,
    ) -> Result<&'static Self> {
        Self::with_progress_interval(capacity, jumbo_capacity, Self::PROGRESS_INTERVAL, progress)
    }

    fn with_progress_interval(
        capacity: usize,
        jumbo_capacity: usize,
        interval: usize,
        mut progress: impl FnMut() -> Result<()>,
    ) -> Result<&'static Self> {
        ensure!(
            (1..=MAX_CAPACITY).contains(&capacity),
            "invalid channel capacity"
        );
        ensure!(
            jumbo_capacity <= usize::from(MAX_JUMBO_CAPACITY),
            "invalid jumbo capacity"
        );
        ensure!(interval != 0, "invalid progress interval");
        let pool = Self {
            regular: ArrayQueue::new(capacity + 2),
            jumbo: (jumbo_capacity != 0).then(|| ArrayQueue::new(jumbo_capacity)),
        };
        let regular = (0..capacity + 2).map(|_| (REGULAR_BYTES, false));
        let jumbo = (0..jumbo_capacity).map(|_| (JUMBO_BYTES, true));
        for (allocated, (size, is_jumbo)) in (1..).zip(regular.chain(jumbo)) {
            pool.add_slot(size, is_jumbo)?;
            if allocated % interval == 0 {
                progress()?;
            }
        }
        // Leaked only after every slot is allocated, so a failed start frees them.
        Ok(Box::leak(Box::new(pool)))
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
            jumbo,
        }));
        Ok(())
    }

    pub(super) fn acquire(&'static self, length: usize) -> Option<PooledBuffer> {
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
            pool: self,
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
    pool: &'static PoolManager,
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
    /// Index of the header word of item `index` in the slot's words; its offset
    /// word follows. Counted from the end, see [`Slot`]. Aborts for an index at
    /// or beyond `MAX_EXTENDED_ITEMS`, which no caller passes.
    fn cell(&self, index: usize) -> usize {
        if index >= MAX_EXTENDED_ITEMS {
            std::process::abort();
        }
        self.slot().bytes.len() - 2 * (index + 1)
    }

    /// Stores item `index` and the payload offset of its data in the table at the
    /// end of the slot. The caller reserved the cell through `acquire`.
    pub(super) fn set_item(
        &mut self,
        index: usize,
        item: &EVENT_HEADER_EXTENDED_DATA_ITEM,
        offset: usize,
    ) {
        let cell = self.cell(index);
        let Ok(offset) = u64::try_from(offset) else {
            std::process::abort();
        };
        let words = &mut self.slot_mut().bytes;
        words[cell] = pack_item(item);
        words[cell + 1] = offset;
    }

    /// Item `index`, with `DataPtr` zero. Valid for `index <` the count passed to
    /// `set_item`; other cells hold stale or zero words.
    pub(super) fn item(&self, index: usize) -> EVENT_HEADER_EXTENDED_DATA_ITEM {
        unpack_item(self.slot().bytes[self.cell(index)])
    }

    pub(super) fn offset(&self, index: usize) -> usize {
        let Ok(offset) = usize::try_from(self.slot().bytes[self.cell(index) + 1]) else {
            std::process::abort();
        };
        offset
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
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
        thread,
    };

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
        assert_eq!(std::mem::size_of::<Slot>(), 24);
        assert_eq!(REGULAR_BYTES, 2048);
        assert_eq!(JUMBO_BYTES, 131_448);
        assert_eq!(std::mem::size_of::<super::super::RawEtwEvent>(), 104);
    }

    #[test]
    fn item_cells_round_trip_at_the_end_of_the_slot() {
        let pool = PoolManager::new(1, 1).expect("bounded test pool");
        for length in [REGULAR_BYTES, JUMBO_BYTES] {
            let mut lease = pool.acquire(length).expect("slot");
            for index in 0..MAX_EXTENDED_ITEMS {
                let value = u16::try_from(index).expect("small index");
                let mut item = EVENT_HEADER_EXTENDED_DATA_ITEM {
                    Reserved1: 0x1000 + value,
                    ExtType: 0x2000 + value,
                    DataSize: 0x3000 + value,
                    DataPtr: 0xdead_beef,
                    ..Default::default()
                };
                item.Anonymous._bitfield = 0x4000 + value;
                lease.set_item(index, &item, 8 * (index + 1));
            }
            for index in 0..MAX_EXTENDED_ITEMS {
                let value = u16::try_from(index).expect("small index");
                let item = lease.item(index);
                assert_eq!(item.Reserved1, 0x1000 + value);
                assert_eq!(item.ExtType, 0x2000 + value);
                assert_eq!(item.Anonymous._bitfield, 0x4000 + value);
                assert_eq!(item.DataSize, 0x3000 + value);
                assert_eq!(item.DataPtr, 0, "DataPtr is not stored");
                assert_eq!(lease.offset(index), 8 * (index + 1));
            }
            // The cells are the last 16 * MAX_EXTENDED_ITEMS bytes; the rest of
            // the slot is untouched (zero on a fresh slot).
            let payload = length - ITEM_TABLE_BYTES * MAX_EXTENDED_ITEMS;
            // SAFETY: The exclusive lease owns `length` initialized bytes.
            let bytes = unsafe { std::slice::from_raw_parts(lease.bytes(), length) };
            assert!(bytes[..payload].iter().all(|byte| *byte == 0));
            assert!(bytes[payload..].iter().any(|byte| *byte != 0));
        }
    }

    /// Allocates with `interval` and returns how often progress was reported.
    fn progress_reports(capacity: usize, jumbo: usize, interval: usize) -> usize {
        let mut reports = 0;
        let pool = PoolManager::with_progress_interval(capacity, jumbo, interval, || {
            reports += 1;
            Ok(())
        })
        .expect("bounded test pool");
        assert!(pool.acquire(1).is_some());
        reports
    }

    #[test]
    fn progress_is_reported_once_per_interval_over_regular_and_jumbo_slots() {
        // capacity + 2 regular slots plus the jumbo slots, integer-divided.
        assert_eq!(progress_reports(1, 0, 4), 0);
        assert_eq!(progress_reports(2, 0, 4), 1);
        assert_eq!(progress_reports(6, 0, 4), 2);
        assert_eq!(progress_reports(6, 4, 4), 3);
        assert_eq!(progress_reports(5, 1, 4), 2);
        assert_eq!(progress_reports(1, 1, 1), 4);
    }

    #[test]
    fn new_reports_no_progress_below_the_interval_and_once_at_it() {
        let mut reports = 0;
        drop(PoolManager::with_progress(1, 1, || {
            reports += 1;
            Ok(())
        }));
        assert_eq!(reports, 0);
        // Exactly PROGRESS_INTERVAL slots: capacity + 2 regular, no jumbo.
        let mut reports = 0;
        drop(PoolManager::with_progress(
            PoolManager::PROGRESS_INTERVAL - 2,
            0,
            || {
                reports += 1;
                Ok(())
            },
        ));
        assert_eq!(reports, 1);
    }

    #[test]
    fn progress_matches_the_startup_budget_at_the_maximum_capacity() {
        // 262_144 + 2 regular slots and 256 jumbo slots: 16 reports, one per
        // budgeted second (the budget allows 16 s of allocation at this size).
        let slots = MAX_CAPACITY + 2 + usize::from(MAX_JUMBO_CAPACITY);
        assert_eq!(slots / PoolManager::PROGRESS_INTERVAL, 16);
        assert_eq!(MAX_CAPACITY.div_ceil(PoolManager::PROGRESS_INTERVAL), 16);
    }

    #[test]
    fn a_failed_progress_report_stops_the_allocation() {
        let mut reports = 0;
        let error = PoolManager::with_progress_interval(6, 0, 2, || {
            reports += 1;
            if reports == 2 {
                anyhow::bail!("status link lost")
            }
            Ok(())
        })
        .err()
        .expect("allocation stops at the failed report");
        assert_eq!(reports, 2);
        assert!(error.to_string().contains("status link lost"));
    }

    #[test]
    fn a_zero_progress_interval_is_rejected() {
        assert!(PoolManager::with_progress_interval(1, 0, 0, || Ok(())).is_err());
    }
}
