//! Golden byte vectors and schema cross-checks against upstream `dnstap.proto`.
//!
//! The oracle here is deliberately independent of the encoder: expected bytes are
//! assembled by hand from the protobuf wire rules, and decoding uses a separate
//! Prost model that mirrors every field number of the upstream schema
//! (<https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto>), not only the
//! subset the encoder emits.

use super::{Encoder, test_oracle::read_varint};
use crate::event::{DecodedDnsEvent, DnsEventMetadata, Transport};
use proptest::prelude::*;
use prost::Message as _;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::{Duration, SystemTime},
};

// message Dnstap { optional bytes identity = 1; optional bytes version = 2;
//   optional bytes extra = 3; optional Message message = 14; required Type type = 15; }
#[derive(Clone, PartialEq, prost::Message)]
struct UpstreamDnstap {
    #[prost(bytes = "vec", optional, tag = "1")]
    identity: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    version: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    extra: Option<Vec<u8>>,
    #[prost(message, optional, tag = "14")]
    message: Option<UpstreamMessage>,
    #[prost(int32, required, tag = "15")]
    r#type: i32,
}

// message Message with every upstream field number, including those this
// converter never sets (query_zone = 11, policy = 15).
#[derive(Clone, PartialEq, prost::Message)]
struct UpstreamMessage {
    #[prost(int32, required, tag = "1")]
    r#type: i32,
    #[prost(int32, optional, tag = "2")]
    socket_family: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    socket_protocol: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "4")]
    query_address: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    response_address: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "6")]
    query_port: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    response_port: Option<u32>,
    #[prost(uint64, optional, tag = "8")]
    query_time_sec: Option<u64>,
    #[prost(fixed32, optional, tag = "9")]
    query_time_nsec: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "10")]
    query_message: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "11")]
    query_zone: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "12")]
    response_time_sec: Option<u64>,
    #[prost(fixed32, optional, tag = "13")]
    response_time_nsec: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "14")]
    response_message: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "15")]
    policy: Option<Vec<u8>>,
}

// Upstream enum values.
const DNSTAP_MESSAGE: i32 = 1;
const CLIENT_QUERY: i32 = 5;
const CLIENT_RESPONSE: i32 = 6;
const INET: i32 = 1;
const INET6: i32 = 2;
const UDP: i32 = 1;
const TCP: i32 = 2;

/// Protobuf base-128 varint, written from the specification.
fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).expect("masked") | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("below 0x80"));
}

fn len_field(tag: u8, bytes: &[u8], out: &mut Vec<u8>) {
    out.push(tag << 3 | 2);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn varint_field(tag: u8, value: u64, out: &mut Vec<u8>) {
    out.push(tag << 3);
    varint(value, out);
}

fn fixed32_field(tag: u8, value: u32, out: &mut Vec<u8>) {
    out.push(tag << 3 | 5);
    out.extend_from_slice(&value.to_le_bytes());
}

fn octets(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    }
}

/// Hand-assembled dnstap bytes in ascending field order, as the encoder and
/// the reference C implementation emit them.
fn golden(event: &DnsEventMetadata, packet: &[u8], identity: &[u8]) -> Vec<u8> {
    let response = event.event_id == 257;
    let elapsed = event
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("test time follows the epoch");
    let mut message = Vec::new();
    varint_field(1, if response { 6 } else { 5 }, &mut message);
    if let Some(ip) = event.peer_address.or(event.local_address) {
        varint_field(2, if ip.is_ipv4() { 1 } else { 2 }, &mut message);
    }
    varint_field(
        3,
        if event.transport == Transport::Tcp {
            2
        } else {
            1
        },
        &mut message,
    );
    if let Some(ip) = event.peer_address {
        len_field(4, &octets(ip), &mut message);
    }
    if let Some(ip) = event.local_address {
        len_field(5, &octets(ip), &mut message);
    }
    if let Some(port) = event.peer_port {
        varint_field(6, u64::from(port), &mut message);
    }
    if let Some(port) = event.local_port {
        varint_field(7, u64::from(port), &mut message);
    }
    if response {
        varint_field(12, elapsed.as_secs(), &mut message);
        fixed32_field(13, elapsed.subsec_nanos(), &mut message);
        len_field(14, packet, &mut message);
    } else {
        varint_field(8, elapsed.as_secs(), &mut message);
        fixed32_field(9, elapsed.subsec_nanos(), &mut message);
        len_field(10, packet, &mut message);
    }
    let mut out = Vec::new();
    if !identity.is_empty() {
        len_field(1, identity, &mut out);
    }
    len_field(14, &message, &mut out);
    varint_field(15, 1, &mut out);
    out
}

