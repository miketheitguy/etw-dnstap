use std::fs::{self, OpenOptions};

use super::*;
use crate::spool::test_support::{
    STOP, TestDir, framed, numbered, numbers, parse_fstrm, start_bytes,
};

fn segment_path(dir: &TestDir) -> std::path::PathBuf {
    dir.path().join("seg-00000000000000000001.fstrm")
}

/// A segment file with START, `body`, and no STOP.
fn raw_segment(dir: &TestDir, body: &[u8]) -> std::path::PathBuf {
    let path = segment_path(dir);
    let mut bytes = start_bytes();
    bytes.extend_from_slice(body);
    fs::write(&path, bytes).expect("segment file is written");
    path
}

#[test]
fn start_and_stop_lengths_match_the_encoder() {
    assert_eq!(start_bytes().len() as u64, START_LEN);
    assert_eq!(expected_start(), start_bytes());
    assert_eq!(STOP.len() as u64, STOP_LEN);
}

#[test]
fn writer_makes_a_valid_frame_streams_file() {
    let dir = TestDir::new("segment-writer");
    let path = segment_path(&dir);
    let mut writer = SegmentWriter::create(&path).expect("segment is created");
    assert_eq!(writer.len(), START_LEN);
    let (first, _) = numbered(0..3, 5);
    let (second, _) = numbered(3..4, 0);
    writer.append(&first).expect("append");
    writer.append(&second).expect("append");
    let expected_len = START_LEN + first.len() as u64 + second.len() as u64;
    assert_eq!(writer.len(), expected_len);
    assert_eq!(writer.seal().expect("seal"), expected_len + STOP_LEN);
    let file = fs::read(&path).expect("segment is readable");
    assert_eq!(file.len() as u64, expected_len + STOP_LEN);
    assert_eq!(parse_fstrm(&file).len(), 4);
}

#[test]
fn writer_never_opens_an_existing_file() {
    let dir = TestDir::new("segment-create-new");
    let path = raw_segment(&dir, &[]);
    let error = SegmentWriter::create(&path)
        .err()
        .expect("an existing file is refused");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn reader_returns_whole_frames_within_the_batch_limit() {
    let dir = TestDir::new("segment-batches");
    let (body, _) = numbered(0..10, 20); // 32 bytes per frame
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0)
        .expect("open")
        .expect("START is valid");
    let mut collected = Vec::new();
    let mut ends = Vec::new();
    loop {
        let mut out = Vec::new();
        let batch = reader.read_batch(&mut out, 100, None).expect("read");
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
    assert_eq!(reader.position(), START_LEN + body.len() as u64);
}

#[test]
fn reader_always_takes_one_frame_even_over_the_limit() {
    let dir = TestDir::new("segment-one-frame");
    let (body, _) = numbered(0..2, 200);
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 10, None).expect("read");
    assert_eq!(batch.frames, 1);
    assert_eq!(batch.end, Some(EndReason::BatchFull));
    assert_eq!(numbers(&out), [0]);
}

#[test]
fn reader_stops_at_the_stop_frame_and_stays_stopped() {
    let dir = TestDir::new("segment-stop");
    let mut contents = numbered(0..2, 0).0;
    contents.extend_from_slice(&STOP);
    let path = raw_segment(&dir, &contents);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!((batch.frames, batch.end), (2, Some(EndReason::Stop)));
    assert_eq!(batch.dropped_bytes, 0);
    let again = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!((again.frames, again.end), (0, Some(EndReason::Stop)));
}

