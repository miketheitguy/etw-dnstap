//! Property tests for the spool: any mix of appends, batch reads, reopens, and a
//! cut-off of the newest segment or a noisy cursor file must deliver frames in
//! order and never twice. Each open spool must account for every frame it was
//! given (a cut-off leftover may add one counted frame per cut segment), and when
//! the spool drops frames it must drop the oldest first.
//!
//! Frames carry a global sequence number and a filler derived from it, so a
//! corrupted or repeated frame is caught by value. The caps are small, so
//! segments roll over and the oldest ones are evicted often.

use std::{cell::Cell, collections::BTreeSet, fs, io, path::PathBuf, sync::atomic::AtomicU64};

use proptest::{prelude::*, test_runner::TestCaseError};

use super::*;
use crate::spool::test_support::{TestDir, framed, segment_payloads, test_key};

/// Reads at least this large drain a spool in a few calls.
const DRAIN_LIMIT: usize = 4096;
/// Calls before a drain gives up. A drain needs one call per batch.
const DRAIN_CALLS: usize = 2000;

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

/// The size cap and the free-space floor of a case.
#[derive(Clone, Copy, Debug)]
struct Limits {
    max_bytes: u64,
    min_free: u64,
    /// Free bytes with no segment file, and the bytes each file takes.
    free_base: u64,
    per_file: u64,
}

impl Limits {
    const fn cap_only(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            min_free: 0,
            free_base: u64::MAX,
            per_file: 0,
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Append a block of `frames` frames with about `pad` filler bytes each.
    Append { frames: u8, pad: u8 },
    /// Read one batch of at most `limit` bytes.
    Read { limit: u16 },
    /// Seal and close. Optionally cut the newest segment to `cut` percent of its
    /// length, and overwrite `cursor.bin` with `(seed, length)` noise. Then open
    /// the folder again.
    Reopen {
        cut: Option<u8>,
        noise: Option<(u8, u8)>,
    },
}

fn op_strategy(reopen: bool, allow_cut: bool, allow_noise: bool) -> BoxedStrategy<Op> {
    let append = (1_u8..=10, 0_u8..=120)
        .prop_map(|(frames, pad)| Op::Append { frames, pad })
        .boxed();
    let read = (1_u16..=1500).prop_map(|limit| Op::Read { limit }).boxed();
    let cut_percent = if allow_cut {
        prop::option::weighted(0.5, 0_u8..100).boxed()
    } else {
        Just(None).boxed()
    };
    let noise = if allow_noise {
        prop::option::weighted(0.6, (any::<u8>(), 0_u8..40)).boxed()
    } else {
        Just(None).boxed()
    };
    let reopen_op = (cut_percent, noise)
        .prop_map(|(cut, noise)| Op::Reopen { cut, noise })
        .boxed();
    if reopen {
        prop_oneof![5 => append, 5 => read, 2 => reopen_op].boxed()
    } else {
        prop_oneof![5 => append, 5 => read].boxed()
    }
}

fn max_bytes_strategy() -> BoxedStrategy<u64> {
    prop_oneof![256_u64..2048, 2048_u64..16_384].boxed()
}

fn cap_strategy() -> BoxedStrategy<Limits> {
    max_bytes_strategy().prop_map(Limits::cap_only).boxed()
}

/// A cap and a nonzero free-space floor that the fake volume reaches as files
/// pile up, so segments are deleted for the floor and the spool pauses.
fn floor_strategy() -> BoxedStrategy<Limits> {
    (max_bytes_strategy(), 1_u64..1500, 0_u64..3000)
        .prop_map(|(max_bytes, min_free, extra)| Limits {
            max_bytes,
            min_free,
            free_base: min_free + max_bytes / 8 + extra,
            per_file: 300,
        })
        .boxed()
}

/// The filler byte at `index` of frame `number`.
fn filler(number: u64, index: usize) -> u8 {
    number.to_le_bytes()[0].wrapping_add(u8::try_from(index % 251).unwrap_or(0))
}

/// The payload of frame `number`: the number, then `pad` filler bytes.
fn payload(number: u64, pad: usize) -> Vec<u8> {
    let mut bytes = number.to_be_bytes().to_vec();
    bytes.extend((0..pad).map(|index| filler(number, index)));
    bytes
}

/// The number in a payload, or an error if the filler is wrong.
fn check_payload(bytes: &[u8]) -> Result<u64, String> {
    let (head, pad) = bytes
        .split_first_chunk::<8>()
        .ok_or_else(|| format!("payload of {} bytes has no number", bytes.len()))?;
    let number = u64::from_be_bytes(*head);
    for (index, byte) in pad.iter().enumerate() {
        if *byte != filler(number, index) {
            return Err(format!("frame {number} is corrupt at byte {index}"));
        }
    }
    Ok(number)
}

/// Splits framed bytes into payloads, or says why they are malformed.
fn split_frames(mut rest: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut frames = Vec::new();
    while !rest.is_empty() {
        let (header, tail) = rest
            .split_first_chunk::<4>()
            .ok_or("a length prefix is cut off")?;
        let length = u32::from_be_bytes(*header) as usize;
        if length == 0 || tail.len() < length {
            return Err(format!("a frame of length {length} is not whole"));
        }
        frames.push(&tail[..length]);
        rest = &tail[length..];
    }
    Ok(frames)
}

/// The segment files in `dir`, oldest first.
fn segment_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().and_then(parse_name).is_some())
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

