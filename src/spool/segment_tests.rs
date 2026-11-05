use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
};

use super::*;
use crate::spool::test_support::{
    TestDir, contains, framed, numbered, numbers, other_key, parse_segment, payloads,
    plaintext_segment, sealed_segment, test_key,
};

/// Reads to the end of the file.
const LEFTOVER: Bounds = Bounds {
    limit: None,
    stop_at: None,
};

/// Reads up to the writer's length.
const fn live(limit: u64) -> Bounds {
    Bounds {
        limit: Some(limit),
        stop_at: None,
    }
}

fn segment_path(dir: &TestDir) -> PathBuf {
    dir.path().join("seg-00000000000000000001.fstrm")
}

/// Writes segment 1 with one data record per entry of `records`.
fn write_segment(dir: &TestDir, records: &[&[u8]], end: bool) -> PathBuf {
    let path = segment_path(dir);
    fs::write(&path, sealed_segment(&test_key(), 1, records, end)).expect("segment is written");
    path
}

fn open_at(path: &Path, offset: u64) -> SegmentReader {
    SegmentReader::open(path, test_key(), 1, offset, LEFTOVER)
        .expect("open")
        .expect("the header is valid")
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).expect("segment metadata").len()
}

#[test]
fn the_lengths_match_the_record_layer() {
    assert_eq!(HEADER_LEN, 32);
    assert_eq!(RECORD_EXTRA, 28);
    assert_eq!(END_LEN, 28);
    let (block, _) = numbered(0..3, 5);
    assert_eq!(sealed_len(&block), Some(block.len() as u64 + 28));
    assert_eq!(sealed_len(&[]), None);
}

#[test]
fn writer_seals_each_append_as_one_record_and_writes_no_plaintext() {
    let dir = TestDir::new("segment-writer");
    let path = segment_path(&dir);
    let mut buf = Vec::new();
    let mut writer = SegmentWriter::create(&path, test_key(), 1).expect("segment is created");
    assert_eq!(writer.len(), HEADER_LEN);
    let (first, _) = numbered(0..3, 40);
    let (second, _) = numbered(3..4, 40);
    writer.append(&first, &mut buf).expect("append");
    writer.append(&second, &mut buf).expect("append");
    let expected_len = HEADER_LEN + first.len() as u64 + second.len() as u64 + 2 * RECORD_EXTRA;
    assert_eq!(writer.len(), expected_len);
    assert_eq!(writer.seal(&mut buf).expect("seal"), expected_len + END_LEN);
    let file = fs::read(&path).expect("segment is readable");
    assert_eq!(file.len() as u64, expected_len + END_LEN);
    let read = parse_segment(&file);
    assert_eq!(read, payloads(&[first.clone(), second.clone()].concat()));
    // No frame, payload, or part of the filler is on disk in the clear.
    for payload in &read {
        assert!(
            !contains(&file, payload),
            "a payload is on disk in plaintext"
        );
    }
    assert!(!contains(&file, &first[..16]));
    assert!(
        !contains(&file, &[0xa5; 16]),
        "the filler is on disk in plaintext"
    );
}

#[test]
fn a_warm_writer_buffer_is_not_reallocated() {
    let dir = TestDir::new("segment-warm");
    let mut buf = Vec::new();
    let mut writer =
        SegmentWriter::create(&segment_path(&dir), test_key(), 1).expect("segment is created");
    let (block, _) = numbered(0..20, 100);
    writer.append(&block, &mut buf).expect("append");
    let (address, capacity) = (buf.as_ptr(), buf.capacity());
    for _ in 0..100 {
        writer.append(&block, &mut buf).expect("append");
        assert_eq!((buf.as_ptr(), buf.capacity()), (address, capacity));
    }
}

#[test]
fn each_segment_gets_its_own_random_nonce_prefix() {
    let dir = TestDir::new("segment-nonce");
    let first = dir.path().join("seg-00000000000000000001.fstrm");
    let second = dir.path().join("seg-00000000000000000002.fstrm");
    drop(SegmentWriter::create(&first, test_key(), 1).expect("create"));
    drop(SegmentWriter::create(&second, test_key(), 2).expect("create"));
    let (first, second) = (
        fs::read(first).expect("read"),
        fs::read(second).expect("read"),
    );
    assert_eq!(first.len(), FILE_HEADER_LEN);
    assert_ne!(first[16..24], second[16..24], "nonce prefixes differ");
    assert_ne!(first[16..24], [0; 8]);
}

