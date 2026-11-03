//! Helpers shared by the spool tests: a throwaway folder, frame builders and
//! decoders, and an independent Frame Streams file parser.

use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::framestream::FrameStreamWriter;

/// A unique folder under the system temp folder, removed on drop.
pub(crate) struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub(crate) fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "etw-dnstap-spool-{label}-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("test folder is created");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Data frames, each a 4-byte big-endian length and the payload.
pub(crate) fn framed(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for payload in payloads {
        bytes.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_be_bytes());
        bytes.extend_from_slice(payload);
    }
    bytes
}

/// One frame per number in `range`. The payload is the number as 8 big-endian
/// bytes followed by `pad` filler bytes. Returns the framed bytes and the count.
pub(crate) fn numbered(range: Range<u64>, pad: usize) -> (Vec<u8>, u32) {
    let payloads: Vec<Vec<u8>> = range
        .clone()
        .map(|number| {
            let mut payload = number.to_be_bytes().to_vec();
            payload.resize(8 + pad, 0xa5);
            payload
        })
        .collect();
    (
        framed(&payloads),
        u32::try_from(payloads.len()).expect("small"),
    )
}

/// Splits framed bytes into payloads. Panics on malformed input.
pub(crate) fn payloads(framed: &[u8]) -> Vec<Vec<u8>> {
    let mut rest = framed;
    let mut payloads = Vec::new();
    while !rest.is_empty() {
        let length = u32::from_be_bytes(rest[..4].try_into().expect("4-byte header")) as usize;
        assert!(length > 0, "data frame length is nonzero");
        payloads.push(rest[4..4 + length].to_vec());
        rest = &rest[4 + length..];
    }
    payloads
}

/// The numbers in framed bytes made by [`numbered`].
pub(crate) fn numbers(framed: &[u8]) -> Vec<u64> {
    payloads(framed)
        .iter()
        .map(|payload| u64::from_be_bytes(payload[..8].try_into().expect("8-byte number")))
        .collect()
}

/// The START control frame a Frame Streams writer emits, from the reference writer.
pub(crate) fn start_bytes() -> Vec<u8> {
    let bytes = FrameStreamWriter::new(Vec::new())
        .expect("START encodes")
        .finish()
        .expect("STOP encodes");
    bytes[..bytes.len() - 12].to_vec()
}

/// The STOP control frame.
pub(crate) const STOP: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3];

/// Parses a sealed dnstap Frame Streams file strictly: exact START, nonzero data
/// frames, then STOP as the last bytes. Returns the payloads. Panics otherwise.
pub(crate) fn parse_fstrm(file: &[u8]) -> Vec<Vec<u8>> {
    let start = start_bytes();
    assert_eq!(&file[..start.len()], start.as_slice(), "START frame");
    assert_eq!(
        &file[file.len() - STOP.len()..],
        STOP.as_slice(),
        "STOP frame"
    );
    payloads(&file[start.len()..file.len() - STOP.len()])
}
