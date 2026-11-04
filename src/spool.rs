//! A disk spool for dnstap frames that cannot be sent yet.
//!
//! The spool is a folder of append-only segment files, `seg-<20 digits>.fstrm`,
//! each a valid unidirectional dnstap Frame Streams file. New writes always go to
//! a new segment, and the read position lives in `cursor.bin` (see [`cursor`]).
//! Delivery is at most once: the cursor moves past a batch before the caller
//! sends it, so a crash can lose a batch but never repeats one. The cursor is a
//! high-water mark that is never deleted, so a segment file that cannot be
//! deleted is skipped at the next start instead of being sent again.
//!
//! [`Spool`] is synchronous and used from one thread. It never flushes to disk,
//! because it guards against a receiver outage, not against power loss. When it
//! is full it deletes its oldest segment and counts what that discarded.

use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime},
};

use self::{
    cursor::{Cursor, CursorFile, CursorRead},
    segment::{EndReason, START_LEN, STOP_LEN, SegmentReader, SegmentWriter},
};
use crate::sink::SinkStats;

mod cursor;
#[allow(unsafe_code)]
pub(crate) mod fsinfo;
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) mod fuzzing;
mod segment;

/// The largest segment: a segment is sealed at `min(this, max_bytes / 8)`.
const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
/// How long the spool refuses blocks after an I/O error or when the free-space
/// floor leaves nothing to delete, and how long it pauses reads after a
/// transient read error or a failed cursor write.
const PAUSE: Duration = Duration::from_secs(10);
/// The largest sequence number. The 20-digit name rule leaves room above it, so
/// `sequence + 1` cannot overflow.
const MAX_SEQUENCE: u64 = 9_999_999_999_999_999_999;
/// Names tried for a new segment before giving up.
const CREATE_ATTEMPTS: u32 = 8;
/// Consecutive transient read failures on one segment before it is deleted.
const TRANSIENT_LIMIT: u32 = 3;
/// `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION`: another process holds
/// the file for now.
const TRANSIENT_OS_ERRORS: [i32; 2] = [32, 33];
/// Segment file name parts: `seg-` + 20 digits + `.fstrm`.
const NAME_PREFIX: &str = "seg-";
const NAME_SUFFIX: &str = ".fstrm";
const NAME_DIGITS: usize = 20;

/// Spool limits and location.
pub(crate) struct SpoolConfig {
    /// The spool folder. The caller has run [`fsinfo::check_spool_dir`] on it.
    pub(crate) dir: PathBuf,
    /// Cap on segment bytes.
    pub(crate) max_bytes: u64,
    /// Free space to keep on the volume.
    pub(crate) min_free_bytes: u64,
}

/// What [`Spool::append`] did with a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(crate) enum Appended {
    /// The block is on disk.
    Stored,
    /// The spool refused the block and counted its frames in `spool_dropped`.
    Dropped,
}

/// Whether the volume has the free space a new segment needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FreeSpace {
    /// The floor is met (or no new segment was needed).
    Met,
    /// The floor is not met and no older segment is left to delete: `free`
    /// bytes are free, and `needed` bytes (`min_free_bytes` plus a segment)
    /// are needed.
    Short { free: u64, needed: u64 },
}

/// Bytes in a MiB, for the free-space log.
const MIB: u64 = 1024 * 1024;

/// When a segment was created.
#[derive(Clone, Copy)]
enum Created {
    /// A segment of this run, on the monotonic clock: a change of the wall clock
    /// does not move it.
    Run(Instant),
    /// A leftover from an earlier run: the file's creation time (or its last
    /// write time, when the volume has none). Only the wall clock can age it.
    Leftover(SystemTime),
}

/// One segment file the spool knows about.
struct SegmentInfo {
    sequence: u64,
    /// File length that holds good data. For the active segment this is the
    /// writer's length.
    bytes: u64,
    /// Frames appended this run, or `None` for a leftover from an earlier run.
    frames: Option<u64>,
    /// Frames of this run's segment already handed out.
    frames_read: u64,
    /// Offset of the next unread byte; 0 while nothing has been read.
    read_offset: u64,
    /// When the segment was created. Used only for the age of the backlog.
    created: Created,
    /// True while the file may hold bytes past `bytes` that are not good data
    /// (the active segment, or one closed after a write error). A cleanly
    /// sealed segment or a leftover is read to the end of the file.
    bounded: bool,
}