#[test]
fn writer_never_opens_an_existing_file() {
    let dir = TestDir::new("segment-create-new");
    let path = write_segment(&dir, &[], false);
    let error = SegmentWriter::create(&path, test_key(), 1)
        .err()
        .expect("an existing file is refused");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn a_block_over_one_mib_is_split_at_frame_boundaries() {
    let dir = TestDir::new("segment-split");
    let path = segment_path(&dir);
    let frames: Vec<Vec<u8>> = (0..20_u8).map(|n| vec![n; 60_000]).collect();
    let block = framed(&frames);
    assert!(block.len() > MAX_PLAINTEXT_LEN);
    assert_eq!(
        sealed_len(&block),
        Some(block.len() as u64 + 2 * RECORD_EXTRA)
    );
    let mut buf = Vec::new();
    let mut writer = SegmentWriter::create(&path, test_key(), 1).expect("create");
    writer.append(&block, &mut buf).expect("append");
    writer.seal(&mut buf).expect("seal");
    assert_eq!(parse_segment(&fs::read(&path).expect("read")), frames);
    // Bytes over 1 MiB that are not whole frames cannot be split.
    assert_eq!(sealed_len(&vec![0xff; MAX_PLAINTEXT_LEN + 1]), None);
    let error = writer_for(&dir).append(&vec![0xff; MAX_PLAINTEXT_LEN + 1], &mut buf);
    assert_eq!(
        error.expect_err("refused").kind(),
        io::ErrorKind::InvalidInput
    );
}

fn writer_for(dir: &TestDir) -> SegmentWriter {
    SegmentWriter::create(
        &dir.path().join("seg-00000000000000000009.fstrm"),
        test_key(),
        9,
    )
    .expect("create")
}

#[test]
fn reader_returns_whole_frames_within_the_batch_limit_across_records() {
    let dir = TestDir::new("segment-batches");
    // 32 bytes per frame, in records of 4, 4, and 2 frames.
    let (body, _) = numbered(0..10, 20);
    let path = write_segment(&dir, &[&body[..128], &body[128..256], &body[256..]], false);
    let mut reader = open_at(&path, 0);
    let mut collected = Vec::new();
    let mut ends = Vec::new();
    loop {
        let mut out = Vec::new();
        let batch = reader.read_batch(&mut out, 100, LEFTOVER).expect("read");
        assert!(
            out.len() <= 100,
            "a batch of whole frames stays within the limit"
        );
        assert_eq!(batch.bytes, out.len() as u64);
        assert_eq!(batch.frames as usize, numbers(&out).len());
        collected.extend(numbers(&out));
        ends.push(batch.end);
        if batch.frames == 0 {
            break;
        }
    }
    assert_eq!(collected, (0..10).collect::<Vec<_>>());
    // Three frames fit in 100 bytes; the last frame ends the file in its batch.
    let batch_full = Some(EndReason::BatchFull);
    let end = Some(EndReason::EndOfFile);
    assert_eq!(ends, [batch_full, batch_full, batch_full, end, end]);
    assert_eq!(reader.position(), body.len() as u64);
    assert_eq!(reader.file_position(), file_len(&path));
}

#[test]
fn reader_always_takes_one_frame_even_over_the_limit() {
    let dir = TestDir::new("segment-one-frame");
    let (body, _) = numbered(0..2, 200);
    let path = write_segment(&dir, &[&body], false);
    let mut reader = open_at(&path, 0);
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 10, LEFTOVER).expect("read");
    assert_eq!(batch.frames, 1);
    assert_eq!(batch.end, Some(EndReason::BatchFull));
    assert_eq!(numbers(&out), [0]);
}

#[test]
fn reader_stops_at_the_end_record_and_stays_stopped() {
    let dir = TestDir::new("segment-stop");
    let path = write_segment(&dir, &[&numbered(0..2, 0).0], true);
    let mut reader = open_at(&path, 0);
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!((batch.frames, batch.end), (2, Some(EndReason::Stop)));
    assert_eq!(batch.dropped_bytes, 0);
    let again = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!((again.frames, again.end), (0, Some(EndReason::Stop)));
}

