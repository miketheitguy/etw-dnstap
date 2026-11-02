//! Benchmark entry points, compiled only with the `bench` feature.
//!
//! `benches/hot_path.rs` is a separate crate target and sees only `pub` items.
//! The wrappers here hand it the hot path (`copy_record`, `DecodePlan::decode`,
//! `Encoder::encode`, and `pipeline::parse_and_encode`) without making any
//! internal type public. The feature is never enabled in a shipped build, and
//! nothing here is part of the supported interface.
//!
//! Records are built by hand in the layout of the saved v0 DNS Server template.
//! Their packets are a 60-byte A query and a 100-byte response, plus a
//! 4,096-byte response that needs a jumbo slot. The parser's schema cache is
//! filled through the production path: the process-local test manifest
//! (`src/etw/testdata/decoder-fixture.man`) is registered with
//! `TdhLoadManifest`, so the first `parse` of each event ID makes a real TDH
//! call and every later one is a cache hit. The manifest defines event version
//! 250, so the records carry that version; the live provider emits version 0.

use std::{os::windows::ffi::OsStrExt, path::PathBuf, sync::Arc, sync::OnceLock};

use windows_sys::Win32::System::Diagnostics::Etw::{
    EVENT_DESCRIPTOR, EVENT_HEADER, EVENT_RECORD, TdhLoadManifest,
};

use super::{
    PROVIDER, Parser, PoolManager, RawEtwEvent, check_status,
    decode::{DecodePlan, saved_template},
    intake,
};
use crate::{dnstap::Encoder, event::DecodedDnsEvent};

/// The event version the fixture manifest defines.
const VERSION: u8 = 250;
/// 2026-10-26 13:33:20 UTC as a FILETIME: 1,793,000,000 Unix seconds plus the
/// 11,644,473,600 seconds from 1601 to 1970, in 100 ns ticks.
const TIMESTAMP: i64 = (1_793_000_000 + 11_644_473_600) * 10_000_000;
/// Wire form of `service.example.com`, 21 bytes.
const QNAME_WIRE: &[u8] = b"\x07service\x07example\x03com\x00";
/// The same name as the provider's `QNAME` property writes it.
const QNAME_TEXT: &str = "(7)service(7)example(3)com(0)";
const INTERFACE_IP: &str = "192.0.2.53";
const CLIENT_IP: &str = "198.51.100.42";
const CLIENT_PORT: u32 = 54_321;
const XID: u32 = 0x1234;
/// Trailing metadata the decoder never reads but the copy must carry.
const GUID_TEXT: &str = "{3F2504E0-4F89-11D3-9A0C-0305E82C3301}";
/// Total length of the jumbo response packet.
const JUMBO_PACKET_LENGTH: usize = 4_096;

/// Regular slots in the benchmark pool. It must hold every lease a benchmark
/// batch keeps outstanding (`benches/hot_path.rs` keeps at most 1,024).
const POOL_CAPACITY: usize = 2_048;
const POOL_JUMBO_CAPACITY: usize = 64;

/// One DNS Server event as ETW would deliver it: the header fields the parser
/// reads and a payload in the saved template's layout.
pub struct Record {
    header: EVENT_HEADER,
    payload: Vec<u8>,
    payload_length: u16,
}

impl Record {
    fn new(event_id: u16, payload: Vec<u8>) -> Self {
        let payload_length = u16::try_from(payload.len()).expect("record payloads fit u16");
        Self {
            header: EVENT_HEADER {
                ProviderId: PROVIDER,
                TimeStamp: TIMESTAMP,
                EventDescriptor: EVENT_DESCRIPTOR {
                    Id: event_id,
                    Version: VERSION,
                    ..Default::default()
                },
                ..Default::default()
            },
            payload,
            payload_length,
        }
    }

    /// Event 256 (client query) over UDP carrying a 60-byte A query.
    #[must_use]
    pub fn query() -> Self {
        Self::new(256, query_payload(&query_packet()))
    }

    /// Event 257 (client response) over UDP carrying a 100-byte response.
    #[must_use]
    pub fn response() -> Self {
        Self::new(257, response_payload(&response_packet()))
    }

    /// Event 257 carrying a 4,096-byte TXT response. The payload exceeds a
    /// regular slot, so `copy_record` takes a jumbo slot.
    #[must_use]
    pub fn jumbo_response() -> Self {
        Self::new(257, response_payload(&jumbo_packet()))
    }

    #[must_use]
    pub fn event_id(&self) -> u16 {
        self.header.EventDescriptor.Id
    }

    /// The ETW user data: every template property, including the trailing
    /// metadata after `PacketData`.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

fn ansi(payload: &mut Vec<u8>, text: &str) {
    payload.extend_from_slice(text.as_bytes());
    payload.push(0);
}

fn unicode(payload: &mut Vec<u8>, text: &str) {
    payload.extend(
        text.encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes),
    );
}

