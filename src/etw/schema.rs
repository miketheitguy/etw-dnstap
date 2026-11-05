//! TDH schema loading and the event [`Parser`].
//!
//! TDH supplies each (event ID, version) schema once; the schema must compile into
//! a single-pass [`DecodePlan`] or its events are rejected. There is no per-event
//! TDH reader. The parser yields a borrowed, format-neutral [`DecodedDnsEvent`];
//! encoding for a particular output format happens downstream.

use std::{
    mem::{offset_of, size_of},
    net::IpAddr,
    ptr::{null, null_mut},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    System::Diagnostics::Etw::{
        DecodingSourceXMLFile, EVENT_HEADER, EVENT_PROPERTY_INFO, EVENT_RECORD,
        PropertyHasCustomSchema, PropertyParamCount, PropertyParamFixedCount,
        PropertyParamFixedLength, PropertyParamLength, PropertyStruct, TDH_INTYPE_ANSISTRING,
        TDH_INTYPE_BOOLEAN, TDH_INTYPE_UINT8, TDH_INTYPE_UINT16, TDH_INTYPE_UINT32,
        TDH_INTYPE_UNICODESTRING, TDH_OUTTYPE_IPV4, TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT,
        TRACE_EVENT_INFO, TdhGetEventInformation,
    },
};

use super::{
    MAX_METADATA, RawEtwEvent, check_status,
    decode::{DecodePlan, FieldSpec},
    dns_provider, supported,
};
use crate::event::{DecodedDnsEvent, DnsEventMetadata, Transport};

/// Schemas cached per parser; the cache is cleared when it fills.
const MAX_CACHED_SCHEMAS: usize = 8;
/// Further events of a key whose TDH metadata did not load that are rejected
/// from the cache before the parser loads that schema again. It is a count of
/// events, not a time, so the reject path reads no clock.
const SCHEMA_RETRY_EVENTS: u32 = 65_536;
/// The `EVENT_PROPERTY_INFO.Flags` bits (`PROPERTY_FLAGS` in `tdh.h`) the
/// single-pass compiler interprets: the length and count arms, fixed or by
/// index. A property with any other bit set is kept as an opaque name.
const SUPPORTED_PROPERTY_FLAGS: u32 =
    (PropertyParamLength | PropertyParamCount | PropertyParamFixedLength | PropertyParamFixedCount)
        .cast_unsigned();
const _: () = assert!(SUPPORTED_PROPERTY_FLAGS == 0x36);

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;

#[derive(Clone, Copy)]
struct PropertyType {
    input: i32,
    output: i32,
}
/// A schema that compiled into a single-pass plan. Schemas that do not compile
/// are never cached here; their keys go to `Parser::rejected_keys`.
struct Schema {
    plan: DecodePlan,
    tcp: PropertyType,
    local: PropertyType,
    peer: PropertyType,
    port: PropertyType,
}

/// Why a schema did not load. The class decides whether the key is retried.
#[derive(Debug)]
enum SchemaFailure {
    /// `TdhGetEventInformation` returned a Win32 error, for example because the
    /// provider manifest is not registered yet. The condition can pass, so the
    /// load is retried after [`SCHEMA_RETRY_EVENTS`] further rejected events.
    Load(anyhow::Error),
    /// TDH returned metadata that this program cannot use: not a manifest
    /// schema, another provider or event, a malformed layout, or a schema that
    /// does not compile into a single-pass plan. The same metadata gives the
    /// same result, so the key is not retried.
    Unsupported(anyhow::Error),
}

/// How later events of a rejected key are treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rejection {
    /// Rejected until the rejected-key list is cleared because it is full.
    Unsupported,
    /// Rejected for `remaining` further events; the next event after those
    /// loads the schema again.
    LoadFailed { remaining: u32 },
}