#[test]
fn reader_follows_a_segment_that_is_still_being_written() {
    let dir = TestDir::new("segment-live");
    let path = segment_path(&dir);
    let mut writer = SegmentWriter::create(&path).expect("create");
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();

    let empty = reader
        .read_batch(&mut out, 1000, Some(writer.len()))
        .expect("read");
    assert_eq!(
        (empty.frames, empty.end),
        (0, Some(EndReason::WriterPosition))
    );

    writer.append(&numbered(0..2, 0).0).expect("append");
    let first = reader
        .read_batch(&mut out, 1000, Some(writer.len()))
        .expect("read");
    assert_eq!(
        (first.frames, first.end),
        (2, Some(EndReason::WriterPosition))
    );

    writer.append(&numbered(2..3, 0).0).expect("append");
    // A limit at the reader's position hides data the caller has not accounted for.
    let hidden = reader
        .read_batch(&mut out, 1000, Some(reader.position()))
        .expect("read");
    assert_eq!(hidden.frames, 0);
    let second = reader
        .read_batch(&mut out, 1000, Some(writer.len()))
        .expect("read");
    assert_eq!(second.frames, 1);
    assert_eq!(numbers(&out), [0, 1, 2]);

    writer.seal().expect("seal");
    let sealed = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!((sealed.frames, sealed.end), (0, Some(EndReason::Stop)));
}