fn u32_le(payload: &mut Vec<u8>, value: u32) {
    payload.extend_from_slice(&value.to_le_bytes());
}

fn buffer_size(packet: &[u8]) -> u32 {
    u32::try_from(packet.len()).expect("packet lengths fit u32")
}

/// Event 256 user data in the `Query` template layout.
fn query_payload(packet: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(packet.len() + 192);
    payload.push(0); // TCP
    ansi(&mut payload, INTERFACE_IP); // InterfaceIP
    ansi(&mut payload, CLIENT_IP); // Source
    payload.push(1); // RD
    ansi(&mut payload, QNAME_TEXT); // QNAME
    u32_le(&mut payload, 1); // QTYPE A
    u32_le(&mut payload, XID); // XID
    u32_le(&mut payload, CLIENT_PORT); // Port
    u32_le(&mut payload, 0x0100); // Flags
    u32_le(&mut payload, buffer_size(packet)); // BufferSize
    payload.extend_from_slice(packet); // PacketData
    unicode(&mut payload, ""); // AdditionalInfo
    unicode(&mut payload, GUID_TEXT); // GUID
    payload
}

/// Event 257 user data in the `Response` template layout.
fn response_payload(packet: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(packet.len() + 256);
    payload.push(0); // TCP
    ansi(&mut payload, INTERFACE_IP); // InterfaceIP
    ansi(&mut payload, CLIENT_IP); // Destination
    payload.push(1); // AA
    payload.push(0); // AD
    ansi(&mut payload, QNAME_TEXT); // QNAME
    u32_le(&mut payload, 1); // QTYPE A
    u32_le(&mut payload, XID); // XID
    payload.push(0); // DNSSEC
    u32_le(&mut payload, 0); // RCODE NOERROR
    u32_le(&mut payload, CLIENT_PORT); // Port
    u32_le(&mut payload, 0x8180); // Flags
    unicode(&mut payload, ""); // Scope
    unicode(&mut payload, "example.com"); // Zone
    unicode(&mut payload, ""); // PolicyName
    u32_le(&mut payload, buffer_size(packet)); // BufferSize
    payload.extend_from_slice(packet); // PacketData
    unicode(&mut payload, ""); // AdditionalInfo
    u32_le(&mut payload, 0); // ElapsedTime
    unicode(&mut payload, GUID_TEXT); // GUID
    unicode(&mut payload, "false"); // StaleRecordsPresent
    payload
}

/// The DNS header: ID, flags, and the four section counts.
fn dns_header(packet: &mut Vec<u8>, flags: u16, answers: u16, additional: u16) {
    packet.extend_from_slice(&0x1234_u16.to_be_bytes());
    packet.extend_from_slice(&flags.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&answers.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&additional.to_be_bytes());
}

/// The question `service.example.com IN A`, 25 bytes.
fn question(packet: &mut Vec<u8>) {
    packet.extend_from_slice(QNAME_WIRE);
    packet.extend_from_slice(&[0, 1, 0, 1]);
}

/// An EDNS0 OPT pseudo-record (11 bytes) whose data is a COOKIE option that
/// carries `cookie` (4 bytes plus the cookie).
fn opt_record(packet: &mut Vec<u8>, cookie: &[u8]) {
    let data_length = u16::try_from(cookie.len() + 4).expect("cookie fits u16");
    let cookie_length = u16::try_from(cookie.len()).expect("cookie fits u16");
    packet.push(0); // root name
    packet.extend_from_slice(&41_u16.to_be_bytes()); // OPT
    packet.extend_from_slice(&1_232_u16.to_be_bytes()); // UDP payload size
    packet.extend_from_slice(&[0, 0, 0, 0]); // extended RCODE, version, flags
    packet.extend_from_slice(&data_length.to_be_bytes());
    packet.extend_from_slice(&10_u16.to_be_bytes()); // COOKIE
    packet.extend_from_slice(&cookie_length.to_be_bytes());
    packet.extend_from_slice(cookie);
}

/// An A record answer for the question name (a compression pointer), 16 bytes.
fn a_answer(packet: &mut Vec<u8>, address: [u8; 4]) {
    packet.extend_from_slice(&[0xC0, 0x0C]); // pointer to offset 12
    packet.extend_from_slice(&[0, 1, 0, 1]); // A, IN
    packet.extend_from_slice(&3_600_u32.to_be_bytes()); // TTL
    packet.extend_from_slice(&4_u16.to_be_bytes());
    packet.extend_from_slice(&address);
}

/// A 60-byte A query for `service.example.com`: header (12), question (25),
/// and an OPT record with an 8-byte client cookie (23).
fn query_packet() -> Vec<u8> {
    let mut packet = Vec::with_capacity(60);
    dns_header(&mut packet, 0x0100, 0, 1);
    question(&mut packet);
    opt_record(
        &mut packet,
        &[0x24, 0x8d, 0x7a, 0x9c, 0x0e, 0x13, 0x55, 0x61],
    );
    packet
}

