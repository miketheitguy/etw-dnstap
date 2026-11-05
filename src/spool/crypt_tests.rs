use std::time::Instant;

use proptest::prelude::*;

use super::*;

const KEY_BYTES: [u8; KEY_LEN] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
const KEY_ID: [u8; KEY_ID_LEN] = [0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8];
const PREFIX: [u8; NONCE_PREFIX_LEN] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];
const SEQUENCE: u64 = 0x0102_0304_0506_0708;

/// The test key, built once: each key takes a locked page, and the process can
/// lock only a few dozen.
fn key() -> Arc<SegmentKey> {
    static KEY: std::sync::OnceLock<Arc<SegmentKey>> = std::sync::OnceLock::new();
    Arc::clone(KEY.get_or_init(|| {
        Arc::new(SegmentKey::new(&KEY_BYTES, KEY_ID).expect("a 16-byte key is accepted"))
    }))
}

fn key_with(bytes: [u8; KEY_LEN], id: [u8; KEY_ID_LEN]) -> Arc<SegmentKey> {
    Arc::new(SegmentKey::new(&bytes, id).expect("a 16-byte key is accepted"))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|at| u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).expect("hex digits"))
        .collect()
}

/// A whole segment: file header, one data record per chunk, and optionally the end record.
fn build_with(
    key: &Arc<SegmentKey>,
    sequence: u64,
    prefix: [u8; NONCE_PREFIX_LEN],
    chunks: &[Vec<u8>],
    end: bool,
) -> Vec<u8> {
    let mut sealer = RecordSealer::new(key.clone(), sequence, prefix);
    let mut bytes = sealer.file_header().to_vec();
    for chunk in chunks {
        sealer
            .seal_data(chunk, &mut bytes)
            .expect("data record seals");
    }
    if end {
        sealer.seal_end(&mut bytes).expect("end record seals");
    }
    bytes
}

fn build(chunks: &[Vec<u8>], end: bool) -> Vec<u8> {
    build_with(&key(), SEQUENCE, PREFIX, chunks, end)
}

/// Why a full read stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stop {
    End,
    Incomplete,
    Corrupt(CorruptKind),
    Header(HeaderError),
}

/// What a full read returned: the data records (plaintext, logical start) and why it stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    records: Vec<(Vec<u8>, u64)>,
    stop: Stop,
}

/// Reads a segment from its bytes, as the spool reader will: header, then records one at a
/// time, decrypting in place in a copy of the bytes.
fn read_all(key: &Arc<SegmentKey>, bytes: &[u8]) -> Read {
    let mut opener = match RecordOpener::new(key.clone(), bytes) {
        Ok(opener) => opener,
        Err(error) => {
            return Read {
                records: Vec::new(),
                stop: Stop::Header(error),
            };
        }
    };
    let mut buf = bytes[FILE_HEADER_LEN..].to_vec();
    let before = buf.clone();
    let mut records = Vec::new();
    let mut at = 0;
    let stop = loop {
        match opener.open_next(&mut buf[at..]) {
            Next::Complete(record) => {
                assert_eq!(record.consumed, record.plaintext.end + TAG_LEN);
                if record.kind == RecordKind::End {
                    assert!(record.plaintext.is_empty());
                    assert_eq!(record.logical_start, opener.logical_offset());
                    break Stop::End;
                }
                let plain = buf[at + record.plaintext.start..at + record.plaintext.end].to_vec();
                records.push((plain, record.logical_start));
                at += record.consumed;
            }
            Next::Incomplete { needed } => {
                assert!(needed > buf.len() - at);
                assert_eq!(
                    buf[at..],
                    before[at..],
                    "an incomplete record is not decrypted"
                );
                break Stop::Incomplete;
            }
            Next::Corrupt(kind) => break Stop::Corrupt(kind),
        }
    };
    Read { records, stop }
}

/// The records `build` would read back as.
fn expected(chunks: &[Vec<u8>]) -> Vec<(Vec<u8>, u64)> {
    let mut offset = 0;
    chunks
        .iter()
        .map(|chunk| {
            let item = (chunk.clone(), offset);
            offset += chunk.len() as u64;
            item
        })
        .collect()
}

fn sample_chunks() -> Vec<Vec<u8>> {
    vec![
        (0..=255).collect(),
        vec![0xaa; 1],
        vec![0x55; 700],
        b"\x00\x00\x00\x05hello\x00\x00\x00\x03abc".to_vec(),
    ]
}

// ---- Format --------------------------------------------------------------------------

/// Pinned bytes of a small segment: change the format and this fails. The expected value
/// was checked against the independent construction in `known_answer_with_ring_directly`.
const PINNED_SEGMENT_HEX: &str = concat!(
    "4544534501000000a1a2a3a4a5a6a7a8", // file header: magic, version, flags, key id
    "10111213141516170807060504030201", // nonce prefix, sequence
    "090000000000000001000000",         // record 0 header: 9 bytes, index 0, data
    "183b98f2b062a7849c",               // record 0 ciphertext
    "50ee4530505c60df6db1e70170cde2e1", // record 0 tag
    "060000000100000001000000",         // record 1 header: 6 bytes, index 1, data
    "14863f74b152",                     // record 1 ciphertext
    "4bee4b3bb5ce6c7d842ef063e7b057e1", // record 1 tag
    "000000000200000002000000",         // end record header: 0 bytes, index 2, end
    "1bb26bc28f3f44a4c2d0aaa2016df8fe", // end record tag
);