#[test]
fn reader_follows_a_segment_that_is_still_being_written() {
    let dir = TestDir::new("segment-live");
    let path = segment_path(&dir);
    let mut buf = Vec::new();
    let mut writer = SegmentWriter::create(&path, test_key(), 1).expect("create");
    let mut reader = SegmentReader::open(&path, test_key(), 1, 0, live(writer.len()))
        .expect("open")
        .expect("header");
    let mut out = Vec::new();

    let empty = reader
        .read_batch(&mut out, 1000, live(writer.len()))
        .expect("read");
    assert_eq!(
        (empty.frames, empty.end),
        (0, Some(EndReason::WriterPosition))
    );

    writer
        .append(&numbered(0..2, 0).0, &mut buf)
        .expect("append");
    let first = reader
        .read_batch(&mut out, 1000, live(writer.len()))
        .expect("read");
    assert_eq!(
        (first.frames, first.end),
        (2, Some(EndReason::WriterPosition))
    );
    assert_eq!(reader.file_position(), writer.len());

    writer
        .append(&numbered(2..3, 0).0, &mut buf)
        .expect("append");
    // A limit at the reader's position hides data the caller has not accounted for.
    let hidden = reader
        .read_batch(&mut out, 1000, live(reader.file_position()))
        .expect("read");
    assert_eq!(
        (hidden.frames, hidden.end),
        (0, Some(EndReason::WriterPosition))
    );
    let second = reader
        .read_batch(&mut out, 1000, live(writer.len()))
        .expect("read");
    assert_eq!(second.frames, 1);
    assert_eq!(numbers(&out), [0, 1, 2]);

    writer.seal(&mut buf).expect("seal");
    let sealed = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!((sealed.frames, sealed.end), (0, Some(EndReason::Stop)));
}

#[test]
fn reader_opens_at_a_logical_offset_inside_a_record_and_can_rewind() {
    let dir = TestDir::new("segment-offset");
    // 12 bytes per frame: frames 0 to 2 in the first record, 3 to 5 in the second.
    let (body, _) = numbered(0..6, 0);
    let path = write_segment(&dir, &[&body[..36], &body[36..]], true);
    let mut reader = open_at(&path, 24);
    assert_eq!(reader.position(), 24);
    // Inside the first record: its header, then two frames.
    assert_eq!(reader.file_position(), HEADER_LEN + 12 + 24);
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 24, LEFTOVER).expect("read");
    assert_eq!(batch.frames, 2);
    assert_eq!(numbers(&out), [2, 3]);
    // Back inside the record the reader is in.
    reader.rewind(48, LEFTOVER).expect("rewind");
    out.clear();
    reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!(numbers(&out), [4, 5]);
    // Back to an earlier record, and to a record boundary.
    for (position, expected) in [
        (24, vec![2, 3, 4, 5]),
        (36, vec![3, 4, 5]),
        (0, (0..6).collect()),
    ] {
        reader.rewind(position, LEFTOVER).expect("rewind");
        assert_eq!(reader.position(), position);
        out.clear();
        let again = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
        assert_eq!(numbers(&out), expected, "rewind to {position}");
        assert_eq!(again.end, Some(EndReason::Stop));
    }
    // A reader opened at a record boundary starts at that record.
    let mut boundary = open_at(&path, 36);
    assert_eq!(boundary.file_position(), HEADER_LEN + 36 + RECORD_EXTRA);
    out.clear();
    boundary.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!(numbers(&out), [3, 4, 5]);
}

#[test]
fn an_offset_inside_a_frame_is_corrupt_from_that_frame() {
    let dir = TestDir::new("segment-offset-in-frame");
    let (body, _) = numbered(0..3, 0);
    let path = write_segment(&dir, &[&body], true);
    let mut reader = open_at(&path, 16);
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!((batch.frames, batch.end), (0, Some(EndReason::Corrupt)));
    // Frame 1 holds the offset: the rest from it is dropped.
    assert_eq!(batch.lost_at, 12);
    assert_eq!(
        batch.dropped_bytes,
        file_len(&path) - (HEADER_LEN + 12 + 12)
    );
    let again = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!(
        (again.end, again.dropped_bytes),
        (Some(EndReason::Corrupt), 0)
    );
}

