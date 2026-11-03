use std::{
    cell::Cell,
    fs,
    ops::Range,
    os::windows::fs::OpenOptionsExt,
    sync::{Mutex, atomic::AtomicU64},
};

use super::*;
use crate::spool::test_support::{STOP, TestDir, numbered, numbers, parse_fstrm, start_bytes};

const MIB: u64 = 1024 * 1024;
/// `FILE_SHARE_READ | FILE_SHARE_WRITE`: others may read and write but not delete.
const SHARE_NO_DELETE: u32 = 3;

thread_local! {
    /// The fake volume: free bytes before any segment file exists.
    static FREE_BASE: Cell<u64> = const { Cell::new(u64::MAX) };
    /// Free bytes the fake takes away for each segment file in the folder.
    static FREE_PER_FILE: Cell<u64> = const { Cell::new(0) };
}

/// Free space that falls with each segment file, so deleting one frees space.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the spool takes a fn pointer that returns a Result"
)]
fn fake_free(dir: &Path) -> io::Result<u64> {
    // The folder may be gone in a test that removes it.
    let files = fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_str().and_then(parse_name).is_some())
            .count() as u64
    });
    Ok(FREE_BASE
        .get()
        .saturating_sub(FREE_PER_FILE.get().saturating_mul(files)))
}

fn set_free(base: u64, per_file: u64) {
    FREE_BASE.set(base);
    FREE_PER_FILE.set(per_file);
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("folder is readable")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn segment_files(dir: &Path) -> Vec<String> {
    names_in(dir)
        .into_iter()
        .filter(|name| parse_name(name).is_some())
        .collect()
}

/// Every file name except the cursor file, which the spool keeps.
fn all_files(dir: &Path) -> Vec<String> {
    names_in(dir)
        .into_iter()
        .filter(|name| name != cursor::FILE_NAME)
        .collect()
}

fn read_cursor(dir: &TestDir) -> CursorRead {
    cursor::read(dir.path()).expect("cursor file is readable")
}

fn open_with(dir: &TestDir, max_bytes: u64, min_free_bytes: u64) -> (Spool, Arc<SinkStats>) {
    set_free(u64::MAX, 0);
    let stats = Arc::new(SinkStats::default());
    let spool = Spool::open(
        SpoolConfig {
            dir: dir.path().to_path_buf(),
            max_bytes,
            min_free_bytes,
        },
        Arc::clone(&stats),
        fake_free,
    )
    .expect("spool opens");
    (spool, stats)
}

fn open(dir: &TestDir, max_bytes: u64) -> (Spool, Arc<SinkStats>) {
    open_with(dir, max_bytes, 0)
}

fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

/// Reads until the spool returns nothing.
fn drain(spool: &mut Spool, limit: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let frames = spool.read_batch(&mut out, limit).expect("read succeeds");
        if frames == 0 {
            return out;
        }
    }
}

/// Appends frames `range` (with `pad` filler bytes each) as one block.
fn append_numbers(spool: &mut Spool, range: Range<u64>, pad: usize) -> Appended {
    let (bytes, frames) = numbered(range, pad);
    spool.append(&bytes, frames)
}

/// Writes a leftover segment file and returns its length.
fn leftover_segment(dir: &TestDir, sequence: u64, body: &[u8], stop: bool) -> u64 {
    let mut bytes = start_bytes();
    bytes.extend_from_slice(body);
    if stop {
        bytes.extend_from_slice(&STOP);
    }
    let name = format!("seg-{sequence:020}.fstrm");
    fs::write(dir.path().join(name), &bytes).expect("leftover is written");
    bytes.len() as u64
}

fn segment_name(sequence: u64) -> String {
    format!("seg-{sequence:020}.fstrm")
}

fn write_cursor(dir: &TestDir, sequence: u64, offset: u64) {
    CursorFile::new(dir.path())
        .write(Cursor { sequence, offset })
        .expect("cursor is written");
}

#[test]
fn append_then_read_returns_the_same_bytes_in_order_across_a_rollover() {
    let dir = TestDir::new("spool-rollover");
    let (mut spool, stats) = open(&dir, MIB); // segments of 128 KiB
    let mut expected = Vec::new();
    for block in 0..40 {
        let (bytes, frames) = numbered(block * 10..block * 10 + 10, 1000);
        assert_eq!(spool.append(&bytes, frames), Appended::Stored);
        expected.extend_from_slice(&bytes);
    }
    assert!(get(&stats.spool_segments) >= 3, "the spool rolled over");
    assert_eq!(
        get(&stats.spool_segments),
        segment_files(dir.path()).len() as u64
    );
    assert_eq!(get(&stats.spooled), 400);
    assert!(spool.has_backlog());

    let read = drain(&mut spool, 64 * 1024);
    assert_eq!(read, expected);
    assert_eq!(get(&stats.replayed), 400);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.spool_dropped_bytes), 0);
    assert_eq!(get(&stats.spool_errors), 0);
}

#[test]
fn every_sealed_segment_parses_as_a_frame_streams_file() {
    let dir = TestDir::new("spool-parse");
    let (mut spool, stats) = open(&dir, MIB);
    for block in 0..40 {
        assert_eq!(
            append_numbers(&mut spool, block * 10..block * 10 + 10, 1000),
            Appended::Stored
        );
    }
    spool.seal().expect("seal");
    let names = segment_files(dir.path());
    assert!(names.len() >= 3);
    assert_eq!(get(&stats.spool_segments), names.len() as u64);
    let mut total_bytes = 0;
    let mut seen = Vec::new();
    for name in &names {
        let file = fs::read(dir.path().join(name)).expect("segment is readable");
        total_bytes += file.len() as u64;
        for payload in parse_fstrm(&file) {
            seen.push(u64::from_be_bytes(payload[..8].try_into().expect("number")));
        }
    }
    assert_eq!(seen, (0..400).collect::<Vec<_>>());
    assert_eq!(get(&stats.spool_bytes), total_bytes);
    // Sealing twice, or with nothing active, does nothing.
    spool.seal().expect("second seal");
}

#[test]
fn the_size_cap_deletes_the_oldest_segment_and_counts_frames_and_bytes() {
    let dir = TestDir::new("spool-cap");
    let max_bytes = 100_000; // segments of 12,500 bytes: one block of 9,960 each
    let (mut spool, stats) = open(&dir, max_bytes);
    for block in 0..30 {
        assert_eq!(
            append_numbers(&mut spool, block * 5..block * 5 + 5, 1980),
            Appended::Stored
        );
        assert!(get(&stats.spool_bytes) <= max_bytes);
    }
    let dropped = get(&stats.spool_dropped);
    assert!(dropped > 0 && dropped.is_multiple_of(5));
    // Each evicted segment was sealed and never read: the whole file counts.
    assert_eq!(
        get(&stats.spool_dropped_bytes),
        dropped / 5 * (42 + 9_960 + 12)
    );

    let read = numbers(&drain(&mut spool, 1 << 20));
    assert_eq!(read, (dropped..150).collect::<Vec<_>>());
    assert_eq!(get(&stats.replayed) + dropped, 150);
}

