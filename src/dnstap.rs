//! Translate DNS Server packet events into the official dnstap wire format.
//!
//! The protobuf field numbers and enum values follow the CC0 dnstap schema,
//! linked by <https://dnstap.info/Source/>:
//! <https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto>.
//! Only fields emitted by this converter are represented here. No `protoc` is
//! needed. RFC 8618 describes the separate C-DNS format, rather than dnstap.

use std::{net::IpAddr, time::SystemTime};

use prost::Message as _;

const DNS_HEADER_LENGTH: usize = 12;
const MAX_DNS_MESSAGE_LENGTH: usize = 65_535;
const MAX_IDENTITY_LENGTH: usize = 1_024;

/// DNS transport reported by the ETW provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Transport {
    Udp,
    Tcp,
}

/// An owned packet and the metadata actually observed in one DNS Server event.
#[derive(Debug)]
pub(crate) struct RawDnsEvent {
    pub(crate) event_id: u16,
    pub(crate) timestamp: SystemTime,
    pub(crate) transport: Transport,
    pub(crate) peer_address: Option<IpAddr>,
    pub(crate) peer_port: Option<u16>,
    pub(crate) local_address: Option<IpAddr>,
    pub(crate) local_port: Option<u16>,
    /// DNS wire bytes, without a TCP length prefix or IP/transport headers.
    pub(crate) packet_data: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EncodeError {
    #[error("unsupported DNS Server event ID {0}")]
    UnsupportedEvent(u16),
    #[error("DNS packet length {0} is outside 12..=65535 bytes")]
    InvalidPacketLength(usize),
    #[error("DNS QR bit disagrees with DNS Server event ID {0}")]
    IncorrectDirection(u16),
    #[error("DNS event timestamp precedes the Unix epoch")]
    InvalidTimestamp,
    #[error("local and peer IP address families disagree")]
    MixedAddressFamilies,
    #[error("dnstap identity exceeds 1024 bytes")]
    IdentityTooLong,
}

/// Encode one observed event, preserving its DNS wire bytes verbatim.
///
/// Incoming client traffic (256) and outgoing client responses (257/258) map to
/// `CLIENT_QUERY`/`CLIENT_RESPONSE`. Recursive traffic (260/261) maps to
/// `RESOLVER_QUERY`/`RESOLVER_RESPONSE`. Events without packets and other provider
/// event types are intentionally unsupported. Unknown addresses and ports stay
/// absent; this function does not infer port 53 or pair separate events.
/// These mappings describe a recursive DNS server; they do not distinguish
/// authoritative or forwarding roles automatically.
///
/// # Errors
///
/// Rejects unsupported event IDs, invalid packet lengths, inconsistent QR or IP
/// family metadata, pre-epoch timestamps, and oversized identities. This checks
/// the DNS header envelope, not the validity of every DNS record.
pub(crate) fn encode_event(event: RawDnsEvent, identity: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let (message_type, is_response, peer_is_initiator) = match event.event_id {
        256 => (MessageType::ClientQuery, false, true),
        257 | 258 => (MessageType::ClientResponse, true, true),
        260 => (MessageType::ResolverQuery, false, false),
        261 => (MessageType::ResolverResponse, true, false),
        other => return Err(EncodeError::UnsupportedEvent(other)),
    };
    if !(DNS_HEADER_LENGTH..=MAX_DNS_MESSAGE_LENGTH).contains(&event.packet_data.len()) {
        return Err(EncodeError::InvalidPacketLength(event.packet_data.len()));
    }
    // The length check above guarantees the third byte is present.
    let flags_high = event
        .packet_data
        .get(2)
        .copied()
        .ok_or(EncodeError::InvalidPacketLength(event.packet_data.len()))?;
    if (flags_high & 0x80 != 0) != is_response {
        return Err(EncodeError::IncorrectDirection(event.event_id));
    }
    if identity.len() > MAX_IDENTITY_LENGTH {
        return Err(EncodeError::IdentityTooLong);
    }
    let timestamp = event
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| EncodeError::InvalidTimestamp)?;
    let socket_family = match (event.local_address, event.peer_address) {
        (Some(IpAddr::V4(_)), Some(IpAddr::V6(_))) | (Some(IpAddr::V6(_)), Some(IpAddr::V4(_))) => {
            return Err(EncodeError::MixedAddressFamilies);
        }
        (Some(IpAddr::V4(_)), _) | (_, Some(IpAddr::V4(_))) => Some(SocketFamily::Inet as i32),
        (Some(IpAddr::V6(_)), _) | (_, Some(IpAddr::V6(_))) => Some(SocketFamily::Inet6 as i32),
        (None, None) => None,
    };
    let (query_address, response_address, query_port, response_port) = if peer_is_initiator {
        (
            event.peer_address,
            event.local_address,
            event.peer_port,
            event.local_port,
        )
    } else {
        (
            event.local_address,
            event.peer_address,
            event.local_port,
            event.peer_port,
        )
    };
    let mut message = DnsMessage {
        r#type: message_type as i32,
        socket_family,
        socket_protocol: Some(match event.transport {
            Transport::Udp => SocketProtocol::Udp as i32,
            Transport::Tcp => SocketProtocol::Tcp as i32,
        }),
        query_address: query_address.map(address_bytes),
        response_address: response_address.map(address_bytes),
        query_port: query_port.map(u32::from),
        response_port: response_port.map(u32::from),
        ..DnsMessage::default()
    };
    if is_response {
        message.response_time_sec = Some(timestamp.as_secs());
        message.response_time_nsec = Some(timestamp.subsec_nanos());
        message.response_message = Some(event.packet_data);
    } else {
        message.query_time_sec = Some(timestamp.as_secs());
        message.query_time_nsec = Some(timestamp.subsec_nanos());
        message.query_message = Some(event.packet_data);
    }
    Ok(Dnstap {
        identity: (!identity.is_empty()).then(|| identity.to_vec()),
        version: None,
        extra: None,
        message: Some(message),
        r#type: DnstapType::Message as i32,
    }
    .encode_to_vec())
}

