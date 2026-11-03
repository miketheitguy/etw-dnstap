//! Runs the fuzz entry point over hand-made inputs, so CI exercises it without
//! libFuzzer.

use proptest::prelude::*;

use super::*;
use crate::spool::test_support::{STOP, framed, start_bytes};

/// Mode byte and limit byte, then the segment bytes.
fn input(mode: u8, limit: u8, segment: &[u8]) -> Vec<u8> {
    let mut data = vec![mode, limit];
    data.extend_from_slice(segment);
    data
}

fn start_then(body: &[u8]) -> Vec<u8> {
    let mut segment = start_bytes();
    segment.extend_from_slice(body);
    segment
}

#[test]
fn an_empty_input_is_an_empty_segment() {
    spool_segment(&[]);
    spool_segment(&[0]);
    spool_segment(&input(0, 0, &[]));
}

#[test]
fn a_segment_with_only_start_is_read_to_its_end() {
    spool_segment(&input(0, 0, &start_then(&[])));
    spool_segment(&input(0, 0, &start_then(&STOP)));
}

#[test]
fn a_huge_frame_length_ends_the_segment() {
    let mut body = u32::MAX.to_be_bytes().to_vec();
    body.extend_from_slice(&[1, 2, 3]);
    spool_segment(&input(0, 0, &start_then(&body)));
    // One byte over the encoder's limit, with the bytes present.
    let mut oversized = u32::try_from(MAX_ENCODED_LENGTH + 1)
        .expect("small")
        .to_be_bytes()
        .to_vec();
    oversized.resize(4 + MAX_ENCODED_LENGTH + 1, 7);
    spool_segment(&input(0, 0, &start_then(&oversized)));
}

#[test]
fn many_control_frames_are_skipped_with_a_tiny_batch_limit() {
    // A START frame repeated, then data: each call may skip only control frames.
    let mut body = Vec::new();
    for _ in 0..200 {
        body.extend_from_slice(&start_bytes());
    }
    body.extend_from_slice(&framed(&[vec![1; 10], vec![2; 20]]));
    body.extend_from_slice(&STOP);
    spool_segment(&input(0, 0, &start_then(&body)));
    spool_segment(&input(4, 0, &start_then(&body)));
}

#[test]
fn good_data_cut_off_data_and_a_bad_control_frame_are_each_survived() {
    let good = framed(&[vec![9; 5], vec![8; 300], vec![7; 1]]);
    spool_segment(&input(0, 3, &start_then(&good)));
    // Cut off in the middle of the last frame.
    spool_segment(&input(0, 3, &start_then(&good[..good.len() - 1])));
    // A zero length frame, then a control frame of length 3 and one of 513.
    let mut bad = good.clone();
    bad.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 3, 1, 2, 3]);
    spool_segment(&input(0, 3, &start_then(&bad)));
    let mut long = good;
    long.extend_from_slice(&[0, 0, 0, 0, 0, 0, 2, 1]);
    long.resize(long.len() + 513, 0);
    spool_segment(&input(0, 3, &start_then(&long)));
}

#[test]
fn cursor_files_of_every_kind_are_survived() {
    let segment = start_then(&framed(&[vec![1; 40], vec![2; 40], vec![3; 40]]));
    // Raw cursor bytes: too short, and 28 bytes of noise.
    let mut raw = vec![1, 0];
    raw.extend_from_slice(b"ETDC");
    raw.extend_from_slice(&segment);
    spool_segment(&raw);
    let mut noisy = vec![1, 0];
    noisy.extend_from_slice(&[0xff; 28]);
    noisy.extend_from_slice(&segment);
    spool_segment(&noisy);
    // A valid cursor in the middle of the segment, at START, beyond the end, for
    // a missing sequence, and for the largest sequence.
    for (mode, selector, offset) in [
        (2_u8, 1_u8, 60_u16),
        (2, 1, 0),
        (2, 1, 5000),
        (2, 0, 60),
        (2, 2, 60),
        (3, 0, 60),
    ] {
        let mut data = vec![mode, 0, selector];
        data.extend_from_slice(&offset.to_be_bytes());
        data.extend_from_slice(&segment);
        spool_segment(&data);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Arbitrary bytes, and arbitrary bytes behind a valid START.
    #[test]
    fn arbitrary_bytes_never_panic_and_always_terminate(
        data in prop::collection::vec(any::<u8>(), 0..400),
        after_start in prop::collection::vec(any::<u8>(), 0..400),
        mode in any::<u8>(),
        limit in any::<u8>(),
    ) {
        spool_segment(&data);
        spool_segment(&input(mode, limit, &start_then(&after_start)));
    }
}
