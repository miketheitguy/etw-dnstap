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

/// Metadata actually observed in one DNS Server event; packet storage is separate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RawDnsEvent {
    pub(crate) event_id: u16,
    pub(crate) timestamp: SystemTime,
    pub(crate) transport: Transport,
    pub(crate) peer_address: Option<IpAddr>,
    pub(crate) peer_port: Option<u16>,
    pub(crate) local_address: Option<IpAddr>,
    pub(crate) local_port: Option<u16>,
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
    #[error("encoded dnstap length {0} exceeds 70 KiB")]
    OutputTooLong(usize),
    #[error("could not reserve bounded dnstap output storage")]
    AllocationFailed,
    #[error("could not encode dnstap protobuf: {0}")]
    Protobuf(prost::EncodeError),
}

/// Reusable Prost storage for one parser thread.
///
/// Incoming client events (256/257/258) map to client messages; recursive
/// events (260/261) map to resolver messages. Unknown endpoints remain absent.
/// Storage requests are bounded to 1,024 identity bytes, 16 bytes per address,
/// and 70 KiB of output. `reserve_exact` avoids geometric output growth; the
/// allocator may internally round its allocation beyond the requested size.
pub(crate) struct Encoder {
    tap: Dnstap,
    message: DnsMessage,
    query_address: Vec<u8>,
    response_address: Vec<u8>,
    output: Vec<u8>,
}

const MAX_ENCODED_LENGTH: usize = 70 * 1_024;

impl Encoder {
    /// Copy the identity once, omitting an empty identity.
    pub(crate) fn new(identity: &[u8]) -> Result<Self, EncodeError> {
        if identity.len() > MAX_IDENTITY_LENGTH {
            return Err(EncodeError::IdentityTooLong);
        }
        Ok(Self {
            tap: Dnstap {
                identity: (!identity.is_empty()).then(|| identity.to_vec()),
                version: None,
                extra: None,
                message: None,
                r#type: DnstapType::Message as i32,
            },
            message: DnsMessage::default(),
            query_address: Vec::with_capacity(16),
            response_address: Vec::with_capacity(16),
            output: Vec::new(),
        })
    }

