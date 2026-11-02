//! The native ETW callbacks and the deep copy of records into pooled storage.
//!
//! The per-event callback is allocation-free and wake-free; see
//! `docs/history/ring-wakeup.md`. The buffer callback, which ETW calls once after
//! it delivers each buffer, wakes the parser when that buffer enqueued events.

use std::{
    cell::{Cell, RefCell},
    ptr::null_mut,
    sync::{Arc, atomic::Ordering},
    thread::Thread,
};

use crossbeam_channel::{Sender, TrySendError};
use windows_sys::Win32::System::Diagnostics::Etw::{
    ETW_BUFFER_CONTEXT, EVENT_HEADER, EVENT_HEADER_EXTENDED_DATA_ITEM, EVENT_RECORD,
    EVENT_TRACE_LOGFILEW,
};

use super::{
    CaptureCounters, MAX_EXTENDED_ITEMS, PoolManager, dns_provider,
    pool::{PooledBuffer, REGULAR_BYTES},
    supported,
};

#[cfg(test)]
#[path = "intake_tests.rs"]
mod tests;

/// Owns the native metadata and an exclusive lease, without borrowed ETW pointers.
pub(crate) struct RawEtwEvent {
    header: EVENT_HEADER,
    buffer_context: ETW_BUFFER_CONTEXT,
    data_length: u16,
    extended_count: u16,
    buffer: PooledBuffer,
}

/// State the native callback reaches through `EVENT_RECORD::UserContext`.
pub(super) struct CallbackContext {
    pool: Arc<PoolManager>,
    counters: Arc<CaptureCounters>,
    /// The only process allowed to emit accepted records (the DNS Server service).
    expected_pid: u32,
}

impl CallbackContext {
    /// Requires `EventHeader.ProcessId == expected_pid` on every accepted record;
    /// there is no way to build a context without that filter. Callers box the
    /// context so the address handed to `OpenTraceW` stays stable.
    pub(super) fn new(
        pool: Arc<PoolManager>,
        counters: Arc<CaptureCounters>,
        expected_pid: u32,
    ) -> Self {
        Self {
            pool,
            counters,
            expected_pid,
        }
    }
}

/// True when a record from `actual` may be accepted given the expected emitter.
fn pid_allowed(expected: u32, actual: u32) -> bool {
    expected == actual
}

struct CallbackProducer {
    context: usize,
    sender: Sender<RawEtwEvent>,
    pool: Arc<PoolManager>,
    counters: Arc<CaptureCounters>,
    /// The parser thread, woken at most once per delivered ETW buffer.
    parser: Thread,
    /// Set by the event callback after an enqueue and cleared by the buffer
    /// callback. Both run on this thread, so a `Cell` is enough.
    enqueued: Cell<bool>,
}

thread_local! {
    // Eagerly installed on the ProcessTrace worker before native delivery starts.
    static PRODUCER: RefCell<Option<CallbackProducer>> = const { RefCell::new(None) };
}

/// Installs the calling thread's producer state. Call on the `ProcessTrace`
/// worker before native delivery starts; `context` must be the value passed as
/// the trace context.
pub(super) fn install_producer(
    context: &CallbackContext,
    sender: Sender<RawEtwEvent>,
    pool: Arc<PoolManager>,
    parser: Thread,
) {
    let context_address = std::ptr::from_ref(context) as usize;
    let counters = Arc::clone(&context.counters);
    PRODUCER.with(|state| {
        *state.borrow_mut() = Some(CallbackProducer {
            context: context_address,
            sender,
            pool,
            counters,
            parser,
            enqueued: Cell::new(false),
        });
    });
}

/// The `BufferCallback` registered with `OpenTraceW`. `ProcessTrace` calls it on
/// the consumer thread after it has delivered every event of one buffer. A real-
/// time session delivers its buffers in bursts (about once per
/// `--etw-flush-seconds`), so the parser is woken while the burst arrives
/// instead of after its next timed poll. The parser keeps one wake token, so no
/// wake is lost.
///
/// It reads no pointer and cannot panic. It returns TRUE, because FALSE would
/// end `ProcessTrace`.
pub(super) extern "system" fn native_buffer_callback(_logfile: *mut EVENT_TRACE_LOGFILEW) -> u32 {
    let _ = PRODUCER.try_with(|state| {
        if let Ok(state) = state.try_borrow()
            && let Some(producer) = state.as_ref()
        {
            producer
                .counters
                .etw_buffers_delivered
                .fetch_add(1, Ordering::Relaxed);
            if producer.enqueued.replace(false) {
                producer
                    .counters
                    .parser_wake_requests
                    .fetch_add(1, Ordering::Relaxed);
                producer.parser.unpark();
            }
        }
    });
    1
}

