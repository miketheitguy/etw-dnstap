//! Helpers shared by the spool tests: a throwaway folder, frame builders and
//! decoders, an independent Frame Streams file parser, and a log capture that
//! other modules' tests also use.

use std::{
    cell::RefCell,
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use tracing::Level;
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::MakeWriter,
    layer::{Layer as _, SubscriberExt as _},
};

use crate::framestream::FrameStreamWriter;

/// Collects log output so a test can count messages.
#[derive(Clone, Default)]
pub(crate) struct LogLines(Arc<Mutex<Vec<u8>>>);

impl LogLines {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log lock")).into_owned()
    }

    pub(crate) fn count(&self, needle: &str) -> usize {
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

/// Runs `work` with the warnings and errors it logs on this thread captured.
pub(crate) fn capture_logs(work: impl FnOnce()) -> LogLines {
    capture_logs_at(Level::WARN, work)
}

/// Runs `work` with the events at `level` and above that it logs on this
/// thread captured. `level` is at most [`Level::INFO`].
///
/// Every capture goes through one process-wide subscriber that writes an event
/// to the capture of the thread that logged it. A scoped subscriber per
/// capture (`tracing::subscriber::with_default`) is not reliable while tests
/// run in parallel: while only one dispatcher is registered, `tracing-core`
/// computes the cached interest of a callsite from the default dispatcher of
/// the thread that reaches it first. A test thread without a capture then
/// caches `Interest::never` for the callsite, and a capture open on another
/// thread misses the event. With one global dispatcher every thread computes
/// the same interest, and the per-event filter reads this thread's capture.
pub(crate) fn capture_logs_at(level: Level, work: impl FnOnce()) -> LogLines {
    assert!(
        level <= Level::INFO,
        "the test subscriber records INFO and above"
    );
    install_capture_subscriber();
    let lines = LogLines::default();
    let previous = CAPTURE.with(|capture| capture.replace(Some((lines.clone(), level))));
    let _restore = RestoreCapture(previous);
    work();
    lines
}

thread_local! {
    /// The capture open on this thread and the most verbose level it keeps.
    static CAPTURE: RefCell<Option<(LogLines, Level)>> = const { RefCell::new(None) };
}

/// Puts back the capture that was open before, also when `work` panics.
struct RestoreCapture(Option<(LogLines, Level)>);

impl Drop for RestoreCapture {
    fn drop(&mut self) {
        let previous = self.0.take();
        CAPTURE.with(|capture| *capture.borrow_mut() = previous);
    }
}

fn install_capture_subscriber() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        // A dynamic filter, so the callsite interest is `sometimes` and the
        // filter runs for each event on the thread that logs it.
        let open_capture = tracing_subscriber::filter::dynamic_filter_fn(|metadata, _context| {
            CAPTURE.with(|capture| {
                capture
                    .borrow()
                    .as_ref()
                    .is_some_and(|(_, level)| metadata.level() <= level)
            })
        })
        .with_max_level_hint(LevelFilter::INFO);
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(ThreadCapture)
            .with_filter(open_capture);
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))
            .expect("no other global subscriber in the test process");
    });
    // A callsite first reached on another thread while the subscriber was
    // being installed can keep the interest computed before it; recompute.
    tracing::callsite::rebuild_interest_cache();
}

/// Makes writers to the capture of the thread that logs the event.
struct ThreadCapture;

impl<'a> MakeWriter<'a> for ThreadCapture {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter(
            CAPTURE.with(|capture| capture.borrow().as_ref().map(|(lines, _)| lines.clone())),
        )
    }
}

/// Writes to a thread's capture, or discards the bytes when there is none.
struct CaptureWriter(Option<LogLines>);

impl io::Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match &mut self.0 {
            Some(lines) => lines.write(bytes),
            None => Ok(bytes.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

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

/// A capture must see an event whose callsite another thread, with no capture
/// active, reached first while the capture was open.
#[test]
fn a_capture_sees_a_callsite_first_reached_on_another_thread() {
    fn log_it() {
        tracing::warn!("callsite first reached on another thread");
    }
    let (opened, wait_for_open) = std::sync::mpsc::channel();
    let (logged, wait_for_log) = std::sync::mpsc::channel();
    let other = std::thread::spawn(move || {
        wait_for_open.recv().expect("capture opens");
        log_it();
        logged.send(()).expect("capture waits");
    });
    let lines = capture_logs(|| {
        opened.send(()).expect("other thread waits");
        wait_for_log.recv().expect("other thread logs");
        log_it();
    });
    other.join().expect("other thread finishes");
    assert_eq!(
        lines.count("callsite first reached on another thread"),
        1,
        "{}",
        lines.text()
    );
}

impl super::Spool {
    /// Reads a batch and reports it delivered at once, for tests of the spool
    /// alone that do not model a link: the read position and the delivered
    /// position move together.
    ///
    /// # Errors
    /// Returns the error of the read.
    pub(crate) fn read_and_deliver(&mut self, out: &mut Vec<u8>, limit: usize) -> io::Result<u32> {
        let before = out.len();
        let frames = self.read_batch(out, limit)?;
        if frames > 0 {
            self.delivered(out.len() - before, frames);
        }
        Ok(frames)
    }
}