fn address_bytes(address: IpAddr) -> Vec<u8> {
    match address {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    }
}

// Handwritten, wire-compatible subset of the upstream proto2 schema. Optional
// scalars retain presence. In particular, the nanosecond fields are fixed32.
#[derive(Clone, PartialEq, prost::Message)]
struct Dnstap {
    #[prost(bytes = "vec", optional, tag = "1")]
    identity: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    version: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    extra: Option<Vec<u8>>,
    #[prost(message, optional, tag = "14")]
    message: Option<DnsMessage>,
    #[prost(enumeration = "DnstapType", required, tag = "15")]
    r#type: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
enum DnstapType {
    Message = 1,
}

#[derive(Clone, PartialEq, prost::Message)]
struct DnsMessage {
    #[prost(enumeration = "MessageType", required, tag = "1")]
    r#type: i32,
    #[prost(enumeration = "SocketFamily", optional, tag = "2")]
    socket_family: Option<i32>,
    #[prost(enumeration = "SocketProtocol", optional, tag = "3")]
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
    #[prost(uint64, optional, tag = "12")]
    response_time_sec: Option<u64>,
    #[prost(fixed32, optional, tag = "13")]
    response_time_nsec: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "14")]
    response_message: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
enum MessageType {
    ResolverQuery = 3,
    ResolverResponse = 4,
    ClientQuery = 5,
    ClientResponse = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
enum SocketFamily {
    Inet = 1,
    Inet6 = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
enum SocketProtocol {
    Udp = 1,
    Tcp = 2,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv4Addr, time::Duration};