#[test]
fn reader_opens_at_an_offset_and_can_rewind() {
    let dir = TestDir::new("segment-offset");
    let (body, _) = numbered(0..6, 0); // 12 bytes per frame
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, START_LEN + 24)
        .expect("open")
        .expect("START");
    assert_eq!(reader.position(), START_LEN + 24);
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 24, None).expect("read");
    assert_eq!(batch.frames, 2);
    assert_eq!(numbers(&out), [2, 3]);
    reader.rewind(START_LEN + 24).expect("rewind");
    out.clear();
    reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!(numbers(&out), [2, 3, 4, 5]);

    // An offset inside START reads from just after START.
    let mut from_start = SegmentReader::open(&path, 3).expect("open").expect("START");
    assert_eq!(from_start.position(), START_LEN);
    out.clear();
    from_start.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!(numbers(&out), [0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_missing_or_wrong_start_is_refused() {
    let dir = TestDir::new("segment-bad-start");
    let path = segment_path(&dir);
    fs::write(&path, [0_u8; 10]).expect("short file");
    assert!(SegmentReader::open(&path, 0).expect("open").is_none());
    let mut wrong = start_bytes();
    wrong[11] = 9; // control type 9 instead of START
    fs::write(&path, &wrong).expect("wrong START");
    assert!(SegmentReader::open(&path, 0).expect("open").is_none());
    fs::write(&path, b"").expect("empty file");
    assert!(SegmentReader::open(&path, 0).expect("open").is_none());
}

#[test]
fn a_cut_off_final_frame_is_counted_and_ends_the_segment() {
    let dir = TestDir::new("segment-cutoff");
    let mut body = numbered(0..2, 0).0;
    body.extend_from_slice(&[0, 0, 0, 100, 1, 2, 3]); // header says 100, 3 bytes follow
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!(batch.frames, 2);
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!(batch.dropped_bytes, 7);
    assert_eq!(batch.dropped_frames, 1);
    let again = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!((again.frames, again.dropped_bytes), (0, 0));
    assert_eq!(again.end, Some(EndReason::CutOff));
}

#[test]
fn a_partial_length_prefix_is_a_cut_off_frame() {
    let dir = TestDir::new("segment-partial-header");
    let path = raw_segment(&dir, &[0, 0, 1]);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let batch = reader
        .read_batch(&mut Vec::new(), 1000, None)
        .expect("read");
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!((batch.dropped_bytes, batch.dropped_frames), (3, 1));
}

#[test]
fn a_bad_data_length_is_corrupt_and_drops_the_rest() {
    let dir = TestDir::new("segment-corrupt");
    let too_long = u32::try_from(MAX_DATA_FRAME + 1).expect("fits");
    let mut body = numbered(0..1, 0).0;
    body.extend_from_slice(&too_long.to_be_bytes());
    body.extend_from_slice(&[7; 30]);
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!(batch.frames, 1);
    assert_eq!(batch.end, Some(EndReason::Corrupt));
    assert_eq!((batch.dropped_bytes, batch.dropped_frames), (34, 0));
}

#[test]
fn a_frame_of_the_largest_allowed_length_is_read() {
    let dir = TestDir::new("segment-max-frame");
    let body = framed(&[vec![3; usize::try_from(MAX_DATA_FRAME).expect("fits")]]);
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!((batch.frames, batch.end), (1, Some(EndReason::EndOfFile)));
}

#[test]
fn control_frames_are_validated() {
    // (body, expected end, dropped bytes)
    let cases: [(&[u8], EndReason, u64); 4] = [
        // Control length 2 is below the 4-byte minimum.
        (&[0, 0, 0, 0, 0, 0, 0, 2, 0, 0], EndReason::Corrupt, 10),
        // Control length 513 is above the maximum.
        (&[0, 0, 0, 0, 0, 0, 2, 1], EndReason::Corrupt, 8),
        // An unknown control type.
        (
            &[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 9],
            EndReason::Corrupt,
            12,
        ),
        // The control body is cut off.
        (
            &[0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 3],
            EndReason::CutOff,
            12,
        ),
    ];
    for (index, (body, end, dropped)) in cases.into_iter().enumerate() {
        let dir = TestDir::new("segment-control");
        let path = raw_segment(&dir, body);
        let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
        let batch = reader
            .read_batch(&mut Vec::new(), 1000, None)
            .expect("read");
        assert_eq!(batch.end, Some(end), "case {index}");
        assert_eq!(batch.dropped_bytes, dropped, "case {index}");
    }
}

#[test]
fn a_second_start_frame_is_skipped() {
    let dir = TestDir::new("segment-second-start");
    let mut body = numbered(0..1, 0).0;
    body.extend_from_slice(&start_bytes());
    body.extend_from_slice(&numbered(1..2, 0).0);
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = Vec::new();
    let batch = reader.read_batch(&mut out, 1000, None).expect("read");
    assert_eq!(numbers(&out), [0, 1]);
    assert_eq!(batch.end, Some(EndReason::EndOfFile));
}

#[test]
fn an_offset_past_the_end_of_the_file_reads_nothing() {
    let dir = TestDir::new("segment-past-end");
    let path = raw_segment(&dir, &numbered(0..1, 0).0);
    let mut reader = SegmentReader::open(&path, 10_000)
        .expect("open")
        .expect("START");
    let batch = reader
        .read_batch(&mut Vec::new(), 1000, None)
        .expect("read");
    assert_eq!((batch.frames, batch.end), (0, Some(EndReason::EndOfFile)));
}

#[test]
fn a_file_cut_short_under_the_reader_keeps_the_earlier_frames() {
    let dir = TestDir::new("segment-truncated");
    let path = raw_segment(&dir, &numbered(0..2, 0).0);
    // The writer says 24 bytes of frames; the file really holds one frame and
    // a part of the second.
    OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for truncation")
        .set_len(START_LEN + 14)
        .expect("truncate");
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut out = vec![9, 9];
    let batch = reader
        .read_batch(&mut out, 1000, Some(START_LEN + 24))
        .expect("a short file is a cut-off frame, not an error");
    assert_eq!(batch.end, Some(EndReason::CutOff));
    assert_eq!(batch.frames, 1);
    assert_eq!(&out[..2], &[9, 9]);
    assert_eq!(numbers(&out[2..]), [0]);
}

#[test]
fn control_frames_count_against_the_batch_budget() {
    let dir = TestDir::new("segment-control-budget");
    // Ten START frames (42 bytes each), then one data frame.
    let mut body = Vec::new();
    for _ in 0..10 {
        body.extend_from_slice(&start_bytes());
    }
    body.extend_from_slice(&numbered(0..1, 0).0);
    let path = raw_segment(&dir, &body);
    let mut reader = SegmentReader::open(&path, 0).expect("open").expect("START");
    let mut calls = 0;
    loop {
        let before = reader.position();
        let mut out = Vec::new();
        let batch = reader.read_batch(&mut out, 100, None).expect("read");
        assert!(
            reader.position() - before <= 100 + 42,
            "one call stays near its budget"
        );
        calls += 1;
        if batch.frames > 0 {
            assert_eq!(numbers(&out), [0]);
            break;
        }
        assert_eq!(batch.end, Some(EndReason::BatchFull));
    }
    assert_eq!(calls, 4);
}

#[test]
fn a_folder_is_not_opened_as_a_segment() {
    let dir = TestDir::new("segment-folder");
    let path = segment_path(&dir);
    fs::create_dir(&path).expect("folder");
    let error = SegmentReader::open(&path, 0)
        .err()
        .expect("a folder is refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}
