//! A disk spool for dnstap frames that cannot be sent yet.
//!
//! The spool is a folder of append-only segment files, `seg-<20 digits>.fstrm`.
//! Each segment is encrypted (see [`crypt`] and [`segment`]): a file header,
//! then one AES-128-GCM record per append, each holding complete dnstap Frame
//! Streams data frames, and an end record when the segment is sealed. Nothing
//! is written to disk in plaintext. New writes always go to a new segment.
//!
//! Positions are logical: byte offsets in a segment's plaintext frame stream.
//! Size accounting (the cap, the segment size, the free-space floor, and the
//! byte counters) is in file bytes, which include the 32-byte header and 28
//! bytes for each record.
//!
//! At open, every segment that the key cannot read is deleted and counted in
//! `spool_dropped_bytes`: a plaintext segment of an earlier version, other
//! bytes, or a segment sealed with another key. With a key that was just
//! created, every segment goes, and the cursor is reset.
//!
//! The spool keeps two positions. The **read position** is the end of the last
//! batch [`Spool::read_batch`] returned. The **delivered position** is the end
//! of the last frame the caller reports with [`Spool::delivered`] as fully
//! handed to the socket. Data between them is **held**: read, not delivered.
//! A failed link gives the held data back with [`Spool::rewind`], which sets
//! the read position back to the delivered one, so the caller reads it again.
//! A segment is deleted only when the delivered position has passed it.
//!
//! `cursor.bin` (see [`cursor`]) holds the delivered position, saved by
//! [`Spool::checkpoint`] and when a segment is deleted. Delivery across a crash
//! is therefore at least once: the held data and the frames delivered since the
//! last save are sent again. A normal stop repeats nothing, because
//! [`Spool::seal`] saves the position. The cursor is a high-water mark that is
//! never deleted, so a segment file that cannot be deleted is skipped at the
//! next start instead of being sent again.
//!
//! [`Spool`] is synchronous and used from one thread. It never flushes to disk,
//! because it guards against a receiver outage, not against power loss. When it
//! is full it deletes its oldest segment that holds no read data and counts
//! what that discarded.

use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime},
};

use self::{
    crypt::{CryptError, SegmentKey},
    cursor::{Cursor, CursorFile, CursorRead},
    segment::{
        Bounds, END_LEN, EndReason, HEADER_LEN, HeaderCheck, PreparedSegment, SegmentReader,
        SegmentWriter,
    },
};
use crate::sink::SinkStats;

pub(crate) mod crypt;
mod cursor;
#[allow(unsafe_code)]
pub(crate) mod fsinfo;
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) mod fuzzing;
#[allow(unsafe_code)]
pub(crate) mod key;
#[allow(unsafe_code)]
mod locked;
#[allow(unsafe_code)]
mod protected;
mod segment;

/// The largest segment: a segment is sealed at `min(this, max_bytes / 8)`.
const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

/// The size at which a spool with the cap `max_bytes` seals a segment. The
/// free-space floor is `--spool-min-free-mib` plus one segment of this size.
pub(crate) const fn segment_bytes_for(max_bytes: u64) -> u64 {
    let eighth = max_bytes / 8;
    if eighth < MAX_SEGMENT_BYTES {
        eighth
    } else {
        MAX_SEGMENT_BYTES
    }
}
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

/// Where the spool key came from, which decides what happens to the segments
/// already in the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyOrigin {
    /// The key was stored before: segments sealed with it are read.
    Loaded,
    /// The key was just made: no segment can be read with it, so all are
    /// deleted and counted, and the cursor is reset.
    Created,
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
    /// Frames of this run's segment already handed out and not given back by a
    /// rewind.
    frames_read: u64,
    /// Logical position of the next unread byte; 0 while nothing has been read.
    read_offset: u64,
    /// File bytes before the read position, as the reader last reported it (an
    /// estimate after a resume, until the reader opens the segment).
    read_disk: u64,
    /// Frames of this segment reported delivered.
    frames_done: u64,
    /// Logical position after the last delivered frame; 0 while none is
    /// delivered. Only the first segment has delivered data.
    done_offset: u64,
    /// File bytes before the delivered position (an estimate after a partial
    /// delivery).
    done_disk: u64,
    /// The reader has finished the segment (the end record, the end of the file, a cut-off
    /// or corrupt rest, or a read error). It is deleted once everything read from
    /// it is delivered. A rewind clears this, because the reader must read the
    /// segment again from the delivered position.
    ended: bool,
    /// The reader is at the writer position of the active segment: nothing more
    /// to read until the next append.
    at_end: bool,
    /// The logical position where the readable data of the segment ends, when
    /// the rest was counted as dropped (a cut-off or corrupt tail, a read error,
    /// or a bad header). A rewind keeps it, so the reader stops there and counts
    /// nothing again.
    cut: Option<u64>,
    /// Frames and file bytes counted in `spool_dropped` when `cut` was set, so a
    /// later eviction does not count them again.
    lost_frames: u64,
    lost_bytes: u64,
    /// When the segment was created. Used only for the age of the backlog.
    created: Created,
    /// True while the file may hold bytes past `bytes` that are not good data
    /// (the active segment, or one closed after a write error). A cleanly
    /// sealed segment or a leftover is read to the end of the file.
    bounded: bool,
}

impl SegmentInfo {
    /// A segment with nothing read or delivered.
    const fn new(
        sequence: u64,
        bytes: u64,
        frames: Option<u64>,
        created: Created,
        bounded: bool,
    ) -> Self {
        Self {
            sequence,
            bytes,
            frames,
            frames_read: 0,
            read_offset: 0,
            read_disk: 0,
            frames_done: 0,
            done_offset: 0,
            done_disk: 0,
            ended: false,
            at_end: false,
            cut: None,
            lost_frames: 0,
            lost_bytes: 0,
            created,
            bounded,
        }
    }
}