/// A key whose schema failed, and how its events are treated now.
#[derive(Clone, Copy, Debug)]
struct RejectedKey {
    key: (u16, u8),
    rejection: Rejection,
    /// TDH load failures of this key so far, for the bounded log.
    load_failures: u32,
}

impl RejectedKey {
    /// Counts one event against the key. Returns true while the event is
    /// rejected from the cache, and false when the retry budget is spent and the
    /// caller loads the schema again.
    const fn hold(&mut self) -> bool {
        match &mut self.rejection {
            Rejection::Unsupported => true,
            Rejection::LoadFailed { remaining: 0 } => false,
            Rejection::LoadFailed { remaining } => {
                *remaining -= 1;
                true
            }
        }
    }
}

/// Reuses initialized text storage on the parser thread.
pub(crate) struct Parser {
    // Owned plans permit borrowing cache hits without allocation or refcount work.
    // Only events 256, 257, and 258 reach this cache, usually with one version each, so a
    // linear scan over at most MAX_CACHED_SCHEMAS entries beats hashing.
    schemas: Vec<((u16, u8), Schema)>,
    // At most 255 nonterminating UTF-16 units, each requiring at most 3 UTF-8 bytes.
    text: [u8; 768],
    // Keys whose schema failed to load or did not compile into a plan. Without
    // this every such event would repeat the TDH calls and their allocations on
    // the parser thread. An unsupported schema stays here; a TDH load failure is
    // retried after SCHEMA_RETRY_EVENTS further events. Bounded like the
    // positive cache and reserved up front, so no update allocates.
    rejected_keys: Vec<RejectedKey>,
    // Events rejected without a TDH call because their key is in `rejected_keys`,
    // plus each failed load (the first failure of a key and each failed retry).
    rejected_events: u64,
    // Set once shutdown has begun: a rejection is still counted but not logged.
    quiet: bool,
}

impl Parser {
    pub(crate) fn new() -> Self {
        Self {
            quiet: false,
            rejected_keys: Vec::with_capacity(MAX_CACHED_SCHEMAS),
            rejected_events: 0,
            schemas: Vec::with_capacity(MAX_CACHED_SCHEMAS),
            text: [0; 768],
        }
    }

    /// Stores a schema, clearing the cache first when it is full.
    /// Capacity was reserved at construction, so insertion never reallocates.
    fn insert_schema(&mut self, key: (u16, u8), schema: Schema) {
        if self.schemas.len() >= MAX_CACHED_SCHEMAS {
            self.schemas.clear();
        }
        self.schemas.push((key, schema));
    }

    /// Events rejected because their (event id, version) schema failed to load or
    /// did not compile into a single-pass plan.
    pub(crate) const fn schema_rejected_events(&self) -> u64 {
        self.rejected_events
    }

    /// Stops the schema warnings once shutdown has begun. Rejections stay
    /// counted.
    pub(crate) const fn set_quiet(&mut self, quiet: bool) {
        self.quiet = quiet;
    }

    /// Remembers a failed schema key, clearing the list first when it is full.
    fn remember_rejected(&mut self, entry: RejectedKey) {
        if self.rejected_keys.len() >= MAX_CACHED_SCHEMAS {
            self.rejected_keys.clear();
        }
        self.rejected_keys.push(entry);
    }

