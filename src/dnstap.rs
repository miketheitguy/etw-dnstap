//! Translate DNS Server packet events into the official dnstap wire format.
//!
//! The protobuf field numbers and enum values follow the CC0 dnstap schema,
//! linked by <https://dnstap.info/Source/>:
//! <https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto>.
//! Only fields emitted by this converter are represented here. No `protoc` is
//! needed. RFC 8618 describes the separate C-DNS format, rather than dnstap.

use std::{net::IpAddr, time::SystemTime};

use prost::Message as _;

use crate::event::{DecodedDnsEvent, DnsEventMetadata, Transport};

const DNS_HEADER_LENGTH: usize = 12;
const MAX_DNS_MESSAGE_LENGTH: usize = 65_535;
const MAX_IDENTITY_LENGTH: usize = 1_024;

/// Protobuf wire type 0 (varint), the low three bits of a tag byte.
const WIRE_TYPE_VARINT: u8 = 0;
/// Protobuf wire type 2 (length-delimited), the low three bits of a tag byte.
const WIRE_TYPE_LENGTH_DELIMITED: u8 = 2;
/// A one-byte protobuf tag, `(field_number << 3) | wire_type`, for fields 1..=15.
const fn tag(field_number: u8, wire_type: u8) -> u8 {
    (field_number << 3) | wire_type
}
/// Tag byte of `Dnstap.message` (field 14, length-delimited) in `dnstap.proto`.
const DNSTAP_MESSAGE_TAG: u8 = tag(14, WIRE_TYPE_LENGTH_DELIMITED);
/// Tag byte of `Dnstap.type` (field 15, varint) in `dnstap.proto`.
const DNSTAP_TYPE_TAG: u8 = tag(15, WIRE_TYPE_VARINT);
/// Value of `Dnstap.Type.MESSAGE` in `dnstap.proto`.
const DNSTAP_TYPE_MESSAGE: u8 = 1;
/// Tag byte of `Message.query_message` (field 10, length-delimited).
const QUERY_MESSAGE_TAG: u8 = tag(10, WIRE_TYPE_LENGTH_DELIMITED);
/// Tag byte of `Message.response_message` (field 14, length-delimited).
const RESPONSE_MESSAGE_TAG: u8 = tag(14, WIRE_TYPE_LENGTH_DELIMITED);

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

/// Reusable metadata and final wire storage for one parser thread.
///
/// Client query (256) events map to `CLIENT_QUERY`. Successful (257) and failed
/// (258) client response events map to `CLIENT_RESPONSE`. Every other event ID
/// is rejected. Unknown endpoints remain absent.
/// Storage requests are bounded to 1,024 identity bytes, 16 bytes per address,
/// and 70 KiB of output. `reserve_exact` avoids geometric output growth; the
/// allocator may internally round its allocation beyond the requested size.
pub(crate) struct Encoder {
    tap: EnvelopeMetadata,
    message: DnsMessage,
    query_address: Vec<u8>,
    response_address: Vec<u8>,
    output: Vec<u8>,
}

pub(crate) const MAX_ENCODED_LENGTH: usize = 70 * 1_024;

impl Encoder {
    /// Copy the identity once, omitting an empty identity.
    pub(crate) fn new(identity: &[u8]) -> Result<Self, EncodeError> {
        if identity.len() > MAX_IDENTITY_LENGTH {
            return Err(EncodeError::IdentityTooLong);
        }
        Ok(Self {
            tap: EnvelopeMetadata {
                identity: (!identity.is_empty()).then(|| identity.to_vec()),
                version: None,
                extra: None,
            },
            message: DnsMessage::default(),
            query_address: Vec::with_capacity(16),
            response_address: Vec::with_capacity(16),
            output: Vec::new(),
        })
    }

    /// Encode a decoded event, copying its packet into the final protobuf range.
    ///
    /// The returned bytes borrow only the encoder, so the decoded view (and the
    /// event lease behind it) can be released immediately afterwards. Every
    /// error clears the output.
    pub(crate) fn encode(&mut self, event: &DecodedDnsEvent<'_>) -> anyhow::Result<&[u8]> {
        let packet = event.packet;
        self.encode_with(event.metadata, packet.len(), |destination| {
            destination.copy_from_slice(packet);
            Ok(())
        })
    }