fn pinned_chunks() -> Vec<Vec<u8>> {
    vec![
        b"\x00\x00\x00\x05hello".to_vec(),
        b"\x00\x00\x00\x02hi".to_vec(),
    ]
}

#[test]
fn known_answer_with_ring_directly() {
    use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

    let plaintext = b"\x00\x00\x00\x05hello".to_vec();
    let bytes = build(std::slice::from_ref(&plaintext), true);

    // The documented construction, written out byte by byte.
    let mut file_header = Vec::new();
    file_header.extend_from_slice(b"EDSE");
    file_header.extend_from_slice(&[1, 0, 0, 0]);
    file_header.extend_from_slice(&KEY_ID);
    file_header.extend_from_slice(&PREFIX);
    file_header.extend_from_slice(&[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(file_header.len(), 32);

    let raw = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &KEY_BYTES).expect("key"));
    let mut want = file_header.clone();
    for (index, kind, body) in [(0_u8, 1_u8, plaintext.clone()), (1, 2, Vec::new())] {
        let mut record_header = Vec::new();
        record_header.extend_from_slice(&u32::try_from(body.len()).expect("small").to_le_bytes());
        record_header.extend_from_slice(&[index, 0, 0, 0]);
        record_header.extend_from_slice(&[kind, 0, 0, 0]);
        let mut nonce = PREFIX.to_vec();
        nonce.extend_from_slice(&[0, 0, 0, index]);
        let mut aad = file_header.clone();
        aad.extend_from_slice(&record_header);
        let mut buffer = body;
        let tag = raw
            .seal_in_place_separate_tag(
                Nonce::try_assume_unique_for_key(&nonce).expect("12 bytes"),
                Aad::from(aad),
                &mut buffer,
            )
            .expect("seal");
        want.extend_from_slice(&record_header);
        want.extend_from_slice(&buffer);
        want.extend_from_slice(tag.as_ref());
    }
    assert_eq!(hex(&bytes), hex(&want));
    assert_eq!(bytes.len(), 32 + 12 + plaintext.len() + 16 + 12 + 16);
}

#[test]
fn pinned_segment_bytes() {
    let bytes = build(&pinned_chunks(), true);
    assert_eq!(hex(&bytes), PINNED_SEGMENT_HEX);
    // And the pinned bytes read back.
    let pinned = unhex(PINNED_SEGMENT_HEX);
    let read = read_all(&key(), &pinned);
    assert_eq!(read.stop, Stop::End);
    assert_eq!(read.records, expected(&pinned_chunks()));
}

#[test]
fn file_header_fields_are_where_the_plan_puts_them() {
    let header = FileHeader {
        key_id: KEY_ID,
        nonce_prefix: PREFIX,
        sequence: SEQUENCE,
    };
    let bytes = header.encode();
    assert_eq!(&bytes[0..4], b"EDSE");
    assert_eq!(bytes[4..6], 1_u16.to_le_bytes());
    assert_eq!(bytes[6..8], 0_u16.to_le_bytes());
    assert_eq!(bytes[8..16], KEY_ID);
    assert_eq!(bytes[16..24], PREFIX);
    assert_eq!(bytes[24..32], SEQUENCE.to_le_bytes());
    assert_eq!(FileHeader::decode(&bytes), Ok(header));
}

#[test]
fn record_overhead_is_28_bytes() {
    let mut out = Vec::new();
    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    sealer.seal_data(&[7; 100], &mut out).expect("seals");
    assert_eq!(out.len(), 100 + 28);
    assert_eq!(RECORD_OVERHEAD, 28);
    assert_eq!(out[0..4], 100_u32.to_le_bytes());
    assert_eq!(out[4..8], 0_u32.to_le_bytes());
    assert_eq!(out[8..12], [1, 0, 0, 0]);
    sealer.seal_end(&mut out).expect("seals");
    assert_eq!(out.len(), 100 + 28 + 28);
    assert_eq!(out[128..132], 0_u32.to_le_bytes());
    assert_eq!(out[132..136], 1_u32.to_le_bytes());
    assert_eq!(out[136..140], [2, 0, 0, 0]);
}

// ---- Round trip ----------------------------------------------------------------------

#[test]
fn round_trip_with_end_record_and_logical_offsets() {
    let chunks = sample_chunks();
    let bytes = build(&chunks, true);
    let read = read_all(&key(), &bytes);
    assert_eq!(read.stop, Stop::End);
    assert_eq!(read.records, expected(&chunks));
    let total: usize = chunks.iter().map(Vec::len).sum();
    assert_eq!(
        bytes.len(),
        FILE_HEADER_LEN + total + (chunks.len() + 1) * RECORD_OVERHEAD
    );
}

#[test]
fn round_trip_without_end_record_stops_incomplete() {
    let chunks = sample_chunks();
    let read = read_all(&key(), &build(&chunks, false));
    assert_eq!(read.stop, Stop::Incomplete);
    assert_eq!(read.records, expected(&chunks));
}

#[test]
fn many_records_round_trip() {
    let chunks: Vec<Vec<u8>> = (0..500_usize)
        .map(|n| vec![u8::try_from(n % 251).expect("small"); 1 + n % 97])
        .collect();
    let read = read_all(&key(), &build(&chunks, true));
    assert_eq!(read.stop, Stop::End);
    assert_eq!(read.records, expected(&chunks));
}