    /// Records a failed load of `key` and returns the error for the caller.
    /// `retried` is the index of the key's entry when this load was a retry; that
    /// entry is then updated in place. The log is bounded: an unsupported schema
    /// is logged once per key, and a load failure on the first four failures of
    /// its key and then on every 60th.
    fn reject(
        &mut self,
        key: (u16, u8),
        retried: Option<usize>,
        failure: SchemaFailure,
    ) -> anyhow::Error {
        self.rejected_events += 1;
        let prior_failures = retried.map_or(0, |index| self.rejected_keys[index].load_failures);
        let (rejection, load_failures, error) = match failure {
            SchemaFailure::Unsupported(error) => {
                if !self.quiet {
                    tracing::warn!(
                        event_id = key.0,
                        version = key.1,
                        "DNS event schema is unsupported and its events are rejected: {error:#}"
                    );
                }
                (Rejection::Unsupported, prior_failures, error)
            }
            SchemaFailure::Load(error) => {
                if (prior_failures < 4 || prior_failures.is_multiple_of(60)) && !self.quiet {
                    tracing::warn!(
                        event_id = key.0,
                        version = key.1,
                        failures = prior_failures.saturating_add(1),
                        retry_after_events = SCHEMA_RETRY_EVENTS,
                        "DNS event schema load failed; its events are rejected until the load \
                        is retried: {error:#}"
                    );
                }
                (
                    Rejection::LoadFailed {
                        remaining: SCHEMA_RETRY_EVENTS,
                    },
                    prior_failures.saturating_add(1),
                    error,
                )
            }
        };
        let entry = RejectedKey {
            key,
            rejection,
            load_failures,
        };
        match retried {
            Some(index) => self.rejected_keys[index] = entry,
            None => self.remember_rejected(entry),
        }
        error.context(format!("DNS event {} version {} schema", key.0, key.1))
    }

    /// Decodes one event into a format-neutral view, or `None` for events this
    /// program does not capture. The view borrows only the event's pooled lease,
    /// so the caller keeps `event` alive until it has consumed the view and then
    /// drops it to recycle the lease. A schema-cache hit makes no TDH call and
    /// allocates nothing.
    pub(crate) fn parse<'e>(
        &mut self,
        event: &'e RawEtwEvent,
    ) -> Result<Option<DecodedDnsEvent<'e>>> {
        let header = event.header();
        let id = header.EventDescriptor.Id;
        if !dns_provider(&header.ProviderId) || !supported(id) {
            return Ok(None);
        }
        let Some(peer_name) = peer_property(id) else {
            return Ok(None);
        };
        let key = (id, header.EventDescriptor.Version);
        if let Some((_, schema)) = self.schemas.iter().find(|(cached, _)| *cached == key) {
            return parse_fast(schema, event.header(), event.payload(), &mut self.text).map(Some);
        }
        // Only a cache miss reaches this scan, so the accepted path does no more work.
        let retried = self
            .rejected_keys
            .iter()
            .position(|rejected| rejected.key == key);
        if let Some(index) = retried
            && self.rejected_keys[index].hold()
        {
            self.rejected_events += 1;
            bail!(
                "DNS event {id} version {} schema was rejected earlier (cached)",
                key.1
            );
        }
        let loaded = match event.with_record(|record| load_schema(record, peer_name)) {
            Ok(schema) => schema,
            Err(failure) => return Err(self.reject(key, retried, failure)),
        };
        // A retry that loads clears the key.
        if let Some(index) = retried {
            self.rejected_keys.swap_remove(index);
        }
        self.insert_schema(key, loaded);
        let cached = self
            .schemas
            .last()
            .map(|(_, schema)| schema)
            .context("missing cached DNS schema")?;
        parse_fast(cached, event.header(), event.payload(), &mut self.text).map(Some)
    }
}

/// The property that holds the peer address: event 256 names it `Source`;
/// 257 and 258 name it `Destination`.
const fn peer_property(id: u16) -> Option<PropertyName> {
    match id {
        256 => Some(PropertyName::Source),
        257 | 258 => Some(PropertyName::Destination),
        _ => None,
    }
}

