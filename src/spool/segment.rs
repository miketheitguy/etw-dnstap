//! One spool segment file in the encrypted version 1 format of [`super::crypt`]:
//! the 32-byte file header, one AES-128-GCM data record per append, and the end
//! record when the segment is sealed.
//!
//! [`SegmentWriter`] creates a segment and seals each append as one record, built
//! in a reused buffer and written with one call. [`SegmentReader`] reads the
//! records back, possibly while the writer is still appending to the same file,
//! decrypts each one in place, hands out its Frame Streams data frames, and
//! reports why a read stopped.
//!
//! Two kinds of position exist. A **logical** position is a byte offset in the
//! segment's plaintext frame stream: the frames of every data record, one after
//! the other, with no header, record overhead, or control frame. Read and
//! delivered positions, and `cursor.bin`, are logical. A **file** position is a
//! byte offset in the file. Size accounting is in file bytes, which include the
//! header and the overhead of each record. Inside a record, the file position of
//! a frame is the start of the record for its first frame and the record's
//! header length plus the frame's offset otherwise, and the end of a record
//! includes its tag.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
    sync::Arc,
};

use ring::rand::{SecureRandom, SystemRandom};

use super::{
    crypt::{
        FILE_HEADER_LEN, FileHeader, HeaderError, MAX_PLAINTEXT_LEN, NONCE_PREFIX_LEN, Next,
        RECORD_OVERHEAD, RecordKind, RecordOpener, RecordSealer, SegmentKey, Skip,
    },
    fsinfo::is_plain_file,
};

/// File bytes of the segment header.
pub(super) const HEADER_LEN: u64 = FILE_HEADER_LEN as u64;
/// File bytes a record adds to its plaintext: its header and its tag.
pub(super) const RECORD_EXTRA: u64 = RECORD_OVERHEAD as u64;
/// File bytes of the end record, which seals a segment.
pub(super) const END_LEN: u64 = RECORD_EXTRA;
/// The longest data frame the spool accepts: the encoder's own limit.
const MAX_DATA_FRAME: usize = crate::dnstap::MAX_ENCODED_LENGTH;
/// Read-ahead for replay; a batch is a few hundred KiB.
const READ_AHEAD: usize = 64 * 1024;

/// The length of the next record's plaintext at the start of `block`: all of it
/// when it fits one record, or the whole frames that fit 1 MiB. `None` when the
/// block is longer than a record and its frames cannot be split.
fn next_chunk(block: &[u8]) -> Option<usize> {
    if block.len() <= MAX_PLAINTEXT_LEN {
        return Some(block.len());
    }
    let mut at = 0;
    loop {
        let length = frame_length(&block[at..])?;
        let end = at + 4 + length;
        if end > MAX_PLAINTEXT_LEN {
            return (at > 0).then_some(at);
        }
        at = end;
    }
}

/// File bytes the records for `block` take: one record, or one per 1 MiB of
/// whole frames for a larger block. `None` for an empty block or one that
/// cannot be split.
pub(super) fn sealed_len(block: &[u8]) -> Option<u64> {
    if block.is_empty() {
        return None;
    }
    let mut rest = block;
    let mut records = 0_u64;
    while !rest.is_empty() {
        let length = next_chunk(rest)?;
        records += 1;
        rest = &rest[length..];
    }
    Some(block.len() as u64 + records * RECORD_EXTRA)
}

/// The payload length of the whole data frame at the start of `bytes`, or
/// `None` when the length is zero, over the encoder's limit, or longer than the
/// bytes.
fn frame_length(bytes: &[u8]) -> Option<usize> {
    let (header, rest) = bytes.split_first_chunk::<4>()?;
    let length = usize::try_from(u32::from_be_bytes(*header)).ok()?;
    ((1..=MAX_DATA_FRAME).contains(&length) && length <= rest.len()).then_some(length)
}

/// Appends sealed records to a new segment file.
pub(super) struct SegmentWriter {
    file: File,
    sealer: RecordSealer,
    len: u64,
}

impl SegmentWriter {
    /// Creates the file with `create_new` (it never opens an existing file) and
    /// writes the file header with a new random nonce prefix. On failure the
    /// half-made file is removed.
    pub(super) fn create(path: &Path, key: Arc<SegmentKey>, sequence: u64) -> io::Result<Self> {
        let mut nonce_prefix = [0_u8; NONCE_PREFIX_LEN];
        SystemRandom::new()
            .fill(&mut nonce_prefix)
            .map_err(|_| io::Error::other("cannot make a random spool nonce prefix"))?;
        let sealer = RecordSealer::new(key, sequence, nonce_prefix);
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        if let Err(error) = file.write_all(&sealer.file_header()) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(Self {
            file,
            sealer,
            len: HEADER_LEN,
        })
    }