/// A consumed or evicted segment file whose delete failed. It still counts in
/// `total_bytes`; the delete is retried, and the next start removes it because
/// its sequence is below the cursor's.
struct PendingDelete {
    path: PathBuf,
    bytes: u64,
}

/// The spool: segments on disk, one writer, one reader.
pub(crate) struct Spool {
    config: SpoolConfig,
    stats: Arc<SinkStats>,
    free_space: fn(&Path) -> io::Result<u64>,
    /// Oldest first. When `writer` is set, the last entry is its segment.
    segments: VecDeque<SegmentInfo>,
    writer: Option<SegmentWriter>,
    /// The reader for the first segment, tagged with that segment's sequence.
    reader: Option<(u64, SegmentReader)>,
    cursor: CursorFile,
    pending: Vec<PendingDelete>,
    next_sequence: u64,
    total_bytes: u64,
    segment_bytes: u64,
    /// No appends until this time: a write error happened.
    suspended_until: Option<Instant>,
    /// No appends until this time: the free-space floor left nothing to delete.
    blocked_until: Option<Instant>,
    /// No reads until this time: a transient read error or a cursor write
    /// failure happened. Appends continue.
    read_paused_until: Option<Instant>,
    /// The segment and count of consecutive transient read failures.
    transient: Option<(u64, u32)>,
    /// Evictions so far, for the bounded log.
    evictions: u64,
    /// Discards of unreadable data so far, for the bounded log.
    discards: u64,
    /// Free-space blocks so far, for the bounded log.
    low_space_blocks: u64,
    /// How long a pause lasts. Tests shorten it.
    pause: Duration,
}

impl Spool {
    /// Opens the spool folder: finds the segments an earlier run left, applies
    /// the cursor, and starts without an active segment (new writes make a new
    /// one). It does not call [`fsinfo::check_spool_dir`]; the caller does.
    ///
    /// `free_space` returns the free bytes on the volume of a folder;
    /// production passes [`fsinfo::available_bytes`].
    ///
    /// # Errors
    /// Returns an error if the folder cannot be listed or the cursor file cannot
    /// be read.
    pub(crate) fn open(
        config: SpoolConfig,
        stats: Arc<SinkStats>,
        free_space: fn(&Path) -> io::Result<u64>,
    ) -> io::Result<Self> {
        let cursor_state = cursor::read(&config.dir)?;
        let scanned = scan(&config.dir)?;
        // Sequence numbers never repeat: stay above every name seen, whatever its
        // type, and above the cursor's sequence (that segment may be deleted).
        let mut next_sequence = scanned.highest.map_or(1, |highest| highest + 1);
        if let CursorRead::Valid(cursor) = &cursor_state {
            next_sequence = next_sequence.max(cursor.sequence.saturating_add(1));
        }
        let segments = scanned.segments;
        let total_bytes = segments.iter().map(|segment| segment.bytes).sum();
        let segment_bytes = MAX_SEGMENT_BYTES.min(config.max_bytes / 8);
        let mut spool = Self {
            cursor: CursorFile::new(&config.dir),
            config,
            stats,
            free_space,
            segments,
            writer: None,
            reader: None,
            pending: Vec::new(),
            next_sequence,
            total_bytes,
            segment_bytes,
            suspended_until: None,
            blocked_until: None,
            read_paused_until: None,
            transient: None,
            evictions: 0,
            discards: 0,
            low_space_blocks: 0,
            pause: PAUSE,
        };
        spool.apply_cursor(cursor_state);
        spool.stats.spool_available.store(true, Ordering::Relaxed);
        spool.update_gauges();
        Ok(spool)
    }

    /// True when segments hold data that is not yet read. Routing must append,
    /// not write directly, while this is true, to keep frames in order.
    pub(crate) fn has_backlog(&self) -> bool {
        !self.segments.is_empty()
    }