#[test]
fn the_largest_record_round_trips_and_one_more_byte_is_refused() {
    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    let mut out = Vec::new();
    sealer
        .seal_data(&vec![9; MAX_PLAINTEXT_LEN + 1], &mut out)
        .expect_err("over 1 MiB is refused");
    assert_eq!(out.len(), 0);
    assert_eq!(sealer.next_index(), 0);
    assert_eq!(sealer.logical_offset(), 0);

    let big = vec![0x42; MAX_PLAINTEXT_LEN];
    sealer.seal_data(&big, &mut out).expect("1 MiB is accepted");
    assert_eq!(sealer.logical_offset(), MAX_PLAINTEXT_LEN as u64);
    let mut bytes = sealer.file_header().to_vec();
    bytes.extend_from_slice(&out);
    let read = read_all(&key(), &bytes);
    assert_eq!(read.records, vec![(big, 0)]);
}

#[test]
fn opener_rejects_a_length_over_one_mib_as_corrupt() {
    let mut bytes = build(&[vec![1; 10]], true);
    let at = FILE_HEADER_LEN;
    let too_big = u32::try_from(MAX_PLAINTEXT_LEN + 1).expect("fits");
    bytes[at..at + 4].copy_from_slice(&too_big.to_le_bytes());
    let read = read_all(&key(), &bytes);
    assert_eq!(read.stop, Stop::Corrupt(CorruptKind::BadLength));
}

#[test]
fn sealer_refuses_an_empty_data_record_and_anything_after_the_end() {
    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    let mut out = Vec::new();
    assert_eq!(
        sealer.seal_data(&[], &mut out),
        Err(CryptError::BadPlaintextLength)
    );
    sealer.seal_end(&mut out).expect("seals");
    let len = out.len();
    assert_eq!(sealer.seal_end(&mut out), Err(CryptError::AlreadyEnded));
    assert_eq!(
        sealer.seal_data(&[1], &mut out),
        Err(CryptError::AlreadyEnded)
    );
    assert_eq!(out.len(), len);
}

#[test]
fn sealer_refuses_a_record_past_u32_max() {
    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    sealer.next_index = u64::from(u32::MAX);
    let mut out = Vec::new();
    sealer
        .seal_data(&[1; 4], &mut out)
        .expect("the last index is usable");
    assert_eq!(out[4..8], u32::MAX.to_le_bytes());
    let len = out.len();
    assert_eq!(
        sealer.seal_data(&[1; 4], &mut out),
        Err(CryptError::TooManyRecords)
    );
    assert_eq!(sealer.seal_end(&mut out), Err(CryptError::TooManyRecords));
    assert_eq!(out.len(), len);
    assert_eq!(sealer.logical_offset(), 4);
}

#[test]
fn sealing_into_a_reserved_buffer_does_not_allocate() {
    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    let mut out = Vec::with_capacity(64 * 1024 + RECORD_OVERHEAD);
    let pointer = out.as_ptr();
    let capacity = out.capacity();
    for round in 0..50 {
        out.clear();
        sealer
            .seal_data(&vec![round; 64 * 1024], &mut out)
            .expect("seals");
        assert_eq!(out.as_ptr(), pointer);
        assert_eq!(out.capacity(), capacity);
    }
}

#[test]
fn sealer_appends_after_existing_bytes() {
    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    let mut out = sealer.file_header().to_vec();
    sealer.seal_data(&[1, 2, 3], &mut out).expect("seals");
    sealer.seal_data(&[4, 5], &mut out).expect("seals");
    sealer.seal_end(&mut out).expect("seals");
    let read = read_all(&key(), &out);
    assert_eq!(read.stop, Stop::End);
    assert_eq!(read.records, vec![(vec![1, 2, 3], 0), (vec![4, 5], 3)]);
}

#[test]
fn records_decrypt_in_place_in_the_middle_of_a_larger_buffer() {
    let chunks = sample_chunks();
    let bytes = build(&chunks, true);
    let mut opener = RecordOpener::new(key(), &bytes).expect("header");
    assert_eq!(opener.sequence(), SEQUENCE);
    // Pad the front so the record is not at the start of the allocation.
    let mut buf = vec![0xee; 5];
    buf.extend_from_slice(&bytes[FILE_HEADER_LEN..]);
    let Next::Complete(record) = opener.open_next(&mut buf[5..]) else {
        panic!("first record opens");
    };
    assert_eq!(record.kind, RecordKind::Data);
    assert_eq!(
        buf[5 + record.plaintext.start..5 + record.plaintext.end],
        chunks[0][..]
    );
    assert_eq!(record.logical_start, 0);
}

// ---- File header ---------------------------------------------------------------------

#[test]
fn header_not_this_format_is_distinct_from_other_key() {
    let good = build(&[vec![1]], true);

    for at in [0, 3, 4, 5, 6, 7] {
        let mut bad = good.clone();
        bad[at] ^= 0x01;
        assert_eq!(
            RecordOpener::new(key(), &bad).expect_err("rejected"),
            HeaderError::NotThisFormat,
            "byte {at}"
        );
    }

    let other = key_with(KEY_BYTES, [9; KEY_ID_LEN]);
    assert_eq!(
        RecordOpener::new(other, &good).expect_err("rejected"),
        HeaderError::OtherKey { found: KEY_ID }
    );

    // An old plaintext segment: Frame Streams START (escape, length, control frame).
    let mut plaintext = vec![0, 0, 0, 0, 0, 0, 0, 0x22, 0, 0, 0, 2, 0, 0, 0, 0x16];
    plaintext.extend_from_slice(b"protobuf:dnstap.Dnstap");
    assert_eq!(
        RecordOpener::new(key(), &plaintext).expect_err("rejected"),
        HeaderError::NotThisFormat
    );
}