#[test]
fn a_block_that_can_never_fit_is_refused_and_counted() {
    let dir = TestDir::new("spool-too-big");
    let (mut spool, stats) = open(&dir, 2_000);
    assert_eq!(append_numbers(&mut spool, 0..4, 1000), Appended::Dropped);
    assert_eq!(get(&stats.spool_dropped), 4);
    assert_eq!(get(&stats.spooled), 0);
    assert!(!spool.has_backlog());
    assert_eq!(all_files(dir.path()), Vec::<String>::new());
}

#[test]
fn an_empty_block_that_claims_frames_is_counted_as_dropped() {
    let dir = TestDir::new("spool-empty-block");
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(spool.append(&[], 0), Appended::Stored);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(spool.append(&[], 3), Appended::Dropped);
    assert_eq!(get(&stats.spool_dropped), 3);
}

#[test]
fn the_free_space_floor_deletes_the_oldest_segments_only_as_needed() {
    let dir = TestDir::new("spool-floor-evict");
    // Segments of 128 KiB, so each block of 100 frames takes a segment.
    let (mut spool, stats) = open_with(&dir, MIB, 0);
    for block in 0..4 {
        assert_eq!(
            append_numbers(&mut spool, block * 100..block * 100 + 100, 1000),
            Appended::Stored
        );
    }
    assert_eq!(segment_files(dir.path()).len(), 4);
    // The volume has 1,000,000 bytes minus 100,000 for each segment file. A new
    // segment needs 681,072 free: that takes 3 files, so one must go.
    set_free(1_000_000, 100_000);
    spool.config.min_free_bytes = 550_000;
    assert_eq!(append_numbers(&mut spool, 400..500, 1000), Appended::Stored);
    assert_eq!(get(&stats.spool_dropped), 100);
    assert_eq!(get(&stats.spool_dropped_bytes), 42 + 100 * 1012 + 12);
    assert_eq!(segment_files(dir.path()).len(), 4);
    let read = numbers(&drain(&mut spool, 1 << 20));
    assert_eq!(read, (100..500).collect::<Vec<_>>());
}

#[test]
fn the_free_space_floor_keeps_the_newest_segment_and_pauses() {
    let dir = TestDir::new("spool-floor-pause");
    let (mut spool, stats) = open_with(&dir, MIB, 1000);
    for block in 0..3 {
        assert_eq!(
            append_numbers(&mut spool, block * 100..block * 100 + 100, 1000),
            Appended::Stored
        );
    }
    // Free space stays at zero however many segments go: the first deletion
    // frees nothing, so the spool stops there instead of deleting everything.
    set_free(0, 0);
    assert_eq!(
        append_numbers(&mut spool, 300..400, 1000),
        Appended::Dropped
    );
    // One segment evicted (100 frames) plus the refused block (100 frames).
    assert_eq!(get(&stats.spool_dropped), 200);
    assert_eq!(get(&stats.spool_dropped_bytes), 42 + 100 * 1012 + 12);
    assert_eq!(get(&stats.spool_segments), 2);
    assert!(spool.has_backlog());
    // The pause clears `spool_available`, as an I/O error pause does.
    assert!(!stats.spool_available.load(Ordering::Relaxed));
    assert!(!spool.is_usable());

    // Paused: the next block is refused without looking at the volume.
    set_free(u64::MAX, 0);
    assert_eq!(
        append_numbers(&mut spool, 400..500, 1000),
        Appended::Dropped
    );
    assert_eq!(get(&stats.spool_dropped), 300);

    // After the pause the spool checks again and accepts.
    spool.blocked_until = Some(Instant::now());
    assert_eq!(append_numbers(&mut spool, 500..600, 1000), Appended::Stored);
    assert_eq!(get(&stats.spool_errors), 0);
    assert!(spool.is_usable(), "the spool is usable once the pause ends");
    assert!(stats.spool_available.load(Ordering::Relaxed));
    let read = numbers(&drain(&mut spool, 1 << 20));
    assert_eq!(
        read,
        (100..300).chain(500..600).collect::<Vec<_>>(),
        "the two newest segments and the new block survive"
    );
}

#[test]
fn a_single_segment_is_never_deleted_for_the_floor() {
    let dir = TestDir::new("spool-floor-one");
    let (mut spool, stats) = open_with(&dir, MIB, 1000);
    assert_eq!(append_numbers(&mut spool, 0..100, 1000), Appended::Stored);
    set_free(0, 0);
    assert_eq!(
        append_numbers(&mut spool, 100..200, 1000),
        Appended::Dropped
    );
    assert_eq!(get(&stats.spool_dropped), 100, "only the refused block");
    assert_eq!(get(&stats.spool_segments), 1);
}

#[test]
fn a_probe_error_counts_an_error_and_pauses_the_spool() {
    fn failing(_: &Path) -> io::Result<u64> {
        Err(io::Error::other("probe failed"))
    }
    let dir = TestDir::new("spool-probe-error");
    let stats = Arc::new(SinkStats::default());
    let mut spool = Spool::open(
        SpoolConfig {
            dir: dir.path().to_path_buf(),
            max_bytes: MIB,
            min_free_bytes: 0,
        },
        Arc::clone(&stats),
        failing,
    )
    .expect("spool opens");
    assert_eq!(append_numbers(&mut spool, 0..3, 10), Appended::Dropped);
    assert_eq!(get(&stats.spool_errors), 1);
    assert_eq!(get(&stats.spool_dropped), 3);
    assert!(!spool.is_usable());
    assert!(!stats.spool_available.load(Ordering::Relaxed));
}

#[test]
fn a_reopened_spool_replays_leftovers_from_the_cursor_and_writes_a_new_segment() {
    for seal_first in [true, false] {
        let dir = TestDir::new("spool-reopen");
        let (mut first, _stats) = open(&dir, MIB);
        // 6 blocks of 10 frames, about 50 KiB each: 3 segments.
        for block in 0..6 {
            assert_eq!(
                append_numbers(&mut first, block * 10..block * 10 + 10, 5000),
                Appended::Stored
            );
        }
        let mut out = Vec::new();
        let read_frames = first.read_batch(&mut out, 60_000).expect("read") as usize;
        let before = numbers(&out);
        assert_eq!(before.len(), read_frames);
        assert!(
            read_frames > 10 && read_frames < 20,
            "the cursor is inside a segment"
        );
        let old_sequences = segment_files(dir.path());
        if seal_first {
            first.seal().expect("seal");
        }
        drop(first);

        let (mut second, stats) = open(&dir, MIB);
        assert!(second.has_backlog());
        assert_eq!(
            append_numbers(&mut second, 100..110, 5000),
            Appended::Stored
        );
        let new_files = segment_files(dir.path());
        let newest = new_files.last().expect("a segment");
        assert!(newest > old_sequences.last().expect("old segment"));
        assert_eq!(new_files.len(), old_sequences.len() + 1);

        let after = numbers(&drain(&mut second, 1 << 20));
        let expected: Vec<u64> = (before.len() as u64..60).chain(100..110).collect();
        assert_eq!(after, expected, "seal_first = {seal_first}");
        assert_eq!(get(&stats.spool_dropped), 0);
        assert_eq!(get(&stats.spool_errors), 0);
        assert_eq!(all_files(dir.path()), Vec::<String>::new());
    }
}

