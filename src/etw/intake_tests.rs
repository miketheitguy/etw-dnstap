use super::*;
use crate::{
    dnstap::Encoder,
    etw::{
        PROVIDER, pool,
        test_support::{AllocCounts, EXCLUDED_EVENT_IDS, assert_no_allocations, measured},
    },
    event::{DnsEventMetadata, Transport},
};
use anyhow::{Result, anyhow, bail};
use std::{
    sync::atomic::{AtomicBool, AtomicU64},
    thread,
    time::{Duration, UNIX_EPOCH},
};
use windows_sys::Win32::System::Diagnostics::Etw::EVENT_DESCRIPTOR;
use windows_sys::core::GUID;

#[test]
fn supported_accepts_only_event_ids_256_and_257() {
    for id in 0..=u16::MAX {
        assert_eq!(supported(id), id == 256 || id == 257, "event id {id}");
    }
}

fn allocator_fixtures() -> Vec<(&'static str, AllocCounts)> {
    let mut results = Vec::new();
    let mut fixture = Fixture::new(1, 1);
    let mut payload = [0x55u8; 512];
    let extra = [0x33u8; 17];
    let mut item = EVENT_HEADER_EXTENDED_DATA_ITEM {
        DataPtr: extra.as_ptr() as u64,
        DataSize: 17,
        ..Default::default()
    };
    fixture.record.UserData = payload.as_mut_ptr().cast();
    fixture.record.UserDataLength = 512;
    fixture.record.ExtendedData = &raw mut item;
    fixture.record.ExtendedDataCount = 1;
    results.push(("success", measured(|| fixture.invoke())));
    results.push(("channel_full", measured(|| fixture.invoke())));
    let event = fixture.receive.try_recv().expect("accepted event");
    let pool = Arc::clone(&fixture.context.pool);
    let recycled = thread::spawn(move || measured(|| drop(event)))
        .join()
        .expect("recycler joined");
    results.push(("recycle", recycled));
    let held: Vec<_> = (0..3)
        .map(|_| pool.acquire(1).expect("regular lease"))
        .collect();
    results.push(("pool_exhausted", measured(|| fixture.invoke())));
    drop(held);
    let mut jumbo_payload = vec![0x77u8; 4096];
    fixture.record.UserData = jumbo_payload.as_mut_ptr().cast();
    fixture.record.UserDataLength = 4096;
    results.push(("jumbo_success", measured(|| fixture.invoke())));
    drop(fixture.receive.try_recv().expect("jumbo event"));
    let jumbo = pool.acquire(4096).expect("jumbo slot returned");
    results.push(("jumbo_exhausted", measured(|| fixture.invoke())));
    drop(jumbo);
    fixture.record.UserData = payload.as_mut_ptr().cast();
    fixture.record.UserDataLength = 512;
    let (_, replacement) = crossbeam_channel::bounded(1);
    drop(std::mem::replace(&mut fixture.receive, replacement));
    results.push(("disconnected", measured(|| fixture.invoke())));
    fixture.record.UserDataLength = 1;
    fixture.record.UserData = null_mut();
    results.push(("malformed", measured(|| fixture.invoke())));
    fixture.record.UserDataLength = 512;
    fixture.record.UserData = payload.as_mut_ptr().cast();
    PRODUCER.with(|state| {
        let _borrow = state.borrow_mut();
        results.push(("reentrant", measured(|| fixture.invoke())));
    });
    PRODUCER.with(|state| {
        state.borrow_mut().as_mut().expect("producer").context += 1;
    });
    results.push(("tls_wrong", measured(|| fixture.invoke())));
    PRODUCER.with(|state| {
        state.borrow_mut().take();
    });
    results.push(("tls_missing", measured(|| fixture.invoke())));
    for (name, count) in &results {
        assert_eq!(
            *count,
            AllocCounts::default(),
            "allocator activity in {name}"
        );
    }
    assert_eq!(
        fixture.context.counters.received.load(Ordering::Relaxed),
        10
    );
    assert_eq!(
        fixture
            .context
            .counters
            .intake_dropped
            .load(Ordering::Relaxed),
        4
    );
    assert_eq!(
        fixture
            .context
            .counters
            .callback_errors
            .load(Ordering::Relaxed),
        4
    );
    assert!(pool.acquire(1).is_some(), "all leases recycled");
    results
}