/// The frame numbers in a segment file, decrypted leniently: stop at the end
/// record, at the end of the file, or at a cut-off or corrupt record.
fn file_numbers(path: &Path) -> Vec<u64> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    segment_payloads(&bytes)
        .iter()
        .map_while(|payload| {
            payload
                .first_chunk::<8>()
                .map(|head| u64::from_be_bytes(*head))
        })
        .collect()
}

type Check = Result<(), TestCaseError>;

fn io_fail(error: &io::Error) -> TestCaseError {
    TestCaseError::fail(format!("I/O error in the test: {error}"))
}

fn open_spool(
    dir: &TestDir,
    limits: Limits,
    stats: &Arc<SinkStats>,
) -> Result<Spool, TestCaseError> {
    Spool::open(
        SpoolConfig {
            dir: dir.path().to_path_buf(),
            max_bytes: limits.max_bytes,
            min_free_bytes: limits.min_free,
        },
        test_key(),
        KeyOrigin::Loaded,
        Arc::clone(stats),
        fake_free,
    )
    .map_err(|error| io_fail(&error))
}

fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

/// The model of one folder across reopens.
struct Harness {
    dir: TestDir,
    limits: Limits,
    /// `None` only while the folder is being reopened.
    spool: Option<Spool>,
    stats: Arc<SinkStats>,
    /// The number of the next frame to append.
    next_number: u64,
    /// The highest number handed out so far.
    last_read: Option<u64>,
    /// First number appended by the current open spool.
    run_start: u64,
    /// Frames the current open spool was given, stored or refused.
    offered: u64,
    /// Frames of the current open spool's own appends that it handed out.
    read_own: u64,
    /// Frames the current open spool handed out, own or leftover.
    read_all: u64,
    /// Segments that were cut and may still be on disk.
    cut_names: Vec<PathBuf>,
    /// Cut segments on disk when the current spool opened. Reading one can count
    /// one cut-off frame in `spool_dropped` that this run was not given.
    cut_budget: u64,
    /// Unread frames on disk after the last operation.
    present: BTreeSet<u64>,
    /// The newest frame the spool dropped so far.
    max_evicted: Option<u64>,
}