#[test]
fn an_offset_past_the_end_reads_nothing() {
    let dir = TestDir::new("segment-past-end");
    let path = write_segment(&dir, &[&numbered(0..1, 0).0], true);
    let batch = open_at(&path, 10_000)
        .read_batch(&mut Vec::new(), 1000, LEFTOVER)
        .expect("read");
    assert_eq!((batch.frames, batch.end), (0, Some(EndReason::Stop)));
    let unsealed = TestDir::new("segment-past-end-unsealed");
    let unsealed_path = write_segment(&unsealed, &[&numbered(0..1, 0).0], false);
    let unsealed_batch = open_at(&unsealed_path, 10_000)
        .read_batch(&mut Vec::new(), 1000, LEFTOVER)
        .expect("read");
    assert_eq!(
        (unsealed_batch.frames, unsealed_batch.end),
        (0, Some(EndReason::EndOfFile))
    );
}

#[test]
fn a_bad_header_is_refused() {
    let dir = TestDir::new("segment-bad-header");
    let path = segment_path(&dir);
    let good = sealed_segment(&test_key(), 1, &[&numbered(0..1, 0).0], true);
    let cases: [(&str, Vec<u8>); 5] = [
        ("empty", Vec::new()),
        ("short", good[..20].to_vec()),
        ("plaintext", plaintext_segment(&numbered(0..1, 0).0)),
        (
            "other key",
            sealed_segment(&other_key(), 1, &[b"\0\0\0\x01x"], true),
        ),
        (
            "other sequence",
            sealed_segment(&test_key(), 2, &[b"\0\0\0\x01x"], true),
        ),
    ];
    for (name, bytes) in cases {
        fs::write(&path, bytes).expect("segment is written");
        let opened = SegmentReader::open(&path, test_key(), 1, 0, LEFTOVER).expect("open");
        assert!(opened.is_none(), "{name}");
    }
}

#[test]
fn the_header_check_tells_the_kinds_of_file_apart() {
    let dir = TestDir::new("segment-check");
    let path = segment_path(&dir);
    let good = sealed_segment(&test_key(), 1, &[&numbered(0..1, 0).0], true);
    let cases = [
        (good.clone(), HeaderCheck::Readable),
        (good[..FILE_HEADER_LEN].to_vec(), HeaderCheck::Readable),
        (good[..20].to_vec(), HeaderCheck::TooShort),
        (Vec::new(), HeaderCheck::TooShort),
        (
            plaintext_segment(&numbered(0..1, 0).0),
            HeaderCheck::NotThisFormat,
        ),
        (vec![0xee; 100], HeaderCheck::NotThisFormat),
        (
            sealed_segment(&other_key(), 1, &[], false),
            HeaderCheck::OtherKey,
        ),
    ];
    for (index, (bytes, expected)) in cases.into_iter().enumerate() {
        fs::write(&path, bytes).expect("segment is written");
        assert_eq!(
            check_header(&path, &test_key()).expect("check"),
            expected,
            "case {index}"
        );
    }
}

#[test]
fn a_cut_off_final_record_is_counted_and_ends_the_segment() {
    let dir = TestDir::new("segment-cutoff");
    let (first, _) = numbered(0..2, 0);
    let (second, _) = numbered(2..4, 0);
    let path = write_segment(&dir, &[&first, &second], false);
    let full = file_len(&path);
    OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for truncation")
        .set_len(full - 5)
        .expect("truncate");
    let mut reader = open_at(&path, 0);
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!(numbers(&out), [0, 1]);
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!(batch.dropped_bytes, second.len() as u64 + RECORD_EXTRA - 5);
    assert_eq!(batch.dropped_frames, 1);
    assert_eq!(batch.lost_at, first.len() as u64);
    let again = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!((again.frames, again.dropped_bytes), (0, 0));
    assert_eq!(again.end, Some(EndReason::CutOff));
}

#[test]
fn a_partial_record_header_is_a_cut_off_record() {
    let dir = TestDir::new("segment-partial-header");
    let path = segment_path(&dir);
    let mut bytes = sealed_segment(&test_key(), 1, &[], false);
    bytes.extend_from_slice(&[0, 0, 1]);
    fs::write(&path, bytes).expect("segment is written");
    let batch = open_at(&path, 0)
        .read_batch(&mut Vec::new(), 1000, LEFTOVER)
        .expect("read");
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!((batch.dropped_bytes, batch.dropped_frames), (3, 1));
}