    /// Seals `framed`, which holds complete data frames, as one record (or one
    /// per 1 MiB of frames) in `buf` and writes it with one call. `buf` is reused
    /// between calls, so a warm buffer allocates nothing. After an error the
    /// length no longer describes the file; close the segment.
    pub(super) fn append(&mut self, framed: &[u8], buf: &mut Vec<u8>) -> io::Result<()> {
        buf.clear();
        let mut rest = framed;
        while !rest.is_empty() {
            let length = next_chunk(rest).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a spool block over 1 MiB does not split into whole frames",
                )
            })?;
            self.sealer
                .seal_data(&rest[..length], buf)
                .map_err(io::Error::other)?;
            rest = &rest[length..];
        }
        self.file.write_all(buf)?;
        self.len += buf.len() as u64;
        Ok(())
    }

    /// File length so far, including the header.
    pub(super) const fn len(&self) -> u64 {
        self.len
    }

    /// Writes the end record and returns the final file length.
    pub(super) fn seal(mut self, buf: &mut Vec<u8>) -> io::Result<u64> {
        buf.clear();
        self.sealer.seal_end(buf).map_err(io::Error::other)?;
        self.file.write_all(buf)?;
        Ok(self.len + buf.len() as u64)
    }

    /// The shared key this writer seals with.
    #[cfg(test)]
    pub(super) const fn key(&self) -> &Arc<SegmentKey> {
        self.sealer.key()
    }
}

/// What the file header of a segment says about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeaderCheck {
    /// A version 1 segment sealed with this key.
    Readable,
    /// Fewer than 32 bytes so far consistent with the format. The reader treats
    /// it as a segment with a bad header.
    TooShort,
    /// Not a version 1 segment: a plaintext segment of an earlier version, or
    /// other bytes.
    NotThisFormat,
    /// A version 1 segment sealed with another key.
    OtherKey,
}

/// Reads the file header of the segment at `path` and checks it against `key`.
///
/// # Errors
/// Returns the I/O error of the open or the read.
pub(super) fn check_header(path: &Path, key: &SegmentKey) -> io::Result<HeaderCheck> {
    let mut header = Vec::with_capacity(FILE_HEADER_LEN);
    File::open(path)?
        .take(HEADER_LEN)
        .read_to_end(&mut header)?;
    Ok(match FileHeader::decode(&header) {
        Ok(found) if found.key_id == key.id() => HeaderCheck::Readable,
        Ok(_) => HeaderCheck::OtherKey,
        Err(HeaderError::TooShort) => HeaderCheck::TooShort,
        Err(HeaderError::NotThisFormat | HeaderError::OtherKey { .. }) => {
            HeaderCheck::NotThisFormat
        }
    })
}

/// Why a read stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndReason {
    /// The next data frame does not fit in the batch limit; more data remains.
    BatchFull,
    /// The end record: the segment is complete.
    Stop,
    /// The reader reached the writer's position, or the end of the readable
    /// data the caller set. More data may follow.
    WriterPosition,
    /// End of file at a record boundary without the end record (an unsealed
    /// leftover).
    EndOfFile,
    /// The file ends inside a record.
    CutOff,
    /// A record that does not authenticate, a bad record header, or a bad frame
    /// inside a record.
    Corrupt,
}

impl EndReason {
    /// True when no more frames can ever be read from this segment.
    pub(super) const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Stop | Self::EndOfFile | Self::CutOff | Self::Corrupt
        )
    }
}

/// The result of one [`SegmentReader::read_batch`] call.
#[derive(Debug, Default)]
pub(super) struct Batch {
    /// Data frames appended to the caller's buffer.
    pub(super) frames: u32,
    /// Plaintext bytes appended to the caller's buffer (length prefixes
    /// included).
    pub(super) bytes: u64,
    /// Why the read stopped.
    pub(super) end: Option<EndReason>,
    /// File bytes skipped because the segment ended in a cut-off or corrupt
    /// record.
    pub(super) dropped_bytes: u64,
    /// Frames skipped: one for a cut-off final record (it held at least one),
    /// none for corrupt data, whose frame count is unknown.
    pub(super) dropped_frames: u32,
    /// Logical position where the skipped data starts, when `dropped_bytes` is
    /// not zero.
    pub(super) lost_at: u64,
}