#[test]
fn scoped_native_record_empty_view_is_null_and_keeps_lease_exclusive() {
    let pool = PoolManager::new(1, 0).expect("pool");
    let guards = [
        pool.acquire(1).expect("first guard"),
        pool.acquire(1).expect("second guard"),
    ];
    let source = EVENT_RECORD::default();
    // SAFETY: The empty record advertises no borrowed storage.
    let event = unsafe { copy_record(&source, &pool) }.expect("empty event");
    let counts = measured(|| {
        event.with_record(|record| {
            assert_eq!(record.ExtendedDataCount, 0);
            assert_eq!(record.UserDataLength, 0);
            assert!(record.ExtendedData.is_null());
            assert!(record.UserData.is_null());
            assert!(record.UserContext.is_null());
            assert!(
                pool.acquire(1).is_none(),
                "view retains the last leased slot"
            );
        });
    });
    assert_eq!(counts, AllocCounts::default());
    // Synthetic TDH-fill seam: exercise synchronous encoding with the scoped
    // native view alive, without claiming provider/native allocation coverage.
    let metadata = DnsEventMetadata {
        event_id: 256,
        timestamp: UNIX_EPOCH,
        transport: Transport::Udp,
        peer_address: None,
        peer_port: None,
        local_address: None,
        local_port: None,
    };
    let mut encoder = Encoder::new(b"lease").expect("encoder");
    let mut fills = 0;
    let failed = event.with_record(|record| {
        encoder.encode_with_for_test(metadata, 12, |destination| {
            fills += 1;
            assert!(record.UserData.is_null());
            assert!(pool.acquire(1).is_none(), "fill retains event lease");
            assert!(destination.iter().all(|byte| *byte == 0));
            destination.fill(0xff);
            bail!("synthetic native extraction failure")
        })
    });
    assert!(failed.is_err());
    assert!(pool.acquire(1).is_none(), "failed fill retains event owner");
    let mut destination_address = 0;
    let payload = event
        .with_record(|record| {
            encoder.encode_with_for_test(metadata, 12, |destination| {
                fills += 1;
                assert!(record.UserData.is_null());
                assert!(pool.acquire(1).is_none(), "fill retains event lease");
                assert!(destination.iter().all(|byte| *byte == 0));
                destination_address = destination.as_ptr() as usize;
                destination.fill(0x53);
                Ok(())
            })
        })
        .expect("successful extraction after failure");
    assert_eq!(fills, 2, "one synchronous fill per encoding attempt");
    drop(event);
    assert!(
        pool.acquire(1).is_some(),
        "slot returns after view and event end"
    );
    // This use after dropping the native owner proves the result borrows only
    // encoder storage. The filled range stays inside the final wire payload.
    let offset = destination_address - payload.as_ptr() as usize;
    assert_eq!(&payload[offset..offset + 12], &[0x53; 12]);
    drop(guards);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keeps source destruction, native-view validation, and reuse of the same leased slot together."
)]
fn scoped_native_record_repairs_all_items_and_clears_stale_reused_slot() {
    let pool = PoolManager::new(1, 0).expect("pool");
    let guards = [
        pool.acquire(1).expect("first guard"),
        pool.acquire(1).expect("second guard"),
    ];
    let event = {
        let mut payload = vec![0x5au8; 37];
        let mut extra: Vec<Vec<u8>> = (0..MAX_EXTENDED_ITEMS)
            .map(|index| {
                let index = u8::try_from(index).expect("bounded index");
                let size = if index.is_multiple_of(8) {
                    0
                } else {
                    usize::from(index % 5 + 1)
                };
                vec![index; size]
            })
            .collect();
        let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS];
        for (index, (item, bytes)) in items.iter_mut().zip(&extra).enumerate() {
            item.DataPtr = if bytes.is_empty() {
                0
            } else {
                bytes.as_ptr() as u64
            };
            item.DataSize = u16::try_from(bytes.len()).expect("small item");
            item.ExtType = u16::try_from(index + 100).expect("small type");
            item.Reserved1 = u16::try_from(index).expect("small metadata");
            item.Anonymous._bitfield = 1;
        }
        let mut source = EVENT_RECORD {
            EventHeader: EVENT_HEADER {
                ProviderId: PROVIDER,
                ThreadId: 42,
                ProcessId: 314,
                TimeStamp: 12345,
                EventDescriptor: EVENT_DESCRIPTOR {
                    Id: 256,
                    Version: 7,
                    Opcode: 9,
                    Level: 5,
                    ..Default::default()
                },
                ..Default::default()
            },
            UserData: payload.as_mut_ptr().cast(),
            UserDataLength: 37,
            ExtendedData: items.as_mut_ptr(),
            ExtendedDataCount: u16::try_from(MAX_EXTENDED_ITEMS).expect("bounded item count"),
            // The copy intentionally never retains or dereferences this value.
            UserContext: std::ptr::dangling_mut::<u8>().cast(),
            ..Default::default()
        };
        source.BufferContext.LoggerId = 0x5678;
        source.BufferContext.Anonymous.ProcessorIndex = 0x1234;
        // SAFETY: Live payloads and typed descriptors back every native range.
        let event = unsafe { copy_record(&source, &pool) }.expect("copied event");
        payload.fill(0);
        for bytes in &mut extra {
            bytes.fill(0);
        }
        event
    };
    // All original source allocations and descriptor storage have ended.
    let base = event.buffer.bytes() as usize;
    let counts = measured(|| {
        event.with_record(|record| {
            assert_eq!(record.EventHeader.ThreadId, 42);
            assert_eq!(record.EventHeader.ProcessId, 314);
            assert_eq!(record.EventHeader.TimeStamp, 12345);
            assert!(dns_provider(&record.EventHeader.ProviderId));
            assert_eq!(record.EventHeader.EventDescriptor.Id, 256);
            assert_eq!(record.EventHeader.EventDescriptor.Version, 7);
            assert_eq!(record.EventHeader.EventDescriptor.Opcode, 9);
            assert_eq!(record.EventHeader.EventDescriptor.Level, 5);
            assert_eq!(record.BufferContext.LoggerId, 0x5678);
            assert_eq!(
                // SAFETY: The fixture initialized the ProcessorIndex union member.
                unsafe { record.BufferContext.Anonymous.ProcessorIndex },
                0x1234
            );
            assert_eq!(record.UserDataLength, 37);
            assert_eq!(record.UserData as usize, base);
            assert_eq!(
                record.ExtendedDataCount,
                u16::try_from(MAX_EXTENDED_ITEMS).expect("bounded item count")
            );
            assert!(record.UserContext.is_null());
            assert_eq!(
                (record.ExtendedData as usize)
                    % std::mem::align_of::<EVENT_HEADER_EXTENDED_DATA_ITEM>(),
                0
            );
            // SAFETY: with_record owns a typed, aligned descriptor array and the
            // lease's initialized user payload throughout this closure.
            let (payload, items) = unsafe {
                (
                    std::slice::from_raw_parts(record.UserData.cast::<u8>(), 37),
                    std::slice::from_raw_parts(record.ExtendedData, MAX_EXTENDED_ITEMS),
                )
            };
            assert!(payload.iter().all(|byte| *byte == 0x5a));
            for (index, item) in items.iter().enumerate() {
                let value = u8::try_from(index).expect("bounded index");
                let size = if value.is_multiple_of(8) {
                    0
                } else {
                    usize::from(value % 5 + 1)
                };
                assert_eq!(usize::from(item.DataSize), size);
                assert_eq!(usize::from(item.ExtType), index + 100);
                assert_eq!(usize::from(item.Reserved1), index);
                assert_eq!(item.Anonymous._bitfield, 1);
                if size == 0 {
                    assert_eq!(item.DataPtr, 0);
                    continue;
                }
                let pointer = usize::try_from(item.DataPtr).expect("native pointer fits target");
                assert!(pointer >= base + 37 && pointer + size <= base + pool::REGULAR_BYTES);
                assert_eq!(pointer % 8, 0);
                // SAFETY: The pointer is repaired into the retained initialized
                // lease range and checked in bounds immediately above.
                let bytes = unsafe { std::slice::from_raw_parts(item.DataPtr as *const u8, size) };
                assert!(bytes.iter().all(|byte| *byte == value));
            }
            assert!(
                pool.acquire(1).is_none(),
                "native view cannot release or share its slot"
            );
        });
    });
    assert_eq!(counts, AllocCounts::default());
    assert!(
        event.buffer.items().iter().all(|item| item.DataPtr == 0),
        "view pointer repair never mutates stored descriptors"
    );
    drop(event);
    // Two guard leases force the next record to reuse exactly the same slot.
    let source = EVENT_RECORD::default();
    // SAFETY: Empty source has no advertised native ranges.
    let empty = unsafe { copy_record(&source, &pool) }.expect("reused empty event");
    assert_eq!(empty.buffer.bytes() as usize, base);
    empty.with_record(|record| {
        assert_eq!(record.UserDataLength, 0);
        assert_eq!(record.ExtendedDataCount, 0);
        assert!(record.UserData.is_null());
        assert!(
            record.ExtendedData.is_null(),
            "stale descriptors remain unexposed"
        );
        assert!(record.UserContext.is_null());
    });
    drop(empty);
    drop(guards);
}

