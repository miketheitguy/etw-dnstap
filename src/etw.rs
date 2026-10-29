//! Owns the real-time DNS ETW session and copies borrowed records into pooled storage.

use std::{
    cell::RefCell,
    collections::HashMap,
    mem::{offset_of, size_of},
    net::IpAddr,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use crossbeam_channel::{Sender, TrySendError};

mod pool;
pub(crate) use pool::PoolManager;
use pool::PooledBuffer;
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_CANCELLED, ERROR_CTX_CLOSE_PENDING, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
            GetLastError,
        },
        System::Diagnostics::Etw::{
            CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, DecodingSourceXMLFile,
            ETW_BUFFER_CONTEXT, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_HEADER,
            EVENT_HEADER_EXTENDED_DATA_ITEM, EVENT_PROPERTY_INFO, EVENT_RECORD,
            EVENT_TRACE_CONTROL_QUERY, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
            EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW,
            PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE,
            PROPERTY_DATA_DESCRIPTOR, ProcessTrace, PropertyHasCustomSchema, PropertyParamCount,
            PropertyStruct, StartTraceW, TDH_INTYPE_ANSISTRING, TDH_INTYPE_BINARY,
            TDH_INTYPE_BOOLEAN, TDH_INTYPE_UINT8, TDH_INTYPE_UINT16, TDH_INTYPE_UINT32,
            TDH_INTYPE_UNICODESTRING, TDH_OUTTYPE_IPV4, TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT,
            TRACE_EVENT_INFO, TdhGetEventInformation, TdhGetProperty, TdhGetPropertySize,
            WNODE_FLAG_TRACED_GUID,
        },
    },
    core::GUID,
};

use crate::dnstap::{RawDnsEvent, Transport};

const PROVIDER: GUID = GUID::from_u128(0xeb79061a_a566_4698_9119_3ed2807060e7);
const MAX_EXTENDED_ITEMS: usize = 64;
const MAX_METADATA: usize = 1_048_576;

#[derive(Default)]
pub(crate) struct CaptureCounters {
    pub(crate) received: AtomicU64,
    pub(crate) dropped: AtomicU64,
    pub(crate) intake_dropped: AtomicU64,
    pub(crate) pool_exhausted: AtomicU64,
    pub(crate) receiver_disconnected: AtomicU64,
    pub(crate) etw_lost: AtomicU64,
    pub(crate) etw_buffers_lost: AtomicU64,
    pub(crate) callback_errors: AtomicU64,
    pub(crate) queue_high_water: AtomicU64,
    pub(crate) stats_valid: AtomicBool,
    pub(crate) stats_errors: AtomicU64,
}

/// Owns the native metadata and an exclusive lease, without borrowed ETW pointers.
pub(crate) struct RawEtwEvent {
    header: EVENT_HEADER,
    buffer_context: ETW_BUFFER_CONTEXT,
    data_length: u16,
    extended_count: u16,
    buffer: PooledBuffer,
}

struct CallbackContext {
    pool: Arc<PoolManager>,
    counters: Arc<CaptureCounters>,
}

struct CallbackProducer {
    context: usize,
    sender: Sender<RawEtwEvent>,
    pool: Arc<PoolManager>,
}

thread_local! {
    // Eagerly installed on the ProcessTrace worker before native delivery starts.
    static PRODUCER: RefCell<Option<CallbackProducer>> = const { RefCell::new(None) };
}

#[repr(C)]
struct SessionProperties {
    properties: EVENT_TRACE_PROPERTIES,
    logger_name: [u16; 1025],
}

impl SessionProperties {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "The fixed native header plus 1025 UTF-16 units fits u32."
    )]
    fn new() -> Self {
        let mut result = Self {
            properties: EVENT_TRACE_PROPERTIES::default(),
            logger_name: [0; 1025],
        };
        result.properties.Wnode.BufferSize = size_of::<Self>() as u32;
        result.properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        // System time, in 100 ns FILETIME units. ProcessTrace also converts timestamps
        // to this representation because RAW_TIMESTAMP is deliberately omitted.
        result.properties.Wnode.ClientContext = 2;
        result.properties.BufferSize = 64;
        result.properties.MinimumBuffers = 4;
        result.properties.MaximumBuffers = 64;
        result.properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        result.properties.FlushTimer = 1;
        result.properties.LoggerNameOffset = offset_of!(Self, logger_name) as u32;
        result
    }
}