/// The decrypted record the reader hands frames out of.
struct Current {
    /// The plaintext in the reader's buffer.
    plain: Range<usize>,
    /// Buffer index of the next frame to hand out.
    next: usize,
    /// File bytes of the whole record.
    consumed: usize,
    /// Logical position of the plaintext's first byte.
    logical_start: u64,
}

/// Reads whole data frames from a segment.
pub(super) struct SegmentReader {
    file: File,
    key: Arc<SegmentKey>,
    header: [u8; FILE_HEADER_LEN],
    opener: RecordOpener,
    /// File bytes read ahead, from file offset `buf_offset`. A decrypted record
    /// holds its plaintext in place.
    buf: Vec<u8>,
    buf_offset: u64,
    /// Buffer index of the current record, or of the next record when there is
    /// none.
    head: usize,
    current: Option<Current>,
    /// Logical position to start at, until the record that holds it is open.
    resume: Option<u64>,
    /// Set once the segment can yield no more frames; later calls return it
    /// again without reading or reporting dropped bytes a second time.
    ended: Option<EndReason>,
    /// The end that positioning the reader met, which the next read returns.
    pending: Option<Batch>,
}

/// Where a read may go: `limit` is the writer's file length for a segment still
/// being written (`None` reads to the end of the file), and `stop_at` the
/// logical position where the readable data ends, when the rest was counted as
/// dropped.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Bounds {
    pub(super) limit: Option<u64>,
    pub(super) stop_at: Option<u64>,
}

