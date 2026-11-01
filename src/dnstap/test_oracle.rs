//! The shared allocating reference encoder for the encoder tests.
//!
//! Its Prost models are separate from the production schema types, field
//! ordering and length arithmetic, so a mistake in the encoder is not repeated
//! here. `golden_tests` keeps a second, hand-assembled oracle that does not use
//! Prost at all.

use crate::event::{DnsEventMetadata, Transport};
use prost::Message as _;
use std::{net::IpAddr, time::SystemTime};

// Separate Prost models keep the oracle independent of the encoder's schema
// types, field ordering helpers, length calculations, and enum definitions.
#[derive(Clone, PartialEq, prost::Message)]
pub(super) struct ReferenceTap {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub(super) identity: Option<Vec<u8>>,
    #[prost(message, optional, tag = "14")]
    pub(super) message: Option<ReferenceMessage>,
    #[prost(uint32, required, tag = "15")]
    pub(super) kind: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub(super) struct ReferenceMessage {
    #[prost(uint32, required, tag = "1")]
    pub(super) kind: u32,
    #[prost(uint32, optional, tag = "2")]
    pub(super) family: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub(super) protocol: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub(super) query_address: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub(super) response_address: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "6")]
    pub(super) query_port: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    pub(super) response_port: Option<u32>,
    #[prost(uint64, optional, tag = "8")]
    pub(super) query_seconds: Option<u64>,
    #[prost(fixed32, optional, tag = "9")]
    pub(super) query_nanoseconds: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "10")]
    pub(super) query_packet: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "12")]
    pub(super) response_seconds: Option<u64>,
    #[prost(fixed32, optional, tag = "13")]
    pub(super) response_nanoseconds: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "14")]
    pub(super) response_packet: Option<Vec<u8>>,
}

pub(super) fn address(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    }
}

pub(super) fn reference(event: DnsEventMetadata, bytes: &[u8], identity: &[u8]) -> ReferenceTap {
    let response = event.event_id == 257;
    let (query_ip, response_ip, query_port, response_port) = (
        event.peer_address,
        event.local_address,
        event.peer_port,
        event.local_port,
    );
    let timestamp = event
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("fixture time");
    let mut message = ReferenceMessage {
        kind: match event.event_id {
            256 => 5,
            257 => 6,
            _ => panic!("unsupported fixture event"),
        },
        family: event
            .peer_address
            .or(event.local_address)
            .map(|ip| if ip.is_ipv4() { 1 } else { 2 }),
        protocol: Some(if event.transport == Transport::Tcp {
            2
        } else {
            1
        }),
        query_address: query_ip.map(address),
        response_address: response_ip.map(address),
        query_port: query_port.map(u32::from),
        response_port: response_port.map(u32::from),
        ..ReferenceMessage::default()
    };
    if response {
        message.response_seconds = Some(timestamp.as_secs());
        message.response_nanoseconds = Some(timestamp.subsec_nanos());
        message.response_packet = Some(bytes.to_vec());
    } else {
        message.query_seconds = Some(timestamp.as_secs());
        message.query_nanoseconds = Some(timestamp.subsec_nanos());
        message.query_packet = Some(bytes.to_vec());
    }
    ReferenceTap {
        identity: (!identity.is_empty()).then(|| identity.to_vec()),
        message: Some(message),
        kind: 1,
    }
}

/// The reference encoding of `event` as bytes.
pub(super) fn reference_bytes(event: DnsEventMetadata, bytes: &[u8], identity: &[u8]) -> Vec<u8> {
    reference(event, bytes, identity).encode_to_vec()
}

/// Reads one protobuf varint; returns the value and the remaining input.
pub(super) fn read_varint(input: &[u8]) -> (u64, &[u8]) {
    let mut value = 0_u64;
    for (index, byte) in input.iter().enumerate() {
        value |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return (value, &input[index + 1..]);
        }
    }
    panic!("truncated varint");
}
