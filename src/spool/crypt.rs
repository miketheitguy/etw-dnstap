//! Spool segment record layer: AES-128-GCM records (task C of
//! `docs/plans/spool-encryption-plan.md`).
//!
//! This module is pure: it works on byte slices and a reused `Vec<u8>` and does no I/O,
//! no clock reads, and no random numbers. The caller supplies the key, the key id, and
//! the random nonce prefix.
//!
//! # Segment format, version 1
//!
//! ```text
//! file header (32)   "EDSE" | version u16 = 1 | flags u16 = 0 | key id [8]
//!                    | nonce prefix [8] | segment sequence u64     (little-endian)
//! record header (12) plaintext length u32 | record index u32 | kind u8
//!                    | 3 reserved bytes = 0                        (little-endian)
//! record body        ciphertext (length bytes) | GCM tag (16)
//! ```
//!
//! A segment is the file header followed by records. Record indexes count 0, 1, 2, and so
//! on. Kind 1 is a data record, kind 2 is the end record. Each record is sealed with
//!
//! - nonce (96 bits) = nonce prefix (8 bytes) followed by the record index (4 bytes,
//!   big-endian), and
//! - additional authenticated data = the 32-byte file header followed by the 12-byte
//!   record header.
//!
//! A record therefore cannot move to another index, segment, or key. A data record's
//! plaintext is one spool append: complete Frame Streams data frames. The end record has
//! an empty plaintext. A data record is never empty and holds at most [`MAX_PLAINTEXT_LEN`]
//! bytes. A segment holds at most `u32::MAX + 1` records.
//!
//! # Logical offsets
//!
//! The logical offset of a record is the sum of the plaintext lengths of the records
//! before it: the byte offset of its first byte in the segment's plaintext frame stream.
//! `cursor.bin` stores logical offsets.
//!
//! # Writing
//!
//! ```text
//! let mut sealer = RecordSealer::new(key, sequence, nonce_prefix);
//! file.write_all(&sealer.file_header())?;
//! buf.clear();
//! sealer.seal_data(frames, &mut buf)?;   // then write buf with one call
//! sealer.seal_end(&mut buf)?;
//! ```
//!
//! # Reading
//!
//! [`RecordOpener::new`] checks the file header. [`RecordOpener::open_next`] then decrypts
//! one record in place at the start of the caller's buffer. To resume at a logical offset,
//! [`RecordOpener::skip_to`] first walks the record headers without decrypting.
//!
//! How a caller maps the results to `EndReason`:
//!
//! | Result | `EndReason` |
//! | --- | --- |
//! | [`Next::Complete`] with kind [`RecordKind::End`] | `Stop` |
//! | [`Next::Incomplete`] or [`Skip::Incomplete`], segment still being written | `WriterPosition` |
//! | [`Next::Incomplete`] or [`Skip::Incomplete`], leftover segment | `CutOff` (`EndOfFile` when no bytes remain at a record boundary) |
//! | [`Next::Corrupt`], [`Skip::Corrupt`] | `Corrupt` |
//! | [`HeaderError::NotThisFormat`] | not a version 1 segment: delete (old plaintext segment) |
//! | [`HeaderError::OtherKey`] | sealed with another key: unreadable |
//! | [`HeaderError::TooShort`] | no complete header yet: `WriterPosition` or `CutOff` |
//!
//! A flipped bit in a record's length field can make a record look longer than the bytes
//! present, which reads as [`Next::Incomplete`] and never as a decrypted record. It
//! becomes [`Next::Corrupt`] once enough bytes follow.

use std::{fmt, ops::Range, sync::Arc};

use ring::aead::{AES_128_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};

use super::locked::{LockError, Locked};

/// File header length in bytes.
pub(crate) const FILE_HEADER_LEN: usize = 32;
/// Record header length in bytes.
pub(crate) const RECORD_HEADER_LEN: usize = 12;
/// GCM tag length in bytes.
pub(crate) const TAG_LEN: usize = 16;
/// Bytes a record adds to its plaintext: header plus tag.
pub(crate) const RECORD_OVERHEAD: usize = RECORD_HEADER_LEN + TAG_LEN;
/// The largest plaintext of one record: 1 MiB.
pub(crate) const MAX_PLAINTEXT_LEN: usize = 1024 * 1024;
/// AES-128 key length in bytes.
pub(crate) const KEY_LEN: usize = 16;
/// Key id length in bytes.
pub(crate) const KEY_ID_LEN: usize = 8;
/// Nonce prefix length in bytes.
pub(crate) const NONCE_PREFIX_LEN: usize = 8;