#[test]
fn a_short_header_is_too_short_unless_it_already_cannot_match() {
    let good = build(&[vec![1]], true);
    for len in 0..FILE_HEADER_LEN {
        assert_eq!(
            RecordOpener::new(key(), &good[..len]).expect_err("rejected"),
            HeaderError::TooShort,
            "length {len}"
        );
    }
    // The magic is already wrong in the first bytes: not this format, however short.
    assert_eq!(
        RecordOpener::new(key(), b"MZ").expect_err("rejected"),
        HeaderError::NotThisFormat
    );
    assert!(RecordOpener::new(key(), &good[..FILE_HEADER_LEN]).is_ok());
}

// ---- Tamper --------------------------------------------------------------------------

/// Every field a flip can reach: each flip is rejected at the header or stops the read before
/// the end, and no record that decrypts differs from the original.
#[test]
fn flipping_any_byte_never_decrypts_to_anything_else() {
    let chunks = sample_chunks();
    let good = build(&chunks, true);
    let original = expected(&chunks);
    for at in 0..good.len() {
        for mask in [0x01_u8, 0x80, 0xff] {
            let mut bad = good.clone();
            bad[at] ^= mask;
            let read = read_all(&key(), &bad);
            assert_ne!(
                read.stop,
                Stop::End,
                "byte {at} mask {mask:#x} was accepted"
            );
            assert!(
                read.records.len() <= original.len()
                    && read.records[..] == original[..read.records.len()],
                "byte {at} mask {mask:#x} changed a decrypted record"
            );
        }
    }
}

#[test]
fn flips_are_classified_by_field() {
    let chunks = vec![vec![1; 20], vec![2; 30], vec![3; 40]];
    let good = build(&chunks, true);
    let second = FILE_HEADER_LEN + RECORD_OVERHEAD + 20;
    let flip = |at: usize| {
        let mut bad = good.clone();
        bad[at] ^= 0x01;
        read_all(&key(), &bad)
    };

    // File header: key id is "other key", the rest of the fixed prefix "not this format",
    // nonce prefix and sequence change the AAD or nonce so the first tag fails.
    assert_eq!(flip(0).stop, Stop::Header(HeaderError::NotThisFormat));
    assert_eq!(flip(5).stop, Stop::Header(HeaderError::NotThisFormat));
    assert_eq!(
        flip(9).stop,
        Stop::Header(HeaderError::OtherKey {
            found: {
                let mut id = KEY_ID;
                id[1] ^= 0x01;
                id
            }
        })
    );
    assert_eq!(flip(17).stop, Stop::Corrupt(CorruptKind::BadTag));
    assert_eq!(flip(25).stop, Stop::Corrupt(CorruptKind::BadTag));
    assert_eq!(flip(25).records.len(), 0);

    // Record header of the second record.
    assert_eq!(
        flip(second + 4).stop,
        Stop::Corrupt(CorruptKind::OutOfOrder)
    );
    assert_eq!(flip(second + 8).stop, Stop::Corrupt(CorruptKind::BadKind));
    assert_eq!(
        flip(second + 9).stop,
        Stop::Corrupt(CorruptKind::ReservedNotZero)
    );
    assert_eq!(
        flip(second + 11).stop,
        Stop::Corrupt(CorruptKind::ReservedNotZero)
    );
    // A length that now claims 1 byte less: the tag fails. One byte more: the third record's
    // first byte joins the body and the tag fails.
    assert_eq!(flip(second).stop, Stop::Corrupt(CorruptKind::BadTag));
    // Ciphertext and tag of the second record: the first record still decrypted.
    for at in second + RECORD_HEADER_LEN..second + RECORD_OVERHEAD + 30 {
        let read = flip(at);
        assert_eq!(read.stop, Stop::Corrupt(CorruptKind::BadTag), "byte {at}");
        assert_eq!(read.records, vec![(vec![1; 20], 0)]);
    }
}

#[test]
fn a_flipped_length_bit_that_runs_past_the_file_reads_as_cut_off_never_as_data() {
    let good = build(&[vec![1; 20]], true);
    let mut bad = good;
    bad[FILE_HEADER_LEN + 2] ^= 0x01; // length + 65536
    let read = read_all(&key(), &bad);
    assert_eq!(read.stop, Stop::Incomplete);
    assert_eq!(read.records.len(), 0);
}