#[test]
fn a_cut_off_final_frame_is_counted_and_skipped() {
    let dir = TestDir::new("spool-cutoff");
    let mut body = numbered(0..2, 0).0;
    body.extend_from_slice(&[0, 0, 0, 100, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    leftover_segment(&dir, 1, &body, false);
    leftover_segment(&dir, 2, &numbered(2..4, 0).0, true);
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2, 3]);
    assert_eq!(get(&stats.spool_dropped), 1);
    assert_eq!(get(&stats.spool_dropped_bytes), 14);
    assert_eq!(get(&stats.replayed), 4);
    assert_eq!(all_files(dir.path()), Vec::<String>::new());
}

#[test]
fn a_corrupt_length_counts_the_remaining_bytes_and_skips_the_segment() {
    let dir = TestDir::new("spool-corrupt");
    let mut body = numbered(0..2, 0).0;
    body.extend_from_slice(&[0x7f, 0xff, 0xff, 0xff]);
    body.extend_from_slice(&[1; 20]);
    leftover_segment(&dir, 1, &body, false);
    leftover_segment(&dir, 2, &numbered(2..3, 0).0, true);
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2]);
    // A leftover's frame count is unknown, so a corrupt tail adds only bytes.
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.spool_dropped_bytes), 24);
}

#[test]
fn lost_frames_are_not_counted_again_when_the_segment_is_evicted_later() {
    let dir = TestDir::new("spool-no-double-count");
    // Segment 1 holds 2 frames and a 14-byte cut-off tail (80 bytes); segment 2
    // holds 2 frames and STOP (78 bytes).
    let mut body = numbered(0..2, 0).0;
    body.extend_from_slice(&[0, 0, 0, 100, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    leftover_segment(&dir, 1, &body, false);
    leftover_segment(&dir, 2, &numbered(2..4, 0).0, true);
    let (mut spool, stats) = open(&dir, 200);
    let mut out = Vec::new();
    assert_eq!(spool.read_batch(&mut out, 1000).expect("read"), 2);
    assert_eq!(get(&stats.spool_dropped_bytes), 14);
    assert_eq!(get(&stats.spool_dropped), 1);
    // The next block needs room, so segment 1 (already ended) is deleted.
    assert_eq!(append_numbers(&mut spool, 9..10, 0), Appended::Stored);
    assert_eq!(get(&stats.spool_dropped_bytes), 14, "no second count");
    assert_eq!(get(&stats.spool_dropped), 1, "no second count");
    assert_eq!(numbers(&drain(&mut spool, 1000)), [2, 3, 9]);
}

#[test]
fn a_missing_start_deletes_the_segment_and_counts_its_bytes() {
    let dir = TestDir::new("spool-no-start");
    fs::write(dir.path().join(segment_name(1)), [0xee_u8; 100]).expect("junk segment");
    leftover_segment(&dir, 2, &numbered(7..9, 0).0, true);
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [7, 8]);
    assert_eq!(get(&stats.spool_dropped_bytes), 100);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(all_files(dir.path()), Vec::<String>::new());
}

#[test]
fn an_invalid_cursor_drops_the_oldest_segment() {
    let dir = TestDir::new("spool-bad-cursor");
    let first = leftover_segment(&dir, 1, &numbered(0..4, 0).0, true);
    leftover_segment(&dir, 2, &numbered(4..6, 0).0, true);
    fs::write(dir.path().join("cursor.bin"), [0xff_u8; 28]).expect("bad cursor");
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(get(&stats.spool_dropped_bytes), first);
    assert_eq!(segment_files(dir.path()).len(), 1);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [4, 5]);
    assert_eq!(all_files(dir.path()), Vec::<String>::new());
    assert!(matches!(read_cursor(&dir), CursorRead::Valid(_)));
}

#[test]
fn an_invalid_cursor_with_no_segments_is_rewritten() {
    let dir = TestDir::new("spool-bad-cursor-empty");
    // Longer than 28 bytes: it must not stay invalid at every start.
    fs::write(dir.path().join("cursor.bin"), [7_u8; 40]).expect("bad cursor");
    let (_spool, stats) = open(&dir, MIB);
    assert_eq!(get(&stats.spool_dropped_bytes), 0);
    assert_eq!(
        read_cursor(&dir),
        CursorRead::Valid(Cursor {
            sequence: 1,
            offset: START_LEN
        })
    );
    let length = fs::metadata(dir.path().join("cursor.bin"))
        .expect("cursor file")
        .len();
    assert_eq!(length, 28);
}

#[test]
fn a_cursor_that_names_a_missing_segment_starts_at_the_next_one() {
    let dir = TestDir::new("spool-missing-cursor");
    leftover_segment(&dir, 3, &numbered(0..4, 0).0, true);
    leftover_segment(&dir, 5, &numbered(4..6, 0).0, true);
    write_cursor(&dir, 4, START_LEN);
    let (mut spool, stats) = open(&dir, MIB);
    // Segment 3 is below the cursor, so it was read; segment 5 is unread.
    assert_eq!(segment_files(dir.path()), [segment_name(5)]);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [4, 5]);
    assert_eq!(get(&stats.spool_dropped_bytes), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}

#[test]
fn a_valid_cursor_resumes_inside_a_segment_and_discards_older_ones_silently() {
    let dir = TestDir::new("spool-good-cursor");
    leftover_segment(&dir, 1, &numbered(0..4, 0).0, true);
    leftover_segment(&dir, 2, &numbered(4..8, 0).0, true);
    leftover_segment(&dir, 3, &numbered(8..10, 0).0, true);
    // Segment 2 is read up to its second frame (12 bytes per frame).
    write_cursor(&dir, 2, START_LEN + 24);
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(segment_files(dir.path()).len(), 2);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [6, 7, 8, 9]);
    assert_eq!(get(&stats.spool_dropped_bytes), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}

#[test]
fn a_cursor_offset_inside_start_drops_its_segment_and_not_the_older_ones() {
    let dir = TestDir::new("spool-cursor-in-start");
    leftover_segment(&dir, 1, &numbered(0..2, 0).0, true);
    let second = leftover_segment(&dir, 2, &numbered(2..4, 0).0, true);
    leftover_segment(&dir, 3, &numbered(4..6, 0).0, true);
    write_cursor(&dir, 2, 10);
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(get(&stats.spool_dropped_bytes), second, "only segment 2");
    assert_eq!(numbers(&drain(&mut spool, 1000)), [4, 5]);
}

#[test]
fn a_cursor_with_no_segments_is_kept_and_sequences_stay_above_it() {
    let dir = TestDir::new("spool-stale-cursor");
    write_cursor(&dir, 7, START_LEN);
    let (mut spool, _stats) = open(&dir, MIB);
    assert!(!spool.has_backlog());
    assert!(matches!(read_cursor(&dir), CursorRead::Valid(_)));
    assert_eq!(append_numbers(&mut spool, 0..2, 10), Appended::Stored);
    let names = segment_files(dir.path());
    assert_eq!(names.len(), 1);
    assert!(names[0] > segment_name(7), "{names:?}");
}

