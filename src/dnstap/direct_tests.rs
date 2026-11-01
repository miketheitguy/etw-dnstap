//! Independent wire and storage checks for direct packet filling.

use super::{
    EncodeError, Encoder,
    test_oracle::{ReferenceTap, read_varint, reference},
};
use crate::event::{DnsEventMetadata, Transport};
use prost::Message as _;
use std::time::{Duration, SystemTime};

fn fixture(id: u16, endpoints: usize, tcp: bool) -> DnsEventMetadata {
    let (peer, local) = match endpoints {
        0 => (Some("192.0.2.1"), Some("192.0.2.53")),
        1 => (Some("2001:db8::1"), Some("2001:db8::53")),
        2 => (None, None),
        3 => (Some("2001:db8::1"), None),
        4 => (None, Some("192.0.2.53")),
        _ => panic!("unknown endpoint fixture"),
    };
    DnsEventMetadata {
        event_id: id,
        timestamp: SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, 987_654_321),
        transport: if tcp { Transport::Tcp } else { Transport::Udp },
        peer_address: peer.map(|ip| ip.parse().expect("fixture address")),
        local_address: local.map(|ip| ip.parse().expect("fixture address")),
        peer_port: (endpoints != 2 && endpoints != 4).then_some(0),
        local_port: (endpoints != 2 && endpoints != 3).then_some(65_535),
    }
}

fn packet(id: u16, length: usize) -> Vec<u8> {
    let mut bytes = vec![0x5a; length];
    bytes[2] = if id == 257 { 0x81 } else { 0x01 };
    bytes
}

fn compare(encoder: &mut Encoder, event: DnsEventMetadata, bytes: &[u8], identity: &[u8]) {
    let expected = reference(event, bytes, identity);
    let actual = encoder
        .encode_with(event, bytes.len(), |destination| {
            destination.copy_from_slice(bytes);
            Ok(())
        })
        .expect("direct encoding succeeds");
    assert_eq!(
        actual,
        expected.encode_to_vec(),
        "id={} packet length={}",
        event.event_id,
        bytes.len()
    );
    assert_eq!(
        ReferenceTap::decode(actual).expect("independent typed decode"),
        expected
    );
}