    /// Encode an event without changing the caller packet's bytes or allocation.
    ///
    /// The returned slice borrows only this encoder. All validation precedes
    /// ownership transfer; every fallible serialization result restores the
    /// packet before returning. This validates the DNS envelope, not records.
    pub(crate) fn encode(
        &mut self,
        event: RawDnsEvent,
        packet: &mut Vec<u8>,
    ) -> Result<&[u8], EncodeError> {
        self.output.clear();
        // Reclaim optional address storage before clearing the per-event fields.
        if let Some(address) = self.message.query_address.take() {
            self.query_address = address;
        }
        if let Some(address) = self.message.response_address.take() {
            self.response_address = address;
        }
        self.message = DnsMessage::default();
        let (message_type, is_response, peer_is_initiator) = match event.event_id {
            256 => (MessageType::ClientQuery, false, true),
            257 | 258 => (MessageType::ClientResponse, true, true),
            260 => (MessageType::ResolverQuery, false, false),
            261 => (MessageType::ResolverResponse, true, false),
            other => return Err(EncodeError::UnsupportedEvent(other)),
        };
        if !(DNS_HEADER_LENGTH..=MAX_DNS_MESSAGE_LENGTH).contains(&packet.len()) {
            return Err(EncodeError::InvalidPacketLength(packet.len()));
        }
        if (packet[2] & 0x80 != 0) != is_response {
            return Err(EncodeError::IncorrectDirection(event.event_id));
        }
        let timestamp = event
            .timestamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| EncodeError::InvalidTimestamp)?;
        let socket_family = match (event.local_address, event.peer_address) {
            (Some(IpAddr::V4(_)), Some(IpAddr::V6(_)))
            | (Some(IpAddr::V6(_)), Some(IpAddr::V4(_))) => {
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
        self.message.r#type = message_type as i32;
        self.message.socket_family = socket_family;
        self.message.socket_protocol = Some(match event.transport {
            Transport::Udp => SocketProtocol::Udp as i32,
            Transport::Tcp => SocketProtocol::Tcp as i32,
        });
        self.message.query_address = fill_address(query_address, &mut self.query_address);
        self.message.response_address = fill_address(response_address, &mut self.response_address);
        self.message.query_port = query_port.map(u32::from);
        self.message.response_port = response_port.map(u32::from);
        let packet_field = if is_response {
            self.message.response_time_sec = Some(timestamp.as_secs());
            self.message.response_time_nsec = Some(timestamp.subsec_nanos());
            &mut self.message.response_message
        } else {
            self.message.query_time_sec = Some(timestamp.as_secs());
            self.message.query_time_nsec = Some(timestamp.subsec_nanos());
            &mut self.message.query_message
        };
        *packet_field = Some(std::mem::take(packet));
        self.tap.message = Some(std::mem::take(&mut self.message));
        // Do not propagate an error until ownership has returned to the caller.
        let result = self.encode_message();
        self.message = self.tap.message.take().unwrap_or_default();
        let packet_field = if is_response {
            &mut self.message.response_message
        } else {
            &mut self.message.query_message
        };
        *packet = packet_field.take().unwrap_or_default();
        if let Err(error) = result {
            self.output.clear();
            return Err(error);
        }
        Ok(&self.output)
    }

    fn encode_message(&mut self) -> Result<(), EncodeError> {
        let length = self.tap.encoded_len();
        if length > MAX_ENCODED_LENGTH {
            return Err(EncodeError::OutputTooLong(length));
        }
        if self.output.capacity() < length {
            self.output
                .try_reserve_exact(length)
                .map_err(|_| EncodeError::AllocationFailed)?;
        }
        // Prost performs its own length check. Pre-reservation keeps Vec's
        // BufMut implementation from growing geometrically while encoding.
        self.tap
            .encode(&mut self.output)
            .map_err(EncodeError::Protobuf)
    }
}

fn fill_address(address: Option<IpAddr>, storage: &mut Vec<u8>) -> Option<Vec<u8>> {
    storage.clear();
    let address = address?;
    match address {
        IpAddr::V4(address) => storage.extend_from_slice(&address.octets()),
        IpAddr::V6(address) => storage.extend_from_slice(&address.octets()),
    }
    Some(std::mem::take(storage))
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

    fn event(event_id: u16) -> (RawDnsEvent, Vec<u8>) {
        let mut packet = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        packet.extend_from_slice(b"\x07example\0\0\x01\0\x01");
        if matches!(event_id, 257 | 258 | 261) {
            packet[2] |= 0x80;
        }
        (
            RawDnsEvent {
                event_id,
                timestamp: SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700),
                transport: Transport::Udp,
                peer_address: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
                peer_port: Some(53_000),
                local_address: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53))),
                local_port: None,
            },
            packet,
        )
    }

    fn assert_restored(packet: &Vec<u8>, pointer: *const u8, capacity: usize, original: &[u8]) {
        assert_eq!(packet.as_ptr(), pointer);
        assert_eq!(packet.capacity(), capacity);
        assert_eq!(packet, original);
    }

    #[test]
    fn mixed_reuse_matches_independent_baseline_and_restores_packet() {
        for identity in [b"".as_slice(), b"dns-server".as_slice()] {
            let mut encoder = Encoder::new(identity).expect("valid identity");
            for (index, id) in [256, 257, 258, 260, 261, 256, 257, 260]
                .into_iter()
                .enumerate()
            {
                let (mut fixture, mut packet) = event(id);
                if index % 3 == 1 {
                    fixture.transport = Transport::Tcp;
                    fixture.peer_address = Some("2001:db8::53".parse().expect("IPv6"));
                    fixture.local_address = Some("2001:db8::1".parse().expect("IPv6"));
                    fixture.peer_port = None;
                    fixture.local_port = Some(0);
                } else if index % 3 == 2 {
                    fixture.peer_address = None;
                    fixture.local_address = None;
                    fixture.peer_port = None;
                    fixture.local_port = None;
                    fixture.timestamp = SystemTime::UNIX_EPOCH;
                }
                if index == 6 {
                    fixture.local_address = None;
                    fixture.peer_address = Some("2001:db8::53".parse().expect("IPv6"));
                } else if index == 7 {
                    fixture.peer_address = None;
                }
                let original = packet.clone();
                let pointer = packet.as_ptr();
                let capacity = packet.capacity();
                let expected =
                    reference(&fixture, packet.clone(), identity).expect("baseline encodes");
                let bytes = encoder
                    .encode(fixture, &mut packet)
                    .expect("valid fixture encodes");
                assert_eq!(bytes, expected);
                let tap = Dnstap::decode(bytes).expect("protobuf decodes");
                assert_eq!(
                    tap.identity.as_deref(),
                    (!identity.is_empty()).then_some(identity)
                );
                let message = tap.message.expect("DNS message");
                let response = matches!(id, 257 | 258 | 261);
                assert_eq!(
                    message.r#type,
                    match id {
                        256 => 5,
                        257 | 258 => 6,
                        260 => 3,
                        _ => 4,
                    }
                );
                if response {
                    assert_eq!(
                        message.response_message.as_deref(),
                        Some(original.as_slice())
                    );
                    assert_eq!(message.query_message, None);
                    assert_eq!(message.query_time_sec, None);
                    assert_eq!(message.query_time_nsec, None);
                } else {
                    assert_eq!(message.query_message.as_deref(), Some(original.as_slice()));
                    assert_eq!(message.response_message, None);
                    assert_eq!(message.response_time_sec, None);
                    assert_eq!(message.response_time_nsec, None);
                }
                assert_restored(&packet, pointer, capacity, &original);
                assert!(encoder.output.capacity() <= MAX_ENCODED_LENGTH);
                assert!(encoder.query_address.capacity() <= 16);
                assert!(encoder.response_address.capacity() <= 16);
                assert!(
                    encoder
                        .message
                        .query_address
                        .as_ref()
                        .map_or(0, Vec::capacity)
                        <= 16
                );
                assert!(
                    encoder
                        .message
                        .response_address
                        .as_ref()
                        .map_or(0, Vec::capacity)
                        <= 16
                );
            }
        }
    }

    #[test]
    fn uses_upstream_tags_and_fixed32_nanoseconds() {
        let (mut fixture, _) = event(256);
        fixture.timestamp = SystemTime::UNIX_EPOCH;
        fixture.peer_address = None;
        fixture.local_address = None;
        fixture.peer_port = None;
        let mut packet = vec![0; 12];
        let mut encoder = Encoder::new(b"").expect("empty identity");
        let bytes = encoder
            .encode(fixture, &mut packet)
            .expect("valid fixture encodes");
        // Independent wire golden: tag 9 is fixed32, including present zero.
        let expected = [
            0x72, 25, 0x08, 5, 0x18, 1, 0x40, 0, 0x4d, 0, 0, 0, 0, 0x52, 12, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0x78, 1,
        ];
        assert_eq!(bytes, expected);
    }

    #[test]
    fn validates_boundaries_and_recovers_without_stale_output() {
        let mut encoder = Encoder::new(&[1; 1_024]).expect("maximum identity");
        assert_eq!(
            encoder.tap.identity.as_ref().map(Vec::capacity),
            Some(1_024)
        );
        assert!(matches!(
            Encoder::new(&[0; 1_025]),
            Err(EncodeError::IdentityTooLong)
        ));
        for length in [0, 1, 11, 12, 65_535, 65_536] {
            let (fixture, _) = event(256);
            let mut packet = vec![0; length];
            let original = packet.clone();
            let pointer = packet.as_ptr();
            let capacity = packet.capacity();
            let result = encoder.encode(fixture, &mut packet);
            if (12..=65_535).contains(&length) {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(EncodeError::InvalidPacketLength(n)) if n == length));
                assert_eq!(encoder.output, [] as [u8; 0]);
            }
            assert_restored(&packet, pointer, capacity, &original);
            assert!(encoder.output.capacity() <= MAX_ENCODED_LENGTH);
        }
        for failure in 0..4 {
            let (mut fixture, mut packet) = event(256);
            match failure {
                0 => fixture.event_id = 259,
                1 => packet[2] |= 0x80,
                2 => fixture.timestamp = SystemTime::UNIX_EPOCH - Duration::from_secs(1),
                _ => fixture.local_address = Some("::1".parse().expect("IPv6")),
            }
            let original = packet.clone();
            let pointer = packet.as_ptr();
            let capacity = packet.capacity();
            let error = encoder
                .encode(fixture, &mut packet)
                .expect_err("fixture rejected");
            assert!(matches!(
                (failure, error),
                (0, EncodeError::UnsupportedEvent(259))
                    | (1, EncodeError::IncorrectDirection(256))
                    | (2, EncodeError::InvalidTimestamp)
                    | (3, EncodeError::MixedAddressFamilies)
            ));
            assert_restored(&packet, pointer, capacity, &original);
            assert_eq!(encoder.output, [] as [u8; 0]);
            let (valid, mut packet) = event(256);
            let expected =
                reference(&valid, packet.clone(), &[1; 1_024]).expect("baseline encodes");
            assert_eq!(
                encoder.encode(valid, &mut packet).expect("recovery"),
                expected
            );
        }
    }

    #[test]
    fn serialization_failure_restores_packet_and_allows_recovery() {
        // This state cannot come from validated input; exercise the defensive
        // serialization ceiling after the packet has moved into Prost storage.
        let mut encoder = Encoder::new(b"").expect("identity");
        encoder.tap.extra = Some(vec![0; MAX_ENCODED_LENGTH]);
        let (fixture, mut packet) = event(257);
        let original = packet.clone();
        let pointer = packet.as_ptr();
        let capacity = packet.capacity();
        assert!(matches!(
            encoder.encode(fixture, &mut packet),
            Err(EncodeError::OutputTooLong(_))
        ));
        assert_restored(&packet, pointer, capacity, &original);
        assert_eq!(encoder.output, [] as [u8; 0]);
        assert_eq!(encoder.output.capacity(), 0);
        encoder.tap.extra = None;
        let (fixture, mut packet) = event(256);
        let expected = reference(&fixture, packet.clone(), b"").expect("baseline encodes");
        assert_eq!(
            encoder.encode(fixture, &mut packet).expect("recovery"),
            expected
        );
    }

    #[test]
    fn warmed_encoder_reuses_storage_without_heap_operations() {
        let mut encoder = Encoder::new(b"dns-server").expect("identity");
        let (fixture, mut packet) = event(256);
        encoder.encode(fixture, &mut packet).expect("warmup");
        let (fixture, _) = event(256);
        let mut result = false;
        crate::etw::assert_no_allocations(|| {
            result = encoder.encode(fixture, &mut packet).is_ok();
        });
        assert!(result);
    }

    // Independent allocating baseline, retained only as a compatibility oracle.
    fn reference(
        event: &RawDnsEvent,
        packet_data: Vec<u8>,
        identity: &[u8],
    ) -> Result<Vec<u8>, EncodeError> {
        let (message_type, is_response, peer_is_initiator) = match event.event_id {
            256 => (MessageType::ClientQuery, false, true),
            257 | 258 => (MessageType::ClientResponse, true, true),
            260 => (MessageType::ResolverQuery, false, false),
            261 => (MessageType::ResolverResponse, true, false),
            other => return Err(EncodeError::UnsupportedEvent(other)),
        };
        if !(DNS_HEADER_LENGTH..=MAX_DNS_MESSAGE_LENGTH).contains(&packet_data.len()) {
            return Err(EncodeError::InvalidPacketLength(packet_data.len()));
        }
        // The length check above guarantees the third byte is present.
        let flags_high = packet_data
            .get(2)
            .copied()
            .ok_or(EncodeError::InvalidPacketLength(packet_data.len()))?;
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
            (Some(IpAddr::V4(_)), Some(IpAddr::V6(_)))
            | (Some(IpAddr::V6(_)), Some(IpAddr::V4(_))) => {
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
            message.response_message = Some(packet_data);
        } else {
            message.query_time_sec = Some(timestamp.as_secs());
            message.query_time_nsec = Some(timestamp.subsec_nanos());
            message.query_message = Some(packet_data);
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
}