#[test]
fn callback_and_cross_thread_recycle_do_not_allocate_or_deallocate() {
    drop(allocator_fixtures());
}

#[test]
fn copies_payload_and_extended_data_before_callback_returns() -> Result<()> {
    let pool = PoolManager::new(1, 1)?;
    let owned = {
        let mut payload = vec![1u8, 2, 3];
        let mut extra = [4u8, 5, 6];
        let mut item = EVENT_HEADER_EXTENDED_DATA_ITEM {
            DataPtr: extra.as_ptr() as u64,
            DataSize: 3,
            ..Default::default()
        };
        let record = EVENT_RECORD {
            UserData: payload.as_mut_ptr().cast(),
            UserDataLength: 3,
            ExtendedData: &raw mut item,
            ExtendedDataCount: 1,
            ..Default::default()
        };
        // SAFETY: These live Rust allocations back every pointer/length
        // pair for the duration of the synchronous deep copy.
        let owned = unsafe { copy_record(&record, &pool) }
            .map_err(|error| anyhow!("deep copy failed: {error:?}"))?;
        payload.fill(0);
        extra.fill(0);
        owned
    };
    // SAFETY: The lease retains both checked ranges after source destruction.
    unsafe {
        assert_eq!(
            std::slice::from_raw_parts(owned.buffer.bytes(), 3),
            [1, 2, 3]
        );
        assert_eq!(
            std::slice::from_raw_parts(owned.buffer.bytes().add(owned.buffer.offset(0)), 3),
            [4, 5, 6]
        );
    }
    assert_eq!(owned.buffer.offset(0) % 8, 0);
    assert_eq!(owned.buffer.items()[0].DataPtr, 0);
    Ok(())
}