#[test]
fn direct_bytes_match_independent_prost_across_event_and_packet_boundaries() {
    for identity in [
        Vec::new(),
        vec![0x33; 127],
        vec![0x44; 128],
        vec![0x55; 1_024],
    ] {
        let mut encoder = Encoder::new(&identity).expect("valid identity");
        for id in [256, 257] {
            for endpoints in 0..5 {
                for tcp in [false, true] {
                    for length in [12, 126, 127, 128, 129, 16_383, 16_384, 65_535] {
                        compare(
                            &mut encoder,
                            fixture(id, endpoints, tcp),
                            &packet(id, length),
                            &identity,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn enclosing_message_lengths_cross_both_varint_boundaries() {
    for id in [256, 257] {
        for endpoints in 0..5 {
            let event = fixture(id, endpoints, true);
            let mut encoder = Encoder::new(&[0x66; 128]).expect("identity");
            for boundary in [128_usize, 16_384] {
                // Only examine a small window: fixed metadata is under 128
                // bytes, so both sides of each nested length transition fit.
                let mut covered = [false; 2];
                for length in boundary.saturating_sub(128).max(12)..=boundary {
                    let bytes = packet(id, length);
                    let expected = reference(event, &bytes, &[0x66; 128]);
                    let message_length = expected.message.as_ref().expect("message").encoded_len();
                    if message_length == boundary - 1 || message_length == boundary {
                        covered[usize::from(message_length == boundary)] = true;
                        compare(&mut encoder, event, &bytes, &[0x66; 128]);
                    }
                }
                assert_eq!(
                    covered,
                    [true, true],
                    "id={id}, endpoints={endpoints}, boundary={boundary}"
                );
            }
        }
    }
}

fn length_field(mut input: &[u8], tag: u64) -> &[u8] {
    while !input.is_empty() {
        let (key, rest) = read_varint(input);
        input = rest;
        match key & 7 {
            0 => input = read_varint(input).1,
            2 => {
                let (length, rest) = read_varint(input);
                let (bytes, rest) = rest.split_at(usize::try_from(length).expect("length"));
                if key >> 3 == tag {
                    return bytes;
                }
                input = rest;
            }
            5 => input = &input[4..],
            wire => panic!("unexpected wire type {wire}"),
        }
    }
    panic!("missing field {tag}")
}

#[test]
fn fill_destination_is_initialized_and_is_the_final_packet_location() {
    let mut encoder = Encoder::new(b"direct").expect("identity");
    for (id, length) in [(256, 65_535), (257, 128), (257, 16_384), (256, 12)] {
        let bytes = packet(id, length);
        let mut calls = 0;
        let mut destination_pointer = std::ptr::null();
        let output = encoder
            .encode_with(fixture(id, 1, true), length, |destination| {
                calls += 1;
                assert_eq!(destination.len(), length);
                assert!(
                    destination.iter().all(|&byte| byte == 0),
                    "fresh and reused ranges are initialized"
                );
                destination_pointer = destination.as_ptr();
                destination.copy_from_slice(&bytes);
                Ok(())
            })
            .expect("encoding");
        let message = length_field(output, 14);
        let wire_packet = length_field(message, if id == 257 { 14 } else { 10 });
        assert_eq!(calls, 1);
        assert_eq!(wire_packet, bytes);
        assert_eq!(
            wire_packet.as_ptr(),
            destination_pointer,
            "packet must not move after callback"
        );
        assert!(
            (output.as_ptr() as usize..output.as_ptr() as usize + output.len())
                .contains(&(destination_pointer as usize))
        );
    }
}

#[test]
fn invalid_metadata_and_sizes_never_invoke_fill_and_clear_previous_output() {
    let mut encoder = Encoder::new(b"test").expect("identity");
    for length in [0, 1, 11, 65_536, usize::MAX] {
        compare(
            &mut encoder,
            fixture(257, 1, true),
            &packet(257, 12),
            b"test",
        );
        let error = encoder
            .encode_with(fixture(256, 0, false), length, |_| {
                panic!("invalid size must precede fill")
            })
            .expect_err("invalid size");
        assert!(
            matches!(error.downcast_ref::<EncodeError>(), Some(EncodeError::InvalidPacketLength(actual)) if *actual == length)
        );
        assert_eq!(encoder.output, [] as [u8; 0]);
    }
    for failure in 0..4 {
        compare(
            &mut encoder,
            fixture(257, 0, false),
            &packet(257, 12),
            b"test",
        );
        let mut event = fixture(256, 0, false);
        match failure {
            0 => event.event_id = 259,
            1 => {
                // Windows SystemTime stores 100 ns FILETIME intervals; a
                // 1 ns subtraction truncates to zero and leaves the epoch.
                event.timestamp = SystemTime::UNIX_EPOCH - Duration::from_nanos(100);
                assert!(event.timestamp < SystemTime::UNIX_EPOCH);
            }
            2 => event.local_address = Some("::1".parse().expect("IPv6")),
            _ => {
                event.peer_address = Some("::1".parse().expect("IPv6"));
            }
        }
        let error = encoder
            .encode_with(event, 12, |_| panic!("invalid metadata must precede fill"))
            .expect_err("invalid metadata");
        assert!(matches!(
            (failure, error.downcast_ref::<EncodeError>()),
            (0, Some(EncodeError::UnsupportedEvent(259)))
                | (1, Some(EncodeError::InvalidTimestamp))
                | (2 | 3, Some(EncodeError::MixedAddressFamilies))
        ));
        assert_eq!(encoder.output, [] as [u8; 0]);
        compare(
            &mut encoder,
            fixture(256, 2, true),
            &packet(256, 12),
            b"test",
        );
    }
    assert!(matches!(
        Encoder::new(&[0; 1_025]),
        Err(EncodeError::IdentityTooLong)
    ));
}

#[test]
fn partial_fill_error_preserves_source_chain_and_qr_errors_recover() {
    let mut encoder = Encoder::new(b"").expect("identity");
    for id in [256, 257] {
        compare(&mut encoder, fixture(id, 1, true), &packet(id, 128), b"");
        let mut calls = 0;
        let error = encoder
            .encode_with(fixture(id, 0, false), 128, |destination| {
                calls += 1;
                destination[..6].fill(0xdd);
                Err(anyhow::Error::new(std::io::Error::from_raw_os_error(42))
                    .context("synthetic packet extraction"))
            })
            .expect_err("partial extraction failed");
        assert_eq!(calls, 1);
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .expect("original source retained")
                .raw_os_error(),
            Some(42)
        );
        assert!(format!("{error:#}").contains("synthetic packet extraction"));
        assert_eq!(encoder.output, [] as [u8; 0]);
        let wrong_direction = packet(if id == 257 { 256 } else { 257 }, 12);
        let error = encoder
            .encode_with(fixture(id, 2, true), 12, |destination| {
                calls += 1;
                destination.copy_from_slice(&wrong_direction);
                Ok(())
            })
            .expect_err("wrong QR");
        assert_eq!(calls, 2);
        assert!(
            matches!(error.downcast_ref::<EncodeError>(), Some(EncodeError::IncorrectDirection(actual)) if *actual == id)
        );
        assert_eq!(encoder.output, [] as [u8; 0]);
        let mut event = fixture(id, 2, false);
        event.timestamp = SystemTime::UNIX_EPOCH;
        compare(&mut encoder, event, &packet(id, 12), b"");
    }
}

#[test]
fn warmed_direct_encoding_reuses_all_storage_without_heap_operations() {
    let mut encoder = Encoder::new(&[0x77; 1_024]).expect("identity");
    let query = packet(256, 65_535);
    let response = packet(257, 65_535);
    for id in [256, 257] {
        encoder
            .encode_with(fixture(id, 1, true), query.len(), |destination| {
                destination.copy_from_slice(if id == 256 { &query } else { &response });
                Ok(())
            })
            .expect("warmup");
    }
    let pointer = encoder.output.as_ptr();
    let capacity = encoder.output.capacity();
    let mut all_succeeded = true;
    crate::etw::assert_no_allocations(|| {
        for index in 0..30 {
            let id = [256, 257][index % 2];
            let bytes = if id == 257 { &response } else { &query };
            let length = [12, 128, 16_384, 65_535][index % 4];
            all_succeeded &= encoder
                .encode_with(
                    fixture(id, index % 5, index % 2 == 0),
                    length,
                    |destination| {
                        destination.copy_from_slice(&bytes[..length]);
                        Ok(())
                    },
                )
                .is_ok();
        }
    });
    assert!(all_succeeded);
    assert_eq!(encoder.output.as_ptr(), pointer);
    assert_eq!(encoder.output.capacity(), capacity);
}