impl Harness {
    fn new(limits: Limits) -> Result<Self, TestCaseError> {
        FREE_BASE.set(limits.free_base);
        FREE_PER_FILE.set(limits.per_file);
        let dir = TestDir::new("property");
        let stats = Arc::new(SinkStats::default());
        let spool = open_spool(&dir, limits, &stats)?;
        let harness = Self {
            dir,
            limits,
            spool: Some(spool),
            stats,
            next_number: 0,
            last_read: None,
            run_start: 0,
            offered: 0,
            read_own: 0,
            read_all: 0,
            cut_names: Vec::new(),
            cut_budget: 0,
            present: BTreeSet::new(),
            max_evicted: None,
        };
        harness.check_gauges()?;
        Ok(harness)
    }

    fn spool(&mut self) -> Result<&mut Spool, TestCaseError> {
        self.spool
            .as_mut()
            .ok_or_else(|| TestCaseError::fail("the spool is closed"))
    }

    /// The gauges equal the files on disk, and the cap holds.
    fn check_gauges(&self) -> Check {
        let paths = segment_paths(self.dir.path());
        let on_disk: u64 = paths
            .iter()
            .map(|path| fs::metadata(path).map_or(0, |metadata| metadata.len()))
            .sum();
        prop_assert_eq!(get(&self.stats.spool_bytes), on_disk, "spool_bytes");
        prop_assert_eq!(
            get(&self.stats.spool_segments),
            paths.len() as u64,
            "spool_segments"
        );
        prop_assert!(
            on_disk <= self.limits.max_bytes,
            "{} bytes on disk, cap {}",
            on_disk,
            self.limits.max_bytes
        );
        Ok(())
    }

    fn append(&mut self, frames: u8, pad: u8) -> Check {
        let payloads: Vec<Vec<u8>> = (0..u64::from(frames))
            .map(|offset| {
                let number = self.next_number + offset;
                payload(
                    number,
                    usize::from(pad) + usize::try_from(number % 5).unwrap_or(0),
                )
            })
            .collect();
        let bytes = framed(&payloads);
        // Stored or refused, the spool counts it; the model counts it as offered.
        let _outcome = self.spool()?.append(&bytes, u32::from(frames));
        self.next_number += u64::from(frames);
        self.offered += u64::from(frames);
        self.check_gauges()?;
        self.track_drops()
    }

    /// Unread frames in the segment files now.
    fn unread_on_disk(&self) -> BTreeSet<u64> {
        segment_paths(self.dir.path())
            .iter()
            .flat_map(|path| file_numbers(path))
            .filter(|number| self.last_read.is_none_or(|last| *number > last))
            .collect()
    }

    /// Drop oldest: a frame that left the disk unread is older than every frame
    /// still on disk, and older than every frame handed out later.
    fn track_drops(&mut self) -> Check {
        let disk = self.unread_on_disk();
        for gone in self.present.difference(&disk) {
            if self.last_read.is_some_and(|last| *gone <= last) {
                continue;
            }
            prop_assert!(
                disk.first().is_none_or(|first| gone < first),
                "frame {} was dropped while the older-or-equal frame {:?} stays",
                gone,
                disk.first()
            );
            self.max_evicted = self.max_evicted.max(Some(*gone));
        }
        self.present = disk;
        Ok(())
    }

    /// Reads one batch and checks its frames: whole, intact, and newer than every
    /// frame handed out before (in order, and never twice).
    fn read(&mut self, limit: usize) -> Check {
        let mut out = Vec::new();
        let frames = self
            .spool()?
            .read_and_deliver(&mut out, limit)
            .map_err(|error| TestCaseError::fail(format!("read failed: {error}")))?;
        let parsed = split_frames(&out).map_err(TestCaseError::fail)?;
        prop_assert_eq!(parsed.len(), frames as usize, "frame count of the batch");
        for body in parsed {
            let number = check_payload(body).map_err(TestCaseError::fail)?;
            prop_assert!(
                number < self.next_number,
                "frame {} was never appended",
                number
            );
            prop_assert!(
                self.last_read.is_none_or(|last| number > last),
                "frame {} came after frame {:?}",
                number,
                self.last_read
            );
            prop_assert!(
                self.max_evicted.is_none_or(|dropped| number > dropped),
                "frame {} came after a newer frame {:?} was dropped",
                number,
                self.max_evicted
            );
            self.last_read = Some(number);
            self.read_all += 1;
            if number >= self.run_start {
                self.read_own += 1;
            }
        }
        prop_assert_eq!(get(&self.stats.replayed), self.read_all, "replayed");
        self.check_gauges()?;
        self.track_drops()
    }