/// The process ID the test callback context accepts.
const TEST_PID: u32 = 4242;

struct Fixture {
    context: Box<CallbackContext>,
    record: EVENT_RECORD,
    receive: crossbeam_channel::Receiver<RawEtwEvent>,
}

impl Fixture {
    fn new(capacity: usize, jumbo: usize) -> Self {
        let pool = PoolManager::new(capacity, jumbo).expect("test pool initializes");
        let mut context = Box::new(CallbackContext::new(
            Arc::clone(&pool),
            Arc::default(),
            TEST_PID,
        ));
        let (sender, receive) = crossbeam_channel::bounded(capacity);
        let context_address = (&raw const *context) as usize;
        PRODUCER.with(|state| {
            *state.borrow_mut() = Some(CallbackProducer {
                context: context_address,
                sender,
                pool,
            });
        });
        let record = EVENT_RECORD {
            EventHeader: EVENT_HEADER {
                ProviderId: PROVIDER,
                ProcessId: TEST_PID,
                EventDescriptor: EVENT_DESCRIPTOR {
                    Id: 256,
                    ..Default::default()
                },
                ..Default::default()
            },
            UserContext: (&raw mut *context).cast(),
            ..Default::default()
        };
        Self {
            context,
            record,
            receive,
        }
    }

