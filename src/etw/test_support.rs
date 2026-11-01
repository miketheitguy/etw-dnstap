//! Test-only helpers shared by the ETW module tests: an allocation-counting
//! global allocator, common event-ID fixtures, and small conversion helpers.

/// Converts a test offset or length to the `u32` the native structures use.
pub(super) fn offset_u32(offset: usize) -> u32 {
    u32::try_from(offset).expect("test offsets fit u32")
}

/// UTF-16 code units of `text` followed by a NUL.
pub(super) fn utf16_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Little-endian bytes of [`utf16_nul`], as ETW stores UTF-16 strings.
pub(super) fn utf16_nul_bytes(text: &str) -> Vec<u8> {
    utf16_nul(text)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AllocCounts {
    pub(super) allocations: u64,
    pub(super) reallocations: u64,
    pub(super) deallocations: u64,
}

thread_local! {
    static ALLOC_COUNTS: std::cell::Cell<AllocCounts> = const { std::cell::Cell::new(AllocCounts { allocations: 0, reallocations: 0, deallocations: 0 }) };
    static TRACK_ALLOCATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct TrackingAllocator;

fn allocation_tick(kind: u8) {
    if TRACK_ALLOCATIONS
        .try_with(std::cell::Cell::get)
        .unwrap_or(false)
    {
        let _updated = ALLOC_COUNTS.try_with(|counts| {
            let mut value = counts.get();
            match kind {
                0 => value.allocations += 1,
                1 => value.reallocations += 1,
                _ => value.deallocations += 1,
            }
            counts.set(value);
        });
    }
}

// SAFETY: Every allocation/deallocation is forwarded unchanged to System.
// Thread-local counter updates allocate no storage and retain no pointers.
unsafe impl std::alloc::GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        allocation_tick(0);
        // SAFETY: The allocator caller supplies a valid layout.
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        allocation_tick(0);
        // SAFETY: The allocator caller supplies a valid layout.
        unsafe { std::alloc::GlobalAlloc::alloc_zeroed(&std::alloc::System, layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        allocation_tick(2);
        // SAFETY: Caller supplies the matching live System allocation/layout.
        unsafe {
            std::alloc::GlobalAlloc::dealloc(&std::alloc::System, pointer, layout);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        allocation_tick(1);
        // SAFETY: Caller supplies the live allocation and a valid new size.
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

struct TrackingGuard;

impl Drop for TrackingGuard {
    fn drop(&mut self) {
        TRACK_ALLOCATIONS.with(|track| track.set(false));
    }
}

pub(super) fn measured(operation: impl FnOnce()) -> AllocCounts {
    ALLOC_COUNTS.with(|counts| counts.set(AllocCounts::default()));
    TRACK_ALLOCATIONS.with(|track| track.set(true));
    let guard = TrackingGuard;
    operation();
    drop(guard);
    ALLOC_COUNTS.with(std::cell::Cell::get)
}

pub(crate) fn assert_no_allocations(operation: impl FnOnce()) {
    let counts = measured(operation);
    assert_eq!(
        counts,
        AllocCounts::default(),
        "unexpected allocator activity"
    );
}

/// Event IDs the callback and parser must ignore.
pub(super) const EXCLUDED_EVENT_IDS: [u16; 9] = [0, 1, 255, 258, 259, 260, 261, 262, u16::MAX];
