//! Runs the fuzz entry point over hand-made inputs, so CI exercises it without
//! libFuzzer.

use proptest::prelude::*;

use super::*;
use crate::spool::test_support::{framed, plaintext_segment};

/// Mode bit 3: seal the body as records.
const SEALED: u8 = 8;
/// Mode bit 4: add the end record (sealed), or write no header (raw).
const END_OR_NO_HEADER: u8 = 16;
/// Mode bit 5: flip one byte after the header.
const FLIP: u8 = 32;

/// Mode byte and limit byte, then the segment bytes.
fn input(mode: u8, limit: u8, segment: &[u8]) -> Vec<u8> {
    let mut data = vec![mode, limit];
    data.extend_from_slice(segment);
    data
}

/// Records for the sealed mode: each a length byte (`n` for `n + 1` bytes),
/// then the bytes.
fn records(plaintexts: &[&[u8]]) -> Vec<u8> {
    let mut body = Vec::new();
    for plaintext in plaintexts {
        body.push(u8::try_from(plaintext.len() - 1).expect("short record"));
        body.extend_from_slice(plaintext);
    }
    body
}

#[test]
fn an_empty_input_is_an_empty_segment() {
    spool_segment(&[]);
    spool_segment(&[0]);
    spool_segment(&input(0, 0, &[]));
    spool_segment(&input(SEALED, 0, &[]));
    spool_segment(&input(SEALED | END_OR_NO_HEADER, 0, &[]));
}

#[test]
fn good_records_are_read_with_and_without_the_end_record() {
    let first = framed(&[vec![9; 5], vec![8; 100]]);
    let second = framed(&[vec![7; 1]]);
    let body = records(&[&first, &second]);
    for mode in [
        SEALED,
        SEALED | END_OR_NO_HEADER,
        SEALED | 4,
        SEALED | END_OR_NO_HEADER | 4,
    ] {
        for limit in [0, 3, 127] {
            spool_segment(&input(mode, limit, &body));
        }
    }
}

#[test]
fn a_flipped_byte_and_bad_frames_inside_a_record_are_survived() {
    let good = framed(&[vec![9; 5], vec![8; 100], vec![7; 1]]);
    for flip in [0, 7, 40, 200] {
        spool_segment(&input(
            SEALED | END_OR_NO_HEADER | FLIP,
            flip,
            &records(&[&good]),
        ));
    }
    // A huge frame length, a zero length, and a frame longer than its record.
    for bad in [
        &[0xff, 0xff, 0xff, 0xff, 1][..],
        &[0, 0, 0, 0, 1],
        &[0, 0, 0, 9, 1, 2],
    ] {
        let mut plaintext = good.clone();
        plaintext.extend_from_slice(bad);
        spool_segment(&input(SEALED, 0, &records(&[&plaintext])));
    }
}

#[test]
fn raw_bytes_behind_a_header_and_without_one_are_survived() {
    let raw = [0, 0, 0, 5, 0, 0, 0, 0, 1, 0, 0, 0, 1, 2, 3];
    spool_segment(&input(0, 0, &raw));
    spool_segment(&input(4, 0, &raw));
    // No header: the startup check deletes and counts the file.
    spool_segment(&input(END_OR_NO_HEADER, 0, &raw));
    spool_segment(&input(
        END_OR_NO_HEADER,
        0,
        &plaintext_segment(&framed(&[vec![1; 40]])),
    ));
}

#[test]
fn cursor_files_of_every_kind_are_survived() {
    let body = records(&[
        &framed(&[vec![1; 40], vec![2; 40]]),
        &framed(&[vec![3; 40]]),
    ]);
    let sealed = SEALED | END_OR_NO_HEADER;
    // Raw cursor bytes: too short, and 28 bytes of noise.
    let mut raw = vec![1 | sealed, 0];
    raw.extend_from_slice(b"ETDC");
    raw.extend_from_slice(&body);
    spool_segment(&raw);
    let mut noisy = vec![1 | sealed, 0];
    noisy.extend_from_slice(&[0xff; 28]);
    noisy.extend_from_slice(&body);
    spool_segment(&noisy);
    // A valid cursor at a frame boundary, inside a frame, at a record boundary,
    // at the start, beyond the end, for a missing sequence, and for the largest
    // sequence.
    for (mode, selector, offset) in [
        (2_u8, 1_u8, 44_u16),
        (2, 1, 50),
        (2, 1, 88),
        (2, 1, 0),
        (2, 1, 5000),
        (2, 0, 44),
        (2, 2, 44),
        (3, 0, 44),
    ] {
        let mut data = vec![mode | sealed, 0, selector];
        data.extend_from_slice(&offset.to_be_bytes());
        data.extend_from_slice(&body);
        spool_segment(&data);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Arbitrary bytes in every mode.
    #[test]
    fn arbitrary_bytes_never_panic_and_always_terminate(
        data in prop::collection::vec(any::<u8>(), 0..400),
        body in prop::collection::vec(any::<u8>(), 0..400),
        mode in any::<u8>(),
        limit in any::<u8>(),
    ) {
        spool_segment(&data);
        spool_segment(&input(mode, limit, &body));
        spool_segment(&input(mode | SEALED, limit, &body));
    }
}
