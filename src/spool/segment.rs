//! One spool segment file: a unidirectional dnstap Frame Streams file (START,
//! data frames, STOP) that the spool writes and later reads back.
//!
//! [`SegmentWriter`] creates a segment and appends pre-framed data. [`SegmentReader`]
//! reads whole data frames back, possibly while the writer is still appending to
//! the same file, and reports why a read stopped.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use super::fsinfo::is_plain_file;
use crate::framestream::{
    CONTENT_TYPE, CONTROL_START, CONTROL_STOP, FIELD_CONTENT_TYPE, FrameStreamWriter,
};

/// Length of the START control frame the writer emits: escape, length, type,
/// content-type field header, and the 22-byte `protobuf:dnstap.Dnstap`.
const START_BYTES: usize = 42;
pub(super) const START_LEN: u64 = START_BYTES as u64;
/// Length of the STOP control frame.
pub(super) const STOP_LEN: u64 = 12;
/// The longest data frame the spool accepts: the encoder's own limit.
const MAX_DATA_FRAME: u64 = crate::dnstap::MAX_ENCODED_LENGTH as u64;
/// The longest control frame the reader accepts, as in the network handshake.
const MAX_CONTROL_BYTES: usize = 512;
const MAX_CONTROL_LENGTH: u64 = MAX_CONTROL_BYTES as u64;
/// Read-ahead for replay; a batch is a few hundred KiB.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Appends framed data to a new segment file.
pub(super) struct SegmentWriter {
    stream: FrameStreamWriter<File>,
    len: u64,
}

impl SegmentWriter {
    /// Creates the file with `create_new` (it never opens an existing file) and
    /// writes START. On failure the half-made file is removed.
    pub(super) fn create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let stream = FrameStreamWriter::new(file).inspect_err(|_error| {
            // The failed writer already dropped the handle.
            let _ = fs::remove_file(path);
        })?;
        Ok(Self {
            stream,
            len: START_LEN,
        })
    }

    /// Appends bytes that hold complete data frames. After an error the stream is
    /// poisoned and the length no longer describes the file; close the segment.
    pub(super) fn append(&mut self, framed: &[u8]) -> io::Result<()> {
        self.stream.write_framed(framed)?;
        self.len += framed.len() as u64;
        Ok(())
    }

    /// File length so far, including START.
    pub(super) const fn len(&self) -> u64 {
        self.len
    }

    /// Writes STOP and returns the final file length.
    pub(super) fn seal(self) -> io::Result<u64> {
        let len = self.len + STOP_LEN;
        self.stream.finish()?;
        Ok(len)
    }
}

/// Why a read stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndReason {
    /// The next data frame does not fit in the batch limit; more data remains.
    BatchFull,
    /// A STOP control frame: the segment is complete.
    Stop,
    /// The reader reached the writer's position. More data may follow.
    WriterPosition,
    /// End of file at a frame boundary without STOP (an unsealed leftover).
    EndOfFile,
    /// The file ends inside a frame.
    CutOff,
    /// A bad frame length or control frame.
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
    /// Bytes appended to the caller's buffer (length prefixes included).
    pub(super) bytes: u64,
    /// Why the read stopped.
    pub(super) end: Option<EndReason>,
    /// Bytes skipped because the segment ended in a cut-off or corrupt frame.
    pub(super) dropped_bytes: u64,
    /// Frames skipped: one for a cut-off final frame, none for corrupt data,
    /// whose frame count is unknown.
    pub(super) dropped_frames: u32,
}

/// Reads whole data frames from a segment.
pub(super) struct SegmentReader {
    file: BufReader<File>,
    /// Offset of the next unread byte.
    position: u64,
    /// Set once the segment can yield no more frames; later calls return it
    /// again without reading or reporting dropped bytes a second time.
    ended: Option<EndReason>,
}