const MAGIC: [u8; 4] = *b"EDSE";
const VERSION: u16 = 1;
const FLAGS: u16 = 0;
const KIND_DATA: u8 = 1;
const KIND_END: u8 = 2;
/// File header followed by record header: the additional authenticated data.
const AAD_LEN: usize = FILE_HEADER_LEN + RECORD_HEADER_LEN;
/// Bytes at the start of the file header that every version 1 segment shares.
const FIXED_PREFIX_LEN: usize = 8;

/// The AES-128 key and the id that names it in segment headers.
///
/// The expanded key schedule lives in one locked page of its own (see
/// [`super::locked`]): never written to the pagefile, left out of WER crash
/// dumps, and zeroed on drop. The type is not `Clone`: writers and readers share one key through an
/// `Arc<SegmentKey>`, so the schedule is never copied. The raw key bytes are not
/// kept; the owner of the 16-byte key (task K) zeroes its copy.
pub(crate) struct SegmentKey {
    key: Locked<LessSafeKey>,
    id: [u8; KEY_ID_LEN],
}

impl SegmentKey {
    /// Builds the key from 16 bytes and its 8-byte id, in a new locked page.
    ///
    /// `ring` expands the schedule on the stack before it is moved into the
    /// page, so stack residue of it can remain.
    ///
    /// # Errors
    ///
    /// [`CryptError::BadKey`] if the cipher rejects the key (it cannot for 16
    /// bytes), [`CryptError::Memory`] if the locked page cannot be made.
    pub(crate) fn new(key: &[u8; KEY_LEN], id: [u8; KEY_ID_LEN]) -> Result<Self, CryptError> {
        let unbound = UnboundKey::new(&AES_128_GCM, key).map_err(|_| CryptError::BadKey)?;
        let schedule = Locked::new(LessSafeKey::new(unbound)).map_err(CryptError::Memory)?;
        Ok(Self { key: schedule, id })
    }

    /// The key id written to and checked in segment headers.
    pub(crate) const fn id(&self) -> [u8; KEY_ID_LEN] {
        self.id
    }

    /// The address of the key schedule, so tests can show that every user
    /// shares the one copy.
    #[cfg(test)]
    pub(crate) fn schedule_address(&self) -> usize {
        std::ptr::from_ref::<LessSafeKey>(&self.key) as usize
    }
}

impl fmt::Debug for SegmentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SegmentKey")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Failure to build a key or seal a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CryptError {
    /// The cipher rejected the key.
    #[error("the cipher rejected the spool key")]
    BadKey,
    /// The key schedule could not be put in a locked page.
    #[error(transparent)]
    Memory(LockError),
    /// A data record carries 1 byte to [`MAX_PLAINTEXT_LEN`] bytes.
    #[error("a data record must hold between 1 byte and 1 MiB")]
    BadPlaintextLength,
    /// The segment already has `u32::MAX + 1` records.
    #[error("the segment has no record index left")]
    TooManyRecords,
    /// The end record is sealed; nothing follows it.
    #[error("the segment already has its end record")]
    AlreadyEnded,
    /// The cipher failed to seal. It cannot for these inputs.
    #[error("the cipher failed to seal a record")]
    Seal,
}

/// A fixed-size segment file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileHeader {
    /// Id of the key that sealed the segment.
    pub(crate) key_id: [u8; KEY_ID_LEN],
    /// Random per segment; the first 8 bytes of every nonce.
    pub(crate) nonce_prefix: [u8; NONCE_PREFIX_LEN],
    /// Segment sequence number (the number in the file name).
    pub(crate) sequence: u64,
}