/// Owns only the session created by its successful `StartTrace` call.
pub(crate) struct Capture {
    session: Option<CONTROLTRACE_HANDLE>,
    consumer: Option<PROCESSTRACE_HANDLE>,
    worker: Option<JoinHandle<u32>>,
    counters: Arc<CaptureCounters>,
}

impl Capture {
    /// Starts an independent real-time session; an existing name is an error.
    pub(crate) fn start(
        name: &str,
        sender: Sender<RawEtwEvent>,
        pool: Arc<PoolManager>,
        counters: Arc<CaptureCounters>,
    ) -> Result<Self> {
        ensure!(
            !name.is_empty() && !name.contains('\0'),
            "invalid ETW session name"
        );
        let mut name: Vec<u16> = name.encode_utf16().collect();
        ensure!(
            name.len() <= 1024,
            "ETW session name exceeds 1024 UTF-16 units"
        );
        name.push(0);
        let mut properties = SessionProperties::new();
        let mut session = CONTROLTRACE_HANDLE::default();
        // SAFETY: The UTF-16 name and aligned properties (including trailing name
        // storage) remain alive for this synchronous API call.
        let status = unsafe {
            StartTraceW(
                &raw mut session,
                name.as_ptr(),
                &raw mut properties.properties,
            )
        };
        check_status(
            status,
            "StartTraceW (run elevated; use an unused session name)",
        )?;
        let mut capture = Self {
            session: Some(session),
            consumer: None,
            worker: None,
            counters,
        };
        // SAFETY: The successful StartTrace call owns this handle. The provider GUID
        // is static and no optional enable parameters are supplied.
        let status = unsafe {
            EnableTraceEx2(
                session,
                &PROVIDER,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                5,
                0x7ffff,
                0,
                5000,
                null(),
            )
        };
        check_status(status, "EnableTraceEx2")?;

        let mut context = Box::new(CallbackContext {
            pool: Arc::clone(&pool),
            counters: Arc::clone(&capture.counters),
        });
        let mut logfile = EVENT_TRACE_LOGFILEW {
            LoggerName: name.as_mut_ptr(),
            Context: (&raw mut *context).cast(),
            ..Default::default()
        };
        logfile.Anonymous1.ProcessTraceMode =
            PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        logfile.Anonymous2.EventRecordCallback = Some(native_etw_callback);
        // SAFETY: All pointers in logfile remain valid throughout OpenTrace. Context
        // has a stable Box address, retained by the worker until ProcessTrace ends.
        let consumer = unsafe { OpenTraceW(&raw mut logfile) };
        if consumer.Value == u64::MAX {
            // SAFETY: GetLastError is read immediately after the failing API call.
            let error = unsafe { GetLastError() };
            bail!(
                "OpenTraceW failed: {}",
                std::io::Error::from_raw_os_error(error.cast_signed())
            );
        }
        capture.consumer = Some(consumer);
        capture.worker = Some(
            thread::Builder::new()
                .name("etw-consumer".into())
                .spawn(move || {
                    // Keep the callback's pointee alive through the entire native call.
                    let context_address = (&raw const *context) as usize;
                    PRODUCER.with(|state| {
                        *state.borrow_mut() = Some(CallbackProducer {
                            context: context_address,
                            sender,
                            pool,
                        });
                    });
                    let _context = context;
                    // SAFETY: consumer is open. It is closed by Capture to request shutdown;
                    // CloseTrace explicitly permits concurrent real-time ProcessTrace calls.
                    let status = unsafe { ProcessTrace(&raw const consumer, 1, null(), null()) };
                    // No callback can recur after ProcessTrace returns. Dropping
                    // the final sender closes the channel. The supervisor publishes
                    // completion and unparks the polling parser after joining us.
                    PRODUCER.with(|state| {
                        state.borrow_mut().take();
                    });
                    status
                })
                .context("spawn ETW consumer")?,
        );
        Ok(capture)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Refreshes loss statistics for this owned session, without stopping it.
    pub(crate) fn query_statistics(&self) {
        if let Some(session) = self.session {
            let mut properties = SessionProperties::new();
            // SAFETY: This live handle belongs to this Capture. The aligned
            // initialized output includes room for the session name.
            let status = unsafe {
                ControlTraceW(
                    session,
                    null(),
                    &raw mut properties.properties,
                    EVENT_TRACE_CONTROL_QUERY,
                )
            };
            self.store_statistics(status, &properties.properties);
        }
    }

    fn store_statistics(&self, status: u32, properties: &EVENT_TRACE_PROPERTIES) {
        self.counters
            .stats_valid
            .store(status == ERROR_SUCCESS, Ordering::Relaxed);
        if status == ERROR_SUCCESS {
            self.counters
                .etw_lost
                .store(u64::from(properties.EventsLost), Ordering::Relaxed);
            self.counters
                .etw_buffers_lost
                .store(u64::from(properties.RealTimeBuffersLost), Ordering::Relaxed);
        } else {
            self.counters.stats_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Stops collection, waits for native callbacks to end, and reports loss.
    pub(crate) fn stop(&mut self) -> Result<()> {
        let mut failure = None;
        if let Some(session) = self.session.take() {
            let mut properties = SessionProperties::new();
            // SAFETY: Only the handle successfully created above is stopped; null
            // name forces handle lookup. The output buffer is aligned and sized.
            let status = unsafe {
                ControlTraceW(
                    session,
                    null(),
                    &raw mut properties.properties,
                    EVENT_TRACE_CONTROL_STOP,
                )
            };
            self.store_statistics(status, &properties.properties);
            if status != ERROR_SUCCESS {
                // Preserve ownership so an explicit retry or Drop can make a
                // second stop attempt after the consumer has been closed.
                self.session = Some(session);
                failure = Some(anyhow!("ControlTraceW(stop) failed: {status}"));
            }
        }
        if let Some(consumer) = self.consumer.take() {
            // SAFETY: This is the OpenTrace handle, closed exactly once. Native
            // callbacks and their Box remain alive until the worker joins below.
            let status = unsafe { CloseTrace(consumer) };
            if status != ERROR_SUCCESS && status != ERROR_CTX_CLOSE_PENDING && failure.is_none() {
                failure = Some(anyhow!("CloseTrace failed: {status}"));
            }
        }
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(ERROR_SUCCESS | ERROR_CANCELLED) => {}
                Ok(status) if failure.is_none() => {
                    failure = Some(anyhow!("ProcessTrace failed: {status}"));
                }
                Err(_) if failure.is_none() => {
                    failure = Some(anyhow!("ETW consumer thread panicked"));
                }
                _ => {}
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // stop is also needed on partially initialized startup failures.
        if let Err(error) = self.stop() {
            eprintln!("ETW cleanup: {error:#}");
        }
    }
}

fn supported(id: u16) -> bool {
    matches!(id, 256 | 257 | 258 | 260 | 261)
}

fn dns_provider(guid: &GUID) -> bool {
    guid.data1 == PROVIDER.data1
        && guid.data2 == PROVIDER.data2
        && guid.data3 == PROVIDER.data3
        && guid.data4 == PROVIDER.data4
}

unsafe extern "system" fn native_etw_callback(record: *mut EVENT_RECORD) {
    // SAFETY: ETW invokes this callback with an EVENT_RECORD valid for the duration
    // of this call. A null defensive check precedes dereferencing the pointer.
    let Some(record) = (unsafe { record.as_ref() }) else {
        return;
    };
    // SAFETY: OpenTrace received this stable Box pointee, retained until all
    // ProcessTrace callbacks finish; it is shared immutably across callbacks.
    let Some(context) = (unsafe { record.UserContext.cast::<CallbackContext>().as_ref() }) else {
        return;
    };
    if !dns_provider(&record.EventHeader.ProviderId)
        || !supported(record.EventHeader.EventDescriptor.Id)
    {
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
                Err(CopyError::Exhausted) => {
                    context
                        .counters
                        .pool_exhausted
                        .fetch_add(1, Ordering::Relaxed);
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            };
            match producer.sender.try_send(owned) {
                Ok(()) => {
                    context
                        .counters
                        .queue_high_water
                        .fetch_max(producer.sender.len() as u64, Ordering::Relaxed);
                }
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
enum CopyError {
    Malformed,
    Exhausted,
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
unsafe fn copy_record(
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
    let mut buffer = pool.acquire(length).ok_or(CopyError::Exhausted)?;
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

fn check_status(status: u32, operation: &str) -> Result<()> {
    if status != ERROR_SUCCESS {
        bail!(
            "{operation}: {} (code {status})",
            std::io::Error::from_raw_os_error(status.cast_signed())
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PropertyType {
    input: i32,
    output: i32,
}
#[derive(Clone, Copy)]
struct Schema {
    tcp: PropertyType,
    local: PropertyType,
    peer: PropertyType,
    port: PropertyType,
    packet: PropertyType,
}

thread_local! {
    // Bound schema cache independently of untrusted event versions.
    static SCHEMAS: RefCell<HashMap<(u16, u8), Schema>> = RefCell::new(HashMap::new());
}

/// Decodes named manifest properties using TDH, never rendered XML or fixed offsets.
#[expect(
    clippy::needless_pass_by_value,
    reason = "Consumes the queued record and frees its storage at the parser boundary."
)]
pub(crate) fn parse(event: RawEtwEvent) -> Result<Option<RawDnsEvent>> {
    let id = event.header.EventDescriptor.Id;
    if !dns_provider(&event.header.ProviderId) || !supported(id) {
        return Ok(None);
    }
    event.with_record(parse_record)
}

impl RawEtwEvent {
    /// Builds a scoped TDH view while this event retains its exclusive lease.
    /// The record and repaired raw pointers are valid only during `operation`.
    fn with_record<T>(&self, operation: impl FnOnce(&EVENT_RECORD) -> T) -> T {
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
                self.buffer.bytes().cast_mut().cast()
            },
            UserContext: null_mut(),
        };
        operation(&record)
    }
}

fn parse_record(record: &EVENT_RECORD) -> Result<Option<RawDnsEvent>> {
    let id = record.EventHeader.EventDescriptor.Id;
    let peer_name = if matches!(id, 256 | 261) {
        PropertyName::Source
    } else {
        PropertyName::Destination
    };
    let schema = SCHEMAS
        .with(|cache| -> Result<Schema> {
            let key = (id, record.EventHeader.EventDescriptor.Version);
            let mut cache = cache.borrow_mut();
            if let Some(schema) = cache.get(&key) {
                return Ok(*schema);
            }
            let schema = load_schema(record, peer_name)?;
            if cache.len() >= 32 {
                cache.clear();
            }
            cache.insert(key, schema);
            Ok(schema)
        })
        .with_context(|| {
            format!(
                "DNS event {id} version {} schema",
                record.EventHeader.EventDescriptor.Version
            )
        })?;
    let tcp = integer(&property(record, PropertyName::Tcp, 4)?, schema.tcp)?;
    ensure!(tcp <= 1, "invalid TCP Boolean");
    let port = integer(&property(record, PropertyName::Port, 4)?, schema.port)?;
    let port = u16::try_from(port).context("Port exceeds u16")?;
    let local = address(
        &property(record, PropertyName::InterfaceIp, 512)?,
        schema.local,
    )?;
    let peer = address(&property(record, peer_name, 512)?, schema.peer)?;
    ensure!(
        schema.packet.input == TDH_INTYPE_BINARY,
        "PacketData is not binary"
    );
    let packet_data = property(record, PropertyName::PacketData, 65535)?;
    ensure!(
        packet_data.len() >= 12,
        "PacketData shorter than a DNS header"
    );
    Ok(Some(RawDnsEvent {
        event_id: id,
        timestamp: timestamp(record.EventHeader.TimeStamp)?,
        transport: if tcp == 1 {
            Transport::Tcp
        } else {
            Transport::Udp
        },
        peer_address: observed_address(peer),
        peer_port: observed_port(port),
        local_address: observed_address(local),
        local_port: None,
        packet_data,
    }))
}

// DNS Server can report wildcard interface addresses and Port=0 for recursive
// traffic. These mean unavailable metadata, not observed socket endpoints.
fn observed_address(address: IpAddr) -> Option<IpAddr> {
    (!address.is_unspecified()).then_some(address)
}

fn observed_port(port: u16) -> Option<u16> {
    (port != 0).then_some(port)
}

fn load_schema(record: &EVENT_RECORD, peer_name: PropertyName) -> Result<Schema> {
    let mut size = 0;
    // SAFETY: record and all its backing buffers remain alive for this TDH call.
    let status = unsafe { TdhGetEventInformation(record, 0, null(), null_mut(), &raw mut size) };
    ensure!(
        status == ERROR_INSUFFICIENT_BUFFER,
        "TDH metadata size query failed: {status}"
    );
    ensure!(
        (size_of::<TRACE_EVENT_INFO>()..=MAX_METADATA).contains(&(size as usize)),
        "invalid TDH metadata size"
    );
    // u64 storage supplies the alignment required by TRACE_EVENT_INFO and its
    // flexible EVENT_PROPERTY_INFO array, unlike a cast from a byte Vec.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let capacity = size;
    // SAFETY: storage is aligned, initialized, and at least size bytes long.
    let status = unsafe {
        TdhGetEventInformation(
            record,
            0,
            null(),
            storage.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    check_status(status, "TdhGetEventInformation")?;
    ensure!(
        size <= capacity && size as usize >= size_of::<TRACE_EVENT_INFO>(),
        "invalid TDH metadata result size"
    );
    // SAFETY: The successful API initialized this aligned header in bounds.
    let info = unsafe { &*storage.as_ptr().cast::<TRACE_EVENT_INFO>() };
    ensure!(
        info.DecodingSource == DecodingSourceXMLFile,
        "DNS provider requires a manifest schema"
    );
    ensure!(
        dns_provider(&info.ProviderGuid),
        "TDH returned a different provider schema"
    );
    ensure!(
        info.EventDescriptor.Id == record.EventHeader.EventDescriptor.Id
            && info.EventDescriptor.Version == record.EventHeader.EventDescriptor.Version,
        "TDH returned a different event schema"
    );
    let array_offset = offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
    let count = info.TopLevelPropertyCount as usize;
    ensure!(
        count <= info.PropertyCount as usize,
        "invalid TDH property count"
    );
    let end = count
        .checked_mul(size_of::<EVENT_PROPERTY_INFO>())
        .and_then(|n| array_offset.checked_add(n))
        .context("TDH property array overflow")?;
    ensure!(end <= size as usize, "TDH property array exceeds metadata");
    // SAFETY: The offset has native struct alignment, and the count fits the
    // initialized TDH allocation as checked above.
    let properties = unsafe {
        std::slice::from_raw_parts(
            storage
                .as_ptr()
                .byte_add(array_offset)
                .cast::<EVENT_PROPERTY_INFO>(),
            count,
        )
    };
    // SAFETY: The successful TDH result initialized size bytes inside storage.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize) };
    let find = |name| find_property(properties, bytes, name);
    Ok(Schema {
        tcp: find(PropertyName::Tcp.name())?,
        local: find(PropertyName::InterfaceIp.name())?,
        peer: find(peer_name.name())?,
        port: find(PropertyName::Port.name())?,
        packet: find(PropertyName::PacketData.name())?,
    })
}

fn find_property(
    properties: &[EVENT_PROPERTY_INFO],
    bytes: &[u8],
    name: &str,
) -> Result<PropertyType> {
    for prop in properties {
        let offset = prop.NameOffset as usize;
        ensure!(
            offset < bytes.len() && offset.is_multiple_of(2),
            "invalid TDH property name offset"
        );
        let units: Vec<u16> = bytes[offset..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|u| *u != 0)
            .collect();
        ensure!(
            offset + units.len() * 2 + 2 <= bytes.len(),
            "unterminated TDH property name"
        );
        if units.iter().copied().eq(name.encode_utf16()) {
            ensure!(
                prop.Flags & (PropertyStruct | PropertyHasCustomSchema | PropertyParamCount) == 0,
                "unsupported structured property {name}"
            );
            // SAFETY: Flags above select the nonStructType and fixed-count
            // union members in the documented EVENT_PROPERTY_INFO layout.
            let (native, count) = unsafe { (prop.Anonymous1.nonStructType, prop.Anonymous2.count) };
            ensure!(count <= 1, "unsupported property array {name}");
            return Ok(PropertyType {
                input: i32::from(native.InType),
                output: i32::from(native.OutType),
            });
        }
    }
    bail!("missing property {name}")
}

/// Closed set of manifest names; callers cannot supply an unterminated pointer.
#[derive(Clone, Copy)]
enum PropertyName {
    Tcp,
    Port,
    InterfaceIp,
    Source,
    Destination,
    PacketData,
}

impl PropertyName {
    fn name(self) -> &'static str {
        self.native().0
    }

    fn native(self) -> (&'static str, *const u16) {
        // w! emits static, nul-terminated UTF-16 storage at compile time. Each
        // arm binds the display name and native spelling as one private value.
        match self {
            Self::Tcp => ("TCP", windows_sys::core::w!("TCP")),
            Self::Port => ("Port", windows_sys::core::w!("Port")),
            Self::InterfaceIp => ("InterfaceIP", windows_sys::core::w!("InterfaceIP")),
            Self::Source => ("Source", windows_sys::core::w!("Source")),
            Self::Destination => ("Destination", windows_sys::core::w!("Destination")),
            Self::PacketData => ("PacketData", windows_sys::core::w!("PacketData")),
        }
    }

    fn descriptor(self) -> PROPERTY_DATA_DESCRIPTOR {
        PROPERTY_DATA_DESCRIPTOR {
            PropertyName: self.native().1 as u64,
            ArrayIndex: u32::MAX,
            Reserved: 0,
        }
    }
}

fn property(record: &EVENT_RECORD, name: PropertyName, maximum: usize) -> Result<Vec<u8>> {
    let descriptor = name.descriptor();
    let mut size = 0;
    // SAFETY: All record backing data and the terminated property name are alive;
    // the descriptor requests a complete top-level property with no TDH contexts.
    let status =
        unsafe { TdhGetPropertySize(record, 0, null(), 1, &raw const descriptor, &raw mut size) };
    check_status(status, "TdhGetPropertySize")?;
    ensure!(
        size as usize <= maximum,
        "TDH property exceeds its size limit"
    );
    let mut bytes = vec![0; size as usize];
    // SAFETY: bytes has exactly the buffer length returned by TDH and every
    // pointer remains valid during this synchronous extraction call.
    let status = unsafe {
        TdhGetProperty(
            record,
            0,
            null(),
            1,
            &raw const descriptor,
            size,
            bytes.as_mut_ptr(),
        )
    };
    check_status(status, "TdhGetProperty")?;
    Ok(bytes)
}

fn integer(bytes: &[u8], kind: PropertyType) -> Result<u32> {
    // TDH's Port output type is explicitly network byte order.
    if kind.output == TDH_OUTTYPE_PORT {
        ensure!(kind.input == TDH_INTYPE_UINT16, "invalid Port input type");
        let bytes = <[u8; 2]>::try_from(bytes).context("invalid Port byte length")?;
        return Ok(u32::from(u16::from_be_bytes(bytes)));
    }
    match (kind.input, bytes) {
        (TDH_INTYPE_UINT8, [value]) => Ok(u32::from(*value)),
        (TDH_INTYPE_UINT16, [a, b]) => Ok(u32::from(u16::from_le_bytes([*a, *b]))),
        (TDH_INTYPE_UINT32 | TDH_INTYPE_BOOLEAN, [a, b, c, d]) => {
            Ok(u32::from_le_bytes([*a, *b, *c, *d]))
        }
        _ => bail!("unexpected integer property type or length"),
    }
}

fn address(bytes: &[u8], kind: PropertyType) -> Result<IpAddr> {
    if kind.output == TDH_OUTTYPE_IPV4 && bytes.len() == 4 {
        return Ok(IpAddr::from(<[u8; 4]>::try_from(bytes)?));
    }
    if kind.output == TDH_OUTTYPE_IPV6 && bytes.len() == 16 {
        return Ok(IpAddr::from(<[u8; 16]>::try_from(bytes)?));
    }
    let text = match kind.input {
        TDH_INTYPE_UNICODESTRING => {
            ensure!(bytes.len().is_multiple_of(2), "odd Unicode address length");
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            ensure!(units.last() == Some(&0), "unterminated Unicode address");
            String::from_utf16(&units[..units.len() - 1]).context("invalid address UTF-16")?
        }
        TDH_INTYPE_ANSISTRING => {
            ensure!(bytes.last() == Some(&0), "unterminated address");
            std::str::from_utf8(&bytes[..bytes.len() - 1])?.to_owned()
        }
        _ => bail!("unsupported address property type"),
    };
    // A zone index is routing context rather than part of the dnstap address.
    let address = text
        .split_once('%')
        .map_or(text.as_str(), |(address, _)| address);
    address.parse().context("invalid DNS event IP address")
}

fn timestamp(filetime: i64) -> Result<SystemTime> {
    const EPOCH: i64 = 116_444_736_000_000_000;
    let ticks = u64::try_from(filetime.checked_sub(EPOCH).context("timestamp overflow")?)
        .context("event timestamp predates Unix epoch")?;
    UNIX_EPOCH
        .checked_add(Duration::new(
            ticks / 10_000_000,
            (ticks % 10_000_000) as u32 * 100,
        ))
        .context("event timestamp exceeds SystemTime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Etw::EVENT_DESCRIPTOR;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct AllocCounts {
        allocations: u64,
        reallocations: u64,
        deallocations: u64,
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
        unsafe fn realloc(
            &self,
            pointer: *mut u8,
            layout: std::alloc::Layout,
            size: usize,
        ) -> *mut u8 {
            allocation_tick(1);
            // SAFETY: Caller supplies the live allocation and a valid new size.
            unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, pointer, layout, size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator;

    fn measured(operation: impl FnOnce()) -> AllocCounts {
        ALLOC_COUNTS.with(|counts| counts.set(AllocCounts::default()));
        TRACK_ALLOCATIONS.with(|track| track.set(true));
        operation();
        TRACK_ALLOCATIONS.with(|track| track.set(false));
        ALLOC_COUNTS.with(std::cell::Cell::get)
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
    fn fixed_property_descriptors_are_terminated_static_and_allocation_free() {
        let names = [
            PropertyName::Tcp,
            PropertyName::Port,
            PropertyName::InterfaceIp,
            PropertyName::Source,
            PropertyName::Destination,
            PropertyName::PacketData,
        ];
        let counts = measured(|| {
            for name in names {
                let (display, wide) = name.native();
                let descriptor = name.descriptor();
                assert_eq!(descriptor.ArrayIndex, u32::MAX);
                assert_eq!(descriptor.Reserved, 0);
                // Constant UTF-16 literals can be duplicated after inlining;
                // correctness depends on static lifetime and content, not identity.
                for pointer in [wide, descriptor.PropertyName as *const u16] {
                    for (index, expected) in display.encode_utf16().chain(Some(0)).enumerate() {
                        // SAFETY: Both pointers come from terminated w! literals.
                        // Prior iterations matched nonzero units; a shorter native
                        // spelling fails at its terminator before advancing.
                        let actual = unsafe { pointer.add(index).read() };
                        assert_eq!(actual, expected, "static name spelling or terminator");
                    }
                }
            }
        });
        assert_eq!(counts, AllocCounts::default());
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
        drop(event);
        assert!(
            pool.acquire(1).is_some(),
            "slot returns after view and event end"
        );
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
            let mut extra: Vec<Vec<u8>> = (0..64u8)
                .map(|index| {
                    let size = if index.is_multiple_of(8) {
                        0
                    } else {
                        usize::from(index % 5 + 1)
                    };
                    vec![index; size]
                })
                .collect();
            let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); 64];
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
                ExtendedDataCount: 64,
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
                assert_eq!(record.ExtendedDataCount, 64);
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
                        std::slice::from_raw_parts(record.ExtendedData, 64),
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
                    let pointer =
                        usize::try_from(item.DataPtr).expect("native pointer fits target");
                    assert!(pointer >= base + 37 && pointer + size <= base + pool::REGULAR_BYTES);
                    assert_eq!(pointer % 8, 0);
                    // SAFETY: The pointer is repaired into the retained initialized
                    // lease range and checked in bounds immediately above.
                    let bytes =
                        unsafe { std::slice::from_raw_parts(item.DataPtr as *const u8, size) };
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
    fn filetime_conversion_preserves_100ns_precision() -> Result<()> {
        assert_eq!(
            timestamp(116_444_736_010_000_001)?,
            UNIX_EPOCH + Duration::new(1, 100)
        );
        assert!(timestamp(116_444_735_999_999_999).is_err());
        Ok(())
    }

    #[test]
    fn unknown_provider_endpoints_stay_absent_and_known_values_survive() -> Result<()> {
        assert_eq!(observed_address("0.0.0.0".parse()?), None);
        assert_eq!(observed_address("::".parse()?), None);
        assert_eq!(observed_port(0), None);
        let ipv4: IpAddr = "172.27.215.60".parse()?;
        let ipv6: IpAddr = "2001:db8::53".parse()?;
        assert_eq!(observed_address(ipv4), Some(ipv4));
        assert_eq!(observed_address(ipv6), Some(ipv6));
        assert_eq!(observed_port(53), Some(53));
        Ok(())
    }

    #[test]
    fn rejects_malformed_addresses_and_boolean_encodings() {
        let unicode = PropertyType {
            input: TDH_INTYPE_UNICODESTRING,
            output: 0,
        };
        assert!(address(&[0x31, 0], unicode).is_err());
        assert!(address(&[0], unicode).is_err());
        assert!(
            integer(
                &[1],
                PropertyType {
                    input: TDH_INTYPE_BOOLEAN,
                    output: 0
                }
            )
            .is_err()
        );
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

    struct Fixture {
        context: Box<CallbackContext>,
        record: EVENT_RECORD,
        receive: crossbeam_channel::Receiver<RawEtwEvent>,
    }

    impl Fixture {
        fn new(capacity: usize, jumbo: usize) -> Self {
            let pool = PoolManager::new(capacity, jumbo).expect("test pool initializes");
            let mut context = Box::new(CallbackContext {
                pool: Arc::clone(&pool),
                counters: Arc::default(),
            });
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
        fixture.record.ExtendedDataCount = 65;
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
        let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); 64];
        // Keep the aggregate extended length at 65535 and exercise alignment gaps.
        let mut offset = 0usize;
        for (index, item) in items.iter_mut().enumerate() {
            let size = if index == 63 { 65535 - 63 * 1025 } else { 1025 };
            item.DataSize = u16::try_from(size).expect("bounded item size");
            // SAFETY: Offset traverses the live allocation, including final item.
            item.DataPtr = unsafe { extra.as_ptr().add(offset) } as u64;
            offset += size;
        }
        let record = EVENT_RECORD {
            UserData: payload.as_mut_ptr().cast(),
            UserDataLength: 65535,
            ExtendedData: items.as_mut_ptr(),
            ExtendedDataCount: 64,
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
    fn decodes_host_and_network_order_ports_and_binary_addresses() -> Result<()> {
        let native_port = PropertyType {
            input: TDH_INTYPE_UINT16,
            output: 0,
        };
        let network_port = PropertyType {
            output: TDH_OUTTYPE_PORT,
            ..native_port
        };
        assert_eq!(integer(&53u16.to_le_bytes(), native_port)?, 53);
        assert_eq!(integer(&53u16.to_be_bytes(), network_port)?, 53);
        assert_eq!(
            address(
                &[192, 0, 2, 1],
                PropertyType {
                    input: TDH_INTYPE_UINT32,
                    output: TDH_OUTTYPE_IPV4,
                }
            )?,
            "192.0.2.1".parse::<IpAddr>()?
        );
        Ok(())
    }
}
