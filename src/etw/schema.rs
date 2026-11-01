//! TDH schema loading, the TDH fallback reader, and the event [`Parser`].
//!
//! The parser yields a borrowed, format-neutral [`DecodedDnsEvent`]; encoding for
//! a particular output format happens downstream.

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
        PROPERTY_DATA_DESCRIPTOR, PropertyHasCustomSchema, PropertyParamCount, PropertyParamLength,
        PropertyStruct, TDH_INTYPE_ANSISTRING, TDH_INTYPE_BINARY, TDH_INTYPE_BOOLEAN,
        TDH_INTYPE_UINT8, TDH_INTYPE_UINT16, TDH_INTYPE_UINT32, TDH_INTYPE_UNICODESTRING,
        TDH_OUTTYPE_IPV4, TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT, TRACE_EVENT_INFO,
        TdhGetEventInformation, TdhGetProperty, TdhGetPropertySize,
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
/// Largest DNS message the fallback reader will copy out of TDH.
const MAX_PACKET_BYTES: usize = 65_535;

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;

#[derive(Clone, Copy)]
struct PropertyType {
    input: i32,
    output: i32,
}
struct Schema {
    plan: Option<DecodePlan>,
    tcp: PropertyType,
    local: PropertyType,
    peer: PropertyType,
    port: PropertyType,
    packet: PropertyType,
}

/// Reuses initialized property, text, and packet storage on the parser thread.
pub(crate) struct Parser {
    // Owned plans permit borrowing cache hits without allocation or refcount work.
    // Only events 256 and 257 reach this cache, usually with one version each, so a
    // linear scan over at most MAX_CACHED_SCHEMAS entries beats hashing.
    schemas: Vec<((u16, u8), Schema)>,
    property: [u8; 512],
    // At most 255 nonterminating UTF-16 units, each requiring at most 3 UTF-8 bytes.
    text: [u8; 768],
    // Backing storage for packets the TDH fallback extracts. Reserved once so the
    // fallback never reallocates; fast-path packets borrow the event payload.
    packet: Vec<u8>,
    // Keys whose schema failed to load. Without this every such event would repeat
    // the TDH calls and their allocations on the parser thread. Bounded like the
    // positive cache and reserved up front.
    rejected_keys: Vec<(u16, u8)>,
    // Events rejected without a TDH call because their key is in `rejected_keys`,
    // plus the first failure of each key.
    rejected_events: u64,
}

impl Parser {
    pub(crate) fn new() -> Self {
        Self {
            rejected_keys: Vec::with_capacity(MAX_CACHED_SCHEMAS),
            rejected_events: 0,
            schemas: Vec::with_capacity(MAX_CACHED_SCHEMAS),
            property: [0; 512],
            text: [0; 768],
            packet: Vec::with_capacity(MAX_PACKET_BYTES),
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

    /// Events rejected because their (event id, version) schema failed to load.
    pub(crate) fn schema_rejected_events(&self) -> u64 {
        self.rejected_events
    }

    /// Remembers a failed schema key, clearing the list first when it is full.
    fn remember_rejected(&mut self, key: (u16, u8)) {
        if self.rejected_keys.len() >= MAX_CACHED_SCHEMAS {
            self.rejected_keys.clear();
        }
        self.rejected_keys.push(key);
    }

    /// Decodes one event into a format-neutral view, or `None` for events this
    /// program does not capture. The view borrows the event's pooled lease (fast
    /// path) or the parser's packet storage (TDH fallback), so the caller keeps
    /// `event` alive until it has consumed the view and then drops it to recycle
    /// the lease. Nothing is allocated on a schema-cache hit.
    pub(crate) fn parse<'a>(
        &'a mut self,
        event: &'a RawEtwEvent,
    ) -> Result<Option<DecodedDnsEvent<'a>>> {
        let header = event.header();
        let id = header.EventDescriptor.Id;
        if !dns_provider(&header.ProviderId) || !supported(id) {
            return Ok(None);
        }
        let peer_name = if id == 256 {
            PropertyName::Source
        } else {
            PropertyName::Destination
        };
        let key = (id, header.EventDescriptor.Version);
        if let Some((_, schema)) = self.schemas.iter().find(|(cached, _)| *cached == key) {
            return parse_schema(
                schema,
                event,
                peer_name,
                &mut self.property,
                &mut self.packet,
                &mut self.text,
            )
            .map(Some);
        }
        if self.rejected_keys.contains(&key) {
            self.rejected_events += 1;
            bail!(
                "DNS event {id} version {} schema was rejected earlier (cached)",
                key.1
            );
        }
        let schema = match event.with_record(|record| load_schema(record, peer_name)) {
            Ok(schema) => schema,
            Err(error) => {
                self.remember_rejected(key);
                self.rejected_events += 1;
                // Logged once per key; later events hit the cache silently.
                tracing::warn!(
                    event_id = id,
                    version = key.1,
                    "DNS event schema failed to load and is cached as rejected: {error:#}"
                );
                return Err(error.context(format!("DNS event {id} version {} schema", key.1)));
            }
        };
        self.insert_schema(key, schema);
        let schema = self
            .schemas
            .last()
            .map(|(_, schema)| schema)
            .context("missing cached DNS schema")?;
        parse_schema(
            schema,
            event,
            peer_name,
            &mut self.property,
            &mut self.packet,
            &mut self.text,
        )
        .map(Some)
    }
}

