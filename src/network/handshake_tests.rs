//! Negative and fragmentation cases for the Frame Streams control handshake.

use super::*;
use proptest::prelude::*;
use std::net::TcpListener;

/// Builds a control frame: escape, length, control type, then fields.
fn control(kind: u32, fields: &[(u32, &[u8])]) -> Vec<u8> {
    let mut body = kind.to_be_bytes().to_vec();
    for (field, value) in fields {
        body.extend_from_slice(&field.to_be_bytes());
        body.extend_from_slice(
            &u32::try_from(value.len())
                .expect("test field")
                .to_be_bytes(),
        );
        body.extend_from_slice(value);
    }
    let mut frame = vec![0, 0, 0, 0];
    frame.extend_from_slice(&u32::try_from(body.len()).expect("test body").to_be_bytes());
    frame.extend_from_slice(&body);
    frame
}

fn accept(fields: &[(u32, &[u8])]) -> Vec<u8> {
    control(CONTROL_ACCEPT, fields)
}

/// A reader that returns at most `chunk` bytes per call, like a slow network.
struct Chunked<'a> {
    bytes: &'a [u8],
    chunk: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = self.chunk.min(out.len()).min(self.bytes.len());
        out[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

fn check(bytes: &[u8], chunk: usize) -> io::Result<()> {
    read_control(&mut Chunked { bytes, chunk }, CONTROL_ACCEPT, true)
}

#[test]
fn wrong_or_missing_content_type_is_invalid_data() {
    for fields in [
        &[][..],
        &[(CONTENT_TYPE_FIELD, &b"protobuf:other.Type"[..])][..],
        &[(CONTENT_TYPE_FIELD, &b""[..])][..],
        &[(CONTENT_TYPE_FIELD, &b"PROTOBUF:DNSTAP.DNSTAP"[..])][..],
        // A prefix or suffix of the right string is not the right string.
        &[(CONTENT_TYPE_FIELD, &CONTENT_TYPE[..CONTENT_TYPE.len() - 1])][..],
        // The right bytes under a different field number do not count.
        &[(7, CONTENT_TYPE)][..],
    ] {
        let error = check(&accept(fields), 1024).expect_err("must not be accepted");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{fields:?}");
    }
}

#[test]
fn duplicate_and_multiple_content_types_accept_if_any_matches() {
    let other: &[u8] = b"protobuf:other.Type";
    // The receiver may list several types; the handshake succeeds when the
    // sender's own type is among them, in any position or repetition.
    for fields in [
        vec![
            (CONTENT_TYPE_FIELD, CONTENT_TYPE),
            (CONTENT_TYPE_FIELD, CONTENT_TYPE),
        ],
        vec![
            (CONTENT_TYPE_FIELD, other),
            (CONTENT_TYPE_FIELD, CONTENT_TYPE),
        ],
        vec![
            (CONTENT_TYPE_FIELD, CONTENT_TYPE),
            (CONTENT_TYPE_FIELD, other),
        ],
        vec![(9, other), (CONTENT_TYPE_FIELD, CONTENT_TYPE), (9, other)],
    ] {
        check(&accept(&fields), 1024).expect("one matching content type is enough");
    }
    for fields in [
        vec![(CONTENT_TYPE_FIELD, other), (CONTENT_TYPE_FIELD, other)],
        vec![(CONTENT_TYPE_FIELD, &b""[..]), (CONTENT_TYPE_FIELD, other)],
    ] {
        assert!(check(&accept(&fields), 1024).is_err());
    }
}

#[test]
fn wrong_control_type_is_rejected_even_with_the_right_content_type() {
    for kind in [0_u32, 2, 3, CONTROL_READY, CONTROL_FINISH, 6, u32::MAX] {
        let frame = control(kind, &[(CONTENT_TYPE_FIELD, CONTENT_TYPE)]);
        let error = check(&frame, 1024).expect_err("unexpected type");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "kind {kind}");
    }
    // FINISH carries no fields, so a field there is a protocol violation.
    let frame = control(CONTROL_FINISH, &[(CONTENT_TYPE_FIELD, CONTENT_TYPE)]);
    assert!(read_control(&mut frame.as_slice(), CONTROL_FINISH, false).is_err());
    let frame = control(CONTROL_FINISH, &[]);
    read_control(&mut frame.as_slice(), CONTROL_FINISH, false).expect("bare FINISH");
}

#[test]
fn oversized_control_frames_are_rejected_without_reading_the_body() {
    // The body is absent. A length error must come from the header alone, not
    // from an end-of-file while trying to read 513 or more bytes.
    for length in [0_u32, 1, 3, 513, 514, 65_536, 1 << 24, u32::MAX] {
        let mut header = vec![0, 0, 0, 0];
        header.extend_from_slice(&length.to_be_bytes());
        let error = check(&header, 1024).expect_err("length outside 4..=512");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "length {length}");
    }
    // A data frame (non-zero escape) is not a control frame.
    let error = check(&[0, 0, 0, 7, 0, 0, 0, 4, 0, 0, 0, 1], 1024).expect_err("data frame");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn control_frames_of_exactly_the_maximum_length_are_accepted() {
    // 512 = 4 (type) + 8 + 22 (content type) + 8 + padding in an unknown field.
    let padding = vec![0xa5; 512 - 4 - (8 + CONTENT_TYPE.len()) - 8];
    let frame = accept(&[(CONTENT_TYPE_FIELD, CONTENT_TYPE), (99, &padding)]);
    assert_eq!(frame.len(), 8 + 512);
    check(&frame, 1024).expect("512-byte payload is the largest allowed");
    let mut longer = accept(&[(CONTENT_TYPE_FIELD, CONTENT_TYPE), (99, &padding)]);
    longer[7] += 1;
    longer.push(0);
    assert!(check(&longer, 1024).is_err());
}