#[test]
fn reading_to_the_writer_position_empties_the_spool_and_moves_the_cursor() {
    let dir = TestDir::new("spool-empty");
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(append_numbers(&mut spool, 0..5, 10), Appended::Stored);
    let mut out = Vec::new();
    assert_eq!(spool.read_batch(&mut out, 1 << 20).expect("read"), 5);
    assert_eq!(numbers(&out), [0, 1, 2, 3, 4]);
    // The cursor file exists while data may remain.
    assert!(dir.path().join("cursor.bin").exists());
    assert_eq!(spool.read_batch(&mut out, 1 << 20).expect("read"), 0);
    assert!(!spool.has_backlog());
    assert_eq!(all_files(dir.path()), Vec::<String>::new());
    // The cursor stays, set to the next sequence.
    assert_eq!(
        read_cursor(&dir),
        CursorRead::Valid(Cursor {
            sequence: 2,
            offset: START_LEN
        })
    );
    assert_eq!(get(&stats.spool_bytes), 0);
    assert_eq!(get(&stats.spool_segments), 0);

    // New writes start a fresh segment and work as before.
    assert_eq!(append_numbers(&mut spool, 5..8, 10), Appended::Stored);
    assert_eq!(segment_files(dir.path()), [segment_name(2)]);
    assert_eq!(numbers(&drain(&mut spool, 1 << 20)), [5, 6, 7]);
    assert_eq!(get(&stats.replayed), 8);
}

#[test]
fn sequences_keep_growing_after_the_spool_empties_and_is_reopened() {
    let dir = TestDir::new("spool-sequences");
    let (mut spool, _stats) = open(&dir, MIB);
    let mut used = Vec::new();
    for round in 0..3 {
        assert_eq!(
            append_numbers(&mut spool, round * 2..round * 2 + 2, 10),
            Appended::Stored
        );
        used.extend(segment_files(dir.path()));
        assert_eq!(
            numbers(&drain(&mut spool, 1000)),
            [round * 2, round * 2 + 1]
        );
    }
    drop(spool);
    let (mut reopened, _reopened_stats) = open(&dir, MIB);
    assert_eq!(append_numbers(&mut reopened, 10..12, 10), Appended::Stored);
    let newest = segment_files(dir.path());
    assert_eq!(newest.len(), 1);
    assert!(
        used.iter().all(|earlier| *earlier < newest[0]),
        "{used:?} {newest:?}"
    );
}

#[test]
fn appending_while_reading_keeps_frames_in_order() {
    let dir = TestDir::new("spool-interleave");
    let (mut spool, _stats) = open(&dir, MIB);
    let mut seen = Vec::new();
    let mut next = 0;
    for _ in 0..30 {
        assert_eq!(
            append_numbers(&mut spool, next..next + 7, 3000),
            Appended::Stored
        );
        next += 7;
        let mut out = Vec::new();
        spool.read_batch(&mut out, 20_000).expect("read");
        seen.extend(numbers(&out));
    }
    seen.extend(numbers(&drain(&mut spool, 20_000)));
    assert_eq!(seen, (0..next).collect::<Vec<_>>());
}

#[test]
fn a_leftover_segment_evicted_for_the_cap_adds_bytes_but_no_frames() {
    let dir = TestDir::new("spool-leftover-evict");
    let size = leftover_segment(&dir, 1, &numbered(0..3, 300).0, true);
    leftover_segment(&dir, 2, &numbered(3..6, 300).0, true);
    // Two leftovers fill 1,980 of 2,200 bytes: a new block needs one gone.
    let (mut spool, stats) = open(&dir, 2_200);
    assert_eq!(append_numbers(&mut spool, 6..7, 300), Appended::Stored);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.spool_dropped_bytes), size);
    assert_eq!(numbers(&drain(&mut spool, 1 << 20)), [3, 4, 5, 6]);
}

#[test]
fn a_write_error_counts_pauses_and_recovers() {
    let dir = TestDir::new("spool-io-error");
    let (mut spool, stats) = open(&dir, MIB);
    assert!(spool.is_usable());
    fs::remove_dir_all(dir.path()).expect("folder is removed");

    assert_eq!(append_numbers(&mut spool, 0..3, 10), Appended::Dropped);
    assert_eq!(get(&stats.spool_errors), 1);
    assert_eq!(get(&stats.spool_dropped), 3);
    assert!(!spool.is_usable());
    assert!(!stats.spool_available.load(Ordering::Relaxed));

    // While paused the spool refuses without another error.
    fs::create_dir_all(dir.path()).expect("folder is back");
    assert_eq!(append_numbers(&mut spool, 3..6, 10), Appended::Dropped);
    assert_eq!(get(&stats.spool_errors), 1);
    assert_eq!(get(&stats.spool_dropped), 6);

    spool.suspended_until = Some(Instant::now());
    assert!(spool.is_usable());
    assert!(stats.spool_available.load(Ordering::Relaxed));
    assert_eq!(append_numbers(&mut spool, 6..9, 10), Appended::Stored);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [6, 7, 8]);
}

#[test]
fn a_closed_active_segment_stays_readable_and_new_writes_start_a_segment() {
    let dir = TestDir::new("spool-closed-active");
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(append_numbers(&mut spool, 0..3, 10), Appended::Stored);
    // What the spool does after a write error: drop the writer, keep the segment.
    spool.writer = None;
    assert_eq!(append_numbers(&mut spool, 3..5, 10), Appended::Stored);
    assert_eq!(segment_files(dir.path()).len(), 2);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2, 3, 4]);
    assert_eq!(get(&stats.spool_errors), 0);
    assert_eq!(all_files(dir.path()), Vec::<String>::new());
}

#[test]
fn files_that_are_not_segments_are_ignored_and_new_names_go_past_them() {
    let dir = TestDir::new("spool-strangers");
    fs::write(dir.path().join("notes.txt"), b"keep").expect("file");
    fs::write(dir.path().join("seg-1.fstrm"), b"keep").expect("short number");
    fs::write(
        dir.path().join("seg-00000000000000000002.fstrm.bak"),
        b"keep",
    )
    .expect("suffix");
    // A folder named like a segment is never read, but its sequence is taken.
    fs::create_dir(dir.path().join(segment_name(5))).expect("folder");
    leftover_segment(&dir, 3, &numbered(0..2, 0).0, true);
    // A segment whose name is in upper case is still a segment.
    let upper = segment_name(7).to_ascii_uppercase();
    let mut bytes = start_bytes();
    bytes.extend_from_slice(&numbered(10..12, 0).0);
    bytes.extend_from_slice(&STOP);
    fs::write(dir.path().join(&upper), bytes).expect("upper-case segment");

    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(get(&stats.spool_segments), 2);
    assert_eq!(append_numbers(&mut spool, 20..22, 0), Appended::Stored);
    assert!(
        dir.path().join(segment_name(8)).is_file(),
        "the new segment goes past the highest name, whatever its type"
    );
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 10, 11, 20, 21]);
    assert_eq!(
        all_files(dir.path()),
        [
            "notes.txt",
            "seg-00000000000000000002.fstrm.bak",
            "seg-00000000000000000005.fstrm",
            "seg-1.fstrm"
        ]
    );
}

#[test]
fn a_taken_segment_name_is_skipped() {
    let dir = TestDir::new("spool-collision");
    let (mut spool, _stats) = open(&dir, MIB);
    // Names appear after the scan: the spool must try the next one.
    fs::create_dir(dir.path().join(segment_name(1))).expect("folder");
    fs::create_dir(dir.path().join(segment_name(2))).expect("folder");
    assert_eq!(append_numbers(&mut spool, 0..2, 0), Appended::Stored);
    assert!(dir.path().join(segment_name(3)).is_file());
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1]);
}