fn parse_schema<'a>(
    schema: &Schema,
    event: &'a RawEtwEvent,
    peer_name: PropertyName,
    property: &mut [u8; 512],
    packet: &'a mut Vec<u8>,
    text: &mut [u8; 768],
) -> Result<DecodedDnsEvent<'a>> {
    if let Some(plan) = &schema.plan {
        parse_fast(schema, plan, event.header(), event.payload(), text)
    } else {
        event
            .with_record(|record| parse_fallback(schema, record, peer_name, property, packet, text))
    }
}

fn parse_fast<'a>(
    schema: &Schema,
    plan: &DecodePlan,
    header: &EVENT_HEADER,
    payload: &'a [u8],
    text: &mut [u8; 768],
) -> Result<DecodedDnsEvent<'a>> {
    // A supported malformed payload is rejected here, never retried with TDH.
    let decoded = plan
        .decode(payload)
        .map_err(|error| anyhow!("invalid DNS payload: {error:?}"))?;
    let tcp = integer(decoded.tcp, schema.tcp)?;
    ensure!(tcp <= 1, "invalid TCP Boolean");
    let port = u16::try_from(integer(decoded.port, schema.port)?).context("Port exceeds u16")?;
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

fn parse_fallback<'a>(
    schema: &Schema,
    record: &EVENT_RECORD,
    peer_name: PropertyName,
    scratch: &mut [u8; 512],
    packet: &'a mut Vec<u8>,
    text: &mut [u8; 768],
) -> Result<DecodedDnsEvent<'a>> {
    let id = record.EventHeader.EventDescriptor.Id;
    let tcp = integer(property(record, PropertyName::Tcp, 4, scratch)?, schema.tcp)?;
    ensure!(tcp <= 1, "invalid TCP Boolean");
    let port = integer(
        property(record, PropertyName::Port, 4, scratch)?,
        schema.port,
    )?;
    let port = u16::try_from(port).context("Port exceeds u16")?;
    let local = address(
        property(record, PropertyName::InterfaceIp, 512, scratch)?,
        schema.local,
        text,
    )?;
    let peer = address(
        property(record, peer_name, 512, scratch)?,
        schema.peer,
        text,
    )?;
    ensure!(
        schema.packet.input == TDH_INTYPE_BINARY,
        "PacketData is not binary"
    );
    let packet_length = property_size(record, PropertyName::PacketData, MAX_PACKET_BYTES)?;
    let metadata = DnsEventMetadata {
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
    };
    // Both the native view and the parser's reserved storage outlive this
    // synchronous fill, and the buffer never grows past its reserved capacity.
    packet.clear();
    packet.resize(packet_length, 0);
    read_property(record, PropertyName::PacketData, packet)?;
    Ok(DecodedDnsEvent {
        metadata,
        packet: packet.as_slice(),
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

fn load_schema(record: &EVENT_RECORD, peer_name: PropertyName) -> Result<Schema> {
    let mut size = 0;
    #[cfg(test)]
    tdh_tick(0);
    // SAFETY: record and all its backing buffers remain alive for this TDH call.
    let status = unsafe { TdhGetEventInformation(record, 0, null(), null_mut(), &raw mut size) };
    ensure!(
        status == ERROR_INSUFFICIENT_BUFFER,
        "TDH metadata size query failed: {}",
        std::io::Error::from_raw_os_error(status.cast_signed())
    );
    ensure!(
        (size_of::<TRACE_EVENT_INFO>()..=MAX_METADATA).contains(&(size as usize)),
        "invalid TDH metadata size"
    );
    // u64 storage supplies the alignment required by TRACE_EVENT_INFO and its
    // flexible EVENT_PROPERTY_INFO array, unlike a cast from a byte Vec.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let capacity = size;
    #[cfg(test)]
    tdh_tick(0);
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
    validate_property_array(info.PropertyCount as usize, count, size as usize)?;
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
    let plan = normalize_fields(properties, bytes)?
        .and_then(|fields| DecodePlan::compile(&fields, peer_name.name()));
    Ok(Schema {
        plan,
        tcp: find(PropertyName::Tcp.name())?,
        local: find(PropertyName::InterfaceIp.name())?,
        peer: find(peer_name.name())?,
        port: find(PropertyName::Port.name())?,
        packet: find(PropertyName::PacketData.name())?,
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

/// Normalize only known union layouts. Unsupported schemas retain TDH fallback.
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
        // At most 256 UTF-16 units per normalized name; a longer native name is
        // legacy-decodable but cannot grow the bounded cold-path plan storage.
        let units = bytes[offset..].as_chunks::<2>().0;
        if !units.iter().take(257).any(|unit| *unit == [0, 0]) {
            if units.len() <= 256 {
                bail!("unterminated TDH property name");
            }
            return Ok(None);
        }
        let name = property_name(bytes, offset)?;
        if prop.Flags & (PropertyStruct | PropertyHasCustomSchema) != 0
            || prop.Flags.cast_unsigned() & !0x36 != 0
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
        // SAFETY: The flags exclude struct/custom union layouts. Count and length
        // arms are selected independently according to their discriminating flags.
        let (native, count, length) = unsafe {
            (
                prop.Anonymous1.nonStructType,
                if prop.Flags & PropertyParamCount != 0 {
                    prop.Anonymous2.countPropertyIndex
                } else {
                    prop.Anonymous2.count
                },
                if prop.Flags & PropertyParamLength != 0 {
                    prop.Anonymous3.lengthPropertyIndex
                } else {
                    prop.Anonymous3.length
                },
            )
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

fn property_size(record: &EVENT_RECORD, name: PropertyName, maximum: usize) -> Result<usize> {
    let descriptor = name.descriptor();
    let mut size = 0;
    #[cfg(test)]
    tdh_tick(1);
    // SAFETY: The event lease retains all record backing data, the descriptor uses
    // a static terminated name, and the initialized size output lives through this call.
    let status =
        unsafe { TdhGetPropertySize(record, 0, null(), 1, &raw const descriptor, &raw mut size) };
    check_status(status, "TdhGetPropertySize")?;
    ensure!(
        size as usize <= maximum,
        "TDH property exceeds its size limit"
    );
    Ok(size as usize)
}

fn read_property(record: &EVENT_RECORD, name: PropertyName, bytes: &mut [u8]) -> Result<()> {
    let descriptor = name.descriptor();
    let size = u32::try_from(bytes.len()).context("TDH property length exceeds u32")?;
    #[cfg(test)]
    tdh_tick(2);
    // SAFETY: The exclusive initialized byte slice has exactly the validated TDH
    // length, alignment 1 suffices, and its owner and the event lease remain alive
    // through the synchronous native call. The property name is static and terminated.
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
    check_status(status, "TdhGetProperty")
}

fn scratch_property(
    scratch: &mut [u8; 512],
    size: usize,
    read: impl FnOnce(&mut [u8]) -> Result<()>,
) -> Result<&[u8]> {
    let bytes = scratch
        .get_mut(..size)
        .context("TDH property exceeds scratch size")?;
    bytes.fill(0);
    read(bytes)?;
    Ok(bytes)
}

fn property<'a>(
    record: &EVENT_RECORD,
    name: PropertyName,
    maximum: usize,
    scratch: &'a mut [u8; 512],
) -> Result<&'a [u8]> {
    let size = property_size(record, name, maximum)?;
    scratch_property(scratch, size, |bytes| read_property(record, name, bytes))
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
            ensure!(bytes.len() <= 512, "Unicode address exceeds size limit");
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
    static TDH_CALLS: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) };
}

#[cfg(test)]
fn tdh_tick(kind: usize) {
    TDH_CALLS.with(|calls| {
        let mut counts = calls.get();
        counts[kind] += 1;
        calls.set(counts);
    });
}