fn parse_fast<'a>(
    schema: &Schema,
    header: &EVENT_HEADER,
    payload: &'a [u8],
    text: &mut [u8; 768],
) -> Result<DecodedDnsEvent<'a>> {
    // A malformed payload is rejected here; there is no TDH retry.
    let decoded = schema
        .plan
        .decode(payload)
        .map_err(|error| anyhow!("invalid DNS payload: {error:?}"))?;
    let tcp = integer(decoded.tcp, schema.tcp)?;
    ensure!(tcp <= 1, "invalid TCP Boolean");
    let port = u16::try_from(integer(decoded.port, schema.port)?).context("port exceeds u16")?;
    ensure!(
        decoded.local.len() <= 512 && decoded.peer.len() <= 512,
        "DNS address exceeds size limit"
    );
    let local = address(decoded.local, schema.local, text)?;
    let peer = address(decoded.peer, schema.peer, text)?;
    Ok(DecodedDnsEvent {
        metadata: DnsEventMetadata {
            event_id: header.EventDescriptor.Id,
            timestamp: timestamp(header.TimeStamp)?,
            transport: if tcp == 1 {
                Transport::Tcp
            } else {
                Transport::Udp
            },
            peer_address: observed_address(peer),
            peer_port: observed_port(port),
            local_address: observed_address(local),
            local_port: None,
        },
        packet: decoded.packet,
    })
}

// DNS Server can report wildcard interface addresses and Port=0 for recursive
// traffic. These mean unavailable metadata, not observed socket endpoints.
fn observed_address(address: IpAddr) -> Option<IpAddr> {
    (!address.is_unspecified()).then_some(address)
}

fn observed_port(port: u16) -> Option<u16> {
    (port != 0).then_some(port)
}