impl FileHeader {
    /// The 32 header bytes.
    pub(crate) fn encode(&self) -> [u8; FILE_HEADER_LEN] {
        let mut bytes = [0_u8; FILE_HEADER_LEN];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&FLAGS.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.key_id);
        bytes[16..24].copy_from_slice(&self.nonce_prefix);
        bytes[24..32].copy_from_slice(&self.sequence.to_le_bytes());
        bytes
    }

    /// Parses a file header from the start of `bytes`. Does not look at the key id's
    /// value.
    ///
    /// # Errors
    ///
    /// [`HeaderError::NotThisFormat`] when the bytes seen so far have a wrong magic,
    /// version, or flags; [`HeaderError::TooShort`] when they are fewer than 32 and
    /// consistent so far.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, HeaderError> {
        // The first 8 bytes are fixed. Compare as many as there are, so a short file
        // that cannot be this format is not reported as one that might grow into it.
        let fixed = Self {
            key_id: [0; KEY_ID_LEN],
            nonce_prefix: [0; NONCE_PREFIX_LEN],
            sequence: 0,
        }
        .encode();
        let seen = bytes.len().min(FIXED_PREFIX_LEN);
        if bytes[..seen] != fixed[..seen] {
            return Err(HeaderError::NotThisFormat);
        }
        let bytes: &[u8; FILE_HEADER_LEN] = bytes
            .get(..FILE_HEADER_LEN)
            .and_then(|head| head.try_into().ok())
            .ok_or(HeaderError::TooShort)?;
        Ok(Self {
            key_id: array_at(bytes, 8),
            nonce_prefix: array_at(bytes, 16),
            sequence: u64::from_le_bytes(array_at(bytes, 24)),
        })
    }
}

/// Why a file header was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum HeaderError {
    /// Fewer than 32 bytes, so far consistent with this format. More bytes may follow.
    #[error("the segment header is not complete")]
    TooShort,
    /// Wrong magic, version, or flags: not a version 1 encrypted segment (for example an
    /// old plaintext segment).
    #[error("the file is not a version 1 encrypted segment")]
    NotThisFormat,
    /// A version 1 segment sealed with another key.
    #[error("the segment was sealed with another key")]
    OtherKey {
        /// The key id found in the file.
        found: [u8; KEY_ID_LEN],
    },
}

/// The kind of a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordKind {
    /// One spool append: complete Frame Streams data frames.
    Data,
    /// The last record: an empty plaintext. Replaces the STOP control frame.
    End,
}

/// Why a record is corrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CorruptKind {
    /// Plaintext length over 1 MiB, an empty data record, or a non-empty end record.
    BadLength,
    /// Kind byte other than 1 or 2.
    BadKind,
    /// A reserved header byte is not zero.
    ReservedNotZero,
    /// The record index is not the next one.
    OutOfOrder,
    /// The authentication tag did not verify (changed bytes, another segment, key, or
    /// position).
    BadTag,
    /// A record was requested after the end record.
    AfterEnd,
}

/// A parsed, unauthenticated record header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordHeader {
    /// Data or end.
    pub(crate) kind: RecordKind,
    /// Plaintext length in bytes, at most [`MAX_PLAINTEXT_LEN`].
    pub(crate) plaintext_len: usize,
    /// Record index.
    pub(crate) index: u32,
}

impl RecordHeader {
    /// Bytes the whole record takes in the file: header, ciphertext, and tag.
    pub(crate) const fn record_len(&self) -> usize {
        RECORD_OVERHEAD + self.plaintext_len
    }

    /// Encodes the header. `length` is the plaintext length, already checked.
    fn encode(kind: RecordKind, length: u32, index: u32) -> [u8; RECORD_HEADER_LEN] {
        let mut bytes = [0_u8; RECORD_HEADER_LEN];
        bytes[0..4].copy_from_slice(&length.to_le_bytes());
        bytes[4..8].copy_from_slice(&index.to_le_bytes());
        bytes[8] = match kind {
            RecordKind::Data => KIND_DATA,
            RecordKind::End => KIND_END,
        };
        bytes
    }
}

/// The 12-byte GCM nonce: nonce prefix, then the record index big-endian.
fn nonce_for(prefix: [u8; NONCE_PREFIX_LEN], index: u32) -> Nonce {
    let mut nonce = [0_u8; NONCE_LEN];
    nonce[..NONCE_PREFIX_LEN].copy_from_slice(&prefix);
    nonce[NONCE_PREFIX_LEN..].copy_from_slice(&index.to_be_bytes());
    Nonce::assume_unique_for_key(nonce)
}

/// The additional authenticated data: file header, then record header.
fn aad_for(file_header: &[u8; FILE_HEADER_LEN], record_header: &[u8]) -> [u8; AAD_LEN] {
    let mut aad = [0_u8; AAD_LEN];
    aad[..FILE_HEADER_LEN].copy_from_slice(file_header);
    aad[FILE_HEADER_LEN..].copy_from_slice(record_header);
    aad
}