#[test]
fn swapped_records_are_corrupt() {
    // Whole records swapped: the index is out of order.
    let a = vec![1; 25];
    let b = vec![2; 25];
    let c = vec![3; 25];
    let good = build(&[a.clone(), b.clone(), c.clone()], true);
    let one = FILE_HEADER_LEN + RECORD_OVERHEAD + 25;
    let two = one + RECORD_OVERHEAD + 25;
    let mut swapped = good[..one].to_vec();
    swapped.extend_from_slice(&good[two..two + RECORD_OVERHEAD + 25]);
    swapped.extend_from_slice(&good[one..two]);
    swapped.extend_from_slice(&good[two + RECORD_OVERHEAD + 25..]);
    let read = read_all(&key(), &swapped);
    assert_eq!(read.stop, Stop::Corrupt(CorruptKind::OutOfOrder));
    assert_eq!(read.records, vec![(a, 0)]);

    // Only the bodies swapped, headers kept in place: the tags fail.
    let mut bodies = good.clone();
    let body = RECORD_HEADER_LEN..RECORD_OVERHEAD + 25;
    let first_body = bodies[one + body.start..one + body.end].to_vec();
    let second_body = bodies[two + body.start..two + body.end].to_vec();
    bodies[one + body.start..one + body.end].copy_from_slice(&second_body);
    bodies[two + body.start..two + body.end].copy_from_slice(&first_body);
    let read_bodies = read_all(&key(), &bodies);
    assert_eq!(read_bodies.stop, Stop::Corrupt(CorruptKind::BadTag));
    assert_eq!(read_bodies.records.len(), 1);

    // A dropped record in the middle: out of order.
    let mut dropped = good[..one].to_vec();
    dropped.extend_from_slice(&good[two..]);
    assert_eq!(
        read_all(&key(), &dropped).stop,
        Stop::Corrupt(CorruptKind::OutOfOrder)
    );

    // A repeated record: out of order.
    let mut repeated = good[..two].to_vec();
    repeated.extend_from_slice(&good[one..two]);
    assert_eq!(
        read_all(&key(), &repeated).stop,
        Stop::Corrupt(CorruptKind::OutOfOrder)
    );
}

#[test]
fn a_record_moved_to_another_segment_key_or_prefix_is_corrupt() {
    let chunks = vec![vec![7; 40]];
    let source = build_with(&key(), 5, PREFIX, &chunks, false);
    let record = &source[FILE_HEADER_LEN..];

    let graft = |header_from: &[u8]| {
        let mut bytes = header_from[..FILE_HEADER_LEN].to_vec();
        bytes.extend_from_slice(record);
        bytes
    };

    // Same key, same prefix, other segment sequence.
    let other_sequence = build_with(&key(), 6, PREFIX, &chunks, false);
    assert_eq!(
        read_all(&key(), &graft(&other_sequence)).stop,
        Stop::Corrupt(CorruptKind::BadTag)
    );

    // Same key bytes under another key id (the id is in the header, so the AAD changes).
    let other_id_key = key_with(KEY_BYTES, [0x55; KEY_ID_LEN]);
    let other_id = build_with(&other_id_key, 5, PREFIX, &chunks, false);
    assert_eq!(
        read_all(&other_id_key, &graft(&other_id)).stop,
        Stop::Corrupt(CorruptKind::BadTag)
    );

    // Other key bytes under the same id.
    let other_bytes_key = key_with([0x77; KEY_LEN], KEY_ID);
    let other_bytes = build_with(&other_bytes_key, 5, PREFIX, &chunks, false);
    assert_eq!(
        read_all(&other_bytes_key, &graft(&other_bytes)).stop,
        Stop::Corrupt(CorruptKind::BadTag)
    );

    // Other nonce prefix.
    let other_prefix = build_with(&key(), 5, [0xcc; NONCE_PREFIX_LEN], &chunks, false);
    assert_eq!(
        read_all(&key(), &graft(&other_prefix)).stop,
        Stop::Corrupt(CorruptKind::BadTag)
    );

    // The unchanged graft reads.
    assert_eq!(read_all(&key(), &graft(&source)).records.len(), 1);
}

#[test]
fn a_segment_read_with_the_wrong_key_bytes_is_corrupt_not_data() {
    let bytes = build(&sample_chunks(), true);
    let wrong = key_with([0x99; KEY_LEN], KEY_ID);
    let read = read_all(&wrong, &bytes);
    assert_eq!(read.stop, Stop::Corrupt(CorruptKind::BadTag));
    assert_eq!(read.records.len(), 0);
}

#[test]
fn nothing_follows_the_end_record() {
    let bytes = build(&[vec![1; 3]], true);
    let mut opener = RecordOpener::new(key(), &bytes).expect("header");
    let mut buf = bytes[FILE_HEADER_LEN..].to_vec();
    let Next::Complete(first) = opener.open_next(&mut buf) else {
        panic!("data record opens");
    };
    let Next::Complete(end) = opener.open_next(&mut buf[first.consumed..]) else {
        panic!("end record opens");
    };
    assert_eq!(end.kind, RecordKind::End);
    assert_eq!(
        opener.open_next(&mut buf[first.consumed + end.consumed..]),
        Next::Corrupt(CorruptKind::AfterEnd)
    );
}

#[test]
fn an_empty_data_record_or_a_full_end_record_is_corrupt() {
    // Forge both with the real key: valid tags, invalid shapes.
    use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let raw = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &KEY_BYTES).expect("key"));
    let sealer = RecordSealer::new(key(), SEQUENCE, PREFIX);
    let file_header = sealer.file_header();
    let forge = |kind: u8, body: &[u8]| {
        let mut header = Vec::new();
        header.extend_from_slice(&u32::try_from(body.len()).expect("small").to_le_bytes());
        header.extend_from_slice(&[0, 0, 0, 0, kind, 0, 0, 0]);
        let mut aad = file_header.to_vec();
        aad.extend_from_slice(&header);
        let mut nonce = PREFIX.to_vec();
        nonce.extend_from_slice(&[0, 0, 0, 0]);
        let mut buffer = body.to_vec();
        let tag = raw
            .seal_in_place_separate_tag(
                Nonce::try_assume_unique_for_key(&nonce).expect("12 bytes"),
                Aad::from(aad),
                &mut buffer,
            )
            .expect("seal");
        let mut bytes = file_header.to_vec();
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&buffer);
        bytes.extend_from_slice(tag.as_ref());
        bytes
    };
    assert_eq!(
        read_all(&key(), &forge(1, &[])).stop,
        Stop::Corrupt(CorruptKind::BadLength)
    );
    assert_eq!(
        read_all(&key(), &forge(2, b"x")).stop,
        Stop::Corrupt(CorruptKind::BadLength)
    );
    // The same forgery path produces the real shapes, so the forger is faithful.
    assert_eq!(read_all(&key(), &forge(1, b"x")).records.len(), 1);
    assert_eq!(read_all(&key(), &forge(2, &[])).stop, Stop::End);
}