    /// Bytes of segment files that are not yet read: the segment bytes less the
    /// part of the first segment already handed out. Unlike the `spool_bytes`
    /// gauge it falls as the replay reads, not when a whole segment is deleted,
    /// so it shows whether the backlog is growing or draining.
    pub(crate) fn unread_bytes(&self) -> u64 {
        let total: u64 = self.segments.iter().map(|segment| segment.bytes).sum();
        let read = self
            .segments
            .front()
            .map_or(0, |head| head.read_offset.min(head.bytes));
        total - read
    }

    /// How long ago the first segment was created, or zero with no backlog. The
    /// first segment holds the oldest unsent frame, so this is an upper bound
    /// on that frame's age: the segment is older than any frame in it, and part
    /// of it may be read already. A segment of this run ages on the monotonic
    /// clock (`now`), so a wall clock that is set back or forward does not move
    /// it. A leftover from an earlier run ages on the wall clock (`wall`) from
    /// the file's creation time, and a clock set back reads as zero.
    pub(crate) fn oldest_age(&self, now: Instant, wall: SystemTime) -> Duration {
        self.segments
            .front()
            .map_or(Duration::ZERO, |head| match head.created {
                Created::Run(created) => now.saturating_duration_since(created),
                Created::Leftover(created) => {
                    wall.duration_since(created).unwrap_or(Duration::ZERO)
                }
            })
    }

    /// False while the spool is paused by an I/O error or by the free-space
    /// floor. Also refreshes the `spool_available` flag.
    pub(crate) fn is_usable(&self) -> bool {
        let now = Instant::now();
        let usable = [self.suspended_until, self.blocked_until]
            .into_iter()
            .flatten()
            .all(|until| now >= until);
        self.stats.spool_available.store(usable, Ordering::Relaxed);
        usable
    }