    /// Fill the packet directly in its final, initialized protobuf location.
    ///
    /// The callback runs once after metadata and size validation. It must not
    /// retain the destination pointer. All output space, including the outer
    /// suffix, is reserved first, so no packet bytes move after filling.
    /// Every error clears the output. This validates the DNS envelope, not records.
    fn encode_with(
        &mut self,
        event: DnsEventMetadata,
        packet_length: usize,
        fill: impl FnOnce(&mut [u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<&[u8]> {
        self.output.clear();
        if let Err(error) = self.encode_into(event, packet_length, fill) {
            self.output.clear();
            return Err(error);
        }
        Ok(&self.output)
    }

    /// Test-only entry to [`Self::encode_with`] for tests outside this module.
    #[cfg(test)]
    pub(crate) fn encode_with_for_test(
        &mut self,
        event: DnsEventMetadata,
        packet_length: usize,
        fill: impl FnOnce(&mut [u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<&[u8]> {
        self.encode_with(event, packet_length, fill)
    }

    fn encode_into(
        &mut self,
        event: DnsEventMetadata,
        packet_length: usize,
        fill: impl FnOnce(&mut [u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        // Reclaim optional address storage before clearing the per-event fields.
        if let Some(address) = self.message.query_address.take() {
            self.query_address = address;
        }
        if let Some(address) = self.message.response_address.take() {
            self.response_address = address;
        }
        self.message = DnsMessage::default();
        let (message_type, is_response) = match event.event_id {
            256 => (MessageType::ClientQuery, false),
            257 | 258 => (MessageType::ClientResponse, true),
            other => return Err(EncodeError::UnsupportedEvent(other).into()),
        };
        if !(DNS_HEADER_LENGTH..=MAX_DNS_MESSAGE_LENGTH).contains(&packet_length) {
            return Err(EncodeError::InvalidPacketLength(packet_length).into());
        }
        let timestamp = event
            .timestamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| EncodeError::InvalidTimestamp)?;
        let socket_family = match (event.local_address, event.peer_address) {
            (Some(IpAddr::V4(_)), Some(IpAddr::V6(_)))
            | (Some(IpAddr::V6(_)), Some(IpAddr::V4(_))) => {
                return Err(EncodeError::MixedAddressFamilies.into());
            }
            (Some(IpAddr::V4(_)), _) | (_, Some(IpAddr::V4(_))) => Some(SocketFamily::Inet as i32),
            (Some(IpAddr::V6(_)), _) | (_, Some(IpAddr::V6(_))) => Some(SocketFamily::Inet6 as i32),
            (None, None) => None,
        };
        // The peer is always the client, which initiated the query.
        let (query_address, response_address, query_port, response_port) = (
            event.peer_address,
            event.local_address,
            event.peer_port,
            event.local_port,
        );
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
        if is_response {
            self.message.response_time_sec = Some(timestamp.as_secs());
            self.message.response_time_nsec = Some(timestamp.subsec_nanos());
        } else {
            self.message.query_time_sec = Some(timestamp.as_secs());
            self.message.query_time_nsec = Some(timestamp.subsec_nanos());
        }
        self.fill_message(event.event_id, is_response, packet_length, fill)
    }

    fn fill_message(
        &mut self,
        event_id: u16,
        is_response: bool,
        packet_length: usize,
        fill: impl FnOnce(&mut [u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        // Each manually emitted field has a one-byte tag. Metadata contains
        // only fields before the packet tag (10 for queries, 14 for responses).
        let packet_field_length = checked_length(&[
            1,
            prost::encoding::encoded_len_varint(packet_length as u64),
            packet_length,
        ])?;
        let message_length = checked_length(&[self.message.encoded_len(), packet_field_length])?;
        let length = checked_length(&[
            self.tap.encoded_len(),
            1,
            prost::encoding::encoded_len_varint(message_length as u64),
            message_length,
            2, // required outer type: tag 15, value 1
        ])?;
        if length > MAX_ENCODED_LENGTH {
            return Err(EncodeError::OutputTooLong(length).into());
        }
        if self.output.capacity() < length {
            self.output
                .try_reserve_exact(length)
                .map_err(|_| EncodeError::AllocationFailed)?;
        }
        // Preserve Prost field order: outer metadata, message, outer type.
        self.tap
            .encode(&mut self.output)
            .map_err(EncodeError::Protobuf)?;
        self.output.push(DNSTAP_MESSAGE_TAG);
        prost::encoding::encode_varint(message_length as u64, &mut self.output);
        self.message
            .encode(&mut self.output)
            .map_err(EncodeError::Protobuf)?;
        self.output.push(if is_response {
            RESPONSE_MESSAGE_TAG
        } else {
            QUERY_MESSAGE_TAG
        });
        prost::encoding::encode_varint(packet_length as u64, &mut self.output);
        let packet_start = self.output.len();
        let packet_end = checked_length(&[packet_start, packet_length])?;
        self.output.resize(length, 0);
        self.output[packet_end..].copy_from_slice(&[DNSTAP_TYPE_TAG, DNSTAP_TYPE_MESSAGE]);
        fill(&mut self.output[packet_start..packet_end])?;
        if (self.output[packet_start + 2] & 0x80 != 0) != is_response {
            return Err(EncodeError::IncorrectDirection(event_id).into());
        }
        Ok(())
    }
}

fn checked_length(parts: &[usize]) -> Result<usize, EncodeError> {
    parts.iter().try_fold(0_usize, |sum, part| {
        sum.checked_add(*part)
            .ok_or(EncodeError::OutputTooLong(usize::MAX))
    })
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
struct EnvelopeMetadata {
    #[prost(bytes = "vec", optional, tag = "1")]
    identity: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    version: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    extra: Option<Vec<u8>>,
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
    #[prost(uint64, optional, tag = "12")]
    response_time_sec: Option<u64>,
    #[prost(fixed32, optional, tag = "13")]
    response_time_nsec: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
enum MessageType {
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
#[path = "dnstap/direct_tests.rs"]
mod direct_tests;

#[cfg(test)]
#[path = "dnstap/golden_tests.rs"]
mod golden_tests;

#[cfg(test)]
#[path = "dnstap/test_oracle.rs"]
mod test_oracle;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv4Addr, time::Duration};

    #[test]
    fn only_client_query_and_response_ids_encode() {
        let mut encoder = Encoder::new(b"").expect("identity");
        for id in [0, 255, 259, 260, 261, 262, u16::MAX] {
            let (fixture, packet) = event(id);
            let error = encoder
                .encode_with(fixture, packet.len(), |destination| {
                    destination.copy_from_slice(&packet);
                    Ok(())
                })
                .expect_err("unsupported event ID");
            assert!(
                matches!(
                    error.downcast_ref::<EncodeError>(),
                    Some(EncodeError::UnsupportedEvent(rejected)) if *rejected == id
                ),
                "event {id}"
            );
        }
    }

    #[test]
    fn failed_response_encodes_as_client_response_and_checks_direction() {
        let mut encoder = Encoder::new(b"").expect("identity");
        let (metadata, packet) = event(258);
        assert_eq!(packet[2] & 0x80, 0x80);
        let bytes = encoder
            .encode_with(metadata, packet.len(), |destination| {
                destination.copy_from_slice(&packet);
                Ok(())
            })
            .expect("failed response encodes")
            .to_vec();
        assert_eq!(bytes, test_oracle::reference_bytes(metadata, &packet, b""));
        let tap = test_oracle::ReferenceTap::decode(bytes.as_slice()).expect("protobuf decodes");
        let message = tap.message.expect("DNS message");
        assert_eq!(message.kind, 6);
        assert_eq!(message.response_packet.as_deref(), Some(packet.as_slice()));
        assert_eq!(message.query_packet, None);
        assert_eq!(message.query_seconds, None);
        assert_eq!(message.response_seconds, Some(1_700_000_000));
        // A clear QR bit is a query, not a response.
        let (query_metadata, mut query_packet) = event(258);
        query_packet[2] &= 0x7f;
        let error = encoder
            .encode_with(query_metadata, query_packet.len(), |destination| {
                destination.copy_from_slice(&query_packet);
                Ok(())
            })
            .expect_err("QR clear on event 258");
        assert!(matches!(
            error.downcast_ref::<EncodeError>(),
            Some(EncodeError::IncorrectDirection(258))
        ));
        assert_eq!(encoder.output, [] as [u8; 0]);
    }

    #[test]
    fn decoded_view_encodes_to_the_same_bytes_as_encode_with() {
        let mut encoder = Encoder::new(b"view").expect("identity");
        for id in [256, 257, 258] {
            let (metadata, packet) = event(id);
            let expected = test_oracle::reference_bytes(metadata, &packet, b"view");
            let decoded = DecodedDnsEvent {
                metadata,
                packet: &packet,
            };
            assert_eq!(encoder.encode(&decoded).expect("encodes"), expected);
        }
    }

    fn event(event_id: u16) -> (DnsEventMetadata, Vec<u8>) {
        let mut packet = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        packet.extend_from_slice(b"\x07example\0\0\x01\0\x01");
        if event_id == 257 || event_id == 258 {
            packet[2] |= 0x80;
        }
        if event_id == 258 {
            // RCODE SERVFAIL.
            packet[3] |= 2;
        }
        (
            DnsEventMetadata {
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
    fn mixed_reuse_matches_independent_baseline_and_preserves_source_packet() {
        for identity in [b"".as_slice(), b"dns-server".as_slice()] {
            let mut encoder = Encoder::new(identity).expect("valid identity");
            for (index, id) in [256, 257, 258, 256, 258, 257, 256, 257, 258]
                .into_iter()
                .enumerate()
            {
                let (mut fixture, packet) = event(id);
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
                let expected = test_oracle::reference_bytes(fixture, &packet, identity);
                let bytes = encoder
                    .encode_with(fixture, packet.len(), |destination| {
                        destination.copy_from_slice(&packet);
                        Ok(())
                    })
                    .expect("valid fixture encodes");
                assert_eq!(bytes, expected);
                let tap = test_oracle::ReferenceTap::decode(bytes).expect("protobuf decodes");
                assert_eq!(
                    tap.identity.as_deref(),
                    (!identity.is_empty()).then_some(identity)
                );
                let message = tap.message.expect("DNS message");
                let response = id != 256;
                assert_eq!(
                    message.kind,
                    match id {
                        256 => 5,
                        _ => 6,
                    }
                );
                if response {
                    assert_eq!(
                        message.response_packet.as_deref(),
                        Some(original.as_slice())
                    );
                    assert_eq!(message.query_packet, None);
                    assert_eq!(message.query_seconds, None);
                    assert_eq!(message.query_nanoseconds, None);
                } else {
                    assert_eq!(message.query_packet.as_deref(), Some(original.as_slice()));
                    assert_eq!(message.response_packet, None);
                    assert_eq!(message.response_seconds, None);
                    assert_eq!(message.response_nanoseconds, None);
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
        let packet = vec![0; 12];
        let mut encoder = Encoder::new(b"").expect("empty identity");
        let bytes = encoder
            .encode_with(fixture, packet.len(), |destination| {
                destination.copy_from_slice(&packet);
                Ok(())
            })
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
            let packet = vec![0; length];
            let original = packet.clone();
            let pointer = packet.as_ptr();
            let capacity = packet.capacity();
            let result = encoder.encode_with(fixture, packet.len(), |destination| {
                destination.copy_from_slice(&packet);
                Ok(())
            });
            if (12..=65_535).contains(&length) {
                assert!(result.is_ok());
            } else {
                assert!(
                    matches!(result.expect_err("invalid length").downcast_ref::<EncodeError>(), Some(EncodeError::InvalidPacketLength(n)) if *n == length)
                );
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
                .encode_with(fixture, packet.len(), |destination| {
                    destination.copy_from_slice(&packet);
                    Ok(())
                })
                .expect_err("fixture rejected");
            assert!(matches!(
                (failure, error.downcast_ref::<EncodeError>()),
                (0, Some(EncodeError::UnsupportedEvent(259)))
                    | (1, Some(EncodeError::IncorrectDirection(256)))
                    | (2, Some(EncodeError::InvalidTimestamp))
                    | (3, Some(EncodeError::MixedAddressFamilies))
            ));
            assert_restored(&packet, pointer, capacity, &original);
            assert_eq!(encoder.output, [] as [u8; 0]);
            let (valid, valid_packet) = event(256);
            let expected = test_oracle::reference_bytes(valid, &valid_packet, &[1; 1_024]);
            assert_eq!(
                encoder
                    .encode_with(valid, valid_packet.len(), |destination| {
                        destination.copy_from_slice(&valid_packet);
                        Ok(())
                    })
                    .expect("recovery"),
                expected
            );
        }
    }

    #[test]
    fn serialization_ceiling_rejects_before_filling_and_allows_recovery() {
        // This state cannot come from validated input; exercise the defensive
        // serialization ceiling before the packet destination is exposed.
        let mut encoder = Encoder::new(b"").expect("identity");
        encoder.tap.extra = Some(vec![0; MAX_ENCODED_LENGTH]);
        let (fixture, packet) = event(257);
        let original = packet.clone();
        let pointer = packet.as_ptr();
        let capacity = packet.capacity();
        assert!(matches!(
            encoder
                .encode_with(fixture, packet.len(), |destination| {
                    destination.copy_from_slice(&packet);
                    Ok(())
                })
                .expect_err("output ceiling")
                .downcast_ref::<EncodeError>(),
            Some(EncodeError::OutputTooLong(_))
        ));
        assert_restored(&packet, pointer, capacity, &original);
        assert_eq!(encoder.output, [] as [u8; 0]);
        assert_eq!(encoder.output.capacity(), 0);
        encoder.tap.extra = None;
        let (recovery, recovery_packet) = event(256);
        let expected = test_oracle::reference_bytes(recovery, &recovery_packet, b"");
        assert_eq!(
            encoder
                .encode_with(recovery, recovery_packet.len(), |destination| {
                    destination.copy_from_slice(&recovery_packet);
                    Ok(())
                })
                .expect("recovery"),
            expected
        );
    }
}