    fn invoke(&mut self) {
        // SAFETY: Box keeps the context stable; test payloads/descriptors, when
        // present, remain live across this synchronous callback invocation.
        unsafe {
            native_etw_callback(&raw mut self.record);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        PRODUCER.with(|state| {
            state.borrow_mut().take();
        });
    }
}

fn assert_no_intake_activity(fixture: &Fixture) {
    assert!(fixture.receive.is_empty());
    let counters = &fixture.context.counters;
    for counter in [
        &counters.received,
        &counters.dropped,
        &counters.intake_dropped,
        &counters.pool_exhausted,
        &counters.receiver_disconnected,
        &counters.callback_errors,
        &counters.etw_lost,
        &counters.etw_buffers_lost,
        &counters.stats_errors,
    ] {
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
    assert!(!counters.stats_valid.load(Ordering::Relaxed));
}

#[test]
fn callback_accepts_only_client_query_and_successful_response() {
    let mut fixture = Fixture::new(2, 0);
    for id in [256, 257] {
        fixture.record.EventHeader.EventDescriptor.Id = id;
        assert_no_allocations(|| fixture.invoke());
    }
    for id in [256, 257] {
        let event = fixture.receive.try_recv().expect("accepted client event");
        assert_eq!(event.header.EventDescriptor.Id, id);
    }
    assert_eq!(fixture.context.counters.received.load(Ordering::Relaxed), 2);
}

#[test]
fn callback_excludes_other_ids_before_payload_pool_and_tls_access() {
    let mut fixture = Fixture::new(1, 0);
    for id in EXCLUDED_EVENT_IDS {
        fixture.record.EventHeader.EventDescriptor.Id = id;
        assert_no_allocations(|| fixture.invoke());
    }
    assert_no_intake_activity(&fixture);
    // All three regular slots are still available after excluded records.
    let held: Vec<_> = (0..3)
        .map(|_| fixture.context.pool.acquire(1).expect("unconsumed slot"))
        .collect();
    assert!(fixture.context.pool.acquire(1).is_none());
    for id in EXCLUDED_EVENT_IDS {
        fixture.record.EventHeader.EventDescriptor.Id = id;
        assert_no_allocations(|| fixture.invoke());
    }
    assert_no_intake_activity(&fixture);
    for id in EXCLUDED_EVENT_IDS {
        fixture.record.EventHeader.EventDescriptor.Id = id;
        // Malformed pointer/length pairs must never reach copying.
        fixture.record.UserDataLength = u16::MAX;
        fixture.record.ExtendedDataCount = u16::MAX;
        PRODUCER.with(|state| {
            let _borrow = state.borrow_mut();
            assert_no_allocations(|| fixture.invoke());
        });
        assert_no_intake_activity(&fixture);
    }
    drop(held);
    let returned: Vec<_> = (0..3)
        .map(|_| fixture.context.pool.acquire(1).expect("retained pool slot"))
        .collect();
    drop(returned);
}

#[test]
fn callback_header_filter_handles_null_record_context_and_wrong_provider() {
    let mut fixture = Fixture::new(1, 0);
    // SAFETY: The callback explicitly accepts a null defensive input.
    assert_no_allocations(|| unsafe { native_etw_callback(null_mut()) });
    fixture.record.UserContext = null_mut();
    for id in EXCLUDED_EVENT_IDS {
        fixture.record.EventHeader.EventDescriptor.Id = id;
        assert_no_allocations(|| fixture.invoke());
    }
    fixture.record.EventHeader.EventDescriptor.Id = 256;
    fixture.record.EventHeader.ProviderId = GUID::from_u128(0);
    assert_no_allocations(|| fixture.invoke());
    assert_no_intake_activity(&fixture);
}

#[test]
fn callback_drops_new_record_when_ring_is_full() {
    let mut fixture = Fixture::new(1, 0);
    fixture.invoke();
    fixture.invoke();
    assert_eq!(fixture.receive.len(), 1);
    assert_eq!(fixture.context.counters.received.load(Ordering::Relaxed), 2);
    assert_eq!(fixture.context.counters.dropped.load(Ordering::Relaxed), 1);
    assert_eq!(
        fixture
            .context
            .counters
            .intake_dropped
            .load(Ordering::Relaxed),
        1
    );
}

#[test]
fn pool_exhaustion_is_counted_and_cross_thread_recycle_survives_owner_exit() {
    let mut fixture = Fixture::new(1, 0);
    let pool = Arc::clone(&fixture.context.pool);
    let first = pool.acquire(1).expect("first slot");
    let second = pool.acquire(1).expect("second slot");
    let third = pool.acquire(1).expect("third slot");
    fixture.invoke();
    assert_eq!(
        fixture
            .context
            .counters
            .pool_exhausted
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(fixture.context.counters.dropped.load(Ordering::Relaxed), 0);
    drop(fixture);
    drop(pool);
    thread::spawn(move || {
        drop(first);
        drop(second);
        drop(third);
    })
    .join()
    .expect("leases survive producer owner");
}

#[test]
fn native_bounds_and_jumbo_disable_are_explicit() {
    let mut fixture = Fixture::new(1, 0);
    fixture.record.UserDataLength = 1;
    fixture.invoke();
    assert_eq!(
        fixture
            .context
            .counters
            .callback_errors
            .load(Ordering::Relaxed),
        1
    );
    let mut payload = vec![1u8; 2049];
    fixture.record.UserData = payload.as_mut_ptr().cast();
    fixture.record.UserDataLength = 2049;
    fixture.invoke();
    assert_eq!(
        fixture
            .context
            .counters
            .pool_exhausted
            .load(Ordering::Relaxed),
        1
    );
    fixture.record.ExtendedDataCount =
        u16::try_from(MAX_EXTENDED_ITEMS + 1).expect("bounded item count");
    fixture.invoke();
    assert_eq!(
        fixture
            .context
            .counters
            .callback_errors
            .load(Ordering::Relaxed),
        2
    );
}

#[test]
fn missing_or_reentrant_tls_is_rejected_without_panicking() {
    let mut fixture = Fixture::new(1, 0);
    PRODUCER.with(|state| {
        let _borrow = state.borrow_mut();
        fixture.invoke();
    });
    PRODUCER.with(|state| {
        state.borrow_mut().take();
    });
    fixture.invoke();
    assert_eq!(fixture.context.counters.received.load(Ordering::Relaxed), 2);
    assert_eq!(
        fixture
            .context
            .counters
            .callback_errors
            .load(Ordering::Relaxed),
        2
    );
}

#[test]
fn queued_records_survive_native_owner_exit_and_drain_after_disconnect() {
    let mut fixture = Fixture::new(2, 0);
    let mut payload = [1u8, 2, 3];
    fixture.record.UserData = payload.as_mut_ptr().cast();
    fixture.record.UserDataLength = 3;
    fixture.record.EventHeader.ThreadId = 42;
    fixture.record.EventHeader.TimeStamp = 12345;
    fixture.invoke();
    fixture.invoke();
    payload.fill(0);
    let receive = fixture.receive.clone();
    let weak = Arc::downgrade(&fixture.context.pool);
    drop(fixture);
    assert!(
        weak.upgrade().is_some(),
        "queued leases retain pool after native owner exits"
    );
    let drained = thread::spawn(move || {
        let mut count = 0;
        while let Ok(event) = receive.try_recv() {
            assert_eq!(event.header.ThreadId, 42);
            assert_eq!(event.header.TimeStamp, 12345);
            assert_eq!(
                // SAFETY: The retained lease owns the initialized three-byte range.
                unsafe { std::slice::from_raw_parts(event.buffer.bytes(), 3) },
                [1, 2, 3]
            );
            count += 1;
            drop(event);
        }
        assert!(matches!(
            receive.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        ));
        count
    })
    .join()
    .expect("drain after native completion");
    assert_eq!(drained, 2);
    assert!(
        weak.upgrade().is_none(),
        "final lease releases the pool after shutdown"
    );
}

#[test]
fn largest_record_and_all_extended_descriptors_fit_and_recycle() {
    let pool = PoolManager::new(1, 1).expect("pool");
    let mut payload = vec![0xaau8; 65535];
    let extra = vec![0xbbu8; 65535];
    let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS];
    // Keep the aggregate extended length at 65535 and exercise alignment gaps.
    let mut offset = 0usize;
    for (index, item) in items.iter_mut().enumerate() {
        let size = if index == MAX_EXTENDED_ITEMS - 1 {
            65535 - (MAX_EXTENDED_ITEMS - 1) * 1025
        } else {
            1025
        };
        item.DataSize = u16::try_from(size).expect("bounded item size");
        // SAFETY: Offset traverses the live allocation, including final item.
        item.DataPtr = unsafe { extra.as_ptr().add(offset) } as u64;
        offset += size;
    }
    let record = EVENT_RECORD {
        UserData: payload.as_mut_ptr().cast(),
        UserDataLength: 65535,
        ExtendedData: items.as_mut_ptr(),
        ExtendedDataCount: u16::try_from(MAX_EXTENDED_ITEMS).expect("bounded item count"),
        ..Default::default()
    };
    // SAFETY: All advertised native ranges are backed by live allocations.
    let owned = unsafe { copy_record(&record, &pool) }.expect("maximum record fits jumbo");
    assert!(
        pool.acquire(pool::REGULAR_BYTES + 1).is_none(),
        "jumbo lease remains exclusive"
    );
    for (index, item) in owned.buffer.items().iter().enumerate() {
        assert_eq!(item.DataPtr, 0);
        assert_eq!(owned.buffer.offset(index) % 8, 0);
        // SAFETY: Each checked item range is initialized in the retained lease.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                owned.buffer.bytes().add(owned.buffer.offset(index)),
                usize::from(item.DataSize),
            )
        };
        assert!(bytes.iter().all(|byte| *byte == 0xbb));
    }
    drop(owned);
    assert!(pool.acquire(pool::JUMBO_BYTES).is_some());
    items[0].DataSize = 65535;
    let malformed = EVENT_RECORD {
        ExtendedData: items.as_mut_ptr(),
        ..record
    };
    assert!(matches!(
        // SAFETY: The preflight rejects aggregate bounds before reading payloads.
        unsafe { copy_record(&malformed, &pool) },
        Err(CopyError::Malformed)
    ));
}

