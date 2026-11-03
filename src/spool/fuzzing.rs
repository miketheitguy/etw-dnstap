//! Fuzzing entry point for the spool reader, compiled for the `fuzzing` feature
//! and for unit tests.
//!
//! It writes arbitrary bytes into a throwaway folder as a segment file (and
//! optionally a cursor file), opens a [`Spool`] on the folder, and reads to the
//! end. The reader must never panic, must stop, and must hand out only whole
//! data frames of a legal length.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use super::{
    NAME_DIGITS, NAME_PREFIX, NAME_SUFFIX, Spool, SpoolConfig,
    cursor::{Cursor, CursorFile},
};
use crate::{dnstap::MAX_ENCODED_LENGTH, sink::SinkStats};

/// Cap on spool size for the run: large enough for any fuzz input.
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The most bytes of the input taken as raw cursor-file contents.
const RAW_CURSOR_BYTES: usize = 32;

/// A throwaway folder under the system temp folder, removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "etw-dnstap-spool-fuzz-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        // A leftover from a crashed run with the same name would add files.
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A fake volume with unlimited free space.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the spool takes a fn pointer that returns a Result"
)]
const fn plenty(_dir: &Path) -> io::Result<u64> {
    Ok(u64::MAX)
}

fn segment_name(sequence: u64) -> String {
    format!("{NAME_PREFIX}{sequence:0NAME_DIGITS$}{NAME_SUFFIX}")
}

/// Writes the segment file(s) and the cursor file that `data` describes.
///
/// Layout of `data`: a mode byte, a limit byte, then the rest.
/// - Mode bits 0 and 1: 0 no cursor file; 1 the next 32 bytes (or fewer) are the
///   raw cursor file; 2 and 3 a valid cursor built from the next three bytes
///   (a sequence selector and a 16-bit offset), with sequence `u64::MAX` for 3.
/// - Mode bit 2: split the segment bytes over two segment files.
/// - Limit byte: its low 7 bits choose the batch limit.
///
/// Returns the batch limit and the number of segment bytes written.
fn write_inputs(dir: &Path, data: &[u8]) -> io::Result<(usize, usize)> {
    let (mode, limit_byte, rest) = match data {
        [mode, limit, rest @ ..] => (*mode, *limit, rest),
        [mode] => (*mode, 0, &[][..]),
        [] => (0, 0, &[][..]),
    };
    let limit = 1 + usize::from(limit_byte & 0x7f) * 61;
    let mut body = rest;
    match mode & 3 {
        0 => {}
        1 => {
            let (raw, tail) = body.split_at(body.len().min(RAW_CURSOR_BYTES));
            fs::write(dir.join(super::cursor::FILE_NAME), raw)?;
            body = tail;
        }
        kind => {
            let (selector, offset, tail) = match body {
                [selector, high, low, tail @ ..] => {
                    (*selector, u16::from_be_bytes([*high, *low]), tail)
                }
                _ => (0, 0, &[][..]),
            };
            let sequence = if kind == 3 {
                u64::MAX
            } else {
                u64::from(selector % 3)
            };
            CursorFile::new(dir).write(Cursor {
                sequence,
                offset: u64::from(offset),
            })?;
            body = tail;
        }
    }
    let written = body.len();
    if mode & 4 == 0 {
        fs::write(dir.join(segment_name(1)), body)?;
    } else {
        let (first, second) = body.split_at(body.len() / 2);
        fs::write(dir.join(segment_name(1)), first)?;
        fs::write(dir.join(segment_name(2)), second)?;
    }
    Ok((limit, written))
}

/// Checks that `out` holds exactly `frames` whole data frames of legal length.
fn check_frames(out: &[u8], frames: u32) {
    let mut rest = out;
    let mut count = 0_u32;
    while let Some((header, tail)) = rest.split_first_chunk::<4>() {
        let length = u32::from_be_bytes(*header) as usize;
        assert!(
            (1..=MAX_ENCODED_LENGTH).contains(&length),
            "frame length {length} out of range"
        );
        assert!(tail.len() >= length, "a frame is cut off in the batch");
        rest = &tail[length..];
        count += 1;
    }
    assert!(rest.is_empty(), "bytes left after the last whole frame");
    assert_eq!(count, frames, "frame count does not match the bytes");
}

/// Drives [`Spool::open`] and [`Spool::read_batch`] over arbitrary bytes.
///
/// The first two bytes of `data` choose what is written (see `write_inputs`);
/// the rest is the content of a segment file, or of two. The function returns
/// normally for any input.
///
/// # Panics
/// Panics if the spool panics, hands out a partial or oversized frame, does not
/// reach the end of the files in a bounded number of reads, or leaves bytes in
/// its gauges after the end. A fuzzer reports each as a crash.
pub fn spool_segment(data: &[u8]) {
    let Ok(scratch) = Scratch::new() else {
        return;
    };
    let Ok((limit, written)) = write_inputs(&scratch.path, data) else {
        return;
    };
    let stats = Arc::new(SinkStats::default());
    let config = SpoolConfig {
        dir: scratch.path.clone(),
        max_bytes: MAX_BYTES,
        min_free_bytes: 0,
    };
    let Ok(mut spool) = Spool::open(config, Arc::clone(&stats), plenty) else {
        return;
    };
    // A read error pauses reads; a fuzz run must not wait for the pause.
    spool.pause = Duration::ZERO;
    // Every call that returns nothing still skips at least one control frame or
    // deletes a segment, so this many calls reach the end.
    let mut out = Vec::new();
    let mut total = 0_u64;
    for _ in 0..written + 16 {
        out.clear();
        if let Ok(frames) = spool.read_batch(&mut out, limit) {
            check_frames(&out, frames);
            total += u64::from(frames);
        }
        if !spool.has_backlog() {
            break;
        }
    }
    assert!(!spool.has_backlog(), "the spool did not reach the end");
    assert_eq!(stats.replayed.load(Ordering::Relaxed), total);
    // One more call retries a delete that failed. A file that still cannot be
    // deleted (an environmental sharing violation) keeps the gauges up, so the
    // check holds only for a run without I/O errors.
    out.clear();
    let _ = spool.read_batch(&mut out, limit);
    if stats.spool_errors.load(Ordering::Relaxed) == 0 {
        assert_eq!(stats.spool_segments.load(Ordering::Relaxed), 0);
        assert_eq!(stats.spool_bytes.load(Ordering::Relaxed), 0);
    }
}

#[cfg(test)]
#[path = "fuzzing_tests.rs"]
mod tests;