#[test]
fn too_many_taken_names_is_an_error_not_a_stall() {
    let dir = TestDir::new("spool-collisions");
    let (mut spool, stats) = open(&dir, MIB);
    for sequence in 1..=8 {
        fs::create_dir(dir.path().join(segment_name(sequence))).expect("folder");
    }
    assert_eq!(append_numbers(&mut spool, 0..2, 0), Appended::Dropped);
    assert_eq!(get(&stats.spool_errors), 1);
    assert_eq!(get(&stats.spool_dropped), 2);
    // The numbers were used up, so a later try goes past them.
    spool.suspended_until = None;
    assert_eq!(append_numbers(&mut spool, 2..4, 0), Appended::Stored);
    assert!(dir.path().join(segment_name(9)).is_file());
}

#[test]
fn running_out_of_sequence_numbers_is_an_error_not_an_overflow() {
    let dir = TestDir::new("spool-sequence-end");
    leftover_segment(&dir, MAX_SEQUENCE, &numbered(0..1, 0).0, true);
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(append_numbers(&mut spool, 1..2, 0), Appended::Dropped);
    assert_eq!(get(&stats.spool_errors), 1);
    assert_eq!(
        numbers(&drain(&mut spool, 1000)),
        [0],
        "leftovers still replay"
    );
}

#[test]
fn opening_a_missing_folder_is_an_error() {
    let dir = TestDir::new("spool-no-folder");
    let missing = dir.path().join("absent");
    let result = Spool::open(
        SpoolConfig {
            dir: missing,
            max_bytes: MIB,
            min_free_bytes: 0,
        },
        Arc::new(SinkStats::default()),
        fake_free,
    );
    assert!(result.is_err());
}

#[test]
fn segment_names_need_exactly_twenty_digits_and_a_sequence_in_range() {
    assert_eq!(parse_name("seg-00000000000000000042.fstrm"), Some(42));
    assert_eq!(parse_name("SEG-00000000000000000042.FSTRM"), Some(42));
    assert_eq!(parse_name("Seg-00000000000000000042.Fstrm"), Some(42));
    assert_eq!(parse_name("seg-0000000000000000042.fstrm"), None);
    assert_eq!(parse_name("seg-000000000000000000042.fstrm"), None);
    assert_eq!(parse_name("seg-0000000000000000004a.fstrm"), None);
    assert_eq!(parse_name("seg-00000000000000000042.fstrm "), None);
    assert_eq!(parse_name("cursor.bin"), None);
    assert_eq!(parse_name("seg-é0000000000000000042.fstrm"), None);
    assert_eq!(parse_name(""), None);
    assert_eq!(
        parse_name("seg-09999999999999999999.fstrm"),
        Some(9_999_999_999_999_999_999)
    );
    assert_eq!(parse_name("seg-10000000000000000000.fstrm"), None);
    assert_eq!(parse_name("seg-18446744073709551615.fstrm"), None);
    assert_eq!(parse_name("seg-18446744073709551616.fstrm"), None);
}

#[test]
fn the_cursor_file_round_trips_and_detects_damage() {
    let dir = TestDir::new("spool-cursor-file");
    assert_eq!(cursor::read(dir.path()).expect("read"), CursorRead::Missing);
    let cursor = Cursor {
        sequence: 0x0102_0304_0506_0708,
        offset: 123_456,
    };
    let mut file = CursorFile::new(dir.path());
    file.write(cursor).expect("write");
    file.write(cursor).expect("overwrite in place");
    let bytes = fs::read(dir.path().join(cursor::FILE_NAME)).expect("file");
    assert_eq!(bytes.len(), 28);
    assert_eq!(&bytes[..4], b"ETDC");
    assert_eq!(
        cursor::read(dir.path()).expect("read"),
        CursorRead::Valid(cursor)
    );

    // Each single-bit change is caught by the magic or the check value.
    for index in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[index] ^= 0x01;
        fs::write(dir.path().join(cursor::FILE_NAME), &damaged).expect("damage");
        assert_eq!(
            cursor::read(dir.path()).expect("read"),
            CursorRead::Invalid,
            "byte {index}"
        );
    }
    let mut longer = bytes;
    longer.push(0);
    fs::write(dir.path().join(cursor::FILE_NAME), &longer).expect("longer");
    assert_eq!(cursor::read(dir.path()).expect("read"), CursorRead::Invalid);
    fs::write(dir.path().join(cursor::FILE_NAME), b"").expect("empty");
    assert_eq!(cursor::read(dir.path()).expect("read"), CursorRead::Invalid);

    // Writing through a long, damaged file cuts it to 28 bytes.
    let mut fresh = CursorFile::new(dir.path());
    fresh.write(cursor).expect("write over a damaged file");
    assert_eq!(
        cursor::read(dir.path()).expect("read"),
        CursorRead::Valid(cursor)
    );
}

#[test]
fn a_folder_named_like_the_cursor_file_is_a_foreign_object() {
    let dir = TestDir::new("spool-cursor-folder");
    fs::create_dir(dir.path().join(cursor::FILE_NAME)).expect("folder");
    assert_eq!(cursor::read(dir.path()).expect("read"), CursorRead::Invalid);
    let error = CursorFile::new(dir.path())
        .write(Cursor {
            sequence: 1,
            offset: START_LEN,
        })
        .expect_err("a folder cannot be written");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

// ---- failure handling ----

#[test]
fn a_segment_that_cannot_be_deleted_is_not_sent_twice() {
    let dir = TestDir::new("spool-stray");
    let first = leftover_segment(&dir, 1, &numbered(0..3, 0).0, true);
    leftover_segment(&dir, 2, &numbered(3..6, 0).0, true);
    // Another process holds segment 1 open without delete sharing.
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_NO_DELETE)
        .open(dir.path().join(segment_name(1)))
        .expect("segment is held");
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2, 3, 4, 5]);
    assert!(get(&stats.spool_errors) >= 1, "the delete failed");
    assert_eq!(segment_files(dir.path()), [segment_name(1)], "the stray");
    assert_eq!(get(&stats.spool_bytes), first, "the stray still counts");
    assert_eq!(get(&stats.spool_segments), 1);
    assert!(!spool.has_backlog());

    // Once the other process lets go, the next idle read removes it.
    drop(hold);
    let mut out = Vec::new();
    assert_eq!(spool.read_batch(&mut out, 1000).expect("read"), 0);
    assert_eq!(segment_files(dir.path()), Vec::<String>::new());
    assert_eq!(get(&stats.spool_bytes), 0);
    assert_eq!(get(&stats.spool_segments), 0);
}

#[test]
fn a_stray_segment_is_removed_at_the_next_start_and_never_replayed() {
    let dir = TestDir::new("spool-stray-reopen");
    leftover_segment(&dir, 1, &numbered(0..3, 0).0, true);
    leftover_segment(&dir, 2, &numbered(3..6, 0).0, true);
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_NO_DELETE)
        .open(dir.path().join(segment_name(1)))
        .expect("segment is held");
    let (mut spool, _stats) = open(&dir, MIB);
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2, 3, 4, 5]);
    // The service stops while the stray is still there.
    drop(spool);
    drop(hold);
    assert_eq!(segment_files(dir.path()), [segment_name(1)]);

    let (mut again, stats) = open(&dir, MIB);
    assert_eq!(
        segment_files(dir.path()),
        Vec::<String>::new(),
        "the stray is below the cursor and goes"
    );
    assert!(!again.has_backlog());
    assert!(drain(&mut again, 1000).is_empty(), "no frame is read twice");
    assert_eq!(get(&stats.replayed), 0);
    assert_eq!(get(&stats.spool_dropped_bytes), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}