/// Loads and compiles the schema of `record`. A Win32 error from
/// `TdhGetEventInformation` is a [`SchemaFailure::Load`]; every other failure
/// is a [`SchemaFailure::Unsupported`].
fn load_schema(record: &EVENT_RECORD, peer_name: PropertyName) -> Result<Schema, SchemaFailure> {
    let mut size = 0;
    #[cfg(test)]
    tdh_tick();
    // SAFETY: record and all its backing buffers remain alive for this TDH call.
    let status = unsafe { TdhGetEventInformation(record, 0, null(), null_mut(), &raw mut size) };
    if status != ERROR_INSUFFICIENT_BUFFER {
        return Err(SchemaFailure::Load(anyhow!(
            "TDH metadata size query failed: {}",
            std::io::Error::from_raw_os_error(status.cast_signed())
        )));
    }
    if !(size_of::<TRACE_EVENT_INFO>()..=MAX_METADATA).contains(&(size as usize)) {
        return Err(SchemaFailure::Unsupported(anyhow!(
            "invalid TDH metadata size"
        )));
    }
    // u64 storage supplies the alignment required by TRACE_EVENT_INFO and its
    // flexible EVENT_PROPERTY_INFO array, unlike a cast from a byte Vec.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let capacity = size;
    #[cfg(test)]
    tdh_tick();
    // SAFETY: storage is aligned, initialized, and at least size bytes long.
    let fill_status = unsafe {
        TdhGetEventInformation(
            record,
            0,
            null(),
            storage.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    check_status(fill_status, "TdhGetEventInformation").map_err(SchemaFailure::Load)?;
    compile_metadata(record, &storage, size, capacity, peer_name)
        .map_err(SchemaFailure::Unsupported)
}

/// Checks the metadata that `TdhGetEventInformation` wrote into `storage` and
/// compiles it. `size` is the byte count TDH reported and `capacity` the byte
/// count it was given. The size checks below keep every read inside `storage`.
fn compile_metadata(
    record: &EVENT_RECORD,
    storage: &[u64],
    size: u32,
    capacity: u32,
    peer_name: PropertyName,
) -> Result<Schema> {
    ensure!(
        size <= capacity
            && size as usize <= size_of_val(storage)
            && size as usize >= size_of::<TRACE_EVENT_INFO>(),
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
    validate_property_array(info.PropertyCount as usize, count, size as usize)?;
    // SAFETY: array_offset lies inside the allocation: size covers at least the
    // fixed TRACE_EVENT_INFO header, which ends with the property array.
    let array = unsafe { storage.as_ptr().byte_add(array_offset) }.cast::<EVENT_PROPERTY_INFO>();
    // SAFETY: The array has native struct alignment, and count entries fit the
    // initialized TDH allocation as checked above.
    let properties = unsafe { std::slice::from_raw_parts(array, count) };
    // SAFETY: The successful TDH result initialized size bytes inside storage.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize) };
    compile_schema(properties, bytes, peer_name)
}

/// Builds a cached schema from TDH property metadata, or fails when the schema
/// does not compile into a single-pass plan. There is no per-event TDH fallback.
fn compile_schema(
    properties: &[EVENT_PROPERTY_INFO],
    bytes: &[u8],
    peer_name: PropertyName,
) -> Result<Schema> {
    let find = |name| find_property(properties, bytes, name);
    let plan = normalize_fields(properties, bytes)?
        .and_then(|fields| DecodePlan::compile(&fields, peer_name.name()))
        .context("schema does not match the single-pass decoder grammar")?;
    Ok(Schema {
        plan,
        tcp: find(PropertyName::Tcp.name())?,
        local: find(PropertyName::InterfaceIp.name())?,
        peer: find(peer_name.name())?,
        port: find(PropertyName::Port.name())?,
    })
}

fn validate_property_array(total: usize, top_level: usize, size: usize) -> Result<()> {
    ensure!(top_level <= total, "invalid TDH property count");
    let end = total
        .checked_mul(size_of::<EVENT_PROPERTY_INFO>())
        .and_then(|length| offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray).checked_add(length))
        .context("TDH property array overflow")?;
    ensure!(end <= size, "TDH property array exceeds metadata");
    Ok(())
}

/// Normalize only known union layouts. `None` means the schema is unsupported.
fn normalize_fields(
    properties: &[EVENT_PROPERTY_INFO],
    bytes: &[u8],
) -> Result<Option<Vec<FieldSpec>>> {
    // Match the compiler's bounded grammar without allocating unbounded descriptors.
    if properties.len() > 256 {
        return Ok(None);
    }
    let mut fields = Vec::with_capacity(properties.len());
    for prop in properties {
        let offset = prop.NameOffset as usize;
        ensure!(
            offset < bytes.len() && offset.is_multiple_of(2),
            "invalid TDH property name offset"
        );
        // At most 256 UTF-16 units per normalized name; a longer native name makes
        // the schema unsupported instead of growing the bounded plan storage.
        let units = bytes[offset..].as_chunks::<2>().0;
        if !units.iter().take(257).any(|unit| *unit == [0, 0]) {
            if units.len() <= 256 {
                bail!("unterminated TDH property name");
            }
            return Ok(None);
        }
        let name = property_name(bytes, offset)?;
        if prop.Flags & (PropertyStruct | PropertyHasCustomSchema) != 0
            || prop.Flags.cast_unsigned() & !SUPPORTED_PROPERTY_FLAGS != 0
        {
            // Keep the name and unsupported flags so the compiler can check
            // duplicate targets while ignoring layouts strictly after its prefix.
            // No native union arm is interpreted for these descriptors.
            fields.push(FieldSpec {
                name,
                input: -1,
                output: -1,
                flags: prop.Flags.cast_unsigned(),
                count: 0,
                length: 0,
            });
            continue;
        }
        // SAFETY: The flags exclude PropertyStruct and PropertyHasCustomSchema,
        // so Anonymous1 holds the nonStructType arm.
        let native = unsafe { prop.Anonymous1.nonStructType };
        let count = if prop.Flags & PropertyParamCount != 0 {
            // SAFETY: PropertyParamCount selects the countPropertyIndex arm.
            unsafe { prop.Anonymous2.countPropertyIndex }
        } else {
            // SAFETY: Without PropertyParamCount, Anonymous2 holds the count arm.
            unsafe { prop.Anonymous2.count }
        };
        let length = if prop.Flags & PropertyParamLength != 0 {
            // SAFETY: PropertyParamLength selects the lengthPropertyIndex arm.
            unsafe { prop.Anonymous3.lengthPropertyIndex }
        } else {
            // SAFETY: Without PropertyParamLength, Anonymous3 holds the length arm.
            unsafe { prop.Anonymous3.length }
        };
        fields.push(FieldSpec {
            name,
            input: i32::from(native.InType),
            output: i32::from(native.OutType),
            flags: prop.Flags.cast_unsigned(),
            count,
            length,
        });
    }
    Ok(Some(fields))
}

fn property_name(bytes: &[u8], offset: usize) -> Result<String> {
    ensure!(
        offset < bytes.len() && offset.is_multiple_of(2),
        "invalid TDH property name offset"
    );
    let units = bytes[offset..].as_chunks::<2>().0;
    let length = units
        .iter()
        .position(|unit| *unit == [0, 0])
        .context("unterminated TDH property name")?;
    let units: Vec<u16> = units[..length]
        .iter()
        .map(|unit| u16::from_le_bytes(*unit))
        .collect();
    String::from_utf16(&units).context("invalid TDH property name UTF-16")
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
            // SAFETY: The flags exclude PropertyStruct and PropertyHasCustomSchema,
            // so Anonymous1 holds the nonStructType arm.
            let native = unsafe { prop.Anonymous1.nonStructType };
            // SAFETY: The flags exclude PropertyParamCount, so Anonymous2 holds
            // the fixed count arm.
            let count = unsafe { prop.Anonymous2.count };
            ensure!(count <= 1, "unsupported property array {name}");
            return Ok(PropertyType {
                input: i32::from(native.InType),
                output: i32::from(native.OutType),
            });
        }
    }
    bail!("missing property {name}")
}

