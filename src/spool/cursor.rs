//! The spool delivered position, kept in `cursor.bin` next to the segments: the
//! end of the last frame the link fully handed to the socket.
//!
//! Format, all little-endian, 28 bytes: magic `ETDC`, sequence `u64`, byte
//! offset `u64`, and a check value `sequence ^ offset ^ CHECK_KEY`. The file is
//! overwritten in place with one positional write, when the router has applied
//! the reports of a pass, when the first segment is deleted, and at a stop. It
//! never passes the delivered position, so a crash sends frames again and never
//! loses one. A torn write fails the check value and reads as
//! [`CursorRead::Invalid`]. The file is a high-water mark for deletion and is
//! never deleted.

use super::fsinfo::is_plain_file;

use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::windows::fs::FileExt,
    path::{Path, PathBuf},
};

/// File name inside the spool folder.
pub(super) const FILE_NAME: &str = "cursor.bin";
const MAGIC: [u8; 4] = *b"ETDC";
const CHECK_KEY: u64 = 0x5bd1_e995_5bd1_e995;
const CURSOR_BYTES: usize = 28;

/// A read position: the segment sequence number and a byte offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Cursor {
    pub(super) sequence: u64,
    pub(super) offset: u64,
}

impl Cursor {
    fn encode(self) -> [u8; CURSOR_BYTES] {
        let mut bytes = [0_u8; CURSOR_BYTES];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4..12].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[12..20].copy_from_slice(&self.offset.to_le_bytes());
        bytes[20..].copy_from_slice(&(self.sequence ^ self.offset ^ CHECK_KEY).to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; CURSOR_BYTES] = bytes.try_into().ok()?;
        if bytes[..4] != MAGIC {
            return None;
        }
        let word = |at: usize| {
            let mut word = [0_u8; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(word)
        };
        let cursor = Self {
            sequence: word(4),
            offset: word(12),
        };
        (word(20) == cursor.sequence ^ cursor.offset ^ CHECK_KEY).then_some(cursor)
    }
}

/// What reading the cursor file found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CursorRead {
    /// No file: nothing was partly read.
    Missing,
    /// A file with the wrong size, magic, or check value.
    Invalid,
    Valid(Cursor),
}

/// Reads `cursor.bin` from `dir`. Only an I/O error other than a missing file is
/// an `Err`.
pub(super) fn read(dir: &Path) -> io::Result<CursorRead> {
    let path = dir.join(FILE_NAME);
    match is_plain_file(&path) {
        Ok(true) => {}
        // A link, folder, or other object is a foreign file, not our cursor.
        Ok(false) => return Ok(CursorRead::Invalid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(CursorRead::Missing),
        Err(error) => return Err(error),
    }
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(CursorRead::Missing),
        Err(error) => return Err(error),
    };
    // One byte more than the format needs, so a longer file reads as invalid.
    let mut bytes = Vec::with_capacity(CURSOR_BYTES + 1);
    file.take(CURSOR_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    Ok(Cursor::decode(&bytes).map_or(CursorRead::Invalid, CursorRead::Valid))
}

/// Writes the cursor file, keeping the handle open between writes.
pub(super) struct CursorFile {
    path: PathBuf,
    file: Option<File>,
}

impl CursorFile {
    pub(super) fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            file: None,
        }
    }

    /// Overwrites the file in place with one write.
    pub(super) fn write(&mut self, cursor: Cursor) -> io::Result<()> {
        let bytes = cursor.encode();
        if self.file.is_none() {
            // Never write through a link that replaced the file.
            if is_plain_file(&self.path).is_ok_and(|plain| !plain) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the cursor path is not a regular file",
                ));
            }
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&self.path)?;
            // A longer file would stay invalid at every start.
            file.set_len(CURSOR_BYTES as u64)?;
            self.file = Some(file);
        }
        let Some(file) = &self.file else {
            return Err(io::Error::other("cursor file is not open"));
        };
        let result = file.seek_write(&bytes, 0);
        match result {
            Ok(written) if written == bytes.len() => Ok(()),
            outcome => {
                // The handle may be stale; open the file again next time.
                self.file = None;
                outcome.and(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "short write to the cursor file",
                )))
            }
        }
    }
}