/// A 100-byte response: header (12), question (25), two A answers (32), and an
/// OPT record with a client and server cookie (31).
fn response_packet() -> Vec<u8> {
    let mut packet = Vec::with_capacity(100);
    dns_header(&mut packet, 0x8180, 2, 1);
    question(&mut packet);
    a_answer(&mut packet, [192, 0, 2, 10]);
    a_answer(&mut packet, [192, 0, 2, 11]);
    opt_record(
        &mut packet,
        &[
            0x24, 0x8d, 0x7a, 0x9c, 0x0e, 0x13, 0x55, 0x61, 0x01, 0x00, 0x00, 0x00, 0x69, 0x4e,
            0x2a, 0xf0,
        ],
    );
    packet
}

/// A 4,096-byte response whose single TXT answer fills the rest of the packet
/// with character strings of at most 255 bytes.
fn jumbo_packet() -> Vec<u8> {
    let mut packet = Vec::with_capacity(JUMBO_PACKET_LENGTH);
    dns_header(&mut packet, 0x8180, 1, 0);
    question(&mut packet);
    packet.extend_from_slice(&[0xC0, 0x0C]); // pointer to offset 12
    packet.extend_from_slice(&[0, 16, 0, 1]); // TXT, IN
    packet.extend_from_slice(&3_600_u32.to_be_bytes()); // TTL
    let data_length = JUMBO_PACKET_LENGTH - packet.len() - 2;
    packet.extend_from_slice(
        &u16::try_from(data_length)
            .expect("TXT data fits u16")
            .to_be_bytes(),
    );
    let mut remaining = data_length;
    while remaining > 0 {
        let length = (remaining - 1).min(255);
        packet.push(u8::try_from(length).expect("character strings fit u8"));
        packet.extend(std::iter::repeat_n(b'x', length));
        remaining -= length + 1;
    }
    packet
}

/// A record pool sized for the benchmark: 2,048 regular slots and 64 jumbo slots.
pub struct Pool(Arc<PoolManager>);

impl Pool {
    /// # Panics
    /// Panics if the pool cannot be allocated.
    #[must_use]
    pub fn new() -> Self {
        Self(
            PoolManager::new(POOL_CAPACITY, POOL_JUMBO_CAPACITY).expect("benchmark pool allocates"),
        )
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

/// A record deep-copied into a pooled slot. Dropping it recycles the slot.
pub struct Copied(RawEtwEvent);

/// Deep-copies `record` into a slot of `pool`, as the ETW callback does for a
/// delivered record.
///
/// # Panics
/// Panics when the slot class the record needs is exhausted.
#[must_use]
pub fn copy_record(pool: &Pool, record: &Record) -> Copied {
    let source = EVENT_RECORD {
        EventHeader: record.header,
        UserDataLength: record.payload_length,
        // The `*mut` is a type requirement of EVENT_RECORD only; the copy reads
        // the payload and never writes through this pointer.
        UserData: record.payload.as_ptr().cast_mut().cast(),
        ..Default::default()
    };
    // SAFETY: `record` owns `payload` for this synchronous call, `UserDataLength`
    // is its length, the record advertises no extended data, and the pool slots
    // are separate allocations that cannot overlap the payload.
    let event = unsafe { intake::copy_record(&source, &pool.0) }.expect("the pool has a free slot");
    Copied(event)
}

/// The single-pass plan compiled from the saved template of one event ID.
pub struct Plan(DecodePlan);

impl Plan {
    /// # Panics
    /// Panics for an event ID other than 256, 257, or 258.
    #[must_use]
    pub fn new(event_id: u16) -> Self {
        let peer = if event_id == 256 {
            "Source"
        } else {
            "Destination"
        };
        Self(
            DecodePlan::compile(&saved_template(event_id), peer)
                .expect("the saved template compiles"),
        )
    }

    /// Decodes one payload and returns its borrowed `TCP`, `Port`,
    /// `InterfaceIP`, peer address, and `PacketData` fields, or `None` for a
    /// payload the plan rejects.
    #[must_use]
    pub fn decode<'a>(&self, payload: &'a [u8]) -> Option<[&'a [u8]; 5]> {
        let decoded = self.0.decode(payload).ok()?;
        Some([
            decoded.tcp,
            decoded.port,
            decoded.local,
            decoded.peer,
            decoded.packet,
        ])
    }
}

/// A decoded view of one event. It borrows the event's pooled lease.
pub struct Decoded<'a>(DecodedDnsEvent<'a>);

/// The parser thread's state: a [`Parser`] with its schema cache and an
/// [`Encoder`] with its reusable output storage.
pub struct Pipeline {
    parser: Parser,
    encoder: Encoder,
}