#[test]
fn a_changed_byte_anywhere_in_a_record_is_corrupt_and_drops_the_rest() {
    let dir = TestDir::new("segment-tamper");
    let path = segment_path(&dir);
    let (first, _) = numbered(0..2, 0);
    let (second, _) = numbered(2..4, 0);
    let good = sealed_segment(&test_key(), 1, &[&first, &second], true);
    let second_start = FILE_HEADER_LEN + first.len() + RECORD_OVERHEAD;
    // A changed length makes the record look longer than the file: it reads as
    // cut off, and nothing of it is handed out.
    let mut longer = good.clone();
    longer[second_start + 1] ^= 0x20;
    fs::write(&path, &longer).expect("segment is written");
    let mut out = Vec::new();
    let batch = open_at(&path, 0)
        .read_batch(&mut out, 1000, LEFTOVER)
        .expect("read");
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!(numbers(&out), [0, 1]);
    assert_eq!(batch.dropped_bytes, (good.len() - second_start) as u64);
    // The second record's index, kind, ciphertext, tag, and the end record.
    for at in [
        second_start + 4,
        second_start + 8,
        second_start + 15,
        good.len() - 30,
        good.len() - 1,
    ] {
        let mut bad = good.clone();
        bad[at] ^= 0x20;
        fs::write(&path, &bad).expect("segment is written");
        let mut kept_out = Vec::new();
        let tampered = open_at(&path, 0)
            .read_batch(&mut kept_out, 1000, LEFTOVER)
            .expect("read");
        assert_eq!(tampered.end, Some(EndReason::Corrupt), "byte {at}");
        let end_record = good.len() - RECORD_OVERHEAD;
        let kept = if at >= end_record {
            vec![0, 1, 2, 3]
        } else {
            vec![0, 1]
        };
        assert_eq!(numbers(&kept_out), kept, "byte {at}");
        assert_eq!(tampered.dropped_frames, 0);
        assert!(tampered.dropped_bytes > 0);
    }
    // A changed sequence in the file header no longer matches the file name.
    let mut renamed = good;
    renamed[FILE_HEADER_LEN - 1] ^= 1;
    fs::write(&path, &renamed).expect("segment is written");
    let opened = SegmentReader::open(&path, test_key(), 1, 0, LEFTOVER).expect("open");
    assert!(opened.is_none());
    // A changed nonce prefix fails the first record.
    let mut bad_prefix = sealed_segment(&test_key(), 1, &[&first], true);
    bad_prefix[16] ^= 1;
    fs::write(&path, &bad_prefix).expect("segment is written");
    let prefix_batch = open_at(&path, 0)
        .read_batch(&mut Vec::new(), 1000, LEFTOVER)
        .expect("read");
    assert_eq!(
        (prefix_batch.frames, prefix_batch.end),
        (0, Some(EndReason::Corrupt))
    );
    assert_eq!(prefix_batch.dropped_bytes, file_len(&path) - HEADER_LEN);
}

#[test]
fn a_bad_frame_inside_a_record_is_corrupt_and_drops_the_rest() {
    let dir = TestDir::new("segment-bad-frame");
    let too_long = u32::try_from(MAX_DATA_FRAME + 1).expect("fits");
    for bad_frame in [
        too_long.to_be_bytes().to_vec(),
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 40],
    ] {
        let mut record = numbered(0..1, 0).0;
        record.extend_from_slice(&bad_frame);
        record.extend_from_slice(&[7; 30]);
        let path = write_segment(&dir, &[&record], false);
        let mut out = Vec::new();
        let batch = open_at(&path, 0)
            .read_batch(&mut out, 1000, LEFTOVER)
            .expect("read");
        assert_eq!(batch.frames, 1);
        assert_eq!(batch.end, Some(EndReason::Corrupt));
        assert_eq!(batch.lost_at, 12);
        // From the bad frame, in the record after its header, to the end.
        assert_eq!(
            batch.dropped_bytes,
            file_len(&path) - (HEADER_LEN + 12 + 12)
        );
        assert_eq!(batch.dropped_frames, 0);
    }
}