/// A consumed or evicted segment file whose delete failed. It still counts in
/// `total_bytes`; the delete is retried, and the next start removes it because
/// its sequence is below the cursor's.
struct PendingDelete {
    path: PathBuf,
    bytes: u64,
}

/// A batch that [`Spool::read_batch`] returned and the caller has not yet
/// reported with [`Spool::delivered`] or given back with [`Spool::rewind`].
#[derive(Clone, Copy, Debug)]
struct Span {
    sequence: u64,
    /// Logical position of the first byte of the batch.
    start: u64,
    /// Logical position after the batch.
    end: u64,
    /// File positions of `start` and `end`.
    disk_start: u64,
    disk_end: u64,
    frames: u32,
}

/// The spool: segments on disk, one writer, one reader.
pub(crate) struct Spool {
    config: SpoolConfig,
    /// The one spool key, encrypted in its locked page between uses, shared
    /// with every writer and reader.
    key: Arc<SegmentKey>,
    /// The writer's record buffer, reused for every append and segment.
    record_buf: Vec<u8>,
    stats: Arc<SinkStats>,
    free_space: fn(&Path) -> io::Result<u64>,
    /// Oldest first. When `writer` is set, the last entry is its segment. The
    /// first segment holds the delivered position; the reader is in the first
    /// segment that has not `ended`.
    segments: VecDeque<SegmentInfo>,
    writer: Option<SegmentWriter>,
    /// The reader for the segment being read, tagged with its sequence.
    reader: Option<(u64, SegmentReader)>,
    /// Batches read and not yet delivered, in read order.
    outstanding: VecDeque<Span>,
    cursor: CursorFile,
    /// The delivered position moved since the cursor file was last written.
    cursor_dirty: bool,
    /// No cursor write until this time: the last one failed. Reads continue.
    cursor_blocked_until: Option<Instant>,
    pending: Vec<PendingDelete>,
    next_sequence: u64,
    total_bytes: u64,
    segment_bytes: u64,
    /// No appends until this time: a write error happened.
    suspended_until: Option<Instant>,
    /// No appends until this time: the free-space floor left nothing to delete.
    blocked_until: Option<Instant>,
    /// No reads until this time: a transient read error happened. Appends
    /// continue.
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
    /// The key was lost for the rest of the run (a memory protection failure
    /// zeroed it). The spool is suspended: it refuses every block before it
    /// deletes or creates anything, reads nothing, and keeps its files and the
    /// cursor for the next start, which unwraps the key again.
    key_lost: bool,
}