#[test]
fn a_sharing_violation_keeps_the_segment_and_pauses_reads() {
    let dir = TestDir::new("spool-sharing");
    let size = leftover_segment(&dir, 1, &numbered(0..3, 0).0, true);
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.path().join(segment_name(1)))
        .expect("segment is held");
    let (mut spool, stats) = open(&dir, MIB);
    let mut out = Vec::new();
    let error = spool
        .read_batch(&mut out, 1000)
        .expect_err("the file is in use");
    assert_eq!(error.raw_os_error(), Some(32));
    assert_eq!(get(&stats.spool_errors), 1);
    assert!(spool.has_backlog());
    assert_eq!(get(&stats.spool_dropped_bytes), 0);

    // Reads are paused: no busy loop, no second error.
    assert_eq!(spool.read_batch(&mut out, 1000).expect("paused"), 0);
    assert_eq!(get(&stats.spool_errors), 1);

    // The other process lets go: the segment is read in full.
    drop(hold);
    spool.read_paused_until = Some(Instant::now());
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2]);
    assert_eq!(get(&stats.spool_dropped_bytes), 0);
    assert_eq!(get(&stats.replayed), 3);
    assert!(size > 0);
}

#[test]
fn three_sharing_violations_in_a_row_delete_the_segment_and_count_it() {
    let dir = TestDir::new("spool-sharing-three");
    let size = leftover_segment(&dir, 1, &numbered(0..3, 0).0, true);
    leftover_segment(&dir, 2, &numbered(3..5, 0).0, true);
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.path().join(segment_name(1)))
        .expect("segment is held");
    let (mut spool, stats) = open(&dir, MIB);
    let mut out = Vec::new();
    for attempt in 1..=3 {
        spool.read_paused_until = None;
        let error = spool
            .read_batch(&mut out, 1000)
            .expect_err("the file is in use");
        assert_eq!(error.raw_os_error(), Some(32), "attempt {attempt}");
        assert_eq!(get(&stats.spool_errors), attempt + u64::from(attempt == 3));
    }
    // The third failure deleted the segment (the delete fails too, so the file
    // is a stray) and counted its bytes; segment 2 is next.
    assert_eq!(get(&stats.spool_dropped_bytes), size);
    spool.read_paused_until = None;
    assert_eq!(numbers(&drain(&mut spool, 1000)), [3, 4]);
    drop(hold);
}

#[test]
fn a_cursor_write_failure_consumes_nothing_and_pauses_reads() {
    let dir = TestDir::new("spool-cursor-failure");
    let (mut spool, stats) = open(&dir, MIB);
    assert_eq!(append_numbers(&mut spool, 0..5, 10), Appended::Stored);
    // A folder where the cursor file belongs: the write cannot succeed.
    fs::create_dir(dir.path().join(cursor::FILE_NAME)).expect("folder");
    let mut out = vec![0xaa];
    let error = spool
        .read_batch(&mut out, 1000)
        .expect_err("the cursor cannot be saved");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(out, [0xaa], "the bytes were handed back");
    assert_eq!(get(&stats.replayed), 0);
    assert_eq!(get(&stats.spool_errors), 1);

    // Reads are paused, while appends go on.
    assert_eq!(spool.read_batch(&mut out, 1000).expect("paused"), 0);
    assert_eq!(get(&stats.spool_errors), 1);
    assert_eq!(append_numbers(&mut spool, 5..6, 10), Appended::Stored);

    // After the cursor file is back, every frame comes out once, in order.
    fs::remove_dir(dir.path().join(cursor::FILE_NAME)).expect("folder is removed");
    spool.read_paused_until = Some(Instant::now());
    assert_eq!(numbers(&drain(&mut spool, 1000)), [0, 1, 2, 3, 4, 5]);
    assert_eq!(get(&stats.replayed), 6);
}

// ---- fsinfo ----

/// `S-1-5-21-1-2-3-1001`, a local user.
const USER_SID: [u8; 28] = [
    1, 5, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0xe9, 3, 0, 0,
];
const SYSTEM_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
const ADMINISTRATORS_SID: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 0x02, 0, 0];
/// `S-1-5-32-545`, BUILTIN\Users.
const USERS_SID: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x21, 0x02, 0, 0];
/// `S-1-1-0`, Everyone.
const EVERYONE_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
/// `S-1-3-0`, CREATOR OWNER.
const CREATOR_OWNER_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];
/// `S-1-5-80-1-2-3-4-5`, a service SID such as `NT SERVICE\EtwDnstap`.
const SERVICE_SID: [u8; 32] = [
    1, 6, 0, 0, 0, 0, 0, 5, 80, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0,
];

fn entry(ace_type: u8, flags: u8, mask: u32, sid: &[u8]) -> fsinfo::AceEntry {
    fsinfo::AceEntry {
        ace_type,
        flags,
        mask,
        sid: sid.to_vec(),
    }
}

#[test]
fn only_system_and_administrators_are_trusted_owners() {
    assert!(fsinfo::owner_is_trusted(&SYSTEM_SID));
    assert!(fsinfo::owner_is_trusted(&ADMINISTRATORS_SID));
    assert!(!fsinfo::owner_is_trusted(&USER_SID));
    // S-1-5-19 (local service) and S-1-1-0 (Everyone) are not trusted.
    assert!(!fsinfo::owner_is_trusted(&[
        1, 1, 0, 0, 0, 0, 0, 5, 19, 0, 0, 0
    ]));
    assert!(!fsinfo::owner_is_trusted(&EVERYONE_SID));
    // Truncated, extended, and empty SIDs are not trusted.
    assert!(!fsinfo::owner_is_trusted(&SYSTEM_SID[..11]));
    let mut longer = SYSTEM_SID.to_vec();
    longer.push(0);
    assert!(!fsinfo::owner_is_trusted(&longer));
    assert!(!fsinfo::owner_is_trusted(&[]));
}

#[test]
fn the_installer_acl_is_safe() {
    // Owner Administrators, no inherited entries: full control for SYSTEM and
    // Administrators, Modify for the service SID.
    let acl = [
        entry(0, 0x03, 0x001f_01ff, &SYSTEM_SID),
        entry(0, 0x03, 0x001f_01ff, &ADMINISTRATORS_SID),
        entry(0, 0x03, 0x0013_01bf, &SERVICE_SID),
    ];
    assert!(fsinfo::dacl_is_safe(&acl));
}

#[test]
fn an_inherited_users_add_file_entry_is_not_safe() {
    let mut acl = vec![entry(0, 0x03, 0x001f_01ff, &SYSTEM_SID)];
    // FILE_ADD_FILE, inherited from the parent.
    acl.push(entry(0, 0x10, 0x0000_0002, &USERS_SID));
    assert!(!fsinfo::dacl_is_safe(&acl));
    // FILE_ADD_SUBDIRECTORY alone is enough.
    assert!(!fsinfo::dacl_is_safe(&[entry(0, 0x10, 0x4, &USERS_SID)]));
}

#[test]
fn an_everyone_generic_write_entry_is_not_safe() {
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0,
        0x4000_0000,
        &EVERYONE_SID
    )]));
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0,
        0x1000_0000,
        &EVERYONE_SID
    )]));
}