impl SegmentReader {
    /// Opens a segment, checks its START, and positions the reader at `offset`
    /// (at least just after START). Returns `None` when START is missing or
    /// wrong, including a file shorter than START.
    pub(super) fn open(path: &Path, offset: u64) -> io::Result<Option<Self>> {
        if !is_plain_file(path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the segment path is not a regular file",
            ));
        }
        let mut file = BufReader::with_capacity(READ_BUFFER_BYTES, File::open(path)?);
        let mut start = [0_u8; START_BYTES];
        match file.read_exact(&mut start) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        if start.as_slice() != expected_start().as_slice() {
            return Ok(None);
        }
        let position = offset.max(START_LEN);
        if position > START_LEN {
            file.seek(SeekFrom::Start(position))?;
        }
        Ok(Some(Self {
            file,
            position,
            ended: None,
        }))
    }

    /// Offset of the next unread byte.
    pub(super) const fn position(&self) -> u64 {
        self.position
    }

    /// Moves back to `position` and clears the end state. Used when a batch
    /// could not be marked consumed, so the caller can read it again.
    pub(super) fn rewind(&mut self, position: u64) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(position))?;
        self.position = position;
        self.ended = None;
        Ok(())
    }

    /// Appends whole data frames to `out` while the batch stays within
    /// `max_bytes` (the first frame is always taken). Control frames count
    /// against the same budget, so one call reads about `max_bytes` of the file
    /// at most; a call that only skipped control frames returns no frames and
    /// `BatchFull`. `limit` is the writer's
    /// length for a segment still being written; `None` reads to the end of the
    /// file. A control frame is consumed: START is skipped, STOP ends the segment.
    ///
    /// On an I/O error `out` is restored to its length at entry.
    pub(super) fn read_batch(
        &mut self,
        out: &mut Vec<u8>,
        max_bytes: usize,
        limit: Option<u64>,
    ) -> io::Result<Batch> {
        if let Some(reason) = self.ended {
            return Ok(Batch {
                end: Some(reason),
                ..Batch::default()
            });
        }
        let start_len = out.len();
        let result = self.read_frames(out, max_bytes, limit);
        if result.is_err() {
            out.truncate(start_len);
        }
        result
    }

    fn read_frames(
        &mut self,
        out: &mut Vec<u8>,
        max_bytes: usize,
        limit: Option<u64>,
    ) -> io::Result<Batch> {
        let end = match limit {
            Some(limit) => limit,
            None => self.file.get_ref().metadata()?.len(),
        };
        let mut batch = Batch::default();
        let budget = max_bytes as u64;
        // File bytes consumed by this call, data and control frames alike.
        let mut spent = 0_u64;
        loop {
            let frame_start = self.position;
            let available = end.saturating_sub(frame_start);
            if available == 0 {
                if limit.is_some() {
                    batch.end = Some(EndReason::WriterPosition);
                } else {
                    self.ended = Some(EndReason::EndOfFile);
                    batch.end = Some(EndReason::EndOfFile);
                }
                return Ok(batch);
            }
            if spent > 0 && spent >= budget {
                batch.end = Some(EndReason::BatchFull);
                return Ok(batch);
            }
            let mut header = [0_u8; 4];
            if available < 4 || !self.fill(&mut header)? {
                return Ok(self.stop_dropping(batch, EndReason::CutOff, frame_start, end));
            }
            self.position += 4;
            let length = u64::from(u32::from_be_bytes(header));
            if length == 0 {
                match self.read_control(available)? {
                    Control::Start => {
                        spent += self.position - frame_start;
                        continue;
                    }
                    Control::Stop => {
                        self.ended = Some(EndReason::Stop);
                        batch.end = Some(EndReason::Stop);
                        return Ok(batch);
                    }
                    Control::CutOff => {
                        return Ok(self.stop_dropping(batch, EndReason::CutOff, frame_start, end));
                    }
                    Control::Corrupt => {
                        return Ok(self.stop_dropping(batch, EndReason::Corrupt, frame_start, end));
                    }
                }
            }
            if length > MAX_DATA_FRAME {
                return Ok(self.stop_dropping(batch, EndReason::Corrupt, frame_start, end));
            }
            if available - 4 < length {
                return Ok(self.stop_dropping(batch, EndReason::CutOff, frame_start, end));
            }
            // Both conversions are bounded by MAX_DATA_FRAME.
            let payload = usize::try_from(length).unwrap_or(usize::MAX);
            if batch.frames > 0 && spent + 4 + length > budget {
                // Leave the frame for the next batch: step back over its header.
                self.file.seek_relative(-4)?;
                self.position -= 4;
                batch.end = Some(EndReason::BatchFull);
                return Ok(batch);
            }
            out.extend_from_slice(&header);
            let at = out.len();
            out.resize(at + payload, 0);
            if !self.fill(&mut out[at..])? {
                out.truncate(at - 4);
                return Ok(self.stop_dropping(batch, EndReason::CutOff, frame_start, end));
            }
            self.position += length;
            batch.frames += 1;
            batch.bytes += 4 + length;
            spent += 4 + length;
        }
    }

    /// Reads the rest of a control frame whose escape word is consumed.
    /// `available` counts bytes from the start of the escape word.
    fn read_control(&mut self, available: u64) -> io::Result<Control> {
        let mut length = [0_u8; 4];
        if available < 8 || !self.fill(&mut length)? {
            return Ok(Control::CutOff);
        }
        self.position += 4;
        let length = u64::from(u32::from_be_bytes(length));
        if !(4..=MAX_CONTROL_LENGTH).contains(&length) {
            return Ok(Control::Corrupt);
        }
        if available - 8 < length {
            return Ok(Control::CutOff);
        }
        let mut body = [0_u8; MAX_CONTROL_BYTES];
        let body = &mut body[..usize::try_from(length).unwrap_or(0)];
        if !self.fill(body)? {
            return Ok(Control::CutOff);
        }
        self.position += length;
        let kind = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
        Ok(match kind {
            CONTROL_START => Control::Start,
            CONTROL_STOP => Control::Stop,
            _ => Control::Corrupt,
        })
    }

    /// Ends the segment at a cut-off or corrupt frame: every byte from the start
    /// of that frame to `end` is dropped, and the reader is positioned at `end`.
    fn stop_dropping(
        &mut self,
        mut batch: Batch,
        reason: EndReason,
        frame_start: u64,
        end: u64,
    ) -> Batch {
        self.ended = Some(reason);
        self.position = end;
        batch.end = Some(reason);
        batch.dropped_bytes = end.saturating_sub(frame_start);
        batch.dropped_frames = u32::from(reason == EndReason::CutOff);
        batch
    }

    /// Fills `buffer`, returning false when the file ends first.
    fn fill(&mut self, buffer: &mut [u8]) -> io::Result<bool> {
        match self.file.read_exact(buffer) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
            Err(error) => Err(error),
        }
    }
}

enum Control {
    Start,
    Stop,
    CutOff,
    Corrupt,
}

/// The exact START bytes `FrameStreamWriter::new` writes: the escape word, the
/// control length, START, and one content-type field.
fn expected_start() -> Vec<u8> {
    let content_length = u32::try_from(CONTENT_TYPE.len()).unwrap_or(u32::MAX);
    let mut start = Vec::with_capacity(START_BYTES);
    start.extend_from_slice(&0_u32.to_be_bytes());
    start.extend_from_slice(&(12 + content_length).to_be_bytes());
    start.extend_from_slice(&CONTROL_START.to_be_bytes());
    start.extend_from_slice(&FIELD_CONTENT_TYPE.to_be_bytes());
    start.extend_from_slice(&content_length.to_be_bytes());
    start.extend_from_slice(CONTENT_TYPE);
    start
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;