#[test]
#[ignore = "manual synthetic callback benchmark; excludes TDH and output"]
fn synthetic_pooled_intake_benchmark() {
    use std::time::Instant;
    let capacity = std::env::var("ETW_BENCH_CAPACITY")
        .ok()
        .map_or(8192, |v| v.parse().expect("capacity integer"));
    let offered: usize = std::env::var("ETW_BENCH_EVENTS")
        .ok()
        .map_or(200_000, |v| v.parse().expect("event count integer"));
    let mode = std::env::var("ETW_BENCH_MODE").unwrap_or_else(|_| "steady".into());
    assert!(matches!(mode.as_str(), "steady" | "bursty"));
    let fixtures = allocator_fixtures();
    let mut fixture = Fixture::new(capacity, 64);
    let mut payload = [0x5au8; 512];
    fixture.record.UserDataLength = 512;
    fixture.record.UserData = payload.as_mut_ptr().cast();
    let done = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(mode == "steady"));
    let consumer = {
        let receiver = fixture.receive.clone();
        let done = Arc::clone(&done);
        let release = Arc::clone(&release);
        thread::spawn(move || {
            let mut dequeued = 0u64;
            loop {
                if release.load(Ordering::Acquire) {
                    match receiver.try_recv() {
                        Ok(event) => {
                            drop(event);
                            dequeued += 1;
                            continue;
                        }
                        Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                        Err(crossbeam_channel::TryRecvError::Empty) => {}
                    }
                }
                if done.load(Ordering::Acquire) && receiver.is_empty() {
                    break;
                }
                thread::park_timeout(Duration::from_millis(2));
            }
            dequeued
        })
    };
    let started = Instant::now();
    for index in 0..offered {
        fixture.invoke();
        if mode == "bursty" && (index + 1).is_multiple_of(capacity + 2) {
            release.store(true, Ordering::Release);
            consumer.thread().unpark();
            while !fixture.receive.is_empty() {
                thread::yield_now();
            }
            release.store(false, Ordering::Release);
        }
    }
    done.store(true, Ordering::Release);
    release.store(true, Ordering::Release);
    consumer.thread().unpark();
    let dequeued = consumer.join().expect("consumer completed");
    let elapsed = started.elapsed().as_nanos();
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let counters = &fixture.context.counters;
    let intake_dropped = load(&counters.intake_dropped);
    let callback_errors = load(&counters.callback_errors);
    let accepted = offered as u64 - intake_dropped - callback_errors;
    assert_eq!(accepted, dequeued);
    assert_eq!(fixture.receive.len(), 0);
    let pool = &fixture.context.pool;
    let leases: Vec<_> = (0..capacity + 2)
        .map(|_| pool.acquire(1).expect("all regular slots returned"))
        .collect();
    assert!(pool.acquire(1).is_none());
    drop(leases);
    let fixture_json = fixtures
        .iter()
        .map(|(name, count)| {
            format!(
                "\"{name}\":{{\"allocations\":{},\"reallocations\":{},\"deallocations\":{}}}",
                count.allocations, count.reallocations, count.deallocations
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "ETW_BENCH_JSON={{\"schema\":1,\"capacity\":{capacity},\"regular_capacity\":{},\"regular_bytes\":{},\"jumbo_capacity\":64,\"jumbo_bytes\":{},\"mode\":\"{mode}\",\"offered\":{offered},\"accepted\":{accepted},\"dequeued\":{dequeued},\"intake_dropped\":{intake_dropped},\"ring_dropped\":{},\"pool_exhausted\":{},\"receiver_disconnected\":{},\"callback_errors\":{callback_errors},\"final_backlog\":0,\"elapsed_ns\":{elapsed},\"allocator_fixtures\":{{{fixture_json}}},\"recycle_verified\":true}}",
        capacity + 2,
        pool::REGULAR_BYTES,
        pool::JUMBO_BYTES,
        load(&counters.dropped),
        load(&counters.pool_exhausted),
        load(&counters.receiver_disconnected)
    );
}

#[test]
fn pid_allowed_matches_only_the_expected_process() {
    assert!(pid_allowed(7, 7));
    assert!(!pid_allowed(7, 8));
    assert!(!pid_allowed(7, 0));
}

#[test]
fn callback_rejects_records_from_other_processes_without_allocating() {
    let mut fixture = Fixture::new(2, 0);
    assert_eq!(fixture.context.expected_pid, TEST_PID);
    fixture.record.EventHeader.ProcessId = TEST_PID;
    assert_no_allocations(|| fixture.invoke());
    fixture.record.EventHeader.ProcessId = 99;
    assert_no_allocations(|| fixture.invoke());
    assert_no_allocations(|| fixture.invoke());
    let counters = &fixture.context.counters;
    assert_eq!(counters.received.load(Ordering::Relaxed), 1);
    assert_eq!(counters.foreign_pid_rejected.load(Ordering::Relaxed), 2);
    assert_eq!(counters.callback_errors.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.receive.len(), 1, "only the genuine record queued");
}

#[test]
fn new_stores_the_required_expected_pid() {
    let pool = PoolManager::new(1, 0).expect("test pool");
    let context = CallbackContext::new(pool, Arc::default(), 5);
    assert_eq!(context.expected_pid, 5);
}

#[test]
fn jumbo_exhaustion_is_counted_separately_from_regular() {
    // No jumbo slots exist, so a 4 KiB record cannot be stored.
    let mut fixture = Fixture::new(2, 0);
    let mut payload = vec![0x11u8; 4096];
    fixture.record.UserData = payload.as_mut_ptr().cast();
    fixture.record.UserDataLength = 4096;
    fixture.invoke();
    let counters = Arc::clone(&fixture.context.counters);
    assert_eq!(counters.pool_exhausted.load(Ordering::Relaxed), 1);
    assert_eq!(counters.pool_exhausted_jumbo.load(Ordering::Relaxed), 1);
    assert_eq!(counters.intake_dropped.load(Ordering::Relaxed), 1);
    // A regular-size record that finds no free slot does not touch the jumbo count.
    let held: Vec<_> = (0..4)
        .map(|_| fixture.context.pool.acquire(1).expect("regular slot"))
        .collect();
    fixture.record.UserDataLength = 16;
    fixture.invoke();
    assert_eq!(counters.pool_exhausted.load(Ordering::Relaxed), 2);
    assert_eq!(counters.pool_exhausted_jumbo.load(Ordering::Relaxed), 1);
    drop(held);
}

mod copy_record_properties {
    use super::*;
    use proptest::prelude::*;

    /// Model of the admission rule: nonnull sources, at most 16 items, and at
    /// most 65535 aggregate extended bytes.
    fn admissible(data: &[u8], null_data: bool, items: &[(Vec<u8>, bool)]) -> bool {
        let aggregate: usize = items.iter().map(|(bytes, _)| bytes.len()).sum();
        (!null_data || data.is_empty())
            && items.len() <= MAX_EXTENDED_ITEMS
            && items.iter().all(|(bytes, null)| !*null || bytes.is_empty())
            && aggregate <= 65535
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Hostile lengths, counts and null pointers never panic; admitted
        /// records are copied byte for byte at aligned, in-bounds offsets.
        #[test]
        fn hostile_descriptors_are_rejected_or_copied_exactly(
            data in prop::collection::vec(any::<u8>(), 0..4096),
            null_data in any::<bool>(),
            items in prop::collection::vec(
                (prop::collection::vec(any::<u8>(), 0..3000), any::<bool>()),
                0..=20,
            ),
        ) {
            let pool = PoolManager::new(1, 1).expect("pool");
            let mut data = data;
            let mut sources: Vec<Vec<u8>> = items.iter().map(|(bytes, _)| bytes.clone()).collect();
            let mut descriptors: Vec<EVENT_HEADER_EXTENDED_DATA_ITEM> = items
                .iter()
                .zip(sources.iter_mut())
                .map(|((bytes, null), source)| EVENT_HEADER_EXTENDED_DATA_ITEM {
                    DataSize: u16::try_from(bytes.len()).expect("bounded item"),
                    DataPtr: if *null { 0 } else { source.as_mut_ptr() as u64 },
                    ..Default::default()
                })
                .collect();
            let record = EVENT_RECORD {
                UserData: if null_data { null_mut() } else { data.as_mut_ptr().cast() },
                UserDataLength: u16::try_from(data.len()).expect("bounded payload"),
                ExtendedData: if descriptors.is_empty() {
                    null_mut()
                } else {
                    descriptors.as_mut_ptr()
                },
                ExtendedDataCount: u16::try_from(descriptors.len()).expect("bounded count"),
                ..Default::default()
            };
            // SAFETY: Every nonnull advertised pointer is backed by a live Vec of
            // at least the advertised length; null pointers are paired with
            // nonzero lengths only to exercise rejection before any read.
            let result = unsafe { copy_record(&record, &pool) };
            if admissible(&data, null_data, &items) {
                let owned = result.expect("admissible record is copied");
                prop_assert_eq!(owned.payload(), &data[..]);
                for (index, (bytes, _)) in items.iter().enumerate() {
                    let offset = owned.buffer.offset(index);
                    prop_assert_eq!(offset % 8, 0);
                    prop_assert_eq!(owned.buffer.items()[index].DataPtr, 0);
                    // SAFETY: copy_record initialized this in-bounds range in the lease.
                    let copied = unsafe {
                        std::slice::from_raw_parts(owned.buffer.bytes().add(offset), bytes.len())
                    };
                    prop_assert_eq!(copied, &bytes[..]);
                }
            } else {
                prop_assert!(matches!(result, Err(CopyError::Malformed)));
            }
        }
    }
}