    /// Frames of the current run still on disk and not yet handed out.
    fn remaining_own(&self) -> u64 {
        segment_paths(self.dir.path())
            .iter()
            .flat_map(|path| file_numbers(path))
            .filter(|number| {
                *number >= self.run_start && self.last_read.is_none_or(|last| *number > last)
            })
            .count() as u64
    }

    /// `offered = read + dropped + still in the spool`, for the frames the
    /// current open spool was given. Leftover frames are not part of it: an
    /// evicted leftover adds bytes to `spool_dropped_bytes`, not frames. Only a
    /// cut-off leftover can add to `spool_dropped`, by one frame per segment.
    fn check_conservation(&self, remaining: u64) -> Check {
        let dropped = get(&self.stats.spool_dropped);
        let accounted = self.read_own + dropped + remaining;
        prop_assert!(
            accounted >= self.offered && accounted <= self.offered + self.cut_budget,
            "offered {} vs read {} + dropped {} + remaining {} (cut budget {})",
            self.offered,
            self.read_own,
            dropped,
            remaining,
            self.cut_budget
        );
        Ok(())
    }

    /// Seals the spool, closes it, optionally cuts the newest segment, and opens
    /// the folder again with new counters.
    fn reopen(&mut self, cut: Option<u8>, noise: Option<(u8, u8)>) -> Check {
        prop_assert!(self.spool()?.seal().is_ok(), "seal failed");
        self.check_gauges()?;
        self.check_conservation(self.remaining_own())?;
        // Close the spool first, so the cut below can open the file.
        self.spool = None;
        if let Some(percent) = cut
            && let Some(newest) = segment_paths(self.dir.path()).last()
        {
            let length = fs::metadata(newest).map_err(|error| io_fail(&error))?.len();
            let shorter = length * u64::from(percent) / 100;
            if shorter < length {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(newest)
                    .map_err(|error| io_fail(&error))?;
                file.set_len(shorter).map_err(|error| io_fail(&error))?;
                self.cut_names.push(newest.clone());
            }
        }
        if let Some((seed, length)) = noise {
            // Never a valid cursor: the magic and the check value are wrong.
            let bytes: Vec<u8> = (0..length)
                .map(|index| seed.wrapping_mul(index | 1) ^ 0x5a)
                .collect();
            fs::write(self.dir.path().join(cursor::FILE_NAME), bytes)
                .map_err(|error| io_fail(&error))?;
        }
        let stats = Arc::new(SinkStats::default());
        self.spool = Some(open_spool(&self.dir, self.limits, &stats)?);
        self.cut_names.retain(|path| path.exists());
        self.cut_budget = self.cut_names.len() as u64;
        self.stats = stats;
        self.run_start = self.next_number;
        self.offered = 0;
        self.read_own = 0;
        self.read_all = 0;
        self.check_gauges()?;
        // A cut or a bad cursor changes what is on disk without a drop to judge.
        self.present = self.unread_on_disk();
        Ok(())
    }

    /// Reads until the spool is empty and checks that nothing is left.
    fn drain(&mut self) -> Check {
        for _ in 0..DRAIN_CALLS {
            self.read(DRAIN_LIMIT)?;
            if !self.spool()?.has_backlog() {
                break;
            }
        }
        prop_assert!(!self.spool()?.has_backlog(), "the spool did not drain");
        prop_assert!(segment_paths(self.dir.path()).is_empty(), "files are left");
        prop_assert_eq!(get(&self.stats.spool_bytes), 0);
        prop_assert_eq!(get(&self.stats.spool_segments), 0);
        self.check_conservation(0)
    }