// ---- Truncation ----------------------------------------------------------------------

#[test]
fn truncating_the_last_record_anywhere_is_incomplete_never_corrupt() {
    let chunks = sample_chunks();
    for with_end in [false, true] {
        let good = build(&chunks, with_end);
        let last_len = if with_end {
            RECORD_OVERHEAD
        } else {
            RECORD_OVERHEAD + chunks.last().expect("chunks").len()
        };
        let last_start = good.len() - last_len;
        let kept_chunks = if with_end {
            chunks.len()
        } else {
            chunks.len() - 1
        };
        for cut in last_start..good.len() {
            let read = read_all(&key(), &good[..cut]);
            assert_eq!(read.stop, Stop::Incomplete, "cut at {cut}");
            assert_eq!(
                read.records,
                expected(&chunks[..kept_chunks]),
                "cut at {cut}"
            );
        }
        // The whole file reads.
        assert_eq!(
            read_all(&key(), &good).stop,
            if with_end {
                Stop::End
            } else {
                Stop::Incomplete
            }
        );
    }
}

#[test]
fn truncating_anywhere_never_gives_corrupt_or_a_wrong_record() {
    let chunks = sample_chunks();
    let good = build(&chunks, true);
    let original = expected(&chunks);
    for cut in FILE_HEADER_LEN..=good.len() {
        let read = read_all(&key(), &good[..cut]);
        assert!(
            read.records[..] == original[..read.records.len()],
            "cut at {cut}"
        );
        if cut < good.len() {
            assert_eq!(read.stop, Stop::Incomplete, "cut at {cut}");
        }
    }
}

#[test]
fn an_incomplete_record_completes_when_the_rest_arrives() {
    let chunks = vec![vec![5; 300]];
    let good = build(&chunks, true);
    let mut opener = RecordOpener::new(key(), &good).expect("header");
    let mut buf = Vec::new();
    let mut saw_incomplete = false;
    for byte in &good[FILE_HEADER_LEN..] {
        buf.push(*byte);
        let mut attempt = buf.clone();
        match opener.open_next(&mut attempt) {
            Next::Incomplete { needed } => {
                saw_incomplete = true;
                assert!(needed > buf.len());
            }
            Next::Complete(record) => {
                assert_eq!(record.kind, RecordKind::Data);
                assert_eq!(attempt[record.plaintext.clone()], chunks[0][..]);
                return;
            }
            Next::Corrupt(kind) => panic!("corrupt: {kind:?}"),
        }
    }
    assert!(saw_incomplete);
    panic!("the record never completed");
}

// ---- Resume walk ---------------------------------------------------------------------

/// Frame Streams-shaped chunk: each frame is a 4-byte big-endian length and a payload.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(payload.len())
        .expect("small")
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(payload);
    out
}

/// Resumes at `target` in `bytes` and returns the plaintext from `target` to the end.
fn resume(key: &Arc<SegmentKey>, bytes: &[u8], target: u64) -> Vec<u8> {
    let mut opener = RecordOpener::new(key.clone(), bytes).expect("header");
    let body = &bytes[FILE_HEADER_LEN..];
    let Skip::Found { consumed } = opener.skip_to(body, target) else {
        panic!("walk finds a record");
    };
    let mut buf = body[consumed..].to_vec();
    let mut at = 0;
    let mut out = Vec::new();
    let mut first = true;
    loop {
        let Next::Complete(record) = opener.open_next(&mut buf[at..]) else {
            panic!("record opens");
        };
        let plain = &buf[at + record.plaintext.start..at + record.plaintext.end];
        if first {
            first = false;
            assert!(
                record.logical_start <= target,
                "starts at or before the target"
            );
            assert!(
                target <= record.logical_start + plain.len() as u64,
                "holds the target, or is the end record at it"
            );
            let inside = usize::try_from(target - record.logical_start).expect("small");
            out.extend_from_slice(&plain[inside..]);
        } else {
            out.extend_from_slice(plain);
        }
        at += record.consumed;
        if record.kind == RecordKind::End {
            return out;
        }
    }
}

#[test]
fn resume_walk_finds_the_record_for_every_frame_boundary() {
    let records: Vec<Vec<Vec<u8>>> = vec![
        vec![vec![1; 5], vec![2; 10]],
        vec![vec![3; 1]],
        vec![vec![4; 50], vec![5; 0], vec![6; 7]],
    ];
    let chunks: Vec<Vec<u8>> = records
        .iter()
        .map(|frames| frames.iter().flat_map(|payload| frame(payload)).collect())
        .collect();
    let stream: Vec<u8> = chunks.concat();
    let bytes = build(&chunks, true);

    let mut boundaries = vec![0_usize];
    for payload in records.iter().flatten() {
        boundaries.push(boundaries.last().expect("first") + 4 + payload.len());
    }
    assert_eq!(*boundaries.last().expect("last"), stream.len());
    for target in boundaries {
        assert_eq!(
            resume(&key(), &bytes, target as u64),
            stream[target..],
            "resume at {target}"
        );
    }
}