fn packet(response: bool, length: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..length)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect();
    bytes[2] = if response { 0x81 } else { 0x01 };
    bytes
}

fn metadata(id: u16) -> DnsEventMetadata {
    DnsEventMetadata {
        event_id: id,
        timestamp: SystemTime::UNIX_EPOCH + Duration::new(1, 500),
        transport: Transport::Udp,
        peer_address: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
        peer_port: Some(53_000),
        local_address: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53))),
        local_port: Some(53),
    }
}

fn encode(
    encoder: &mut Encoder,
    event: DnsEventMetadata,
    packet: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let decoded = DecodedDnsEvent {
        metadata: event,
        packet,
    };
    encoder.encode(&decoded).map(<[u8]>::to_vec)
}

#[test]
fn query_matches_a_literal_byte_vector() {
    let header = [
        0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut encoder = Encoder::new(b"ab").expect("identity");
    let bytes = encode(&mut encoder, metadata(256), &header).expect("encodes");
    let mut expected = vec![0x0a, 0x02, b'a', b'b', 0x72, 0x2d];
    expected.extend_from_slice(&[
        0x08, 0x05, // type = CLIENT_QUERY
        0x10, 0x01, // socket_family = INET
        0x18, 0x01, // socket_protocol = UDP
        0x22, 0x04, 192, 0, 2, 1, // query_address
        0x2a, 0x04, 192, 0, 2, 53, // response_address
        0x30, 0x88, 0x9e, 0x03, // query_port = 53000 (three-byte varint)
        0x38, 0x35, // response_port = 53
        0x40, 0x01, // query_time_sec = 1
        0x4d, 0xf4, 0x01, 0x00, 0x00, // query_time_nsec = 500 (fixed32 LE)
        0x52, 0x0c, // query_message, 12 bytes
    ]);
    expected.extend_from_slice(&header);
    expected.extend_from_slice(&[0x78, 0x01]); // type = MESSAGE
    assert_eq!(bytes, expected);
}

#[test]
fn response_uses_response_fields_and_ipv6_round_trips_exactly() {
    let peer: Ipv6Addr = "2001:db8::1".parse().expect("literal");
    let local: Ipv6Addr = "2001:db8:ffff::53".parse().expect("literal");
    let mut event = metadata(257);
    event.transport = Transport::Tcp;
    event.peer_address = Some(peer.into());
    event.local_address = Some(local.into());
    let reply = packet(true, 40);
    let mut encoder = Encoder::new(b"").expect("identity");
    let bytes = encode(&mut encoder, event, &reply).expect("encodes");
    assert_eq!(bytes, golden(&event, &reply, b""));
    let decoded = UpstreamDnstap::decode(bytes.as_slice()).expect("upstream schema decodes");
    assert_eq!(decoded.r#type, DNSTAP_MESSAGE);
    let message = decoded.message.expect("message present");
    assert_eq!(message.r#type, CLIENT_RESPONSE);
    assert_eq!(message.socket_family, Some(INET6));
    assert_eq!(message.socket_protocol, Some(TCP));
    assert_eq!(message.query_address.as_deref(), Some(&peer.octets()[..]));
    assert_eq!(
        message.response_address.as_deref(),
        Some(&local.octets()[..])
    );
    assert_eq!(message.response_message.as_deref(), Some(reply.as_slice()));
    assert_eq!(message.query_message, None);
    assert_eq!(message.query_time_sec, None);
    assert_eq!(message.response_time_sec, Some(1));
    assert_eq!(message.response_time_nsec, Some(500));
}

#[test]
fn varint_width_transitions_match_hand_encoded_bytes() {
    // Packet lengths straddle one/two-byte (127/128) and two/three-byte
    // (16383/16384) varints for the packet field; the enclosing message length
    // crosses the same limits a few bytes later. 65535 is the DNS maximum.
    for id in [256_u16, 257] {
        let event = metadata(id);
        let mut encoder = Encoder::new(b"id").expect("identity");
        for length in [
            12, 100, 126, 127, 128, 129, 150, 16_300, 16_340, 16_350, 16_383, 16_384, 16_385,
            65_535,
        ] {
            let payload = packet(id == 257, length);
            let wire = encode(&mut encoder, event, &payload).expect("encodes");
            assert_eq!(
                wire,
                golden(&event, &payload, b"id"),
                "id {id} length {length}"
            );
        }
    }
}

#[test]
fn absent_optional_fields_are_omitted_not_zeroed() {
    let mut event = metadata(256);
    event.peer_address = None;
    event.local_address = None;
    event.peer_port = None;
    event.local_port = None;
    let query = packet(false, 12);
    let mut encoder = Encoder::new(b"").expect("identity");
    let bytes = encode(&mut encoder, event, &query).expect("encodes");
    assert_eq!(bytes, golden(&event, &query, b""));
    let outer = UpstreamDnstap::decode(bytes.as_slice()).expect("decodes");
    assert_eq!(
        (outer.identity, outer.version, outer.extra),
        (None, None, None)
    );
    let message = outer.message.expect("message");
    assert_eq!(message.socket_family, None, "family needs an address");
    for absent in [
        message.query_address,
        message.response_address,
        message.query_zone,
        message.response_message,
        message.policy,
    ] {
        assert_eq!(absent, None);
    }
    assert_eq!(
        (message.query_port, message.response_port),
        (None, None),
        "unknown ports must not be reported as port 0"
    );
    assert_eq!(message.socket_protocol, Some(UDP));
}

fn arbitrary_ip(v6: bool) -> BoxedStrategy<IpAddr> {
    if v6 {
        any::<[u8; 16]>()
            .prop_map(|bytes| IpAddr::V6(Ipv6Addr::from(bytes)))
            .boxed()
    } else {
        any::<[u8; 4]>()
            .prop_map(|bytes| IpAddr::V4(Ipv4Addr::from(bytes)))
            .boxed()
    }
}

type Case = (DnsEventMetadata, usize, Vec<u8>);

fn arbitrary_event() -> impl Strategy<Value = Case> {
    (
        prop::sample::select(vec![256_u16, 257]),
        (any::<bool>(), any::<bool>(), any::<bool>()),
        (0_u64..=4_000_000_000, 0_u32..1_000_000_000),
        (
            prop::option::of(any::<u16>()),
            prop::option::of(any::<u16>()),
        ),
        12_usize..3_000,
        prop::collection::vec(any::<u8>(), 0..64),
    )
        .prop_flat_map(
            |(id, (v6, tcp, mix), (secs, nanos), (peer_port, local_port), length, identity)| {
                (
                    prop::option::of(arbitrary_ip(v6)),
                    // `mix` lets the local side use the other family sometimes.
                    prop::option::of(arbitrary_ip(v6 ^ (mix && length % 7 == 0))),
                )
                    .prop_map(move |(peer, local)| {
                        let event = DnsEventMetadata {
                            event_id: id,
                            timestamp: SystemTime::UNIX_EPOCH + Duration::new(secs, nanos),
                            transport: if tcp { Transport::Tcp } else { Transport::Udp },
                            peer_address: peer,
                            peer_port,
                            local_address: local,
                            local_port,
                        };
                        (event, length, identity.clone())
                    })
            },
        )
}

/// Walks the top-level fields without schema knowledge; returns field numbers.
fn wire_fields(mut input: &[u8]) -> Vec<u64> {
    let mut numbers = Vec::new();
    while !input.is_empty() {
        let (key, rest) = read_varint(input);
        input = rest;
        numbers.push(key >> 3);
        match key & 7 {
            0 => input = read_varint(input).1,
            2 => {
                let (length, rest) = read_varint(input);
                input = &rest[usize::try_from(length).expect("length")..];
            }
            5 => input = &input[4..],
            wire => panic!("unexpected wire type {wire}"),
        }
    }
    numbers
}

proptest! {
    #[test]
    fn encoded_events_decode_with_the_upstream_schema_and_preserve_every_field(
        (event, length, identity) in arbitrary_event(),
    ) {
        let response = event.event_id == 257;
        let bytes = packet(response, length);
        let mut encoder = Encoder::new(&identity).expect("identity within bounds");
        let encoded = encode(&mut encoder, event, &bytes);
        let mixed = matches!(
            (event.peer_address, event.local_address),
            (Some(IpAddr::V4(_)), Some(IpAddr::V6(_))) | (Some(IpAddr::V6(_)), Some(IpAddr::V4(_)))
        );
        prop_assert_eq!(encoded.is_err(), mixed);
        let Ok(encoded) = encoded else { return Ok(()); };
        prop_assert_eq!(&encoded, &golden(&event, &bytes, &identity));

        let outer = UpstreamDnstap::decode(encoded.as_slice()).expect("upstream decode");
        prop_assert_eq!(outer.r#type, DNSTAP_MESSAGE);
        prop_assert_eq!(outer.identity.as_deref(), (!identity.is_empty()).then_some(identity.as_slice()));
        let message = outer.message.expect("message");
        prop_assert_eq!(message.r#type, if response { CLIENT_RESPONSE } else { CLIENT_QUERY });
        let family = event
            .peer_address
            .or(event.local_address)
            .map(|ip| if ip.is_ipv4() { INET } else { INET6 });
        prop_assert_eq!(message.socket_family, family);
        prop_assert_eq!(
            message.socket_protocol,
            Some(if event.transport == Transport::Tcp { TCP } else { UDP })
        );
        prop_assert_eq!(message.query_address, event.peer_address.map(octets));
        prop_assert_eq!(message.response_address, event.local_address.map(octets));
        prop_assert_eq!(message.query_port, event.peer_port.map(u32::from));
        prop_assert_eq!(message.response_port, event.local_port.map(u32::from));
        let elapsed = event.timestamp.duration_since(SystemTime::UNIX_EPOCH).expect("epoch");
        let (sec, nsec, own, other) = if response {
            (
                message.response_time_sec,
                message.response_time_nsec,
                message.response_message,
                message.query_message,
            )
        } else {
            (
                message.query_time_sec,
                message.query_time_nsec,
                message.query_message,
                message.response_message,
            )
        };
        prop_assert_eq!(sec, Some(elapsed.as_secs()));
        prop_assert_eq!(nsec, Some(elapsed.subsec_nanos()));
        prop_assert_eq!(own.as_deref(), Some(bytes.as_slice()));
        prop_assert_eq!(other, None);
        prop_assert_eq!(message.query_zone, None);
        prop_assert_eq!(message.policy, None);

        // Outer fields appear once, in ascending order, and only 1, 14, 15.
        let outer_fields = wire_fields(&encoded);
        prop_assert!(outer_fields.windows(2).all(|pair| pair[0] < pair[1]));
        prop_assert!(outer_fields.iter().all(|number| matches!(number, 1 | 14 | 15)));
    }
}