/// Copies `N` bytes at `at`. The caller guarantees `at + N <= M`.
fn array_at<const N: usize, const M: usize>(bytes: &[u8; M], at: usize) -> [u8; N] {
    let mut out = [0_u8; N];
    out.copy_from_slice(&bytes[at..at + N]);
    out
}

/// Seals the records of one segment.
///
/// Keeps the next record index and the logical plaintext offset. After an error nothing
/// has been appended to `out` and the state is unchanged.
#[derive(Debug)]
pub(crate) struct RecordSealer {
    key: Arc<SegmentKey>,
    file_header: [u8; FILE_HEADER_LEN],
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    /// Index of the next record; `u32::MAX + 1` when the indexes are used up.
    next_index: u64,
    logical_offset: u64,
    ended: bool,
}

impl RecordSealer {
    /// Starts a segment with sequence number `sequence`. `nonce_prefix` must be random
    /// and never reused with the same key.
    pub(crate) fn new(
        key: Arc<SegmentKey>,
        sequence: u64,
        nonce_prefix: [u8; NONCE_PREFIX_LEN],
    ) -> Self {
        let file_header = FileHeader {
            key_id: key.id(),
            nonce_prefix,
            sequence,
        }
        .encode();
        Self {
            key,
            file_header,
            nonce_prefix,
            next_index: 0,
            logical_offset: 0,
            ended: false,
        }
    }

    /// The 32 bytes to write at the start of the file.
    pub(crate) const fn file_header(&self) -> [u8; FILE_HEADER_LEN] {
        self.file_header
    }

    /// The shared key this sealer uses.
    #[cfg(test)]
    pub(crate) const fn key(&self) -> &Arc<SegmentKey> {
        &self.key
    }

    /// Logical offset of the next data record: the plaintext bytes sealed so far.
    #[cfg(test)]
    pub(crate) const fn logical_offset(&self) -> u64 {
        self.logical_offset
    }

    /// Index the next record gets (`u32::MAX` also when none is left).
    #[cfg(test)]
    pub(crate) fn next_index(&self) -> u32 {
        u32::try_from(self.next_index).unwrap_or(u32::MAX)
    }

    /// Appends one complete data record for `frames` to `out`. Does not allocate when
    /// `out` has room for `frames.len() + RECORD_OVERHEAD` more bytes.
    ///
    /// # Errors
    ///
    /// [`CryptError::BadPlaintextLength`] if `frames` is empty or over 1 MiB,
    /// [`CryptError::TooManyRecords`] past `u32::MAX + 1` records,
    /// [`CryptError::AlreadyEnded`] after [`Self::seal_end`].
    pub(crate) fn seal_data(&mut self, frames: &[u8], out: &mut Vec<u8>) -> Result<(), CryptError> {
        if frames.is_empty() || frames.len() > MAX_PLAINTEXT_LEN {
            return Err(CryptError::BadPlaintextLength);
        }
        self.seal(RecordKind::Data, frames, out)?;
        self.logical_offset += frames.len() as u64;
        Ok(())
    }

    /// Appends the end record to `out`. Nothing may be sealed after it.
    ///
    /// # Errors
    ///
    /// [`CryptError::TooManyRecords`] if no index is left, [`CryptError::AlreadyEnded`]
    /// if called twice.
    pub(crate) fn seal_end(&mut self, out: &mut Vec<u8>) -> Result<(), CryptError> {
        self.seal(RecordKind::End, &[], out)?;
        self.ended = true;
        Ok(())
    }