/// Drops the calling thread's producer state, closing the channel's last sender.
pub(super) fn clear_producer() {
    PRODUCER.with(|state| {
        state.borrow_mut().take();
    });
}

/// The `EventRecordCallback` registered with `OpenTraceW`.
pub(super) unsafe extern "system" fn native_etw_callback(record: *mut EVENT_RECORD) {
    // SAFETY: ETW invokes this callback with an EVENT_RECORD valid for the duration
    // of this call. A null defensive check precedes dereferencing the pointer.
    let Some(record) = (unsafe { record.as_ref() }) else {
        return;
    };
    if !dns_provider(&record.EventHeader.ProviderId)
        || !supported(record.EventHeader.EventDescriptor.Id)
    {
        return;
    }
    // SAFETY: OpenTrace received this stable Box pointee, retained until all
    // ProcessTrace callbacks finish; it is shared immutably across callbacks.
    let Some(context) = (unsafe { record.UserContext.cast::<CallbackContext>().as_ref() }) else {
        return;
    };
    // A forged provider registration in another process is rejected here even if
    // the ETW-side PID filter was not honored. Allocation-free: one compare.
    if !pid_allowed(context.expected_pid, record.EventHeader.ProcessId) {
        context
            .counters
            .foreign_pid_rejected
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    context.counters.received.fetch_add(1, Ordering::Relaxed);
    let handled = PRODUCER
        .try_with(|state| {
            let Ok(mut state) = state.try_borrow_mut() else {
                return false;
            };
            let Some(producer) = state.as_mut() else {
                return false;
            };
            if producer.context != record.UserContext as usize
                || !Arc::ptr_eq(&producer.pool, &context.pool)
            {
                return false;
            }
            // SAFETY: ETW owns the readable native record and each advertised
            // pointer/length pair for this call. The pool is an independent allocation.
            let owned = match unsafe { copy_record(record, &producer.pool) } {
                Ok(owned) => owned,
                Err(CopyError::Malformed) => return false,
                Err(CopyError::Exhausted { jumbo }) => {
                    context
                        .counters
                        .pool_exhausted
                        .fetch_add(1, Ordering::Relaxed);
                    if jumbo {
                        context
                            .counters
                            .pool_exhausted_jumbo
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            };
            match producer.sender.try_send(owned) {
                // The buffer callback wakes the parser; this path never does.
                Ok(()) => producer.enqueued.set(true),
                Err(TrySendError::Full(_owned)) => {
                    context.counters.dropped.fetch_add(1, Ordering::Relaxed);
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    // The returned lease recycles immediately on this path.
                }
                Err(TrySendError::Disconnected(_owned)) => {
                    context
                        .counters
                        .receiver_disconnected
                        .fetch_add(1, Ordering::Relaxed);
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            true
        })
        .unwrap_or(false);
    if !handled {
        context
            .counters
            .callback_errors
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub(super) enum CopyError {
    Malformed,
    /// No free slot; `jumbo` says whether the record needed a jumbo slot.
    Exhausted {
        jumbo: bool,
    },
}

fn align_payload(length: usize) -> Option<usize> {
    length.checked_add(7).map(|length| length & !7)
}

/// Deep-copies the record into an exclusive preallocated slot.
///
/// # Safety
/// The native record and its nonempty buffers must be readable for their stated
/// lengths until return, and must not overlap pool allocations. ETW guarantees
/// these native input lifetimes; size/null checks cannot validate arbitrary pointers.
pub(super) unsafe fn copy_record(
    record: &EVENT_RECORD,
    pool: &Arc<PoolManager>,
) -> std::result::Result<RawEtwEvent, CopyError> {
    let invalid = || CopyError::Malformed;
    let data_length = usize::from(record.UserDataLength);
    let count = usize::from(record.ExtendedDataCount);
    if (data_length != 0 && record.UserData.is_null())
        || count > MAX_EXTENDED_ITEMS
        || (count != 0 && record.ExtendedData.is_null())
    {
        return Err(CopyError::Malformed);
    }
    let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS];
    let mut offsets = [0; MAX_EXTENDED_ITEMS];
    let mut length = data_length;
    let mut extended_length = 0usize;
    for index in 0..count {
        // SAFETY: Caller supplies count initialized descriptors. Reading unaligned
        // also handles defensive fixtures without creating an unaligned reference.
        let item = unsafe { record.ExtendedData.add(index).read_unaligned() };
        let size = usize::from(item.DataSize);
        if size != 0 && item.DataPtr == 0 {
            return Err(CopyError::Malformed);
        }
        extended_length = extended_length.checked_add(size).ok_or_else(invalid)?;
        if extended_length > 65535 {
            return Err(CopyError::Malformed);
        }
        length = align_payload(length).ok_or_else(invalid)?;
        offsets[index] = length;
        length = length.checked_add(size).ok_or_else(invalid)?;
        items[index] = item;
    }
    let mut buffer = pool.acquire(length).ok_or(CopyError::Exhausted {
        jumbo: length > REGULAR_BYTES,
    })?;
    if data_length != 0 {
        // SAFETY: The validated nonempty source is readable by caller contract.
        // The exclusive pool slot has at least length >= data_length bytes;
        // native source and owned slot do not overlap. u8 requires alignment 1.
        unsafe {
            std::ptr::copy_nonoverlapping(
                record.UserData.cast::<u8>(),
                buffer.bytes_mut(),
                data_length,
            );
        }
    }
    for index in 0..count {
        let mut item = items[index];
        let size = usize::from(item.DataSize);
        if size != 0 {
            // SAFETY: Caller guarantees readable item storage. Preflight checked
            // offset + size <= acquired capacity, and each offset is 8-aligned.
            // The independent slot cannot overlap a native ETW input buffer.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    item.DataPtr as *const u8,
                    buffer.bytes_mut().add(offsets[index]),
                    size,
                );
            }
        }
        item.DataPtr = 0;
        buffer.set_item(index, item, offsets[index]);
    }
    Ok(RawEtwEvent {
        header: record.EventHeader,
        buffer_context: record.BufferContext,
        data_length: record.UserDataLength,
        extended_count: record.ExtendedDataCount,
        buffer,
    })
}

impl RawEtwEvent {
    /// The native event header copied at intake.
    pub(super) fn header(&self) -> &EVENT_HEADER {
        &self.header
    }

    /// Mutable header access so schema tests can corrupt a copied event.
    #[cfg(test)]
    pub(super) fn header_mut(&mut self) -> &mut EVENT_HEADER {
        &mut self.header
    }

    /// The user payload bytes, valid while this event retains its lease.
    pub(super) fn payload(&self) -> &[u8] {
        // SAFETY: copy_record initialized precisely data_length bytes at offset zero
        // of this exclusive lease. The borrow cannot outlive the retained event.
        unsafe { std::slice::from_raw_parts(self.buffer.bytes(), usize::from(self.data_length)) }
    }

    /// Builds a scoped TDH view while this event retains its exclusive lease.
    /// The record and repaired raw pointers are valid only during `operation`.
    pub(super) fn with_record<T>(&self, operation: impl FnOnce(&EVENT_RECORD) -> T) -> T {
        let count = usize::from(self.extended_count);
        let mut extended = *self.buffer.items();
        for (index, item) in extended.iter_mut().take(count).enumerate() {
            item.DataPtr = if item.DataSize == 0 {
                0
            } else {
                // SAFETY: copy_record checked and stored this aligned in-bounds offset;
                // self retains the exclusive lease throughout every synchronous TDH call.
                unsafe { self.buffer.bytes().add(self.buffer.offset(index)) as u64 }
            };
        }
        let record = EVENT_RECORD {
            EventHeader: self.header,
            BufferContext: self.buffer_context,
            ExtendedDataCount: self.extended_count,
            UserDataLength: self.data_length,
            ExtendedData: if count == 0 {
                null_mut()
            } else {
                extended.as_mut_ptr()
            },
            UserData: if self.data_length == 0 {
                null_mut()
            } else {
                // The `*mut` is a type requirement of EVENT_RECORD only. TDH reads
                // UserData and never writes it, so deriving the pointer from this
                // shared borrow does not permit a write through it.
                self.buffer.bytes().cast_mut().cast()
            },
            UserContext: null_mut(),
        };
        operation(&record)
    }
}