#[test]
fn resume_walk_skips_without_decrypting() {
    // Damage the ciphertext of the skipped records: the walk does not look at it.
    let chunks = vec![vec![1; 50], vec![2; 50], vec![3; 50]];
    let mut bytes = build(&chunks, true);
    let damaged = FILE_HEADER_LEN..FILE_HEADER_LEN + 2 * (RECORD_OVERHEAD + 50);
    for (at, byte) in bytes.iter_mut().enumerate() {
        if damaged.contains(&at)
            && (at - FILE_HEADER_LEN) % (RECORD_OVERHEAD + 50) >= RECORD_HEADER_LEN
        {
            *byte ^= 0xff;
        }
    }
    let mut opener = RecordOpener::new(key(), &bytes).expect("header");
    let body = &bytes[FILE_HEADER_LEN..];
    assert_eq!(
        opener.skip_to(body, 100),
        Skip::Found {
            consumed: 2 * (RECORD_OVERHEAD + 50)
        }
    );
    assert_eq!(opener.logical_offset(), 100);
    let mut buf = body[2 * (RECORD_OVERHEAD + 50)..].to_vec();
    let Next::Complete(record) = opener.open_next(&mut buf) else {
        panic!("the third record is intact");
    };
    assert_eq!(record.logical_start, 100);
    assert_eq!(buf[record.plaintext], vec![3; 50][..]);
}

#[test]
fn resume_walk_target_cases() {
    let chunks = vec![vec![1; 10], vec![2; 10]];
    let bytes = build(&chunks, true);
    let body = &bytes[FILE_HEADER_LEN..];
    let one = RECORD_OVERHEAD + 10;

    let walk = |target: u64| {
        RecordOpener::new(key(), &bytes)
            .expect("header")
            .skip_to(body, target)
    };
    assert_eq!(walk(0), Skip::Found { consumed: 0 });
    assert_eq!(walk(9), Skip::Found { consumed: 0 });
    assert_eq!(walk(10), Skip::Found { consumed: one });
    assert_eq!(walk(19), Skip::Found { consumed: one });
    // At the end of the data, and past it: the end record.
    assert_eq!(walk(20), Skip::Found { consumed: 2 * one });
    assert_eq!(walk(10_000), Skip::Found { consumed: 2 * one });

    // No end record yet: the walk wants more bytes at the boundary.
    let live = build(&chunks, false);
    let mut opener = RecordOpener::new(key(), &live).expect("header");
    assert_eq!(
        opener.skip_to(&live[FILE_HEADER_LEN..], 20),
        Skip::Incomplete {
            consumed: 2 * one,
            needed: RECORD_HEADER_LEN
        }
    );
}

#[test]
fn resume_walk_continues_across_buffer_refills() {
    let chunks: Vec<Vec<u8>> = (0..6).map(|n| vec![n; 40]).collect();
    let bytes = build(&chunks, true);
    let body = &bytes[FILE_HEADER_LEN..];
    let target = 5 * 40 + 3;

    // Feed the body in uneven pieces, moving the buffer to `consumed` each time.
    let mut opener = RecordOpener::new(key(), &bytes).expect("header");
    let mut fed = 0;
    let mut start = 0;
    let found = loop {
        fed = (fed + 37).min(body.len());
        match opener.skip_to(&body[start..fed], target) {
            Skip::Found { consumed } => break start + consumed,
            Skip::Incomplete { consumed, needed } => {
                start += consumed;
                assert!(fed - start < needed);
                assert!(fed < body.len(), "the refill ran out of bytes");
            }
            Skip::Corrupt(kind) => panic!("corrupt: {kind:?}"),
        }
    };
    assert_eq!(found, 5 * (RECORD_OVERHEAD + 40));
    let mut buf = body[found..].to_vec();
    let Next::Complete(record) = opener.open_next(&mut buf) else {
        panic!("record opens");
    };
    assert_eq!(record.logical_start, 200);
    assert_eq!(buf[record.plaintext][3..], vec![5; 37][..]);
}

#[test]
fn resume_walk_reports_a_bad_header() {
    let mut bytes = build(&[vec![1; 10], vec![2; 10]], true);
    bytes[FILE_HEADER_LEN + RECORD_OVERHEAD + 10 + 8] = 9; // second record kind
    let mut opener = RecordOpener::new(key(), &bytes).expect("header");
    assert_eq!(
        opener.skip_to(&bytes[FILE_HEADER_LEN..], 15),
        Skip::Corrupt(CorruptKind::BadKind)
    );
}

#[test]
fn a_misled_walk_fails_the_tag_of_the_record_it_stops_at() {
    // Shorten the first record's length in its header while keeping the stream parseable by
    // forging nothing else: the walk lands on a header that does not exist.
    let bytes = build(&[vec![1; 10], vec![2; 10]], true);
    let mut bad = bytes.clone();
    bad[FILE_HEADER_LEN] = 5; // length 10 -> 5; the next "header" is mid-ciphertext
    let mut opener = RecordOpener::new(key(), &bad).expect("header");
    match opener.skip_to(&bad[FILE_HEADER_LEN..], 7) {
        // Target 7 is past the first record's claimed end (5), so the walk steps over it and
        // reads garbage as a header: corrupt or incomplete, never a decrypted record.
        Skip::Corrupt(_) | Skip::Incomplete { .. } => {}
        Skip::Found { consumed } => {
            let mut buf = bad[FILE_HEADER_LEN + consumed..].to_vec();
            assert!(!matches!(opener.open_next(&mut buf), Next::Complete(_)));
        }
    }
}