#[test]
fn each_protected_right_is_refused_for_an_ordinary_account() {
    // FILE_READ_DATA, FILE_WRITE_DATA, FILE_APPEND_DATA, FILE_DELETE_CHILD,
    // DELETE, WRITE_DAC, WRITE_OWNER, GENERIC_WRITE, GENERIC_ALL, GENERIC_READ.
    for mask in [
        0x1_u32,
        0x8000_0000,
        0x2,
        0x4,
        0x40,
        0x1_0000,
        0x4_0000,
        0x8_0000,
        0x4000_0000,
        0x1000_0000,
    ] {
        assert!(
            !fsinfo::ace_is_safe(0, 0, mask, &USER_SID),
            "mask {mask:#x}"
        );
        assert!(
            fsinfo::ace_is_safe(0, 0, mask, &SYSTEM_SID),
            "mask {mask:#x}"
        );
        assert!(
            fsinfo::ace_is_safe(0, 0, mask, &SERVICE_SID),
            "mask {mask:#x}"
        );
    }
}

#[test]
fn an_inherit_only_creator_owner_entry_is_safe() {
    // CONTAINER_INHERIT | OBJECT_INHERIT | INHERIT_ONLY, full control.
    assert!(fsinfo::dacl_is_safe(&[entry(
        0,
        0x0b,
        0x001f_01ff,
        &CREATOR_OWNER_SID
    )]));
}

/// `S-1-5-11`, Authenticated Users.
const AUTHENTICATED_USERS_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 11, 0, 0, 0];

#[test]
fn the_installer_acl_with_inheritance_flags_is_safe() {
    // SYSTEM F (OI)(CI), Administrators F (OI)(CI), service SID Modify (OI)(CI).
    let acl = [
        entry(0, 0x03, 0x001f_01ff, &SYSTEM_SID),
        entry(0, 0x03, 0x001f_01ff, &ADMINISTRATORS_SID),
        entry(0, 0x03, 0x0013_01bf, &SERVICE_SID),
    ];
    assert!(fsinfo::dacl_is_safe(&acl));
}

#[test]
fn users_and_authenticated_users_cannot_read_or_write_the_folder() {
    // BUILTIN\Users read and execute, inherited, not inherit-only.
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0x10,
        0x0012_00a9,
        &USERS_SID
    )]));
    // (OI)(IO) Users read: it decides who reads the segment files.
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0x09,
        0x0012_0089,
        &USERS_SID
    )]));
    // (OI)(CI)(IO) Users write, and (OI)(IO) add-file.
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0x0b,
        0x4000_0000,
        &USERS_SID
    )]));
    assert!(!fsinfo::dacl_is_safe(&[entry(0, 0x09, 0x2, &USERS_SID)]));
    // Authenticated Users read, and GENERIC_READ.
    let authenticated = &AUTHENTICATED_USERS_SID;
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0,
        0x0012_0089,
        authenticated
    )]));
    assert!(!fsinfo::dacl_is_safe(&[entry(
        0,
        0,
        0x8000_0000,
        authenticated
    )]));
}

#[test]
fn creator_owner_is_allowed_only_on_an_inherit_only_entry() {
    assert!(fsinfo::ace_is_safe(
        0,
        0x0b,
        0x001f_01ff,
        &CREATOR_OWNER_SID
    ));
    assert!(!fsinfo::ace_is_safe(
        0,
        0x03,
        0x001f_01ff,
        &CREATOR_OWNER_SID
    ));
    // Inherit-only entries for other accounts get no exception.
    assert!(!fsinfo::ace_is_safe(0, 0x0b, 0x001f_01ff, &EVERYONE_SID));
    assert!(fsinfo::ace_is_safe(0, 0x0b, 0x001f_01ff, &SERVICE_SID));
}

#[test]
fn read_only_entries_deny_entries_and_unknown_entry_types() {
    // Read and run (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE) includes
    // FILE_READ_DATA, which the spool keeps from ordinary accounts.
    assert!(!fsinfo::ace_is_safe(0, 0x10, 0x0012_00a9, &USERS_SID));
    // SYNCHRONIZE and FILE_READ_ATTRIBUTES alone read no data.
    assert!(fsinfo::ace_is_safe(0, 0x10, 0x0010_0080, &USERS_SID));
    // A deny entry never grants access, even with every right set.
    assert!(fsinfo::ace_is_safe(1, 0, 0xffff_ffff, &EVERYONE_SID));
    // Object, callback, and other entry types are refused.
    for ace_type in [2_u8, 4, 5, 6, 9, 11, 17] {
        assert!(
            !fsinfo::ace_is_safe(ace_type, 0, 0, &SYSTEM_SID),
            "{ace_type}"
        );
    }
    // A SID that is neither trusted nor a service SID, or is malformed.
    assert!(!fsinfo::ace_is_safe(0, 0, 0x2, &[]));
    assert!(!fsinfo::ace_is_safe(0, 0, 0x2, &SERVICE_SID[..31]));
    // S-1-5-80 with one sub-authority is not a service SID.
    assert!(!fsinfo::ace_is_safe(
        0,
        0,
        0x2,
        &[1, 1, 0, 0, 0, 0, 0, 5, 80, 0, 0, 0]
    ));
    assert!(fsinfo::dacl_is_safe(&[]), "an empty ACL grants nothing");
}

#[test]
fn the_acl_of_a_real_folder_is_read_without_error() {
    let dir = TestDir::new("spool-real-acl");
    let security = fsinfo::read_security(dir.path()).expect("security is readable");
    assert_eq!(security.owner[0], 1, "SID revision");
    let entries = security.dacl.expect("a temp folder has an ACL");
    assert!(entries.len() > 1);
    for entry in &entries {
        assert!(matches!(entry.ace_type, 0 | 1), "{entry:?}");
        assert!(entry.sid.len() >= 12, "{entry:?}");
        assert_eq!(entry.sid.len(), 8 + 4 * usize::from(entry.sid[1]));
    }
    // Reading is repeatable and the rule runs on real data.
    let again = fsinfo::read_security(dir.path()).expect("security is readable");
    assert_eq!(again.dacl.as_deref(), Some(entries.as_slice()));
    let _ = fsinfo::dacl_is_safe(&entries);
}

#[test]
fn free_space_of_the_temp_folder_is_reported() {
    let dir = TestDir::new("spool-free-space");
    let free = fsinfo::available_bytes(dir.path()).expect("free space is available");
    assert!(free > 0);
    assert!(fsinfo::available_bytes(&dir.path().join("absent")).is_err());
}