#[test]
fn content_type_field_longer_than_256_bytes_is_rejected_even_if_it_fits() {
    let long = vec![b'x'; 257];
    assert!(check(&accept(&[(CONTENT_TYPE_FIELD, &long)]), 1024).is_err());
    // A field whose declared length overruns the frame is malformed.
    let mut frame = accept(&[(CONTENT_TYPE_FIELD, CONTENT_TYPE)]);
    let length_at = frame.len() - CONTENT_TYPE.len() - 4;
    frame[length_at..length_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(check(&frame, 1024).is_err());
}

#[test]
fn byte_at_a_time_delivery_matches_contiguous_delivery() {
    let frame = accept(&[(CONTENT_TYPE_FIELD, CONTENT_TYPE)]);
    check(&frame, 1).expect("fragmented valid ACCEPT");
    for end in 0..frame.len() {
        assert!(check(&frame[..end], 1).is_err(), "prefix {end}");
        assert!(check(&frame[..end], 1024).is_err(), "prefix {end}");
    }
    let phase = |bytes: &[u8]| {
        let mut storage = [0; MAX_CONTROL_LENGTH];
        read_control_frame(&mut Chunked { bytes, chunk: 1 }, &mut storage)
            .map(<[u8]>::len)
            .map_err(|(phase, _)| phase)
    };
    assert_eq!(phase(&frame[..3]), Err(ControlPhase::HeaderRead));
    assert_eq!(phase(&frame[..12]), Err(ControlPhase::BodyRead));
    assert_eq!(phase(&frame), Ok(frame.len() - 8));
}

proptest! {
    #[test]
    fn chunking_never_changes_the_verdict(
        bytes in prop::collection::vec(any::<u8>(), 0..600),
        chunk in 1_usize..64,
        valid in any::<bool>(),
    ) {
        // Either arbitrary bytes or a valid frame with arbitrary extra fields.
        let input = if valid {
            let extra: &[u8] = &bytes[..bytes.len().min(100)];
            accept(&[(CONTENT_TYPE_FIELD, CONTENT_TYPE), (42, extra)])
        } else {
            bytes
        };
        let whole = check(&input, usize::MAX).map_err(|error| error.kind());
        let split = check(&input, chunk).map_err(|error| error.kind());
        prop_assert_eq!(whole, split);
    }
}

/// Runs the receiver side of the exchange with `script`, which controls what
/// the sender sees after READY.
fn serve(
    script: impl FnOnce(&mut TcpStream) + Send + 'static,
) -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("sender connects");
        let mut ready = [0; 8 + 12 + 22];
        stream.read_exact(&mut ready).expect("READY");
        script(&mut stream);
    });
    (address, handle)
}

#[test]
fn wrong_content_type_over_tcp_is_a_fatal_protocol_error() {
    let (address, receiver) = serve(|stream| {
        stream
            .write_all(&accept(&[(CONTENT_TYPE_FIELD, b"protobuf:other.Type")]))
            .expect("ACCEPT");
        // The sender must hang up without sending START or data.
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap_or(0), 0);
    });
    let error = Connection::connect(address)
        .err()
        .expect("handshake must fail");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    receiver.join().expect("receiver assertions pass");
    let (address, receiver) = serve(|stream| {
        stream
            .write_all(&accept(&[(CONTENT_TYPE_FIELD, b"protobuf:other.Type")]))
            .expect("ACCEPT");
    });
    let stats = Arc::new(SinkStats::default());
    let error = NetworkSink::start(address, stats)
        .err()
        .expect("a rejected content type is a configuration error, not an outage");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    receiver.join().expect("receiver ends");
}

#[test]
fn accept_trickled_one_byte_at_a_time_still_completes_the_handshake() {
    let (address, receiver) = serve(|stream| {
        stream.set_nodelay(true).expect("nodelay");
        for byte in accept(&[(CONTENT_TYPE_FIELD, CONTENT_TYPE)]) {
            stream.write_all(&[byte]).expect("one byte");
            thread::sleep(Duration::from_millis(2));
        }
        let mut start = [0; 8 + 12 + 22];
        stream.read_exact(&mut start).expect("START follows ACCEPT");
        assert_eq!(&start[8..12], &2_u32.to_be_bytes());
    });
    let connection = Connection::connect(address).expect("fragmented ACCEPT is valid");
    drop(connection);
    receiver.join().expect("receiver assertions pass");
}

#[test]
fn receiver_that_answers_ready_with_garbage_or_eof_is_not_a_protocol_error_loop() {
    // EOF right after READY is an outage (retryable), not InvalidData.
    let (address, receiver) = serve(|_stream| {});
    let error = Connection::connect(address).err().expect("no ACCEPT");
    assert_ne!(error.kind(), io::ErrorKind::InvalidData);
    receiver.join().expect("receiver ends");
    // Random bytes that look like neither a control frame nor ACCEPT are invalid.
    let (address, receiver) = serve(|stream| {
        stream.write_all(&[0xde; 64]).expect("garbage");
        let mut sink = [0; 16];
        let _ = stream.read(&mut sink);
    });
    let error = Connection::connect(address)
        .err()
        .expect("garbage rejected");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    receiver.join().expect("receiver ends");
}