/// Closed set of manifest names the parser looks up.
#[derive(Clone, Copy)]
enum PropertyName {
    Tcp,
    Port,
    InterfaceIp,
    Source,
    Destination,
}

impl PropertyName {
    const fn name(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Port => "Port",
            Self::InterfaceIp => "InterfaceIP",
            Self::Source => "Source",
            Self::Destination => "Destination",
        }
    }
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

fn address(bytes: &[u8], kind: PropertyType, scratch: &mut [u8; 768]) -> Result<IpAddr> {
    if kind.output == TDH_OUTTYPE_IPV4 && bytes.len() == 4 {
        return Ok(IpAddr::from(<[u8; 4]>::try_from(bytes)?));
    }
    if kind.output == TDH_OUTTYPE_IPV6 && bytes.len() == 16 {
        return Ok(IpAddr::from(<[u8; 16]>::try_from(bytes)?));
    }
    let text = match kind.input {
        TDH_INTYPE_UNICODESTRING => {
            ensure!(bytes.len().is_multiple_of(2), "odd Unicode address length");
            ensure!(bytes.len() <= 512, "UTF-16 address exceeds size limit");
            ensure!(bytes.ends_with(&[0, 0]), "unterminated Unicode address");
            let units = bytes[..bytes.len() - 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]));
            let mut length = 0;
            for character in char::decode_utf16(units) {
                let character = character.context("invalid address UTF-16")?;
                length += character.encode_utf8(&mut scratch[length..]).len();
            }
            std::str::from_utf8(&scratch[..length])?
        }
        TDH_INTYPE_ANSISTRING => {
            ensure!(bytes.last() == Some(&0), "unterminated address");
            std::str::from_utf8(&bytes[..bytes.len() - 1])?
        }
        _ => bail!("unsupported address property type"),
    };
    // Validate the complete string before ignoring routing context after `%`.
    // Interior NULs in the address prefix reach IP parsing and are rejected;
    // the suffix retains the manifest parser's existing opaque-text behavior.
    let address = text.split_once('%').map_or(text, |(address, _)| address);
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
thread_local! {
    static TDH_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn tdh_tick() {
    TDH_CALLS.with(|calls| calls.set(calls.get() + 1));
}