#[test]
fn a_path_with_a_nul_is_not_passed_to_win32() {
    let error = fsinfo::available_bytes(Path::new("C:\\a\0b")).expect_err("NUL is refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn the_owner_of_a_folder_is_a_valid_sid() {
    let dir = TestDir::new("spool-owner");
    let sid = fsinfo::owner_sid(dir.path()).expect("owner is readable");
    assert!((8..=68).contains(&sid.len()));
    assert_eq!(sid[0], 1, "SID revision");
    assert_eq!(sid.len(), 8 + 4 * usize::from(sid[1]));
    assert!(fsinfo::owner_sid(&dir.path().join("absent")).is_err());
}

#[test]
fn the_spool_folder_check_refuses_unsafe_paths() {
    let dir = TestDir::new("spool-check");
    assert!(matches!(
        fsinfo::check_spool_dir(Path::new("relative\\spool")),
        Err(fsinfo::SpoolDirError::NotAbsolute)
    ));
    for dotted in [
        "C:\\Temp\\..\\spool",
        "C:\\Temp\\.\\spool",
        "C:/Temp/../spool",
    ] {
        assert!(
            matches!(
                fsinfo::check_spool_dir(Path::new(dotted)),
                Err(fsinfo::SpoolDirError::DotComponent)
            ),
            "{dotted}"
        );
    }
    assert!(matches!(
        fsinfo::check_spool_dir(&dir.path().join("absent")),
        Err(fsinfo::SpoolDirError::Missing)
    ));
    let file = dir.path().join("file");
    fs::write(&file, b"x").expect("file");
    assert!(matches!(
        fsinfo::check_spool_dir(&file),
        Err(fsinfo::SpoolDirError::NotAFolder)
    ));
    // The owner and ACL of a folder made by this account depend on the account
    // and the temp folder's inheritance, so the real check may accept or refuse
    // it for the owner or the ACL, but never for another reason.
    assert!(matches!(
        fsinfo::check_spool_dir(dir.path()),
        Ok(()) | Err(fsinfo::SpoolDirError::UntrustedOwner | fsinfo::SpoolDirError::UnsafeDacl)
    ));
}

#[test]
fn path_shape_accepts_only_local_drive_paths() {
    for good in [r"C:\Spool", r"d:\a\b", r"\\?\C:\Spool"] {
        assert!(fsinfo::check_path_shape(Path::new(good)).is_ok(), "{good}");
    }
    for bad in [
        r"\\host\share\spool",
        r"\\?\UNC\host\share\spool",
        r"\\.\C:\spool",
        r"\\?\GLOBALROOT\Device\Spool",
    ] {
        assert!(
            matches!(
                fsinfo::check_path_shape(Path::new(bad)),
                Err(fsinfo::SpoolDirError::NotLocalDisk)
            ),
            "{bad}"
        );
        // The runtime check refuses the same paths before any file system call.
        assert!(
            matches!(
                fsinfo::check_spool_dir(Path::new(bad)),
                Err(fsinfo::SpoolDirError::NotLocalDisk)
            ),
            "{bad}"
        );
    }
}

#[test]
fn only_a_fixed_disk_is_an_allowed_drive_type() {
    // GetDriveTypeW codes: 0 unknown, 1 no root directory, 2 removable, 3 fixed,
    // 4 remote, 5 CD-ROM, 6 RAM disk.
    let allowed: Vec<u32> = (0..=7)
        .filter(|&code| fsinfo::drive_type_is_allowed(code))
        .collect();
    assert_eq!(allowed, [3]);
}

#[test]
fn the_free_space_floor_pause_clears_spool_available() {
    let dir = TestDir::new("spool-floor-flag");
    let (mut spool, stats) = open(&dir, MIB);
    assert!(spool.is_usable());
    assert!(stats.spool_available.load(Ordering::Relaxed));
    spool.blocked_until = Some(Instant::now() + Duration::from_secs(60));
    assert!(!spool.is_usable());
    assert!(!stats.spool_available.load(Ordering::Relaxed));
    spool.blocked_until = Some(Instant::now());
    assert!(spool.is_usable());
    assert!(stats.spool_available.load(Ordering::Relaxed));
}

// ---- log output ----

const DISCARD_MESSAGE: &str = "discarded unreadable segment data";
const EVICTION_MESSAGE: &str = "deleted its oldest segment unsent";

/// Collects log output so a test can count messages.
#[derive(Clone, Default)]
struct LogLines(Arc<Mutex<Vec<u8>>>);

impl LogLines {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log lock")).into_owned()
    }

    fn count(&self, needle: &str) -> usize {
        self.text().matches(needle).count()
    }
}

impl io::Write for LogLines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogLines {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Runs `work` with warnings captured.
fn capture_logs(work: impl FnOnce()) -> LogLines {
    let lines = LogLines::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(lines.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::with_default(subscriber, work);
    lines
}

#[test]
fn discarded_unreadable_data_is_logged_four_times_and_then_rarely() {
    let dir = TestDir::new("spool-discard-log");
    for sequence in 1..=6 {
        let mut body = numbered(0..1, 0).0;
        body.extend_from_slice(&[0x7f, 0xff, 0xff, 0xff, 1, 2, 3]);
        leftover_segment(&dir, sequence, &body, false);
    }
    let lines = capture_logs(|| {
        let (mut spool, stats) = open(&dir, MIB);
        assert_eq!(drain(&mut spool, 1000).len(), 6 * 12);
        assert_eq!(get(&stats.spool_dropped_bytes), 6 * 7);
    });
    assert_eq!(lines.count(DISCARD_MESSAGE), 4, "{}", lines.text());
    assert!(lines.text().contains("bytes=7"), "{}", lines.text());
    assert_eq!(lines.count(EVICTION_MESSAGE), 0);
}

#[test]
fn a_bad_start_and_an_invalid_cursor_are_logged_as_discards() {
    let dir = TestDir::new("spool-discard-start");
    fs::write(dir.path().join(segment_name(1)), [0xee_u8; 100]).expect("junk segment");
    let lines = capture_logs(|| {
        let (mut spool, _stats) = open(&dir, MIB);
        assert_eq!(drain(&mut spool, 1000), Vec::<u8>::new());
    });
    assert_eq!(lines.count(DISCARD_MESSAGE), 1, "{}", lines.text());
    assert!(lines.text().contains("bytes=100"), "{}", lines.text());

    let other = TestDir::new("spool-discard-cursor");
    leftover_segment(&other, 1, &numbered(0..2, 0).0, true);
    fs::write(other.path().join("cursor.bin"), [0xff_u8; 28]).expect("bad cursor");
    let cursor_lines = capture_logs(|| {
        let (_spool, _stats) = open(&other, MIB);
    });
    assert_eq!(
        cursor_lines.count(DISCARD_MESSAGE),
        1,
        "{}",
        cursor_lines.text()
    );
}

#[test]
fn a_segment_deleted_after_repeated_sharing_violations_is_logged_as_a_discard() {
    let dir = TestDir::new("spool-discard-sharing");
    leftover_segment(&dir, 1, &numbered(0..3, 0).0, true);
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.path().join(segment_name(1)))
        .expect("segment is held");
    let lines = capture_logs(|| {
        let (mut spool, _stats) = open(&dir, MIB);
        let mut out = Vec::new();
        for _ in 0..3 {
            spool.read_paused_until = None;
            spool.read_batch(&mut out, 1000).expect_err("in use");
        }
    });
    drop(hold);
    assert_eq!(lines.count(DISCARD_MESSAGE), 1, "{}", lines.text());
}

#[test]
fn an_eviction_for_space_is_not_logged_as_a_discard() {
    let dir = TestDir::new("spool-eviction-log");
    let lines = capture_logs(|| {
        let (mut spool, stats) = open(&dir, 100_000);
        for block in 0..30 {
            assert_eq!(
                append_numbers(&mut spool, block * 5..block * 5 + 5, 1980),
                Appended::Stored
            );
        }
        assert!(get(&stats.spool_dropped) > 0);
    });
    assert!(lines.count(EVICTION_MESSAGE) >= 1, "{}", lines.text());
    assert_eq!(lines.count(DISCARD_MESSAGE), 0);
}