    fn run(&mut self, ops: &[Op]) -> Check {
        for op in ops {
            match op {
                Op::Append { frames, pad } => self.append(*frames, *pad)?,
                Op::Read { limit } => self.read(usize::from(*limit))?,
                Op::Reopen { cut, noise } => self.reopen(*cut, *noise)?,
            }
        }
        self.drain()?;
        // An emptied spool stays empty across a reopen: nothing is read twice.
        self.reopen(None, None)?;
        let before = self.last_read;
        self.drain()?;
        prop_assert_eq!(self.last_read, before, "a frame came back after the drain");
        prop_assert_eq!(self.read_all, 0);
        Ok(())
    }
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 64,
        max_shrink_iters: 300,
        // No regression files in the source tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// One run: appends and reads give frames in order, every offered frame is
    /// read or counted dropped, the oldest frames go first, and the cap and the
    /// gauges hold throughout.
    #[test]
    fn one_run_conserves_frames_and_respects_the_cap(
        limits in cap_strategy(),
        ops in prop::collection::vec(op_strategy(false, false, false), 0..40),
    ) {
        Harness::new(limits)?.run(&ops)?;
    }

    /// Several runs on one folder: the order holds across reopens, no frame is
    /// read twice, and each run accounts for the frames it was given.
    #[test]
    fn reopened_runs_keep_order_and_account_per_run(
        limits in cap_strategy(),
        ops in prop::collection::vec(op_strategy(true, false, false), 0..50),
    ) {
        Harness::new(limits)?.run(&ops)?;
    }

    /// A cut-off of the newest segment loses frames but never reorders or
    /// repeats them, never corrupts one that is handed out, and adds at most one
    /// counted frame per cut segment to a later run.
    #[test]
    fn a_cut_newest_segment_never_reorders_or_repeats_frames(
        limits in cap_strategy(),
        ops in prop::collection::vec(op_strategy(true, true, false), 0..50),
    ) {
        Harness::new(limits)?.run(&ops)?;
    }

    /// A cursor file overwritten with noise costs the oldest segment (at-most-once)
    /// but never reorders or repeats frames, and later runs still account for
    /// every frame they were given.
    #[test]
    fn a_noisy_cursor_file_never_reorders_or_repeats_frames(
        limits in cap_strategy(),
        ops in prop::collection::vec(op_strategy(true, true, true), 0..50),
    ) {
        Harness::new(limits)?.run(&ops)?;
    }

    /// With a free-space floor that the fake volume reaches, the spool deletes
    /// segments for the floor and pauses. Order, conservation, and drop-oldest
    /// still hold.
    #[test]
    fn a_free_space_floor_keeps_order_conservation_and_drop_oldest(
        limits in floor_strategy(),
        ops in prop::collection::vec(op_strategy(true, false, false), 0..50),
    ) {
        Harness::new(limits)?.run(&ops)?;
    }
}

// ---- held data, the delivered position, and the rewind ----

/// One step of a link and a router over a spool that holds batches.
#[derive(Clone, Debug)]
enum HeldOp {
    /// Append a block of `frames` frames with about `pad` filler bytes each.
    Append { frames: u8, pad: u8 },
    /// Read one batch of at most `limit` bytes and keep it held.
    Read { limit: u16 },
    /// The link takes the oldest held batch whole.
    Deliver,
    /// The link takes `frames` frames (modulo the batch) of the oldest held batch
    /// and fails: everything held goes back.
    Fail { frames: u8 },
    /// The link fails with nothing taken.
    Rewind,
    /// The router saves the delivered position.
    Checkpoint,
    /// A stop: give back what is held, seal, and open the folder again.
    Reopen,
}