// ---- Property tests ------------------------------------------------------------------

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    }
}

fn chunk_strategy() -> impl Strategy<Value = Vec<Vec<u8>>> {
    prop::collection::vec(prop::collection::vec(any::<u8>(), 1..2500), 0..10)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn random_records_round_trip(
        key_bytes in any::<[u8; KEY_LEN]>(),
        key_id in any::<[u8; KEY_ID_LEN]>(),
        prefix in any::<[u8; NONCE_PREFIX_LEN]>(),
        sequence in any::<u64>(),
        chunks in chunk_strategy(),
    ) {
        let key = key_with(key_bytes, key_id);
        let bytes = build_with(&key, sequence, prefix, &chunks, true);
        let read = read_all(&key, &bytes);
        prop_assert_eq!(read.stop, Stop::End);
        prop_assert_eq!(read.records, expected(&chunks));
        let opener = RecordOpener::new(key, &bytes).expect("header");
        prop_assert_eq!(opener.sequence(), sequence);
    }

    #[test]
    fn a_single_bit_flip_never_decrypts(
        chunks in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..600), 1..6),
        position in any::<prop::sample::Index>(),
        bit in 0_u8..8,
    ) {
        let good = build(&chunks, true);
        let mut bad = good.clone();
        let at = position.index(bad.len());
        bad[at] ^= 1 << bit;
        let read = read_all(&key(), &bad);
        prop_assert_ne!(read.stop, Stop::End);
        let original = expected(&chunks);
        prop_assert!(read.records.len() <= original.len());
        prop_assert!(read.records[..] == original[..read.records.len()]);
    }

    #[test]
    fn any_truncation_is_incomplete(
        chunks in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..600), 1..6),
        position in any::<prop::sample::Index>(),
    ) {
        let good = build(&chunks, true);
        let cut = FILE_HEADER_LEN + position.index(good.len() - FILE_HEADER_LEN);
        let read = read_all(&key(), &good[..cut]);
        prop_assert_eq!(read.stop, Stop::Incomplete);
        let original = expected(&chunks);
        prop_assert!(read.records[..] == original[..read.records.len()]);
    }

    #[test]
    fn resume_at_a_random_frame_boundary_finds_the_right_offset(
        records in prop::collection::vec(
            prop::collection::vec(prop::collection::vec(any::<u8>(), 0..120), 1..6),
            1..8,
        ),
        pick in any::<prop::sample::Index>(),
    ) {
        let chunks: Vec<Vec<u8>> = records
            .iter()
            .map(|frames| frames.iter().flat_map(|payload| frame(payload)).collect())
            .collect();
        let stream: Vec<u8> = chunks.concat();
        let mut boundaries = vec![0_usize];
        for payload in records.iter().flatten() {
            boundaries.push(boundaries.last().expect("first") + 4 + payload.len());
        }
        let target = boundaries[pick.index(boundaries.len())];
        let bytes = build(&chunks, true);
        prop_assert_eq!(resume(&key(), &bytes, target as u64), stream[target..].to_vec());
    }
}

// ---- Benchmark -----------------------------------------------------------------------

/// Run by hand: `cargo test --release spool::crypt::crypt_tests::bench -- --ignored --nocapture`.
#[test]
#[ignore = "microbenchmark; run by hand in release mode"]
fn bench_seal_and_open_64_kib_records() {
    const RECORD: usize = 64 * 1024;
    const TOTAL: usize = 512 * 1024 * 1024;
    let rounds = TOTAL / RECORD;
    let plain: Vec<u8> = (0..=250).cycle().take(RECORD).collect();

    let mut sealer = RecordSealer::new(key(), 1, PREFIX);
    let mut out = Vec::with_capacity(RECORD + RECORD_OVERHEAD);
    let seal_started = Instant::now();
    for _ in 0..rounds {
        out.clear();
        sealer.seal_data(&plain, &mut out).expect("seals");
        std::hint::black_box(&out);
    }
    let seal_secs = seal_started.elapsed().as_secs_f64();

    // Open the same record again and again, each time with a fresh opener at index 0.
    let mut single = RecordSealer::new(key(), 1, PREFIX);
    let header = single.file_header();
    let mut framed = Vec::new();
    single.seal_data(&plain, &mut framed).expect("seals");
    let mut scratch = vec![0_u8; framed.len()];
    let open_started = Instant::now();
    for _ in 0..rounds {
        scratch.copy_from_slice(&framed);
        let mut opener = RecordOpener::new(key(), &header).expect("header");
        assert!(matches!(opener.open_next(&mut scratch), Next::Complete(_)));
        std::hint::black_box(&scratch);
    }
    let open_secs = open_started.elapsed().as_secs_f64();

    #[expect(clippy::cast_precision_loss, reason = "a rate for people to read")]
    let mib = TOTAL as f64 / (1024.0 * 1024.0);
    println!(
        "seal: {:.0} MB/s, open: {:.0} MB/s (64 KiB records, {mib:.0} MiB each way)",
        mib / seal_secs,
        mib / open_secs
    );
}