    fn seal(
        &mut self,
        kind: RecordKind,
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptError> {
        if self.ended {
            return Err(CryptError::AlreadyEnded);
        }
        let index = u32::try_from(self.next_index).map_err(|_| CryptError::TooManyRecords)?;
        // The callers keep the length at or under MAX_PLAINTEXT_LEN.
        let length = u32::try_from(plaintext.len()).map_err(|_| CryptError::BadPlaintextLength)?;
        let header = RecordHeader::encode(kind, length, index);
        let aad = aad_for(&self.file_header, &header);
        let start = out.len();
        out.reserve(plaintext.len() + RECORD_OVERHEAD);
        out.extend_from_slice(&header);
        out.extend_from_slice(plaintext);
        let sealed = self.key.key.seal_in_place_separate_tag(
            nonce_for(self.nonce_prefix, index),
            Aad::from(aad.as_slice()),
            &mut out[start + RECORD_HEADER_LEN..],
        );
        let Ok(tag) = sealed else {
            out.truncate(start);
            return Err(CryptError::Seal);
        };
        out.extend_from_slice(tag.as_ref());
        self.next_index += 1;
        Ok(())
    }
}

/// A decrypted record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    /// Data or end.
    pub(crate) kind: RecordKind,
    /// Where the plaintext is in the caller's buffer, which now holds it decrypted.
    pub(crate) plaintext: Range<usize>,
    /// Bytes the record took from the start of the buffer; the next record starts there.
    pub(crate) consumed: usize,
    /// Logical offset of the plaintext's first byte in the segment's frame stream.
    pub(crate) logical_start: u64,
}

/// The result of [`RecordOpener::open_next`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// A record that authenticated. The opener has moved past it.
    Complete(Record),
    /// The buffer ends inside the record. `needed` is the buffer length that holds the
    /// whole record, or the header length when the header is not complete yet. Nothing
    /// changed.
    Incomplete {
        /// Buffer bytes needed to finish the record.
        needed: usize,
    },
    /// The record is invalid. The rest of the segment cannot be trusted.
    Corrupt(CorruptKind),
}

/// The result of [`RecordOpener::skip_to`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Skip {
    /// The record at `consumed` holds the target offset, or is the end record. Call
    /// [`RecordOpener::open_next`] on the buffer from `consumed`.
    Found {
        /// Bytes of whole records skipped from the start of the buffer.
        consumed: usize,
    },
    /// The buffer ends before the walk could finish. The first `consumed` bytes are
    /// skipped; the buffer from there must grow to `needed` bytes (a record header, or
    /// the whole record when it lies before the target) and the walk continues.
    Incomplete {
        /// Bytes of whole records skipped from the start of the buffer.
        consumed: usize,
        /// Buffer length, counted from `consumed`, that lets the walk go on.
        needed: usize,
    },
    /// A record header is invalid.
    Corrupt(CorruptKind),
}

/// Reads and decrypts the records of one segment, in order.
#[derive(Debug)]
pub(crate) struct RecordOpener {
    key: Arc<SegmentKey>,
    file_header: [u8; FILE_HEADER_LEN],
    header: FileHeader,
    /// Index of the next record; above `u32::MAX` when none can follow.
    next_index: u64,
    logical_offset: u64,
    ended: bool,
}

impl RecordOpener {
    /// Checks the file header at the start of `bytes` against `key`.
    ///
    /// # Errors
    ///
    /// [`HeaderError::NotThisFormat`] for a wrong magic, version, or flags;
    /// [`HeaderError::OtherKey`] when the key id differs; [`HeaderError::TooShort`] when
    /// fewer than 32 bytes are there.
    pub(crate) fn new(key: Arc<SegmentKey>, bytes: &[u8]) -> Result<Self, HeaderError> {
        let header = FileHeader::decode(bytes)?;
        if header.key_id != key.id() {
            return Err(HeaderError::OtherKey {
                found: header.key_id,
            });
        }
        Ok(Self {
            key,
            file_header: header.encode(),
            header,
            next_index: 0,
            logical_offset: 0,
            ended: false,
        })
    }

    /// The shared key this opener uses.
    #[cfg(test)]
    pub(crate) const fn key(&self) -> &Arc<SegmentKey> {
        &self.key
    }

    /// The segment sequence number in the header.
    pub(crate) const fn sequence(&self) -> u64 {
        self.header.sequence
    }

    /// Logical offset of the next record's plaintext.
    pub(crate) const fn logical_offset(&self) -> u64 {
        self.logical_offset
    }