fn held_op_strategy(reopen: bool) -> BoxedStrategy<HeldOp> {
    let append = (1_u8..=10, 0_u8..=120)
        .prop_map(|(frames, pad)| HeldOp::Append { frames, pad })
        .boxed();
    let read = (1_u16..=1500)
        .prop_map(|limit| HeldOp::Read { limit })
        .boxed();
    let fail = any::<u8>()
        .prop_map(|frames| HeldOp::Fail { frames })
        .boxed();
    let ops = prop_oneof![
        5 => append,
        6 => read,
        3 => Just(HeldOp::Deliver),
        2 => fail,
        1 => Just(HeldOp::Rewind),
        1 => Just(HeldOp::Checkpoint),
    ];
    if reopen {
        prop_oneof![18 => ops, 1 => Just(HeldOp::Reopen)].boxed()
    } else {
        ops.boxed()
    }
}

/// The model of a spool whose batches are held until a link reports them.
struct HeldHarness {
    dir: TestDir,
    limits: Limits,
    spool: Option<Spool>,
    stats: Arc<SinkStats>,
    next_number: u64,
    /// Held batches, oldest first: their frame numbers and bytes.
    held: VecDeque<(Vec<u64>, Vec<u8>)>,
    /// Frames reported delivered, in order.
    delivered: Vec<u64>,
    /// The newest frame handed out and not given back.
    last_read: Option<u64>,
}

impl HeldHarness {
    fn new(limits: Limits) -> Result<Self, TestCaseError> {
        FREE_BASE.set(limits.free_base);
        FREE_PER_FILE.set(limits.per_file);
        let dir = TestDir::new("property-held");
        let stats = Arc::new(SinkStats::default());
        let spool = open_spool(&dir, limits, &stats)?;
        Ok(Self {
            dir,
            limits,
            spool: Some(spool),
            stats,
            next_number: 0,
            held: VecDeque::new(),
            delivered: Vec::new(),
            last_read: None,
        })
    }

    fn spool(&mut self) -> Result<&mut Spool, TestCaseError> {
        self.spool
            .as_mut()
            .ok_or_else(|| TestCaseError::fail("the spool is closed"))
    }

    /// Gives back everything held, as a failed link does.
    fn give_back(&mut self) -> Check {
        self.held.clear();
        self.spool()?.rewind();
        self.last_read = self.delivered.last().copied();
        Ok(())
    }

    fn apply(&mut self, op: &HeldOp) -> Check {
        match *op {
            HeldOp::Append { frames, pad } => {
                let payloads: Vec<Vec<u8>> = (0..u64::from(frames))
                    .map(|offset| {
                        let number = self.next_number + offset;
                        let pad = usize::from(pad) + usize::try_from(number % 5).unwrap_or(0);
                        payload(number, pad)
                    })
                    .collect();
                let _outcome = self.spool()?.append(&framed(&payloads), u32::from(frames));
                self.next_number += u64::from(frames);
            }
            HeldOp::Read { limit } => {
                let mut out = Vec::new();
                let frames = self
                    .spool()?
                    .read_batch(&mut out, usize::from(limit))
                    .map_err(|error| io_fail(&error))?;
                let mut numbers = Vec::new();
                for body in split_frames(&out).map_err(TestCaseError::fail)? {
                    numbers.push(check_payload(body).map_err(TestCaseError::fail)?);
                }
                prop_assert_eq!(numbers.len(), frames as usize, "frame count of the batch");
                for number in &numbers {
                    prop_assert!(
                        self.last_read.is_none_or(|last| *number > last),
                        "frame {} came after frame {:?}",
                        number,
                        self.last_read
                    );
                    self.last_read = Some(*number);
                }
                if frames > 0 {
                    self.held.push_back((numbers, out));
                }
            }
            HeldOp::Deliver => {
                if let Some((numbers, bytes)) = self.held.pop_front() {
                    let frames = u32::try_from(numbers.len()).unwrap_or(u32::MAX);
                    self.spool()?.delivered(bytes.len(), frames);
                    self.delivered.extend(numbers);
                }
            }
            HeldOp::Fail { frames } => {
                if let Some((numbers, bytes)) = self.held.pop_front() {
                    let taken = usize::from(frames) % (numbers.len() + 1);
                    let mut at = 0;
                    for body in split_frames(&bytes)
                        .map_err(TestCaseError::fail)?
                        .iter()
                        .take(taken)
                    {
                        at += 4 + body.len();
                    }
                    let count = u32::try_from(taken).unwrap_or(u32::MAX);
                    self.spool()?.delivered(at, count);
                    self.delivered.extend(&numbers[..taken]);
                }
                self.give_back()?;
            }
            HeldOp::Rewind => self.give_back()?,
            HeldOp::Checkpoint => self.spool()?.checkpoint(),
            HeldOp::Reopen => {
                self.give_back()?;
                self.spool()?.seal().map_err(|error| io_fail(&error))?;
                self.spool = None;
                self.spool = Some(open_spool(&self.dir, self.limits, &self.stats)?);
            }
        }
        self.check()
    }