impl SegmentReader {
    /// Opens a segment, checks its file header against `key` and the
    /// `sequence` in its file name, and positions the reader at logical
    /// position `offset`. Returns `None` when the header is missing, short, of
    /// another format, of another key, or of another sequence (a renamed file).
    ///
    /// Positioning walks the record headers before `offset` without decrypting
    /// them and decrypts the record that holds `offset`. An end it meets (a
    /// corrupt record, an offset inside a frame, or the end of the data) is
    /// returned by the first [`Self::read_batch`].
    pub(super) fn open(
        path: &Path,
        key: Arc<SegmentKey>,
        sequence: u64,
        offset: u64,
        bounds: Bounds,
    ) -> io::Result<Option<Self>> {
        if !is_plain_file(path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the segment path is not a regular file",
            ));
        }
        let mut file = File::open(path)?;
        let mut header = [0_u8; FILE_HEADER_LEN];
        match file.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        let Ok(opener) = RecordOpener::new(Arc::clone(&key), &header) else {
            return Ok(None);
        };
        if opener.sequence() != sequence {
            return Ok(None);
        }
        let mut reader = Self {
            file,
            key,
            header,
            opener,
            buf: Vec::new(),
            buf_offset: HEADER_LEN,
            head: 0,
            current: None,
            resume: None,
            ended: None,
            pending: None,
        };
        reader.position_at(offset, bounds)?;
        Ok(Some(reader))
    }

    /// Logical position of the next frame to hand out.
    pub(super) fn position(&self) -> u64 {
        self.current.as_ref().map_or_else(
            || self.opener.logical_offset(),
            |current| current.logical_start + (current.next - current.plain.start) as u64,
        )
    }

    /// File position of the next frame to hand out.
    pub(super) fn file_position(&self) -> u64 {
        let index = self.current.as_ref().map_or(self.head, |current| {
            if current.next == current.plain.start {
                self.head
            } else if current.next == current.plain.end {
                self.head + current.consumed
            } else {
                current.next
            }
        });
        self.buf_offset + index as u64
    }

    /// Moves back to logical position `position` and clears the end state. Used
    /// when a failed link gives back the batches it did not take, so the caller
    /// can read them again. A position inside the current record costs nothing;
    /// any other position walks the segment again from its first record.
    pub(super) fn rewind(&mut self, position: u64, bounds: Bounds) -> io::Result<()> {
        self.ended = None;
        self.pending = None;
        if let Some(current) = self.current.as_mut() {
            let end = current.logical_start + current.plain.len() as u64;
            if (current.logical_start..=end).contains(&position) {
                let skip = usize::try_from(position - current.logical_start).unwrap_or(usize::MAX);
                if let Ok(next) = walk_frames(&self.buf[current.plain.clone()], skip) {
                    current.next = current.plain.start + next;
                    return Ok(());
                }
            }
        }
        self.opener = RecordOpener::new(Arc::clone(&self.key), &self.header)
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.file.seek(SeekFrom::Start(HEADER_LEN))?;
        self.buf.clear();
        self.buf_offset = HEADER_LEN;
        self.head = 0;
        self.current = None;
        self.position_at(position, bounds)
    }

    /// Sets the resume target and, when it is past the start, walks to it now,
    /// so the file position is known. A terminal end it meets is kept for the
    /// next read.
    fn position_at(&mut self, offset: u64, bounds: Bounds) -> io::Result<()> {
        if offset == 0 {
            return Ok(());
        }
        self.resume = Some(offset);
        let mut none = Vec::new();
        let batch = self.read_frames(&mut none, 0, bounds, false)?;
        if batch.end.is_some_and(EndReason::is_terminal) {
            self.pending = Some(batch);
        }
        Ok(())
    }

    /// Appends whole data frames to `out` while the batch stays within
    /// `max_bytes` of plaintext (the first frame is always taken). `bounds`
    /// limits how far the read goes. An end record ends the segment.
    ///
    /// On an I/O error `out` is restored to its length at entry.
    pub(super) fn read_batch(
        &mut self,
        out: &mut Vec<u8>,
        max_bytes: usize,
        bounds: Bounds,
    ) -> io::Result<Batch> {
        if let Some(batch) = self.pending.take() {
            return Ok(batch);
        }
        if let Some(reason) = self.ended {
            return Ok(Batch {
                end: Some(reason),
                ..Batch::default()
            });
        }
        let start_len = out.len();
        let result = self.read_frames(out, max_bytes, bounds, true);
        if result.is_err() {
            out.truncate(start_len);
        }
        result
    }

    /// The read loop. With `take` false it only positions the reader: it returns
    /// with no end as soon as a frame is ready.
    fn read_frames(
        &mut self,
        out: &mut Vec<u8>,
        max_bytes: usize,
        bounds: Bounds,
        take: bool,
    ) -> io::Result<Batch> {
        let mut batch = Batch::default();
        let budget = max_bytes as u64;
        loop {
            if let Some(current) = self.current.as_mut()
                && current.next < current.plain.end
            {
                // The readable data can end inside a record: before a bad frame
                // that a read before a rewind already counted.
                let position = current.logical_start + (current.next - current.plain.start) as u64;
                if bounds.stop_at.is_some_and(|stop| position >= stop) {
                    batch.end = Some(EndReason::WriterPosition);
                    return Ok(batch);
                }
                if !take {
                    return Ok(batch);
                }
                let at = current.next;
                let Some(length) = frame_length(&self.buf[at..current.plain.end]) else {
                    return self.stop_dropping(batch, EndReason::Corrupt, bounds);
                };
                let frame = 4 + length;
                if batch.frames > 0 && batch.bytes + frame as u64 > budget {
                    batch.end = Some(EndReason::BatchFull);
                    return Ok(batch);
                }
                out.extend_from_slice(&self.buf[at..at + frame]);
                current.next += frame;
                batch.frames += 1;
                batch.bytes += frame as u64;
                continue;
            }
            // The current record is used up: the next record starts after it.
            if let Some(done) = self.current.take() {
                self.head += done.consumed;
            }
            if bounds
                .stop_at
                .is_some_and(|stop| self.opener.logical_offset() >= stop)
            {
                batch.end = Some(EndReason::WriterPosition);
                return Ok(batch);
            }
            if let Some(target) = self.resume
                && target > self.opener.logical_offset()
            {
                match self.opener.skip_to(&self.buf[self.head..], target) {
                    Skip::Found { consumed } => self.head += consumed,
                    Skip::Incomplete { consumed, needed } => {
                        self.head += consumed;
                        if !self.fill(needed, bounds.limit)? {
                            return self.out_of_data(batch, bounds);
                        }
                        continue;
                    }
                    Skip::Corrupt(_) => {
                        return self.stop_dropping(batch, EndReason::Corrupt, bounds);
                    }
                }
            }
            match self.opener.open_next(&mut self.buf[self.head..]) {
                Next::Complete(record) if record.kind == RecordKind::End => {
                    self.head += record.consumed;
                    self.resume = None;
                    self.ended = Some(EndReason::Stop);
                    batch.end = Some(EndReason::Stop);
                    return Ok(batch);
                }
                Next::Complete(record) => {
                    let plain =
                        self.head + record.plaintext.start..self.head + record.plaintext.end;
                    let mut current = Current {
                        next: plain.start,
                        plain,
                        consumed: record.consumed,
                        logical_start: record.logical_start,
                    };
                    let resume = self.resume.take();
                    let skip =
                        resume.map_or(0, |target| target.saturating_sub(record.logical_start));
                    let skip = usize::try_from(skip).unwrap_or(usize::MAX);
                    let walked = walk_frames(&self.buf[current.plain.clone()], skip);
                    current.next += match walked {
                        Ok(next) | Err(next) => next,
                    };
                    self.current = Some(current);
                    if walked.is_err() {
                        // The resume position is not a frame boundary.
                        return self.stop_dropping(batch, EndReason::Corrupt, bounds);
                    }
                }
                Next::Incomplete { needed } => {
                    if !self.fill(needed, bounds.limit)? {
                        return self.out_of_data(batch, bounds);
                    }
                }
                Next::Corrupt(_) => return self.stop_dropping(batch, EndReason::Corrupt, bounds),
            }
        }
    }

    /// Makes the buffer hold at least `needed` bytes from `head`, reading ahead
    /// up to `limit` (or to the end of the file). Returns false when the file or
    /// the limit ends first. Called only between records.
    fn fill(&mut self, needed: usize, limit: Option<u64>) -> io::Result<bool> {
        if self.buf.len() - self.head >= needed {
            return Ok(true);
        }
        if self.head > 0 {
            self.buf.drain(..self.head);
            self.buf_offset += self.head as u64;
            self.head = 0;
        }
        let mut want = needed.max(READ_AHEAD);
        if let Some(limit) = limit {
            let room = usize::try_from(limit.saturating_sub(self.buf_offset)).unwrap_or(usize::MAX);
            want = want.min(room);
        }
        let mut filled = self.buf.len();
        if want > filled {
            self.buf.resize(want, 0);
            while filled < want {
                match self.file.read(&mut self.buf[filled..]) {
                    Ok(0) => break,
                    Ok(read) => filled += read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        self.buf.truncate(filled);
                        return Err(error);
                    }
                }
            }
            self.buf.truncate(filled);
        }
        Ok(self.buf.len() >= needed)
    }

    /// The read met the end of the data before a whole record: the writer's
    /// position for a segment still being written, otherwise the end of a
    /// leftover, clean at a record boundary or inside a cut-off record. A file
    /// that ends before the writer's length was cut short under the reader, and
    /// its rest is cut off.
    fn out_of_data(&mut self, mut batch: Batch, bounds: Bounds) -> io::Result<Batch> {
        let read_to = self.buf_offset + self.buf.len() as u64;
        if let Some(limit) = bounds.limit {
            if read_to >= limit {
                batch.end = Some(EndReason::WriterPosition);
                return Ok(batch);
            }
            return self.stop_dropping(batch, EndReason::CutOff, bounds);
        }
        if self.buf.len() == self.head {
            self.ended = Some(EndReason::EndOfFile);
            batch.end = Some(EndReason::EndOfFile);
            return Ok(batch);
        }
        self.stop_dropping(batch, EndReason::CutOff, bounds)
    }

    /// Ends the segment at a cut-off or corrupt record or frame: every file byte
    /// from the next frame to the end (the limit or the end of the file) is
    /// dropped.
    fn stop_dropping(
        &mut self,
        mut batch: Batch,
        reason: EndReason,
        bounds: Bounds,
    ) -> io::Result<Batch> {
        let end = match bounds.limit {
            Some(limit) => limit,
            None => self.file.metadata()?.len(),
        };
        self.ended = Some(reason);
        self.resume = None;
        batch.end = Some(reason);
        batch.dropped_bytes = end.saturating_sub(self.file_position());
        batch.dropped_frames = u32::from(reason == EndReason::CutOff);
        batch.lost_at = self.position();
        Ok(batch)
    }

    /// The shared key this reader opens with.
    #[cfg(test)]
    pub(super) const fn key(&self) -> &Arc<SegmentKey> {
        self.opener.key()
    }
}

/// Walks whole frames from the start of `plain` up to byte `skip`. `Ok` with
/// the offset `skip` when it is a frame boundary (or the end); `Err` with the
/// offset of the frame that holds `skip`, or of a bad frame on the way.
fn walk_frames(plain: &[u8], skip: usize) -> Result<usize, usize> {
    let mut at = 0;
    while at < skip {
        let Some(length) = frame_length(&plain[at..]) else {
            return Err(at);
        };
        if at + 4 + length > skip {
            return Err(at);
        }
        at += 4 + length;
    }
    if at == skip { Ok(at) } else { Err(at) }
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;