impl Pipeline {
    /// Registers the fixture manifest with TDH (once per process) so the parser
    /// can load each record's schema, then starts with an empty schema cache.
    /// The first `parse` of each event ID fills the cache; later ones hit it.
    ///
    /// # Panics
    /// Panics if the manifest does not load. Run under `cargo bench` so
    /// `CARGO_MANIFEST_DIR` locates it.
    #[must_use]
    pub fn new() -> Self {
        load_manifest();
        Self {
            parser: Parser::new(),
            encoder: Encoder::new(b"etw-dnstap").expect("the identity is within bounds"),
        }
    }

    /// Decodes `event` through [`Parser::parse`].
    ///
    /// # Panics
    /// Panics if the parser rejects the event.
    pub fn parse<'e>(&mut self, event: &'e Copied) -> Decoded<'e> {
        Decoded(
            self.parser
                .parse(&event.0)
                .expect("the record parses")
                .expect("the event ID is supported"),
        )
    }

    /// Encodes a decoded view to dnstap bytes, which borrow only the encoder.
    ///
    /// # Panics
    /// Panics if the encoder rejects the view.
    pub fn encode(&mut self, decoded: &Decoded<'_>) -> &[u8] {
        self.encoder
            .encode(&decoded.0)
            .expect("the decoded event encodes")
    }

    /// The production composition of parse and encode. The event's lease is
    /// recycled before this returns.
    ///
    /// # Panics
    /// Panics if the parser or the encoder rejects the event.
    pub fn parse_and_encode(&mut self, event: Copied) -> &[u8] {
        crate::pipeline::parse_and_encode(&mut self.parser, event.0, &mut self.encoder)
            .expect("the record parses and encodes")
            .expect("the event ID is supported")
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// The registered manifest path, kept alive for the process lifetime.
static MANIFEST: OnceLock<Vec<u16>> = OnceLock::new();

/// Registers `src/etw/testdata/decoder-fixture.man` with TDH for this process,
/// once. It stays registered until the process exits, so every `Pipeline` shares
/// it. This registers a decoding manifest only, not an ETW publisher.
fn load_manifest() {
    MANIFEST.get_or_init(|| {
        let root = std::env::var_os("CARGO_MANIFEST_DIR")
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let path = std::path::absolute(root.join("src/etw/testdata/decoder-fixture.man"))
            .expect("the manifest path resolves");
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `path` is a NUL-terminated UTF-16 string that outlives this
        // synchronous call and is kept alive for the process in `MANIFEST`.
        let status = unsafe { TdhLoadManifest(path.as_ptr()) };
        check_status(status, "load the benchmark manifest")
            .expect("the fixture manifest loads (run under cargo bench)");
        path
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etw::pool::REGULAR_BYTES;

    #[test]
    fn packets_have_the_documented_lengths() {
        assert_eq!(query_packet().len(), 60);
        assert_eq!(response_packet().len(), 100);
        assert_eq!(jumbo_packet().len(), JUMBO_PACKET_LENGTH);
        assert!(Record::query().payload().len() <= REGULAR_BYTES);
        assert!(Record::response().payload().len() <= REGULAR_BYTES);
        assert!(Record::jumbo_response().payload().len() > REGULAR_BYTES);
    }

    #[test]
    fn records_decode_with_the_saved_template() {
        for (record, packet_length) in [
            (Record::query(), 60),
            (Record::response(), 100),
            (Record::jumbo_response(), JUMBO_PACKET_LENGTH),
        ] {
            let plan = Plan::new(record.event_id());
            let fields = plan.decode(record.payload()).expect("the record decodes");
            assert_eq!(fields[0], [0]);
            assert_eq!(fields[1], CLIENT_PORT.to_le_bytes());
            assert_eq!(fields[2], b"192.0.2.53\0");
            assert_eq!(fields[3], b"198.51.100.42\0");
            assert_eq!(fields[4].len(), packet_length);
            assert_eq!(fields[4][2] & 0x80 != 0, record.event_id() != 256);
        }
    }

    #[test]
    fn copied_records_parse_and_encode_through_the_manifest_schema() {
        let pool = Pool::new();
        let mut pipeline = Pipeline::new();
        for (record, packet_length) in [
            (Record::query(), 60),
            (Record::response(), 100),
            (Record::jumbo_response(), JUMBO_PACKET_LENGTH),
        ] {
            let event = copy_record(&pool, &record);
            let decoded = pipeline.parse(&event);
            let frame = pipeline.encode(&decoded).to_vec();
            assert!(frame.len() > packet_length);
            assert!(frame.ends_with(&[0x78, 1]));
            let event = copy_record(&pool, &record);
            assert_eq!(pipeline.parse_and_encode(event), frame);
        }
    }
}