    /// True while reads are paused after a transient error or a failed cursor
    /// write. [`Self::read_batch`] then returns 0 although a backlog remains.
    pub(crate) fn reads_paused(&self) -> bool {
        self.read_paused_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Appends a block of complete data frames (`frames` of them) to the active
    /// segment, starting a new one when needed. Deletes the oldest segments to
    /// stay under the size cap and the free-space floor.
    ///
    /// A refused block is counted in `spool_dropped`: the spool is paused, the
    /// floor leaves nothing to delete, the block can never fit, it is empty but
    /// claims frames, or an I/O error occurred. An I/O error also pauses the
    /// spool for 10 s.
    pub(crate) fn append(&mut self, bytes: &[u8], frames: u32) -> Appended {
        if bytes.is_empty() {
            return if frames == 0 {
                Appended::Stored
            } else {
                self.refuse(frames)
            };
        }
        let now = Instant::now();
        let paused = [self.suspended_until, self.blocked_until]
            .into_iter()
            .flatten()
            .any(|until| now < until);
        // A block must fit with its segment's START and STOP.
        let fits =
            (bytes.len() as u64).saturating_add(START_LEN + STOP_LEN) <= self.config.max_bytes;
        if paused || !fits {
            return self.refuse(frames);
        }
        match self.try_append(bytes, frames) {
            Ok(FreeSpace::Met) => {
                // A suspension that has ended is over once a block is stored.
                self.stats.spool_available.store(true, Ordering::Relaxed);
                Appended::Stored
            }
            Ok(FreeSpace::Short { free, needed }) => {
                self.note_low_space(free, needed);
                self.blocked_until = Some(now + self.pause);
                self.stats.spool_available.store(false, Ordering::Relaxed);
                self.refuse(frames)
            }
            Err(error) => {
                self.note_error(&error);
                // Close the active segment without STOP and pause the spool.
                self.writer = None;
                self.suspended_until = Some(Instant::now() + self.pause);
                self.stats.spool_available.store(false, Ordering::Relaxed);
                self.refuse(frames)
            }
        }
    }

    /// Reads whole data frames from the oldest segment into `out`, up to about
    /// `limit` bytes (at least one frame when any is waiting), and returns the
    /// frame count. The read position is saved to the cursor file before this
    /// returns, so the caller may send the bytes at once: they are consumed.
    ///
    /// Returns 0 when the spool has nothing to read now: it is empty, reads are
    /// paused, or the call only skipped control frames (call again). A segment
    /// that is finished, cut off, or corrupt is deleted and counted. When the
    /// last data is read, the spool deletes its files, moves the cursor to the
    /// next sequence, and [`Self::has_backlog`] turns false.
    ///
    /// # Errors
    /// Returns the I/O error. After an error the spool pauses reads for 10 s, so
    /// a caller that retries at once gets 0.
    /// - A sharing or lock violation (another process holds the file) keeps the
    ///   segment. The third failure in a row on one segment deletes it, with its
    ///   unread data counted as dropped.
    /// - Any other read error deletes the segment the same way.
    /// - If the cursor cannot be saved, nothing is consumed and the same data is
    ///   read again after the pause.
    pub(crate) fn read_batch(&mut self, out: &mut Vec<u8>, limit: usize) -> io::Result<u32> {
        let start = out.len();
        if self.reads_paused() {
            return Ok(0);
        }
        loop {
            let Some(first) = self.segments.front() else {
                self.retry_pending();
                return Ok(0);
            };
            let sequence = first.sequence;
            if let Err(error) = self.ensure_reader() {
                return Err(self.read_failed(error, sequence));
            }
            let Some(current) = self.segments.front() else {
                continue;
            };
            let bound = current.bounded.then_some(current.bytes);
            let Some((_, active_reader)) = self.reader.as_mut() else {
                // `ensure_reader` deleted a segment with a bad START.
                continue;
            };
            let before = active_reader.position();
            let batch = match active_reader.read_batch(out, limit, bound) {
                Ok(batch) => batch,
                Err(error) => return Err(self.read_failed(error, sequence)),
            };
            let position = active_reader.position();
            if batch.frames > 0 {
                if let Err(error) = self.cursor.write(Cursor {
                    sequence,
                    offset: position,
                }) {
                    // Nothing is consumed: give the bytes back so a retry reads them again.
                    out.truncate(start);
                    let rewound = self
                        .reader
                        .as_mut()
                        .is_some_and(|(_, stale)| stale.rewind(before).is_ok());
                    if !rewound {
                        self.reader = None;
                    }
                    self.note_error(&error);
                    self.read_paused_until = Some(Instant::now() + self.pause);
                    return Err(error);
                }
                self.transient = None;
                if let Some(read_from) = self.segments.front_mut() {
                    read_from.frames_read += u64::from(batch.frames);
                    read_from.read_offset = position;
                }
                self.stats
                    .replayed
                    .fetch_add(u64::from(batch.frames), Ordering::Relaxed);
                self.count_lost(&batch);
                return Ok(batch.frames);
            }
            match batch.end {
                Some(end) if end.is_terminal() => {
                    self.count_lost(&batch);
                    self.remove_oldest(true);
                }
                // The reader is at the end of the active segment: all data is read.
                Some(EndReason::WriterPosition)
                    if self.writer.is_some() && self.segments.len() == 1 =>
                {
                    self.remove_oldest(true);
                    self.retry_pending();
                    return Ok(0);
                }
                // A segment closed after a write error ends at its good length.
                Some(EndReason::WriterPosition) => self.remove_oldest(true),
                _ => return Ok(0),
            }
        }
    }

    /// Seals the active segment with STOP. Call it at shutdown. Segments stay on
    /// disk for the next run.
    ///
    /// # Errors
    /// Returns the I/O error; the segment is then closed without STOP, which a
    /// later run reads as an unsealed leftover.
    pub(crate) fn seal(&mut self) -> io::Result<()> {
        self.seal_active()
            .inspect_err(|error| self.note_error(error))
    }

    // ---- appending ----

    /// Returns [`FreeSpace::Met`] when the block is stored, and
    /// [`FreeSpace::Short`] when the free-space floor leaves nothing to delete.
    fn try_append(&mut self, bytes: &[u8], frames: u32) -> io::Result<FreeSpace> {
        let length = bytes.len() as u64;
        // Start a new segment when this block would overfill the active one.
        if self.writer.as_ref().is_some_and(|writer| {
            writer.len() > START_LEN && writer.len() + length + STOP_LEN > self.segment_bytes
        }) {
            self.seal_active()?;
        }
        // Make room under the size cap, oldest first. A new segment also needs
        // START, and STOP is reserved so a sealed segment still fits.
        while self.total_bytes + self.new_segment_bytes() + length + STOP_LEN
            > self.config.max_bytes
        {
            if !self.evict_oldest() {
                // Files that could not be deleted still count; try them again.
                self.retry_pending();
                break;
            }
        }
        if self.writer.is_none() {
            let space = self.ensure_free_space()?;
            if space != FreeSpace::Met {
                return Ok(space);
            }
            self.create_segment()?;
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(io::Error::other("no active spool segment"));
        };
        writer.append(bytes)?;
        let written = writer.len();
        if let Some(active) = self.segments.back_mut() {
            self.total_bytes += written - active.bytes;
            active.bytes = written;
            active.frames = active.frames.map(|total| total + u64::from(frames));
        }
        self.stats
            .spooled
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        self.update_gauges();
        Ok(FreeSpace::Met)
    }

    /// Deletes the oldest segments until the volume has `min_free_bytes` plus a
    /// segment free. It keeps the newest segment, and it stops when deleting a
    /// segment does not raise the free space the probe reports (something else
    /// uses the space). Returns [`FreeSpace::Short`] with the last free space
    /// the probe reported when the floor is still not met.
    fn ensure_free_space(&mut self) -> io::Result<FreeSpace> {
        let needed = self
            .config
            .min_free_bytes
            .saturating_add(self.segment_bytes);
        let mut free = (self.free_space)(&self.config.dir)?;
        loop {
            if free >= needed {
                return Ok(FreeSpace::Met);
            }
            if self.segments.len() <= 1 {
                return Ok(FreeSpace::Short { free, needed });
            }
            self.evict_oldest();
            let after = (self.free_space)(&self.config.dir)?;
            if after <= free {
                return Ok(FreeSpace::Short {
                    free: after,
                    needed,
                });
            }
            free = after;
        }
    }

    /// Hands out the next sequence number.
    fn take_sequence(&mut self) -> io::Result<u64> {
        let sequence = self.next_sequence;
        match sequence.checked_add(1) {
            Some(next) if sequence <= MAX_SEQUENCE => {
                self.next_sequence = next;
                Ok(sequence)
            }
            _ => Err(io::Error::other(
                "spool segment sequence numbers are used up",
            )),
        }
    }

    /// Creates the next segment. A name that is already taken, by an entry the
    /// scan did not see, is skipped.
    fn create_segment(&mut self) -> io::Result<()> {
        self.retry_pending();
        for _ in 0..CREATE_ATTEMPTS {
            let sequence = self.take_sequence()?;
            let path = self.segment_path(sequence);
            match SegmentWriter::create(&path) {
                Ok(writer) => {
                    self.writer = Some(writer);
                    self.segments.push_back(SegmentInfo {
                        sequence,
                        bytes: START_LEN,
                        frames: Some(0),
                        frames_read: 0,
                        read_offset: 0,
                        created: Created::Run(Instant::now()),
                        bounded: true,
                    });
                    self.total_bytes += START_LEN;
                    self.update_gauges();
                    return Ok(());
                }
                // A folder or link in the way may fail with "access denied", not
                // "already exists", so look at whether the name is in use.
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists
                        || fs::symlink_metadata(&path).is_ok() => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("no free spool segment name"))
    }

    /// Writes STOP to the active segment and closes it. The writer is dropped
    /// even if STOP fails.
    fn seal_active(&mut self) -> io::Result<()> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        let sealed = writer.seal()?;
        if let Some(active) = self.segments.back_mut() {
            self.total_bytes += sealed - active.bytes;
            active.bytes = sealed;
            active.bounded = false;
        }
        self.update_gauges();
        Ok(())
    }

    /// Bytes a new segment adds before any data: zero while one is active.
    const fn new_segment_bytes(&self) -> u64 {
        if self.writer.is_some() { 0 } else { START_LEN }
    }

    fn refuse(&self, frames: u32) -> Appended {
        self.stats
            .spool_dropped
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        Appended::Dropped
    }

    /// Logs that the free-space floor blocks appends for a pause: the first four
    /// times, then every 60th. Free space is rounded down and the need up to
    /// whole MiB, so a small shortfall never shows two equal values.
    fn note_low_space(&mut self, free: u64, needed: u64) {
        let blocks = self.low_space_blocks;
        self.low_space_blocks += 1;
        if blocks < 4 || blocks.is_multiple_of(60) {
            tracing::warn!(
                free_mib = free / MIB,
                needed_mib = needed.div_ceil(MIB),
                pause_s = self.pause.as_secs(),
                "dnstap spool is refusing blocks: the volume is below the free-space floor \
                and no older segment is left to delete"
            );
        }
    }

    /// Counts an I/O error. The log is bounded: the first four, then every 60th.
    fn note_error(&self, error: &io::Error) {
        let errors = self.stats.spool_errors.fetch_add(1, Ordering::Relaxed);
        if errors < 4 || errors.is_multiple_of(60) {
            tracing::warn!(%error, "dnstap spool I/O error");
        }
    }

    /// Handles a read error: counts it and drops the reader. A transient error
    /// (another process holds the file) pauses reads and keeps the segment, up to
    /// [`TRANSIENT_LIMIT`] failures in a row. Any other error deletes the first
    /// segment, with its unread data counted as dropped, so reads make progress.
    fn read_failed(&mut self, error: io::Error, sequence: u64) -> io::Error {
        self.note_error(&error);
        self.reader = None;
        if error
            .raw_os_error()
            .is_some_and(|code| TRANSIENT_OS_ERRORS.contains(&code))
        {
            let failures = match self.transient {
                Some((owner, count)) if owner == sequence => count + 1,
                _ => 1,
            };
            if failures < TRANSIENT_LIMIT {
                self.transient = Some((sequence, failures));
                self.read_paused_until = Some(Instant::now() + self.pause);
                return error;
            }
        }
        self.transient = None;
        self.evict_unreadable();
        error
    }

    // ---- reading and removing ----

    /// Opens the reader for the first segment if it is not open. A segment whose
    /// START is missing or wrong is deleted and all its bytes are counted.
    fn ensure_reader(&mut self) -> io::Result<()> {
        let Some(head) = self.segments.front() else {
            return Ok(());
        };
        let (sequence, offset) = (head.sequence, head.read_offset);
        if self
            .reader
            .as_ref()
            .is_some_and(|(owner, _)| *owner == sequence)
        {
            return Ok(());
        }
        self.reader = None;
        if let Some(reader) = SegmentReader::open(&self.segment_path(sequence), offset)? {
            self.reader = Some((sequence, reader));
        } else {
            if let Some(unreadable) = self.segments.front_mut() {
                unreadable.read_offset = 0;
            }
            self.evict_unreadable();
        }
        Ok(())
    }

    /// Counts the bytes and frames a read skipped in a cut-off or corrupt tail,
    /// then marks the segment as fully accounted for so a later eviction does
    /// not count them again. A segment from this run knows its frame total, so
    /// the frames lost are those not yet read; a leftover reports one for a
    /// cut-off final frame.
    fn count_lost(&mut self, batch: &segment::Batch) {
        if batch.dropped_bytes == 0 {
            return;
        }
        let Some(head) = self.segments.front_mut() else {
            return;
        };
        let frames = match head.frames {
            Some(total) => {
                let lost = total.saturating_sub(head.frames_read);
                head.frames_read = total;
                lost
            }
            None => u64::from(batch.dropped_frames),
        };
        head.read_offset = head.read_offset.max(head.bytes);
        self.note_discard(frames, batch.dropped_bytes);
        self.stats
            .spool_dropped
            .fetch_add(frames, Ordering::Relaxed);
        self.stats
            .spool_dropped_bytes
            .fetch_add(batch.dropped_bytes, Ordering::Relaxed);
    }

    /// Deletes the oldest segment and counts its unread data as dropped. A
    /// leftover segment adds only bytes, because its frame count is unknown.
    /// Returns false when there is no segment.
    fn evict_oldest(&mut self) -> bool {
        self.evict_with(false)
    }

    /// Like [`Self::evict_oldest`], for a segment that cannot be read (bad
    /// START, read error, or an invalid cursor). It logs as a discard of
    /// unreadable data, not as an eviction for space.
    fn evict_unreadable(&mut self) -> bool {
        self.evict_with(true)
    }

    fn evict_with(&mut self, unreadable: bool) -> bool {
        let Some(head) = self.segments.front() else {
            return false;
        };
        let frames = head
            .frames
            .map_or(0, |total| total.saturating_sub(head.frames_read));
        let bytes = head.bytes.saturating_sub(head.read_offset);
        self.stats
            .spool_dropped
            .fetch_add(frames, Ordering::Relaxed);
        self.stats
            .spool_dropped_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        if unreadable {
            self.note_discard(frames, bytes);
        } else if frames > 0 || bytes > 0 {
            // Counts only, never data. Bounded like the I/O error log.
            let evictions = self.evictions;
            self.evictions += 1;
            if evictions < 4 || evictions.is_multiple_of(60) {
                tracing::warn!(
                    frames,
                    bytes,
                    "dnstap spool deleted its oldest segment unsent"
                );
            }
        }
        self.remove_oldest(true);
        true
    }

    /// Logs unread data discarded because it cannot be read: the first four
    /// times, then every 60th. Counts only, never data.
    fn note_discard(&mut self, frames: u64, bytes: u64) {
        if frames == 0 && bytes == 0 {
            return;
        }
        let discards = self.discards;
        self.discards += 1;
        if discards < 4 || discards.is_multiple_of(60) {
            tracing::warn!(
                frames,
                bytes,
                "dnstap spool discarded unreadable segment data"
            );
        }
    }

    /// Closes and deletes the first segment. With `move_cursor`, the cursor is
    /// first moved to the start of the next segment, or to the next unused
    /// sequence when none is left. The cursor is never deleted. A file that
    /// cannot be deleted keeps counting in `total_bytes` and is retried.
    fn remove_oldest(&mut self, move_cursor: bool) {
        if self.segments.is_empty() {
            return;
        }
        if move_cursor {
            let target = self
                .segments
                .get(1)
                .map_or(self.next_sequence, |next| next.sequence);
            self.save_cursor(target);
        }
        // Close this segment's handles before deleting its file.
        self.reader = None;
        if self.segments.len() == 1 {
            self.writer = None;
        }
        if let Some(removed) = self.segments.pop_front() {
            let path = self.segment_path(removed.sequence);
            match remove_file(&path) {
                Ok(()) => self.total_bytes = self.total_bytes.saturating_sub(removed.bytes),
                Err(error) => {
                    self.note_error(&error);
                    self.pending.push(PendingDelete {
                        path,
                        bytes: removed.bytes,
                    });
                }
            }
        }
        self.update_gauges();
    }

    /// Retries the deletes that failed.
    fn retry_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        for file in std::mem::take(&mut self.pending) {
            if remove_file(&file.path).is_ok() {
                self.total_bytes = self.total_bytes.saturating_sub(file.bytes);
            } else {
                self.pending.push(file);
            }
        }
        self.update_gauges();
    }