#[test]
fn a_frame_of_the_largest_allowed_length_is_read() {
    let dir = TestDir::new("segment-max-frame");
    let body = framed(&[vec![3; MAX_DATA_FRAME]]);
    let path = write_segment(&dir, &[&body], false);
    let batch = open_at(&path, 0)
        .read_batch(&mut Vec::new(), 1000, LEFTOVER)
        .expect("read");
    assert_eq!((batch.frames, batch.end), (1, Some(EndReason::EndOfFile)));
}

#[test]
fn a_file_cut_short_under_the_reader_keeps_the_earlier_records() {
    let dir = TestDir::new("segment-truncated");
    let (first, _) = numbered(0..1, 0);
    let (second, _) = numbered(1..2, 0);
    let path = write_segment(&dir, &[&first, &second], false);
    let full = file_len(&path);
    // The writer says the file is whole; it really holds one record and a part
    // of the second.
    OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for truncation")
        .set_len(full - 10)
        .expect("truncate");
    let mut reader = open_at(&path, 0);
    let mut out = vec![9, 9];
    let batch = reader
        .read_batch(&mut out, 1000, live(full))
        .expect("a short file is a cut-off record, not an error");
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!(batch.frames, 1);
    assert_eq!(batch.dropped_bytes, second.len() as u64 + RECORD_EXTRA);
    assert_eq!(&out[..2], &[9, 9]);
    assert_eq!(numbers(&out[2..]), [0]);
}

#[test]
fn the_readable_data_can_end_before_the_file() {
    let dir = TestDir::new("segment-stop-at");
    let (body, _) = numbered(0..4, 0);
    let path = write_segment(&dir, &[&body[..24], &body[24..]], true);
    let bounds = Bounds {
        limit: None,
        stop_at: Some(24),
    };
    let mut reader = SegmentReader::open(&path, test_key(), 1, 0, bounds)
        .expect("open")
        .expect("header");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, bounds).expect("read");
    assert_eq!(numbers(&out), [0, 1]);
    assert_eq!(batch.end, Some(EndReason::WriterPosition));
}

#[test]
fn the_writer_and_the_reader_share_the_one_key_page() {
    let dir = TestDir::new("segment-shared-key");
    let path = segment_path(&dir);
    let key = test_key();
    let writer = SegmentWriter::create(&path, Arc::clone(&key), 1).expect("create");
    let reader = SegmentReader::open(&path, Arc::clone(&key), 1, 0, live(writer.len()))
        .expect("open")
        .expect("header");
    assert!(Arc::ptr_eq(writer.key(), &key));
    assert!(Arc::ptr_eq(reader.key(), &key));
    assert_eq!(writer.key().page_address(), reader.key().page_address());
}

#[test]
fn a_folder_is_not_opened_as_a_segment() {
    let dir = TestDir::new("segment-folder");
    let path = segment_path(&dir);
    fs::create_dir(&path).expect("folder");
    let error = SegmentReader::open(&path, test_key(), 1, 0, LEFTOVER)
        .err()
        .expect("a folder is refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn the_readable_data_can_end_inside_a_record() {
    let dir = TestDir::new("segment-stop-inside");
    let mut record = numbered(0..2, 0).0;
    record.extend_from_slice(&[0, 0, 0, 0, 9, 9]);
    let path = write_segment(&dir, &[&record], true);
    let mut reader = open_at(&path, 0);
    let mut out = Vec::new();
    let first = reader.read_batch(&mut out, 1000, LEFTOVER).expect("read");
    assert_eq!((first.frames, first.end), (2, Some(EndReason::Corrupt)));
    assert_eq!(first.lost_at, 24);
    // The caller counted the rest and reads again up to it: nothing is
    // reported a second time.
    let bounds = Bounds {
        limit: None,
        stop_at: Some(first.lost_at),
    };
    reader.rewind(0, bounds).expect("rewind");
    out.clear();
    let again = reader.read_batch(&mut out, 1000, bounds).expect("read");
    assert_eq!(numbers(&out), [0, 1]);
    assert_eq!(again.end, Some(EndReason::WriterPosition));
    assert_eq!(again.dropped_bytes, 0);
}