    fn event(event_id: u16) -> RawDnsEvent {
        let mut packet_data = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        // example. IN A, with exact bytes retained rather than reserialized.
        packet_data.extend_from_slice(b"\x07example\0\0\x01\0\x01");
        if matches!(event_id, 257 | 258 | 261) {
            packet_data[2] |= 0x80;
        }
        RawDnsEvent {
            event_id,
            timestamp: SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700),
            transport: Transport::Udp,
            peer_address: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
            peer_port: Some(53_000),
            local_address: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53))),
            local_port: None,
            packet_data,
        }
    }

    fn decode(event: RawDnsEvent) -> DnsMessage {
        let bytes = encode_event(event, b"dns-server").expect("valid fixture encodes");
        let tap = Dnstap::decode(bytes.as_slice()).expect("protobuf decodes");
        assert_eq!(tap.r#type, 1);
        assert_eq!(tap.identity.as_deref(), Some(b"dns-server".as_slice()));
        tap.message.expect("message payload exists")
    }

    #[test]
    fn maps_every_supported_event_and_preserves_packet_bytes() {
        for (event_id, expected_type, response, peer_initiates) in [
            (256, 5, false, true),
            (257, 6, true, true),
            (258, 6, true, true),
            (260, 3, false, false),
            (261, 4, true, false),
        ] {
            let fixture = event(event_id);
            let original = fixture.packet_data.clone();
            let message = decode(fixture);
            assert_eq!(message.r#type, expected_type);
            assert_eq!(message.socket_family, Some(1));
            assert_eq!(message.socket_protocol, Some(1));
            if response {
                assert_eq!(message.response_message, Some(original));
                assert_eq!(message.response_time_sec, Some(1_700_000_000));
                assert_eq!(message.response_time_nsec, Some(123_456_700));
                assert_eq!(message.query_message, None);
                assert_eq!(message.query_time_sec, None);
                assert_eq!(message.query_time_nsec, None);
            } else {
                assert_eq!(message.query_message, Some(original));
                assert_eq!(message.query_time_sec, Some(1_700_000_000));
                assert_eq!(message.query_time_nsec, Some(123_456_700));
                assert_eq!(message.response_message, None);
                assert_eq!(message.response_time_sec, None);
                assert_eq!(message.response_time_nsec, None);
            }
            if peer_initiates {
                assert_eq!(message.query_address, Some(vec![192, 0, 2, 1]));
                assert_eq!(message.response_address, Some(vec![192, 0, 2, 53]));
                assert_eq!(message.query_port, Some(53_000));
                assert_eq!(message.response_port, None);
            } else {
                assert_eq!(message.query_address, Some(vec![192, 0, 2, 53]));
                assert_eq!(message.response_address, Some(vec![192, 0, 2, 1]));
                assert_eq!(message.query_port, None);
                assert_eq!(message.response_port, Some(53_000));
            }
        }
    }

    #[test]
    fn preserves_unknown_endpoints_and_encodes_ipv6_tcp() {
        let mut fixture = event(260);
        fixture.transport = Transport::Tcp;
        fixture.peer_address = Some("2001:db8::53".parse().expect("valid IPv6 fixture"));
        fixture.peer_port = None;
        fixture.local_address = None;
        let message = decode(fixture);
        assert_eq!(message.socket_family, Some(2));
        assert_eq!(message.socket_protocol, Some(2));
        assert_eq!(message.query_address, None);
        assert_eq!(message.query_port, None);
        assert_eq!(message.response_port, None);
        assert_eq!(message.response_address.as_ref().map(Vec::len), Some(16));
    }

    #[test]
    fn uses_upstream_tags_and_fixed32_nanoseconds() {
        let mut fixture = event(256);
        fixture.timestamp = SystemTime::UNIX_EPOCH;
        fixture.peer_address = None;
        fixture.local_address = None;
        fixture.peer_port = None;
        fixture.packet_data = vec![0; 12];
        let bytes = encode_event(fixture, b"").expect("valid fixture encodes");
        // Independently specified protobuf wire bytes: top-level MESSAGE tag 15;
        // nested tag 9 is fixed32 (wire type 5), not a varint.
        let expected = [
            0x72, 25, 0x08, 5, 0x18, 1, 0x40, 0, 0x4d, 0, 0, 0, 0, 0x52, 12, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0x78, 1,
        ];
        assert_eq!(bytes, expected);
    }

    #[test]
    fn rejects_malformed_envelopes_and_metadata() {
        for length in [0, 1, 11, 65_536] {
            let mut fixture = event(256);
            fixture.packet_data = vec![0; length];
            assert!(matches!(
                encode_event(fixture, b""),
                Err(EncodeError::InvalidPacketLength(_))
            ));
        }
        let mut fixture = event(256);
        fixture.packet_data[2] |= 0x80;
        assert!(matches!(
            encode_event(fixture, b""),
            Err(EncodeError::IncorrectDirection(256))
        ));
        let mut fixture = event(256);
        fixture.timestamp = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
        assert!(matches!(
            encode_event(fixture, b""),
            Err(EncodeError::InvalidTimestamp)
        ));
        let mut fixture = event(256);
        fixture.local_address = Some("::1".parse().expect("valid IPv6 fixture"));
        assert!(matches!(
            encode_event(fixture, b""),
            Err(EncodeError::MixedAddressFamilies)
        ));
        assert!(matches!(
            encode_event(event(259), b""),
            Err(EncodeError::UnsupportedEvent(259))
        ));
        assert!(matches!(
            encode_event(event(256), &[0; 1_025]),
            Err(EncodeError::IdentityTooLong)
        ));
    }
}