    /// Records `(sequence, START_LEN)` in the cursor file.
    fn save_cursor(&mut self, sequence: u64) {
        let cursor = Cursor {
            sequence,
            offset: START_LEN,
        };
        if let Err(error) = self.cursor.write(cursor) {
            self.note_error(&error);
        }
    }

    // ---- startup ----

    /// Applies what `cursor.bin` held to the leftover segments. Segments below
    /// the cursor's sequence were read before the last run ended and are deleted
    /// uncounted (a delete that fails is retried). The cursor's own segment, if
    /// present, resumes at the cursor's offset; if it is missing, reading starts
    /// at the next segment's START. A cursor that is invalid, or has an offset
    /// inside START, makes the segment that was being read count as read: it is
    /// deleted and counted as dropped, which keeps delivery at most once.
    fn apply_cursor(&mut self, state: CursorRead) {
        match state {
            CursorRead::Missing => {}
            CursorRead::Invalid => {
                // With no segment, only rewrite the file so it is valid again.
                if !self.drop_unreadable_oldest() {
                    self.save_cursor(self.next_sequence);
                }
            }
            CursorRead::Valid(cursor) => {
                while self
                    .segments
                    .front()
                    .is_some_and(|segment| segment.sequence < cursor.sequence)
                {
                    self.remove_oldest(false);
                }
                let is_cursor_segment = self
                    .segments
                    .front()
                    .is_some_and(|segment| segment.sequence == cursor.sequence);
                if is_cursor_segment {
                    if cursor.offset >= START_LEN {
                        if let Some(head) = self.segments.front_mut() {
                            head.read_offset = cursor.offset;
                        }
                    } else {
                        self.drop_unreadable_oldest();
                    }
                }
            }
        }
    }