    /// Delivered frames are in order and counted once; the held frames are still
    /// on disk; and every offered frame is delivered, dropped, or on disk and not
    /// delivered.
    fn check(&self) -> Check {
        prop_assert!(
            self.delivered.windows(2).all(|pair| pair[0] < pair[1]),
            "order or repeat: {:?}",
            self.delivered
        );
        prop_assert_eq!(get(&self.stats.replayed), self.delivered.len() as u64);
        let last = self.delivered.last().copied();
        let on_disk: BTreeSet<u64> = segment_paths(self.dir.path())
            .iter()
            .flat_map(|path| file_numbers(path))
            .collect();
        for (numbers, _) in &self.held {
            for number in numbers {
                prop_assert!(
                    on_disk.contains(number),
                    "held frame {} left the disk",
                    number
                );
            }
        }
        let undelivered = on_disk
            .iter()
            .filter(|number| last.is_none_or(|last| **number > last))
            .count() as u64;
        prop_assert_eq!(
            self.next_number,
            self.delivered.len() as u64 + get(&self.stats.spool_dropped) + undelivered,
            "offered = delivered + dropped + undelivered on disk"
        );
        Ok(())
    }

    /// The link takes everything that is left, and the spool empties.
    fn finish(&mut self) -> Check {
        self.give_back()?;
        for _ in 0..DRAIN_CALLS {
            let mut out = Vec::new();
            let frames = self
                .spool()?
                .read_and_deliver(&mut out, DRAIN_LIMIT)
                .map_err(|error| io_fail(&error))?;
            for body in split_frames(&out).map_err(TestCaseError::fail)? {
                self.delivered
                    .push(check_payload(body).map_err(TestCaseError::fail)?);
            }
            if frames == 0 {
                break;
            }
        }
        self.check()?;
        prop_assert!(!self.spool()?.has_backlog(), "the spool did not empty");
        prop_assert_eq!(
            self.next_number,
            self.delivered.len() as u64 + get(&self.stats.spool_dropped)
        );
        Ok(())
    }

    fn run(&mut self, ops: &[HeldOp]) -> Check {
        for op in ops {
            self.apply(op)?;
        }
        self.finish()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Whatever the link takes and gives back, the frames it was handed are in
    /// order and none twice, held frames are never deleted, and every offered
    /// frame is delivered, counted dropped, or still on disk. Small caps evict
    /// while batches are held.
    #[test]
    fn held_batches_and_rewinds_conserve_frames_and_keep_order(
        limits in cap_strategy(),
        ops in prop::collection::vec(held_op_strategy(false), 0..60),
    ) {
        HeldHarness::new(limits)?.run(&ops)?;
    }

    /// A stop gives back what is held and saves the delivered position: the next
    /// run repeats nothing and loses nothing.
    #[test]
    fn a_stop_after_a_rewind_repeats_nothing_and_loses_nothing(
        ops in prop::collection::vec(held_op_strategy(true), 0..60),
    ) {
        HeldHarness::new(Limits::cap_only(1 << 24))?.run(&ops)?;
    }
}