impl Spool {
    /// Opens the spool folder: finds the segments an earlier run left, deletes
    /// and counts those `key` cannot read (all of them when the key's `origin`
    /// is [`KeyOrigin::Created`], which also resets the cursor), applies the
    /// cursor, and starts without an active segment (new writes make a new
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
        key: Arc<SegmentKey>,
        origin: KeyOrigin,
        stats: Arc<SinkStats>,
        free_space: fn(&Path) -> io::Result<u64>,
    ) -> io::Result<Self> {
        let mut cursor_state = cursor::read(&config.dir)?;
        let scanned = scan(&config.dir)?;
        // Sequence numbers never repeat: stay above every name seen, whatever its
        // type, and above the cursor's sequence (that segment may be deleted).
        let mut next_sequence = scanned.highest.map_or(1, |highest| highest + 1);
        if let CursorRead::Valid(cursor) = &cursor_state {
            next_sequence = next_sequence.max(cursor.sequence.saturating_add(1));
        }
        let segments = scanned.segments;
        let total_bytes = segments.iter().map(|segment| segment.bytes).sum();
        let segment_bytes = segment_bytes_for(config.max_bytes);
        let mut spool = Self {
            cursor: CursorFile::new(&config.dir),
            config,
            key,
            record_buf: Vec::new(),
            stats,
            free_space,
            segments,
            writer: None,
            reader: None,
            outstanding: VecDeque::new(),
            cursor_dirty: false,
            cursor_blocked_until: None,
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
            key_lost: false,
        };
        let created = origin == KeyOrigin::Created;
        spool.drop_unreadable(created);
        if created {
            // The positions in the old key's segments mean nothing now.
            if cursor_state != CursorRead::Missing {
                spool.save_cursor(spool.next_sequence);
            }
            cursor_state = CursorRead::Missing;
        }
        spool.apply_cursor(cursor_state);
        spool.stats.spool_available.store(true, Ordering::Relaxed);
        spool.update_gauges();
        Ok(spool)
    }

    /// True while segments hold data that is not yet delivered: unread, or read
    /// and held. Routing must append, not write directly, while this is true, to
    /// keep frames in order.
    ///
    /// False once the key is lost: this run cannot read the backlog, so live
    /// frames go straight to the receiver while it is up, and the files wait
    /// for the next start ([`Self::keeps_segments`]).
    pub(crate) fn has_backlog(&self) -> bool {
        !self.is_suspended() && !self.segments.is_empty()
    }

    /// True once the key is lost for the run (see [`Self::key_lost`]): the
    /// spool takes no block and reads nothing until the service restarts. Also
    /// true before the spool's next operation has noticed the loss.
    pub(crate) fn is_suspended(&self) -> bool {
        self.key_lost || self.key.is_lost()
    }

    /// True while segment files stay in the folder for the next start, also
    /// when this run cannot read them.
    pub(crate) fn keeps_segments(&self) -> bool {
        !self.segments.is_empty()
    }

    /// True when the reader is before the end of the data: a read may return
    /// frames. False while everything is held, or the reader waits for the next
    /// append.
    pub(crate) fn has_unread(&self) -> bool {
        !self.is_suspended()
            && self
                .segments
                .iter()
                .any(|segment| !segment.ended && !segment.at_end)
    }

    /// Bytes of segment files that are not yet read: the bytes of the segments
    /// the reader has not finished, less the part of the first of them already
    /// handed out. Unlike the `spool_bytes` gauge it falls as the replay reads,
    /// not when a whole segment is deleted, so it shows whether the backlog is
    /// growing or draining. A rewind raises it.
    pub(crate) fn unread_bytes(&self) -> u64 {
        let mut open = self.segments.iter().filter(|segment| !segment.ended);
        let Some(first) = open.next() else {
            return 0;
        };
        let rest: u64 = open.map(|segment| segment.bytes).sum();
        first.bytes - first.read_disk.min(first.bytes) + rest
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
    /// floor, and for good once the key is lost. Also refreshes the
    /// `spool_available` flag.
    pub(crate) fn is_usable(&self) -> bool {
        let now = Instant::now();
        let usable = !self.is_suspended()
            && [self.suspended_until, self.blocked_until]
                .into_iter()
                .flatten()
                .all(|until| now >= until);
        self.stats.spool_available.store(usable, Ordering::Relaxed);
        usable
    }

    /// True while reads are paused after a transient read error, and for good
    /// once the key is lost. [`Self::read_batch`] then returns 0 although a
    /// backlog remains.
    pub(crate) fn reads_paused(&self) -> bool {
        self.is_suspended()
            || self
                .read_paused_until
                .is_some_and(|until| Instant::now() < until)
    }

    /// Appends a block of complete data frames (`frames` of them) to the active
    /// segment, starting a new one when needed. Deletes the oldest segments to
    /// stay under the size cap and the free-space floor.
    ///
    /// The block is sealed as one encrypted record. A block over 1 MiB, which
    /// the router never sends, is split at frame boundaries into one record per
    /// 1 MiB.
    ///
    /// A refused block is counted in `spool_dropped`: the spool is paused, the
    /// floor leaves nothing to delete, the block can never fit, it is empty but
    /// claims frames, it is over 1 MiB and does not split into whole frames, or
    /// an I/O error occurred. An I/O error also pauses the spool for 10 s.
    ///
    /// Once the key is lost every block is refused at once: nothing is evicted
    /// and no segment is created. A key failure that leaves the key usable (a
    /// failed `CryptUnprotectMemory`) is an ordinary append error.
    pub(crate) fn append(&mut self, bytes: &[u8], frames: u32) -> Appended {
        if bytes.is_empty() {
            return if frames == 0 {
                Appended::Stored
            } else {
                self.refuse(frames)
            };
        }
        if self.key_lost() {
            return self.refuse(frames);
        }
        let now = Instant::now();
        let paused = [self.suspended_until, self.blocked_until]
            .into_iter()
            .flatten()
            .any(|until| now < until);
        let Some(sealed) = segment::sealed_len(bytes) else {
            return self.refuse(frames);
        };
        // A block must fit with its segment's header and end record.
        let fits = sealed.saturating_add(HEADER_LEN + END_LEN) <= self.config.max_bytes;
        if paused || !fits {
            return self.refuse(frames);
        }
        match self.try_append(bytes, sealed, frames) {
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
            Err(_) if self.key_lost() => self.refuse(frames),
            Err(error) => {
                self.note_error(&error);
                // Close the active segment without its end record and pause the
                // spool.
                self.writer = None;
                self.suspended_until = Some(Instant::now() + self.pause);
                self.stats.spool_available.store(false, Ordering::Relaxed);
                self.refuse(frames)
            }
        }
    }

    /// Reads whole data frames from the segment being read into `out`, up to
    /// about `limit` bytes (at least one frame when any is waiting), and returns
    /// the frame count. The batch is **held**: it stays on disk and counts as
    /// read until the caller reports it with [`Self::delivered`] or gives it
    /// back with [`Self::rewind`]. The cursor file is not written here.
    ///
    /// Returns 0 when the spool has nothing to read now: it is empty, everything
    /// is held, reads are paused, or the call only finished a segment (call
    /// again). A segment that is finished, cut off, or corrupt is marked ended
    /// and counted; it is deleted once everything read from it is delivered.
    ///
    /// # Errors
    /// Returns the I/O error. After an error the spool pauses reads for 10 s, so
    /// a caller that retries at once gets 0.
    /// - A sharing or lock violation (another process holds the file) keeps the
    ///   segment. The third failure in a row on one segment ends it, with its
    ///   unread data counted as dropped.
    /// - Any other read error ends the segment the same way.
    pub(crate) fn read_batch(&mut self, out: &mut Vec<u8>, limit: usize) -> io::Result<u32> {
        if self.key_lost() || self.reads_paused() {
            return Ok(0);
        }
        loop {
            let Some(first) = self.segments.iter().position(|segment| !segment.ended) else {
                self.retry_pending();
                return Ok(0);
            };
            let sequence = self.segments[first].sequence;
            if let Err(error) = self.ensure_reader(first) {
                return self.read_error(error, sequence);
            }
            // `ensure_reader` ends, and may delete, a segment with a bad header.
            let Some(index) = self
                .segments
                .iter()
                .position(|segment| segment.sequence == sequence)
            else {
                continue;
            };
            let current = &self.segments[index];
            if current.ended {
                continue;
            }
            let bounds = self.bounds(index);
            let Some((_, active_reader)) = self.reader.as_mut() else {
                return Ok(0);
            };
            let before = active_reader.position();
            let disk_before = active_reader.file_position();
            let batch = match active_reader.read_batch(out, limit, bounds) {
                Ok(batch) => batch,
                Err(error) => return self.read_error(error, sequence),
            };
            let position = active_reader.position();
            let disk = active_reader.file_position();
            if batch.frames > 0 {
                self.transient = None;
                let read_from = &mut self.segments[index];
                read_from.frames_read += u64::from(batch.frames);
                read_from.read_offset = position;
                read_from.read_disk = disk;
                self.outstanding.push_back(Span {
                    sequence,
                    start: before,
                    end: position,
                    disk_start: disk_before,
                    disk_end: disk,
                    frames: batch.frames,
                });
                self.note_end(index, &batch);
                return Ok(batch.frames);
            }
            self.note_end(index, &batch);
            let after = &self.segments[index];
            let (ended, caught_up) = (after.ended, after.at_end);
            if ended || caught_up {
                // Nothing is left in this segment. Delete it if everything read
                // from it is delivered.
                self.sweep();
            }
            if ended {
                continue;
            }
            if caught_up {
                self.retry_pending();
            }
            return Ok(0);
        }
    }

    /// Reports that the session fully handed the first `frames` frames of the
    /// oldest outstanding batch, `bytes` bytes, to the socket. The batch is read
    /// in order, so it is the front one. A whole batch passes it all; a failed
    /// session passes the frames before the cut and gives the rest back with
    /// [`Self::rewind`]. Counts the frames in `replayed`, and deletes the
    /// segments that the delivered position has passed.
    pub(crate) fn delivered(&mut self, bytes: usize, frames: u32) {
        self.pass_front(bytes, frames, true);
    }

    /// Passes the oldest outstanding batch whole without counting it in `replayed`:
    /// the caller counted its frames as lost, so the spool must not read them
    /// again, and no frame may be in two counters.
    pub(crate) fn written_off(&mut self) {
        self.pass_front(usize::MAX, u32::MAX, false);
    }

    /// Moves the delivered position past the first `frames` frames, `bytes` bytes,
    /// of the oldest outstanding batch. With `count`, adds the frames to `replayed`.
    fn pass_front(&mut self, bytes: usize, frames: u32, count: bool) {
        let Some(span) = self.outstanding.pop_front() else {
            return;
        };
        let frames = frames.min(span.frames);
        if let Some(segment) = self
            .segments
            .iter_mut()
            .find(|segment| segment.sequence == span.sequence)
        {
            if frames == span.frames {
                segment.done_offset = span.end;
                segment.done_disk = span.disk_end;
            } else {
                let taken = (bytes as u64).min(span.end - span.start);
                segment.done_offset = span.start + taken;
                segment.done_disk = (span.disk_start + taken).min(span.disk_end);
            }
            segment.frames_done += u64::from(frames);
        }
        if frames > 0 {
            if count {
                self.stats
                    .replayed
                    .fetch_add(u64::from(frames), Ordering::Relaxed);
            }
            self.cursor_dirty = true;
        }
        self.sweep();
    }

    /// Gives back everything held: the read position goes back to the delivered
    /// position, and the next [`Self::read_batch`] returns the frames after the
    /// last delivered one. Call it when the link has failed or is ending, after
    /// every batch it finished is reported. Nothing is counted: the frames are
    /// still in the spool. The cursor file needs no write, because it never
    /// passes the delivered position.
    pub(crate) fn rewind(&mut self) {
        let held = !self.outstanding.is_empty()
            || self.segments.iter().any(|segment| {
                segment.read_offset != segment.done_offset
                    || segment.frames_read != segment.frames_done
            });
        if !held {
            return;
        }
        self.outstanding.clear();
        for segment in &mut self.segments {
            segment.read_offset = segment.done_offset;
            segment.read_disk = segment.done_disk;
            segment.frames_read = segment.frames_done;
            segment.ended = false;
            segment.at_end = false;
        }
        // The reader goes back with the position when it is in the first
        // segment; otherwise the next read opens that segment again.
        let first = self
            .segments
            .front()
            .map(|segment| (segment.sequence, segment.done_offset));
        let bounds = if self.segments.is_empty() {
            Bounds::default()
        } else {
            self.bounds(0)
        };
        let mut key_failed = false;
        let kept = match (self.reader.as_mut(), first) {
            (Some((owner, reader)), Some((sequence, offset))) if *owner == sequence => {
                match reader.rewind(offset, bounds) {
                    Ok(()) => Some(reader.file_position()),
                    Err(error) => {
                        key_failed = is_key_error(&error);
                        None
                    }
                }
            }
            _ => None,
        };
        match (kept, self.segments.front_mut()) {
            (Some(disk), Some(front)) => front.read_disk = disk,
            _ => self.reader = None,
        }
        if key_failed {
            // A lost key suspends the spool here; a key that is still usable
            // only makes the next read open the segment again.
            self.key_lost();
        }
    }

    /// Writes the delivered position to the cursor file when it moved since the
    /// last write. Call it once per pass, after the pass applied every report.
    /// A failed write is counted and leaves the older position in the file, which
    /// only means that a crash repeats more frames. The spool tries again after
    /// the pause, and reads go on: they do not depend on the file.
    pub(crate) fn checkpoint(&mut self) {
        if !self.cursor_dirty {
            return;
        }
        let now = Instant::now();
        if self.cursor_blocked_until.is_some_and(|until| now < until) {
            return;
        }
        let Some(head) = self.segments.front() else {
            // The last segment was deleted and that wrote the cursor.
            self.cursor_dirty = false;
            return;
        };
        let cursor = Cursor {
            sequence: head.sequence,
            offset: head.done_offset,
        };
        match self.cursor.write(cursor) {
            Ok(()) => {
                self.cursor_dirty = false;
                self.cursor_blocked_until = None;
            }
            Err(error) => {
                self.note_error(&error);
                self.cursor_blocked_until = Some(now + self.pause);
            }
        }
    }

    /// Seals the active segment with its end record. Call it at shutdown. Writes the
    /// delivered position to the cursor file first, so the next run repeats
    /// nothing, whether or not batches are still held (no rewind is needed: the
    /// file never holds the read position). Segments stay on disk for the next
    /// run.
    ///
    /// # Errors
    /// Returns the I/O error; the segment is then closed without its end
    /// record, which a later run reads as an unsealed leftover.
    ///
    /// A lost key is not a seal error: the active segment was closed without
    /// its end record when the loss was found, or is closed so now.
    pub(crate) fn seal(&mut self) -> io::Result<()> {
        self.key_lost();
        self.cursor_blocked_until = None;
        self.checkpoint();
        match self.seal_active() {
            Err(error) if is_key_error(&error) && self.key_lost() => Ok(()),
            result => result.inspect_err(|error| self.note_error(error)),
        }
    }

    // ---- appending ----

    /// Returns [`FreeSpace::Met`] when the block is stored, and
    /// [`FreeSpace::Short`] when the free-space floor leaves nothing to delete.
    /// `sealed` is the file bytes of the block's records.
    ///
    /// The block is sealed before anything is deleted or created, so a key
    /// failure leaves the folder as it was: only a roll, whose end record is
    /// itself a use of the key, comes first.
    fn try_append(&mut self, bytes: &[u8], sealed: u64, frames: u32) -> io::Result<FreeSpace> {
        // Start a new segment when this block would overfill the active one.
        if self.writer.as_ref().is_some_and(|writer| {
            writer.len() > HEADER_LEN && writer.len() + sealed + END_LEN > self.segment_bytes
        }) {
            self.seal_active()?;
        }
        let mut prepared = match self.writer.as_mut() {
            Some(writer) => {
                writer.seal_block(bytes, &mut self.record_buf)?;
                None
            }
            None => Some(self.prepare_segment(bytes)?),
        };
        // Make room under the size cap, oldest first, skipping the segments that
        // hold read data. A new segment also needs its header, and the end record
        // is reserved so a sealed segment still fits.
        let had_writer = self.writer.is_some();
        while self.total_bytes + self.new_segment_bytes() + sealed + END_LEN > self.config.max_bytes
        {
            if !self.evict_oldest(false) {
                // Files that could not be deleted still count, and read data is
                // not deleted; try the files again.
                self.retry_pending();
                break;
            }
        }
        if had_writer && self.writer.is_none() {
            // The eviction removed the active segment: its sealed records are
            // void, so seal the block again for a new segment.
            prepared = Some(self.prepare_segment(bytes)?);
        }
        if let Some(segment) = prepared {
            let space = self.ensure_free_space()?;
            if space != FreeSpace::Met {
                return Ok(space);
            }
            self.create_segment(segment, bytes)?;
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(io::Error::other("no active spool segment"));
        };
        writer.write_sealed(&self.record_buf)?;
        let written = writer.len();
        if let Some(active) = self.segments.back_mut() {
            self.total_bytes += written - active.bytes;
            active.bytes = written;
            active.frames = active.frames.map(|total| total + u64::from(frames));
            // The reader has something new to read.
            active.at_end = false;
        }
        self.stats
            .spooled
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        self.update_gauges();
        Ok(FreeSpace::Met)
    }

    /// Deletes the oldest segments that hold no read data until the volume has
    /// `min_free_bytes` plus a segment free. It keeps the newest segment, and it
    /// stops when no other segment can go or deleting a segment does not raise
    /// the free space the probe reports (something else uses the space). Returns
    /// [`FreeSpace::Short`] with the last free space the probe reported when the
    /// floor is still not met.
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
            if !self.evict_oldest(true) {
                return Ok(FreeSpace::Short { free, needed });
            }
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

    /// A new segment for the next sequence number, with `bytes` sealed into the
    /// record buffer. The sequence is not taken and no file is made yet.
    fn prepare_segment(&mut self, bytes: &[u8]) -> io::Result<PreparedSegment> {
        if self.next_sequence > MAX_SEQUENCE {
            return Err(io::Error::other(
                "spool segment sequence numbers are used up",
            ));
        }
        let mut segment = PreparedSegment::new(Arc::clone(&self.key), self.next_sequence)?;
        segment.seal_block(bytes, &mut self.record_buf)?;
        Ok(segment)
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

    /// Creates the file of `prepared`, whose first block (`bytes`) is sealed in
    /// the record buffer. A name that is already taken, by an entry the scan
    /// did not see, is skipped; the block is then sealed again for the next
    /// sequence number.
    fn create_segment(&mut self, prepared: PreparedSegment, bytes: &[u8]) -> io::Result<()> {
        self.retry_pending();
        let mut next = Some(prepared);
        for _ in 0..CREATE_ATTEMPTS {
            let segment = match next.take() {
                Some(segment) if segment.sequence() == self.next_sequence => segment,
                _ => self.prepare_segment(bytes)?,
            };
            let sequence = self.take_sequence()?;
            let path = self.segment_path(sequence);
            match segment.create(&path) {
                Ok(writer) => {
                    self.writer = Some(writer);
                    self.segments.push_back(SegmentInfo::new(
                        sequence,
                        HEADER_LEN,
                        Some(0),
                        Created::Run(Instant::now()),
                        true,
                    ));
                    self.total_bytes += HEADER_LEN;
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

    /// Writes the end record to the active segment and closes it. The writer is
    /// dropped even if the write fails.
    fn seal_active(&mut self) -> io::Result<()> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        let sealed = writer.seal(&mut self.record_buf)?;
        if let Some(active) = self.segments.back_mut() {
            self.total_bytes += sealed - active.bytes;
            active.bytes = sealed;
            active.bounded = false;
            // The file now ends with the end record, which the reader still has
            // to read.
            active.at_end = false;
        }
        self.update_gauges();
        Ok(())
    }

    /// Bytes a new segment adds before any data: zero while one is active.
    const fn new_segment_bytes(&self) -> u64 {
        if self.writer.is_some() { 0 } else { HEADER_LEN }
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
        if (blocks < 4 || blocks.is_multiple_of(60)) && !self.stats.is_stopping() {
            tracing::warn!(
                target: crate::event_ids::LOSS_SPOOL,
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
        if (errors < 4 || errors.is_multiple_of(60)) && !self.stats.is_stopping() {
            tracing::warn!(
                target: crate::event_ids::RISK_SPOOL_IO_ERROR,
                %error,
                "dnstap spool I/O error"
            );
        }
    }

    /// Handles a read error. A key error is not a problem of the segment: the
    /// segment, its read position, and the cursor stay as they are. When the
    /// key is lost, reads stop for the run without a count or an error;
    /// otherwise (a failed `CryptUnprotectMemory`) the error is counted and
    /// reads pause, as for a transient read error. Any other error goes to
    /// [`Self::read_failed`].
    fn read_error(&mut self, error: io::Error, sequence: u64) -> io::Result<u32> {
        if !is_key_error(&error) {
            return Err(self.read_failed(error, sequence));
        }
        // The reader may have moved past frames it did not hand out; the next
        // read opens the segment again at the recorded read position.
        self.reader = None;
        if self.key_lost() {
            return Ok(0);
        }
        self.note_error(&error);
        self.read_paused_until = Some(Instant::now() + self.pause);
        Err(error)
    }

    /// True once the key is lost for the run. The first call that finds it
    /// lost suspends the spool: the active segment is closed without its end
    /// record (the next start reads it as an unsealed leftover), the reader is
    /// dropped, `spool_available` goes false, and one warning names the cause.
    /// No file is deleted and the cursor is not written.
    fn key_lost(&mut self) -> bool {
        if self.key_lost {
            return true;
        }
        let Some(loss) = self.key.loss() else {
            return false;
        };
        self.key_lost = true;
        self.writer = None;
        self.reader = None;
        self.stats.spool_available.store(false, Ordering::Relaxed);
        if !self.stats.is_stopping() {
            tracing::warn!(
                target: crate::event_ids::RISK_SPOOL_DISABLED,
                "the spool key is unusable after a memory protection failure ({loss}); the spool \
                 is suspended until the service restarts; spooled files are kept"
            );
        }
        true
    }

    /// Handles a read error: counts it and drops the reader. A transient error
    /// (another process holds the file) pauses reads and keeps the segment, up to
    /// [`TRANSIENT_LIMIT`] failures in a row. Any other error ends the segment
    /// at the last good offset, with its unread data counted as dropped, so
    /// reads make progress. The file stays until the delivered position has
    /// passed the data already read from it.
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
        self.abandon(sequence);
        error
    }

    // ---- reading and removing ----

    /// How far the reader may read the segment at `index`: up to the writer for
    /// a segment still being written, and up to where a dropped rest starts.
    fn bounds(&self, index: usize) -> Bounds {
        let segment = &self.segments[index];
        Bounds {
            limit: segment.bounded.then_some(segment.bytes),
            stop_at: segment.cut,
        }
    }

    /// Opens the reader for the segment at `index` if it is not open. A segment
    /// whose header is missing or wrong is ended and all its bytes are counted.
    fn ensure_reader(&mut self, index: usize) -> io::Result<()> {
        let segment = &self.segments[index];
        let (sequence, offset) = (segment.sequence, segment.read_offset);
        if self
            .reader
            .as_ref()
            .is_some_and(|(owner, _)| *owner == sequence)
        {
            return Ok(());
        }
        self.reader = None;
        let path = self.segment_path(sequence);
        let bounds = self.bounds(index);
        if let Some(reader) =
            SegmentReader::open(&path, Arc::clone(&self.key), sequence, offset, bounds)?
        {
            if offset > 0 {
                // The reader walked to the logical offset, so the file position
                // is known now.
                self.segments[index].read_disk = reader.file_position();
            }
            self.reader = Some((sequence, reader));
        } else {
            let unreadable = &mut self.segments[index];
            unreadable.read_offset = 0;
            unreadable.read_disk = 0;
            self.abandon(sequence);
        }
        Ok(())
    }

    /// Applies why a read stopped to the segment at `index`. A cut-off or
    /// corrupt rest is counted and the segment ends there; a finished segment
    /// ends; the reader at the writer position of the active segment waits for
    /// the next append.
    fn note_end(&mut self, index: usize, batch: &segment::Batch) {
        self.count_lost(index, batch);
        let active = self.writer.is_some() && index + 1 == self.segments.len();
        let cut = self.segments[index].cut.is_some();
        match batch.end {
            Some(end) if end.is_terminal() => self.end_segment(index),
            Some(EndReason::WriterPosition) if cut => self.end_segment(index),
            Some(EndReason::WriterPosition) if active => self.segments[index].at_end = true,
            // A segment closed after a write error ends at its good length.
            Some(EndReason::WriterPosition) => self.end_segment(index),
            _ => {}
        }
    }

    /// Marks the segment at `index` as finished by the reader. The active
    /// segment gets no more appends: its writer is closed, as after a write
    /// error, so new blocks start a new segment.
    fn end_segment(&mut self, index: usize) {
        if index + 1 == self.segments.len() {
            self.writer = None;
        }
        self.segments[index].ended = true;
    }

    /// Counts the file bytes and frames a read skipped in a cut-off or corrupt
    /// tail, and ends the segment where the tail starts. The segment keeps that
    /// logical position, so a later eviction does not count them again and a
    /// read after a rewind stops there without counting. A segment from this run
    /// knows its frame total, so the frames lost are those not yet read; a
    /// leftover reports one for a cut-off final record.
    fn count_lost(&mut self, index: usize, batch: &segment::Batch) {
        if batch.dropped_bytes == 0 {
            return;
        }
        // A segment with a `cut` is read only up to it, so a read after a rewind
        // does not meet the tail again and this runs once for a tail.
        let segment = &mut self.segments[index];
        let frames = match segment.frames {
            Some(total) => total.saturating_sub(segment.frames_read),
            None => u64::from(batch.dropped_frames),
        };
        segment.cut = Some(batch.lost_at);
        segment.lost_frames = frames;
        segment.lost_bytes = batch.dropped_bytes;
        self.end_segment(index);
        self.note_discard(frames, batch.dropped_bytes);
        self.stats
            .spool_dropped
            .fetch_add(frames, Ordering::Relaxed);
        self.stats
            .spool_dropped_bytes
            .fetch_add(batch.dropped_bytes, Ordering::Relaxed);
    }

    /// Ends a segment that cannot be read (bad header, read error, or an invalid
    /// cursor) at its read offset, and counts its unread data as dropped. It
    /// logs as a discard of unreadable data, not as an eviction for space. The
    /// data already read stays until it is delivered; the segment is deleted
    /// then, or at once when nothing is held.
    fn abandon(&mut self, sequence: u64) {
        let Some(index) = self
            .segments
            .iter()
            .position(|segment| segment.sequence == sequence)
        else {
            return;
        };
        self.end_segment(index);
        let segment = &mut self.segments[index];
        if segment.cut.is_none() {
            let frames = segment
                .frames
                .map_or(0, |total| total.saturating_sub(segment.frames_read));
            let bytes = segment.bytes.saturating_sub(segment.read_disk);
            segment.cut = Some(segment.read_offset);
            segment.lost_frames = frames;
            segment.lost_bytes = bytes;
            self.stats
                .spool_dropped
                .fetch_add(frames, Ordering::Relaxed);
            self.stats
                .spool_dropped_bytes
                .fetch_add(bytes, Ordering::Relaxed);
            self.note_discard(frames, bytes);
        }
        self.sweep();
    }

    /// Deletes the oldest segment that holds no read data and counts its unread
    /// data as dropped. With `keep_last`, the newest segment is not a candidate.
    /// A leftover segment adds only bytes, because its frame count is unknown.
    /// Returns false when there is no candidate.
    fn evict_oldest(&mut self, keep_last: bool) -> bool {
        let candidates = self.segments.len() - usize::from(keep_last && !self.segments.is_empty());
        let Some(index) = self
            .segments
            .iter()
            .take(candidates)
            // Read data is held until it is delivered: it is not deleted for space.
            .position(|segment| !segment.ended && segment.read_offset <= segment.done_offset)
        else {
            return false;
        };
        let segment = &self.segments[index];
        let frames = segment.frames.map_or(0, |total| {
            total
                .saturating_sub(segment.frames_read)
                .saturating_sub(segment.lost_frames)
        });
        let bytes = segment
            .bytes
            .saturating_sub(segment.read_disk)
            .saturating_sub(segment.lost_bytes);
        self.stats
            .spool_dropped
            .fetch_add(frames, Ordering::Relaxed);
        self.stats
            .spool_dropped_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        if frames > 0 || bytes > 0 {
            // Counts only, never data. Bounded like the I/O error log.
            let evictions = self.evictions;
            self.evictions += 1;
            if (evictions < 4 || evictions.is_multiple_of(60)) && !self.stats.is_stopping() {
                tracing::warn!(
                    target: crate::event_ids::LOSS_SPOOL,
                    frames,
                    bytes,
                    "dnstap spool deleted its oldest segment unsent"
                );
            }
        }
        // Deleting the first segment moves the cursor past it; a segment further
        // on leaves the cursor where it is, ahead of nothing it holds.
        self.remove_segment(index, index == 0);
        true
    }

    /// Deletes the first segments while everything read from them is delivered:
    /// the reader has ended them, or it waits at the writer position of the only
    /// segment, the active one. A segment with read data that is not yet
    /// delivered stays.
    fn sweep(&mut self) {
        while let Some(head) = self.segments.front() {
            let done = head.frames_done == head.frames_read;
            let active_only = self.writer.is_some() && self.segments.len() == 1 && head.at_end;
            if !done || !(head.ended || active_only) {
                break;
            }
            self.remove_segment(0, true);
        }
    }

    /// Logs unread data discarded because it cannot be read: the first four
    /// times, then every 60th. Counts only, never data.
    fn note_discard(&mut self, frames: u64, bytes: u64) {
        if frames == 0 && bytes == 0 {
            return;
        }
        let discards = self.discards;
        self.discards += 1;
        if (discards < 4 || discards.is_multiple_of(60)) && !self.stats.is_stopping() {
            tracing::warn!(
                target: crate::event_ids::LOSS_SPOOL,
                frames,
                bytes,
                "dnstap spool discarded unreadable segment data"
            );
        }
    }

    /// Closes and deletes the segment at `index`. With `move_cursor` (the first
    /// segment only), the cursor is first moved to the start of the next
    /// segment, or to the next unused sequence when none is left: the delivered
    /// position has passed this segment. The cursor is never deleted. A file that
    /// cannot be deleted keeps counting in `total_bytes` and is retried.
    fn remove_segment(&mut self, index: usize, move_cursor: bool) {
        let Some(segment) = self.segments.get(index) else {
            return;
        };
        let sequence = segment.sequence;
        if move_cursor {
            let target = self
                .segments
                .get(index + 1)
                .map_or(self.next_sequence, |next| next.sequence);
            self.save_cursor(target);
        }
        // Close this segment's handles before deleting its file.
        if self
            .reader
            .as_ref()
            .is_some_and(|(owner, _)| *owner == sequence)
        {
            self.reader = None;
        }
        if index + 1 == self.segments.len() {
            self.writer = None;
        }
        if let Some(removed) = self.segments.remove(index) {
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

    /// Records `(sequence, 0)`, the start of that segment, in the cursor file.
    /// A failed write is counted; the older position stays in the file and the
    /// next write tries again.
    fn save_cursor(&mut self, sequence: u64) {
        let cursor = Cursor {
            sequence,
            offset: 0,
        };
        match self.cursor.write(cursor) {
            Ok(()) => self.cursor_dirty = false,
            Err(error) => self.note_error(&error),
        }
    }

    // ---- startup ----

    /// Applies what `cursor.bin` held, the delivered position of the last run,
    /// to the leftover segments. Segments below the cursor's sequence were
    /// delivered before the last run ended and are deleted uncounted (a delete
    /// that fails is retried). The cursor's own segment, if present, resumes at
    /// the cursor's logical offset. If it is missing (deleted, unreadable, or
    /// older than the oldest segment), reading starts at the start of the next
    /// segment: the segments between were delivered. A cursor that is invalid
    /// is a damaged file, not a crash: the position is lost, so the segment that
    /// was being read is deleted and counted as dropped, instead of being sent
    /// again in full. An offset that is not a frame boundary of its segment is
    /// found by the reader, which counts the rest of the segment as corrupt.
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
                    self.remove_segment(0, false);
                }
                let is_cursor_segment = self
                    .segments
                    .front()
                    .is_some_and(|segment| segment.sequence == cursor.sequence);
                if is_cursor_segment
                    && cursor.offset > 0
                    && let Some(head) = self.segments.front_mut()
                {
                    head.read_offset = cursor.offset;
                    head.done_offset = cursor.offset;
                    // An estimate: the reader knows the file position once it
                    // opens the segment.
                    let disk = HEADER_LEN.saturating_add(cursor.offset).min(head.bytes);
                    head.read_disk = disk;
                    head.done_disk = disk;
                }
            }
        }
    }

    /// Deletes the oldest segment as unreadable and counts all its bytes.
    /// Returns false when there is no segment.
    fn drop_unreadable_oldest(&mut self) -> bool {
        let Some(head) = self.segments.front_mut() else {
            return false;
        };
        head.read_offset = 0;
        head.read_disk = 0;
        head.done_offset = 0;
        head.done_disk = 0;
        let sequence = head.sequence;
        self.abandon(sequence);
        true
    }

    /// Deletes the leftover segments the key cannot read, or every segment with
    /// `all`, and counts their file bytes in `spool_dropped_bytes` (their frame
    /// count is unknown). Logs one warning with the counts when any went. A
    /// segment whose header cannot be read now stays; the reader handles it.
    fn drop_unreadable(&mut self, all: bool) {
        let (mut earlier_format, mut other_key, mut segments, mut bytes) = (0_u64, 0_u64, 0_u64, 0);
        let mut kept = VecDeque::with_capacity(self.segments.len());
        for segment in std::mem::take(&mut self.segments) {
            let path = self.segment_path(segment.sequence);
            match segment::check_header(&path, &self.key) {
                Ok(HeaderCheck::NotThisFormat) => earlier_format += 1,
                Ok(HeaderCheck::OtherKey) => other_key += 1,
                Ok(HeaderCheck::Readable | HeaderCheck::TooShort) | Err(_) if !all => {
                    kept.push_back(segment);
                    continue;
                }
                Ok(HeaderCheck::Readable | HeaderCheck::TooShort) | Err(_) => {}
            }
            segments += 1;
            bytes += segment.bytes;
            if let Err(error) = remove_file(&path) {
                self.note_error(&error);
                self.pending.push(PendingDelete {
                    path,
                    bytes: segment.bytes,
                });
            } else {
                self.total_bytes = self.total_bytes.saturating_sub(segment.bytes);
            }
        }
        self.segments = kept;
        self.update_gauges();
        if segments == 0 {
            return;
        }
        self.stats
            .spool_dropped_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        if !self.stats.is_stopping() {
            tracing::warn!(
                target: crate::event_ids::LOSS_SPOOL,
                segments,
                bytes,
                earlier_format,
                other_key,
                new_key = all,
                "dnstap spool deleted segments it cannot read at startup: segments of an \
                earlier version or of another spool key"
            );
        }
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
        .map(|(sequence, bytes, created)| SegmentInfo::new(sequence, bytes, None, created, false))
        .collect();
    Ok(Scan { segments, highest })
}

/// True when `error` carries a failure of the spool key itself (see
/// [`CryptError::is_key_error`]), not of the file or the record.
fn is_key_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<CryptError>())
        .copied()
        .is_some_and(CryptError::is_key_error)
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