    /// Deletes the oldest segment as fully read and counts all its bytes.
    /// Returns false when there is no segment.
    fn drop_unreadable_oldest(&mut self) -> bool {
        if let Some(head) = self.segments.front_mut() {
            head.read_offset = 0;
        }
        self.evict_unreadable()
    }

    fn segment_path(&self, sequence: u64) -> PathBuf {
        self.config.dir.join(format!(
            "{NAME_PREFIX}{sequence:0NAME_DIGITS$}{NAME_SUFFIX}"
        ))
    }

    fn update_gauges(&self) {
        self.stats
            .spool_bytes
            .store(self.total_bytes, Ordering::Relaxed);
        self.stats.spool_segments.store(
            (self.segments.len() + self.pending.len()) as u64,
            Ordering::Relaxed,
        );
    }
}

/// What a scan of the spool folder found.
struct Scan {
    /// Regular segment files, oldest first.
    segments: VecDeque<SegmentInfo>,
    /// The highest sequence in any matching name, whatever the entry's type, so
    /// a folder or link named like a segment is never reused.
    highest: Option<u64>,
}

/// Lists the segment files in `dir`. Names match without regard to case. A
/// matching folder, link, or other reparse point is not read but still counts
/// toward `highest`. Other names are ignored.
fn scan(dir: &Path) -> io::Result<Scan> {
    let mut found = Vec::new();
    let mut highest = None::<u64>;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(sequence) = entry.file_name().to_str().and_then(parse_name) else {
            continue;
        };
        highest = highest.max(Some(sequence));
        // `file_type` and `metadata` do not follow links.
        if !entry.file_type()?.is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        if fsinfo::is_reparse_point(&metadata) {
            continue;
        }
        // Creation time is closest to the oldest frame in the file. Some
        // volumes report none; the last write time then stands in, and a file
        // with neither counts as new.
        let created = metadata
            .created()
            .or_else(|_| metadata.modified())
            .unwrap_or_else(|_| SystemTime::now());
        found.push((sequence, metadata.len(), Created::Leftover(created)));
    }
    found.sort_unstable_by_key(|(sequence, bytes, _)| (*sequence, *bytes));
    found.dedup_by_key(|(sequence, _, _)| *sequence);
    let segments = found
        .into_iter()
        .map(|(sequence, bytes, created)| SegmentInfo {
            sequence,
            bytes,
            frames: None,
            frames_read: 0,
            read_offset: 0,
            created,
            bounded: false,
        })
        .collect();
    Ok(Scan { segments, highest })
}

/// The sequence in `seg-<20 digits>.fstrm` (any letter case), or `None` for any
/// other name or a sequence above [`MAX_SEQUENCE`].
fn parse_name(name: &str) -> Option<u64> {
    let digits_end = name.len().checked_sub(NAME_SUFFIX.len())?;
    let prefix = name.get(..NAME_PREFIX.len())?;
    let digits = name.get(NAME_PREFIX.len()..digits_end)?;
    let suffix = name.get(digits_end..)?;
    if !prefix.eq_ignore_ascii_case(NAME_PREFIX)
        || !suffix.eq_ignore_ascii_case(NAME_SUFFIX)
        || digits.len() != NAME_DIGITS
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|sequence| *sequence <= MAX_SEQUENCE)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "spool/test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "spool/spool_tests.rs"]
mod spool_tests;

#[cfg(test)]
#[path = "spool/property_tests.rs"]
mod property_tests;