    /// Parses and checks the record header at the start of `buf` without decrypting.
    /// `Ok(None)` means fewer than 12 bytes.
    ///
    /// # Errors
    ///
    /// The [`CorruptKind`] of an invalid header.
    pub(crate) fn peek_header(&self, buf: &[u8]) -> Result<Option<RecordHeader>, CorruptKind> {
        if self.ended {
            return Err(CorruptKind::AfterEnd);
        }
        let Some(head) = buf
            .get(..RECORD_HEADER_LEN)
            .and_then(|head| <&[u8; RECORD_HEADER_LEN]>::try_from(head).ok())
        else {
            return Ok(None);
        };
        let kind = match head[8] {
            KIND_DATA => RecordKind::Data,
            KIND_END => RecordKind::End,
            _ => return Err(CorruptKind::BadKind),
        };
        if head[9..] != [0, 0, 0] {
            return Err(CorruptKind::ReservedNotZero);
        }
        let index = u32::from_le_bytes(array_at(head, 4));
        if u64::from(index) != self.next_index {
            return Err(CorruptKind::OutOfOrder);
        }
        let length = u32::from_le_bytes(array_at(head, 0));
        let plaintext_len = usize::try_from(length).map_err(|_| CorruptKind::BadLength)?;
        let length_ok = match kind {
            RecordKind::Data => (1..=MAX_PLAINTEXT_LEN).contains(&plaintext_len),
            RecordKind::End => plaintext_len == 0,
        };
        if !length_ok {
            return Err(CorruptKind::BadLength);
        }
        Ok(Some(RecordHeader {
            kind,
            plaintext_len,
            index,
        }))
    }

    /// Decrypts the record at the start of `buf` in place.
    ///
    /// On [`Next::Complete`] the opener moves to the next record; on any other result it
    /// stays, so a caller can add bytes and call again after [`Next::Incomplete`]. An
    /// invalid header is [`Next::Corrupt`] even when the body has not arrived. After
    /// [`Next::Corrupt`] the contents of `buf` past the header are unspecified.
    pub(crate) fn open_next(&mut self, buf: &mut [u8]) -> Next {
        let header = match self.peek_header(buf) {
            Ok(Some(header)) => header,
            Ok(None) => {
                return Next::Incomplete {
                    needed: RECORD_HEADER_LEN,
                };
            }
            Err(kind) => return Next::Corrupt(kind),
        };
        let total = header.record_len();
        if buf.len() < total {
            return Next::Incomplete { needed: total };
        }
        let aad = aad_for(&self.file_header, &buf[..RECORD_HEADER_LEN]);
        let opened = self.key.key.open_in_place(
            nonce_for(self.header.nonce_prefix, header.index),
            Aad::from(aad.as_slice()),
            &mut buf[RECORD_HEADER_LEN..total],
        );
        match opened {
            Ok(plain) if plain.len() == header.plaintext_len => {}
            _ => return Next::Corrupt(CorruptKind::BadTag),
        }
        let logical_start = self.logical_offset;
        self.advance(&header);
        Next::Complete(Record {
            kind: header.kind,
            plaintext: RECORD_HEADER_LEN..RECORD_HEADER_LEN + header.plaintext_len,
            consumed: total,
            logical_start,
        })
    }

    /// Walks record headers from the start of `buf` without decrypting, skipping the
    /// records that end at or before logical offset `target`, to find the record that
    /// holds `target` for a resume. Stops at the first record that holds `target`
    /// (`start <= target < start + length`) or at the end record. The opener's index and
    /// logical offset move past the skipped records.
    ///
    /// Skipped ciphertext is not authenticated. The record the walk stops at is, when
    /// [`Self::open_next`] decrypts it, and that fails if a skipped header misled the
    /// walk. After [`Skip::Incomplete`], continue with the buffer moved to `consumed` and
    /// grown to at least `needed` bytes.
    pub(crate) fn skip_to(&mut self, buf: &[u8], target: u64) -> Skip {
        let mut consumed = 0;
        loop {
            let header = match self.peek_header(&buf[consumed..]) {
                Ok(Some(header)) => header,
                Ok(None) => {
                    return Skip::Incomplete {
                        consumed,
                        needed: RECORD_HEADER_LEN,
                    };
                }
                Err(kind) => return Skip::Corrupt(kind),
            };
            let end = self.logical_offset + header.plaintext_len as u64;
            if header.kind == RecordKind::End || target < end {
                return Skip::Found { consumed };
            }
            let total = header.record_len();
            if buf.len() - consumed < total {
                return Skip::Incomplete {
                    consumed,
                    needed: total,
                };
            }
            self.advance(&header);
            consumed += total;
        }
    }

    fn advance(&mut self, header: &RecordHeader) {
        self.next_index += 1;
        self.logical_offset += header.plaintext_len as u64;
        self.ended = header.kind == RecordKind::End;
    }
}

#[cfg(test)]
#[path = "crypt_tests.rs"]
mod crypt_tests;
