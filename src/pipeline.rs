//! Supervise ingestion and parsing, with bounded memory and explicit loss counts.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    net::SocketAddr,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser};
use crossbeam_channel::Receiver;

use crate::{
    dnstap::encode_event,
    etw::{self, Capture, CaptureCounters, PoolManager, RawEtwEvent},
    framestream::{FrameStreamWriter, STOP_FRAME_LENGTH},
    network::NetworkSink,
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Windows service for DNS Server ETW to dnstap",
    group(ArgGroup::new("destination").required(true).args(["output", "receiver"]))
)]
pub(crate) struct CaptureArgs {
    /// Absolute output base path; each start adds a unique timestamp/PID suffix.
    #[arg(short, long, value_parser = parse_output)]
    output: Option<PathBuf>,

    /// Plaintext Frame Streams TCP receiver as an IP:PORT (trusted networks only).
    #[arg(long, value_parser = parse_receiver)]
    receiver: Option<SocketAddr>,

    /// Event channel capacity; full channels drop the newest event.
    #[arg(long, default_value = "8192", value_parser = parse_capacity)]
    capacity: usize,

    /// Concurrent records over 2 KiB; zero disables their capture (maximum 256).
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u16).range(0..=256))]
    jumbo_capacity: u16,

    /// Stop after this many seconds; otherwise capture until SCM requests STOP.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=604_800))]
    duration: Option<u64>,

    /// ETW session name; defaults to etw-dnstap-PROCESS_ID.
    #[arg(long, value_parser = parse_session_name)]
    session_name: Option<String>,

    /// Optional dnstap producer identity, at most 1024 UTF-8 bytes.
    #[arg(long, default_value = "etw-dnstap", value_parser = parse_identity)]
    identity: String,

    /// Maximum file bytes; stops file capture at the limit (unused for receiver).
    #[arg(long, default_value = "1073741824", value_parser = clap::value_parser!(u64).range(131_072..=1_099_511_627_776))]
    max_output_bytes: u64,

    /// Optional metrics JSONL base path in a provisioned ACL-protected directory.
    #[arg(long, value_parser = parse_output)]
    metrics: Option<PathBuf>,
}

fn parse_receiver(value: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| "receiver must be a numeric IP:PORT")?;
    if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
        return Err("receiver must have a unicast IP and a nonzero port".into());
    }
    Ok(address)
}

enum OutputSink {
    File(FrameStreamWriter<BufWriter<File>>),
    Network(NetworkSink),
}

impl OutputSink {
    fn open(output: Option<PathBuf>, receiver: Option<SocketAddr>) -> Result<(Self, String)> {
        match (output, receiver) {
            (Some(output), None) => {
                let (file, path) = create_output(&output)?;
                let writer =
                    FrameStreamWriter::new(BufWriter::new(file)).context("write dnstap START")?;
                Ok((Self::File(writer), path.display().to_string()))
            }
            (None, Some(receiver)) => Ok((
                Self::Network(
                    NetworkSink::connect(receiver)
                        .context("connect/negotiation with dnstap receiver")?,
                ),
                receiver.to_string(),
            )),
            _ => bail!("specify exactly one of --output or --receiver"),
        }
    }

    fn write_frame(&mut self, payload: &[u8], max_file_bytes: u64) -> Result<bool> {
        match self {
            Self::File(writer) => write_with_limit(writer, payload, max_file_bytes),
            Self::Network(writer) => {
                writer.write_frame(payload).context("send dnstap packet")?;
                Ok(true)
            }
        }
    }

    fn flush(&mut self) -> Result<()> {
        match self {
            Self::File(writer) => writer.flush().context("flush dnstap output"),
            Self::Network(writer) => writer.flush().context("flush dnstap TCP sender"),
        }
    }

    fn finish(self) -> Result<()> {
        match self {
            Self::File(writer) => writer
                .finish()
                .context("write dnstap STOP and flush")?
                .get_ref()
                .sync_all()
                .context("synchronize dnstap file"),
            Self::Network(writer) => writer
                .finish()
                .context("finish dnstap STOP/FINISH exchange"),
        }
    }
}

fn parse_capacity(value: &str) -> Result<usize, String> {
    let capacity: usize = value.parse().map_err(|_| "capacity must be an integer")?;
    if !(1..=16_384).contains(&capacity) {
        return Err("capacity must be between 1 and 16384 events".into());
    }
    Ok(capacity)
}

fn parse_output(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    let local_drive = matches!(path.components().next(),
        Some(std::path::Component::Prefix(prefix))
        if matches!(prefix.kind(), std::path::Prefix::Disk(_)));
    if !local_drive
        || !path.is_absolute()
        || path.file_name().is_none()
        || value
            .get(2..)
            .is_none_or(|suffix| suffix.contains([':', '\0']))
        || value
            .replace('/', "\\")
            .split('\\')
            .skip(1)
            .any(invalid_output_component)
    {
        return Err("output must be an absolute local drive filename without device paths, alternate streams, or dot components".into());
    }
    Ok(path)
}

fn invalid_output_component(component: &str) -> bool {
    if matches!(component, "." | "..")
        || component.ends_with(['.', ' '])
        || component.contains(['<', '>', '"', '|', '?', '*'])
    {
        return true;
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn create_output(base: &Path) -> Result<(File, PathBuf)> {
    create_unique_output(base, "dnstap")
}

fn create_unique_output(base: &Path, extension: &str) -> Result<(File, PathBuf)> {
    // Provision an ACL-protected parent directory before starting the service.
    // This check complements that prerequisite; it cannot prevent a trusted
    // administrator from racing directory replacement.
    for ancestor in base
        .parent()
        .context("output parent is missing")?
        .ancestors()
    {
        let metadata = std::fs::symlink_metadata(ancestor)
            .with_context(|| format!("inspect output directory {}", ancestor.display()))?;
        anyhow::ensure!(
            metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
            "output ancestors must be real directories without reparse points"
        );
    }
    let filename = base
        .file_name()
        .context("output base filename is missing")?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("system time precedes Unix epoch")?;
    for attempt in 0..32 {
        let mut unique_name = filename.to_os_string();
        unique_name.push(format!(
            ".{}-{:09}-{}-{attempt}.{extension}",
            timestamp.as_secs(),
            timestamp.subsec_nanos(),
            std::process::id()
        ));
        let path = base.with_file_name(unique_name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create output file {}", path.display()));
            }
        }
    }
    bail!("could not create a unique output file after 32 attempts")
}

fn parse_session_name(value: &str) -> Result<String, String> {
    if value.is_empty() || value.encode_utf16().count() > 200 || value.contains('\0') {
        return Err("session name must contain 1..=200 UTF-16 units and no NUL".into());
    }
    Ok(value.into())
}

fn parse_identity(value: &str) -> Result<String, String> {
    if value.len() > 1024 {
        return Err("identity must be at most 1024 UTF-8 bytes".into());
    }
    Ok(value.into())
}

#[derive(Default)]
struct ParseCounters {
    dequeued: AtomicU64,
    encoded: AtomicU64,
    ignored: AtomicU64,
    rejected: AtomicU64,
    output_limit_dropped: AtomicU64,
    output_failed_events: AtomicU64,
    output_errors: AtomicU64,
    parse_ns: AtomicU64,
    output_ns: AtomicU64,
}

struct MetricsWorker {
    stop: Sender<()>,
    worker: thread::JoinHandle<Result<()>>,
}

impl MetricsWorker {
    fn start(
        file: File,
        session: &str,
        queue: Receiver<RawEtwEvent>,
        capture: Arc<CaptureCounters>,
        parser: Arc<ParseCounters>,
    ) -> Result<Self> {
        let session = json_string(session);
        let run = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let (stop, receive) = mpsc::channel();
        let worker = thread::Builder::new().name("metrics-writer".into()).spawn(move || {
            let mut writer = BufWriter::new(file);
            let mut bytes = 0usize;
            loop {
                let final_sample = match receive.recv_timeout(Duration::from_secs(1)) {
                    Ok(()) => true,
                    Err(mpsc::RecvTimeoutError::Timeout) => false,
                    Err(mpsc::RecvTimeoutError::Disconnected) => bail!("metrics supervisor disconnected"),
                };
                let epoch_ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_millis();
                let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
                let line = format!(
                    "{{\"schema\":2,\"run_id\":\"{run}-{}\",\"pid\":{},\"session\":{session},\"epoch_ms\":{epoch_ms},\"final\":{final_sample},\"received\":{},\"ring_dropped\":{},\"intake_dropped\":{},\"pool_exhausted\":{},\"receiver_disconnected\":{},\"callback_errors\":{},\"queue_backlog\":{},\"queue_high_water_observed\":{},\"dequeued\":{},\"encoded\":{},\"ignored\":{},\"rejected\":{},\"output_limit_dropped\":{},\"output_failed_events\":{},\"output_errors\":{},\"parse_ns\":{},\"output_ns\":{},\"etw_events_lost\":{},\"etw_realtime_buffers_lost\":{},\"etw_stats_valid\":{},\"etw_stats_errors\":{}}}\n",
                    std::process::id(), std::process::id(), load(&capture.received), load(&capture.dropped), load(&capture.intake_dropped), load(&capture.pool_exhausted), load(&capture.receiver_disconnected),
                    load(&capture.callback_errors), queue.len(), load(&capture.queue_high_water), load(&parser.dequeued),
                    load(&parser.encoded), load(&parser.ignored), load(&parser.rejected), load(&parser.output_limit_dropped),
                    load(&parser.output_failed_events), load(&parser.output_errors), load(&parser.parse_ns), load(&parser.output_ns),
                    load(&capture.etw_lost), load(&capture.etw_buffers_lost), capture.stats_valid.load(Ordering::Relaxed), load(&capture.stats_errors),
                );
                bytes = bytes.checked_add(line.len()).context("metrics byte count overflow")?;
                anyhow::ensure!(bytes <= 1_073_741_824, "metrics file reached 1 GiB limit");
                writer.write_all(line.as_bytes()).context("write metrics")?;
                writer.flush().context("flush metrics")?;
                if final_sample {
                    writer.get_ref().sync_all().context("synchronize final metrics")?;
                    return Ok(());
                }
            }
        }).context("start metrics writer")?;
        Ok(Self { stop, worker })
    }

    fn finish(self) -> Result<()> {
        // Join even if send fails, to retain the worker's actual I/O error.
        let _sent = self.stop.send(());
        self.worker
            .join()
            .map_err(|_| anyhow::anyhow!("metrics writer panicked"))?
    }
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            character if character < ' ' => {
                use std::fmt::Write as _;
                let _written = write!(result, "\\u{:04x}", u32::from(character));
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Holds the shutdown deadline until all native callbacks and file writes end.
pub(crate) struct ShutdownWatchdog {
    disarm: Sender<()>,
    worker: thread::JoinHandle<()>,
}

impl ShutdownWatchdog {
    pub(crate) fn arm() -> Self {
        match Self::spawn(SHUTDOWN_TIMEOUT, || std::process::abort()) {
            Ok(watchdog) => watchdog,
            // Failure reporting can itself block; the fail-stop path performs
            // no logging or cleanup before process termination.
            Err(_) => std::process::abort(),
        }
    }

    fn spawn(
        timeout: Duration,
        on_expiry: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (disarm, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("shutdown-watchdog".into())
            .spawn(move || {
                // Disconnection also fails closed: only explicit completion
                // disarms the watchdog, never dropping a native worker handle.
                if receive.recv_timeout(timeout).is_err() {
                    on_expiry();
                }
            })?;
        Ok(Self { disarm, worker })
    }

    pub(crate) fn disarm(self) -> Result<()> {
        self.disarm.send(()).context("disarm shutdown watchdog")?;
        self.worker
            .join()
            .map_err(|_| anyhow::anyhow!("shutdown watchdog panicked"))
    }
}

pub(crate) fn parse_args() -> CaptureArgs {
    CaptureArgs::parse()
}

#[expect(
    clippy::too_many_lines,
    reason = "Keeps native capture, parser and metrics shutdown ownership together."
)]
pub(crate) fn run_capture(
    args: CaptureArgs,
    stop: Arc<AtomicBool>,
    on_running: impl FnOnce() -> Result<()>,
    on_stopping: impl FnOnce() -> Result<()>,
) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize diagnostics: {error}"))?;

    let session_name = args
        .session_name
        .unwrap_or_else(|| format!("etw-dnstap-{}", std::process::id()));
    let metrics_file = args
        .metrics
        .as_ref()
        .map(|path| create_unique_output(path, "jsonl"))
        .transpose()?;
    let measure_timing = metrics_file.is_some();
    let (writer, destination) = OutputSink::open(args.output, args.receiver)?;
    let (sender, queue) = crossbeam_channel::bounded(args.capacity);
    let pool = PoolManager::new(args.capacity, usize::from(args.jumbo_capacity))?;
    let producer_done = Arc::new(AtomicBool::new(false));
    let capture_counts = Arc::new(CaptureCounters::default());
    let parse_counts = Arc::new(ParseCounters::default());

    let controller_stop = Arc::clone(&stop);
    let parser = {
        let queue = queue.clone();
        let producer_done = Arc::clone(&producer_done);
        let counters = Arc::clone(&parse_counts);
        thread::Builder::new()
            .name("dnstap-parser".into())
            .spawn(move || {
                parse_loop(
                    &queue,
                    &producer_done,
                    &counters,
                    &args.identity,
                    &stop,
                    args.max_output_bytes,
                    writer,
                    measure_timing,
                )
            })
            .context("start dnstap parser thread")?
    };

    let startup_watchdog = ShutdownWatchdog::arm();
    let capture = Capture::start(&session_name, sender, pool, Arc::clone(&capture_counts));
    let mut capture = match capture {
        Ok(capture) => capture,
        Err(error) => {
            startup_watchdog.disarm()?;
            let watchdog = ShutdownWatchdog::arm();
            producer_done.store(true, Ordering::Release);
            parser.thread().unpark();
            let parser_result = join_parser(parser);
            report_counts(&capture_counts, &parse_counts);
            watchdog.disarm()?;
            return combine_results(Err(error), parser_result);
        }
    };
    let metrics = metrics_file
        .map(|(file, _path)| {
            MetricsWorker::start(
                file,
                &session_name,
                queue.clone(),
                Arc::clone(&capture_counts),
                Arc::clone(&parse_counts),
            )
        })
        .transpose();
    let (metrics, running_result) = match metrics {
        Ok(metrics) => (metrics, on_running()),
        Err(error) => (None, Err(error)),
    };
    tracing::info!(
        session = session_name,
        destination,
        capacity = args.capacity,
        "capture started"
    );
    startup_watchdog.disarm()?;
    let started = Instant::now();
    let duration = args.duration.map(Duration::from_secs);
    let mut last_statistics = Instant::now();
    let unexpected_exit = loop {
        if running_result.is_err() {
            break false;
        }
        if controller_stop.load(Ordering::Relaxed)
            || duration.is_some_and(|limit| started.elapsed() >= limit)
        {
            break false;
        }
        if parser.is_finished()
            || capture.is_finished()
            || metrics
                .as_ref()
                .is_some_and(|worker| worker.worker.is_finished())
        {
            break true;
        }
        if measure_timing && last_statistics.elapsed() >= Duration::from_secs(1) {
            capture.query_statistics();
            last_statistics = Instant::now();
        }
        thread::park_timeout(Duration::from_millis(50));
    };

    let watchdog = ShutdownWatchdog::arm();
    let stopping_result = on_stopping();
    // stop joins ProcessTrace, so no producer can enqueue after this publication.
    let capture_result = capture.stop();
    // A failed stop may retain the handle for Drop to retry. Keep that retry
    // inside the deadline as well, while native resources are still retained.
    drop(capture);
    producer_done.store(true, Ordering::Release);
    parser.thread().unpark();
    let parser_result = join_parser(parser);
    let metrics_result = metrics.map_or(Ok(()), MetricsWorker::finish);
    report_counts(&capture_counts, &parse_counts);
    watchdog.disarm()?;
    combine_results(
        combine_results(capture_result, parser_result),
        metrics_result,
    )?;
    running_result?;
    stopping_result?;
    if unexpected_exit {
        bail!("a capture worker exited before shutdown was requested");
    }
    Ok(())
}

fn join_parser(parser: thread::JoinHandle<Result<()>>) -> Result<()> {
    parser
        .join()
        .map_err(|_| anyhow::anyhow!("dnstap parser thread panicked"))?
}

fn combine_results(capture: Result<()>, parser: Result<()>) -> Result<()> {
    match (capture, parser) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(capture), Err(parser)) => {
            Err(capture.context(format!("parser also failed: {parser:#}")))
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Explicit borrowed worker state and opt-in timing avoid shared mutable state."
)]
fn parse_loop(
    queue: &Receiver<RawEtwEvent>,
    producer_done: &AtomicBool,
    counters: &ParseCounters,
    identity: &str,
    stop: &AtomicBool,
    max_output_bytes: u64,
    mut writer: OutputSink,
    measure_timing: bool,
) -> Result<()> {
    let mut last_flush = Instant::now();
    let mut limit_reached = false;
    drain_ring(queue, producer_done, |event| {
        if let Some(event) = event {
            counters.dequeued.fetch_add(1, Ordering::Relaxed);
            let parse_started = measure_timing.then(Instant::now);
            let result = etw::parse(event).and_then(|event| {
                event
                    .map(|event| encode_event(event, identity.as_bytes()))
                    .transpose()
                    .map_err(Into::into)
            });
            if let Some(started) = parse_started {
                add_elapsed(&counters.parse_ns, started);
            }
            match result {
                Ok(Some(payload)) => {
                    let output_started = measure_timing.then(Instant::now);
                    let output = if limit_reached {
                        Ok(false)
                    } else {
                        writer.write_frame(&payload, max_output_bytes)
                    };
                    if let Some(started) = output_started {
                        add_elapsed(&counters.output_ns, started);
                    }
                    let written = output.inspect_err(|_error| {
                        counters
                            .output_failed_events
                            .fetch_add(1, Ordering::Relaxed);
                        counters.output_errors.fetch_add(1, Ordering::Relaxed);
                    })?;
                    if !written {
                        limit_reached = true;
                        counters
                            .output_limit_dropped
                            .fetch_add(1, Ordering::Relaxed);
                        stop.store(true, Ordering::Relaxed);
                        return Ok(());
                    }
                    counters.encoded.fetch_add(1, Ordering::Relaxed);
                }
                Ok(None) => {
                    counters.ignored.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    let previous = counters.rejected.fetch_add(1, Ordering::Relaxed);
                    // Bound diagnostics and never print event payloads or domain names.
                    if previous < 4 {
                        tracing::warn!(%error, "rejected ETW event; further errors counted");
                    }
                }
            }
        }
        if last_flush.elapsed() >= Duration::from_secs(1) {
            let output_started = measure_timing.then(Instant::now);
            let flushed = writer.flush();
            if let Some(started) = output_started {
                add_elapsed(&counters.output_ns, started);
            }
            flushed.inspect_err(|_error| {
                counters.output_errors.fetch_add(1, Ordering::Relaxed);
            })?;
            last_flush = Instant::now();
        }
        Ok(())
    })?;
    let output_started = measure_timing.then(Instant::now);
    let finished = writer.finish();
    if let Some(started) = output_started {
        add_elapsed(&counters.output_ns, started);
    }
    finished.inspect_err(|_error| {
        counters.output_errors.fetch_add(1, Ordering::Relaxed);
    })
}

fn add_elapsed(counter: &AtomicU64, started: Instant) {
    counter.fetch_add(
        u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

fn write_with_limit<W: std::io::Write>(
    writer: &mut FrameStreamWriter<W>,
    payload: &[u8],
    max_output_bytes: u64,
) -> Result<bool> {
    let frame_length = u64::try_from(payload.len()).context("dnstap frame length exceeds u64")?;
    let projected_length = writer
        .bytes_written()
        .checked_add(frame_length)
        .and_then(|length| length.checked_add(4 + STOP_FRAME_LENGTH))
        .context("output byte counter overflow")?;
    if projected_length > max_output_bytes {
        return Ok(false);
    }
    writer.write_frame(payload).context("write dnstap packet")?;
    Ok(true)
}

// The completion flag is separate from the queue: after observing completion,
// retry receive so a final send between the previous receive and flag load drains.
fn drain_ring<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    mut consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    loop {
        if let Ok(event) = queue.try_recv() {
            consume(Some(event))?;
        } else if producer_done.load(Ordering::Acquire) {
            while let Ok(event) = queue.try_recv() {
                consume(Some(event))?;
            }
            return Ok(());
        } else {
            consume(None)?;
            thread::park_timeout(Duration::from_millis(2));
        }
    }
}

fn report_counts(capture: &CaptureCounters, parser: &ParseCounters) {
    tracing::info!(
        received = capture.received.load(Ordering::Relaxed),
        encoded = parser.encoded.load(Ordering::Relaxed),
        ring_dropped = capture.dropped.load(Ordering::Relaxed),
        intake_dropped = capture.intake_dropped.load(Ordering::Relaxed),
        pool_exhausted = capture.pool_exhausted.load(Ordering::Relaxed),
        receiver_disconnected = capture.receiver_disconnected.load(Ordering::Relaxed),
        etw_lost = capture.etw_lost.load(Ordering::Relaxed),
        etw_buffers_lost = capture.etw_buffers_lost.load(Ordering::Relaxed),
        callback_errors = capture.callback_errors.load(Ordering::Relaxed),
        rejected = parser.rejected.load(Ordering::Relaxed),
        output_limit_dropped = parser.output_limit_dropped.load(Ordering::Relaxed),
        "capture stopped"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_escape_control_characters_and_propagate_write_failures() -> Result<()> {
        assert_eq!(
            json_string("x\"\n\t\\\u{0001}中"),
            "\"x\\\"\\u000a\\u0009\\\\\\u0001中\""
        );
        let base = std::env::temp_dir().join(format!("etw-metrics-test-{}", std::process::id()));
        let (file, path) = create_unique_output(&base, "jsonl")?;
        drop(file);
        let readonly = File::open(&path)?;
        let worker = MetricsWorker::start(
            readonly,
            "test",
            crossbeam_channel::bounded(1).1,
            Arc::new(CaptureCounters::default()),
            Arc::new(ParseCounters::default()),
        )?;
        let result = worker.finish();
        std::fs::remove_file(path)?;
        assert!(
            result.is_err(),
            "a metrics I/O failure must invalidate capture"
        );
        Ok(())
    }

    #[test]
    fn metrics_schema_two_keeps_loss_categories_distinct() -> Result<()> {
        let base = std::env::current_dir()?
            .join("target")
            .join(format!("schema-two-{}", std::process::id()));
        let (file, path) = create_unique_output(&base, "jsonl")?;
        let capture = Arc::new(CaptureCounters::default());
        capture.received.store(12, Ordering::Relaxed);
        capture.dropped.store(2, Ordering::Relaxed);
        capture.intake_dropped.store(9, Ordering::Relaxed);
        capture.pool_exhausted.store(3, Ordering::Relaxed);
        capture.receiver_disconnected.store(4, Ordering::Relaxed);
        capture.callback_errors.store(1, Ordering::Relaxed);
        let parser = Arc::new(ParseCounters::default());
        parser.dequeued.store(2, Ordering::Relaxed);
        MetricsWorker::start(
            file,
            "schema-two",
            crossbeam_channel::bounded(1).1,
            capture,
            parser,
        )?
        .finish()?;
        let text = std::fs::read_to_string(&path)?;
        std::fs::remove_file(path)?;
        assert!(text.contains("\"schema\":2"));
        assert!(text.contains("\"received\":12,\"ring_dropped\":2,\"intake_dropped\":9,\"pool_exhausted\":3,\"receiver_disconnected\":4,\"callback_errors\":1,\"queue_backlog\":0"));
        assert!(text.contains("\"dequeued\":2"));
        Ok(())
    }

    #[test]
    fn validates_memory_and_cli_bounds() {
        assert!(parse_capacity("0").is_err());
        assert!(parse_capacity("16385").is_err());
        assert!(parse_capacity("-1").is_err());
        assert_eq!(parse_capacity("1024"), Ok(1024));
        assert_eq!(parse_capacity("8192"), Ok(8192));
        assert_eq!(
            CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"])
                .expect("receiver arguments are valid")
                .capacity,
            8192
        );
        for capacity in ["0", "256"] {
            assert!(
                CaptureArgs::try_parse_from([
                    "etw-dnstap",
                    "--receiver",
                    "127.0.0.1:6000",
                    "--jumbo-capacity",
                    capacity
                ])
                .is_ok()
            );
        }
        assert!(
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:6000",
                "--jumbo-capacity",
                "257"
            ])
            .is_err()
        );
        assert_eq!(
            CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"])
                .expect("default pool arguments")
                .jumbo_capacity,
            64
        );
        assert!(parse_session_name("").is_err());
        assert!(parse_session_name("bad\0name").is_err());
        assert!(parse_identity(&"a".repeat(1025)).is_err());
        assert!(parse_output("relative.dnstap").is_err());
        assert!(parse_output("C:\\").is_err());
        for invalid in [
            "\\\\server\\share\\dns",
            "\\\\?\\C:\\dns",
            "C:\\dns:stream",
            "C:\\x\\..\\dns",
            "C:\\NUL",
            "C:\\aux.dnstap",
            "C:\\captures.\\dns",
        ] {
            assert!(parse_output(invalid).is_err());
        }
        assert_eq!(
            parse_output("C:\\captures\\dns"),
            Ok(PathBuf::from("C:\\captures\\dns"))
        );
        assert!(
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "-o",
                "C:\\captures\\dns",
                "--duration",
                "0"
            ])
            .is_err()
        );
    }

    #[test]
    fn requires_exactly_one_destination_and_a_numeric_receiver() {
        assert!(CaptureArgs::try_parse_from(["etw-dnstap"]).is_err());
        assert!(
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "--output",
                "C:\\captures\\dns",
                "--receiver",
                "127.0.0.1:6000"
            ])
            .is_err()
        );
        assert!(
            CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"]).is_ok()
        );
        for invalid in [
            "collector:6000",
            "127.0.0.1:0",
            "0.0.0.0:6000",
            "224.0.0.1:6000",
        ] {
            assert!(parse_receiver(invalid).is_err());
        }
    }

    #[test]
    fn ring_drops_newest_and_drains_in_order_after_completion() {
        let (send, queue) = crossbeam_channel::bounded(2);
        send.try_send(1).expect("space available");
        send.try_send(2).expect("space available");
        assert_eq!(
            send.try_send(3),
            Err(crossbeam_channel::TrySendError::Full(3))
        );
        let done = AtomicBool::new(true);
        let mut values = Vec::new();
        drain_ring(&queue, &done, |value| {
            values.extend(value);
            Ok(())
        })
        .expect("drain succeeds");
        assert_eq!(values, [1, 2]);
    }

    #[test]
    fn completion_publication_keeps_final_event() {
        let (send, queue) = crossbeam_channel::bounded(1);
        let done = AtomicBool::new(false);
        let mut values = Vec::new();
        drain_ring(&queue, &done, |value| {
            if let Some(value) = value {
                values.push(value);
            } else {
                send.try_send(42).expect("empty queue");
                done.store(true, Ordering::Release);
            }
            Ok(())
        })
        .expect("drain succeeds");
        assert_eq!(values, [42]);
    }

    #[test]
    fn idle_parser_drains_final_enqueue_before_joined_producer_completion() -> Result<()> {
        let (send, queue) = crossbeam_channel::bounded(1);
        let done = Arc::new(AtomicBool::new(false));
        let (idle_send, idle_receive) = mpsc::channel();
        let (finished_send, finished_receive) = mpsc::channel();
        let parser = {
            let queue = queue.clone();
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let mut idle_send = Some(idle_send);
                let mut values = Vec::new();
                let result = drain_ring(&queue, &done, |value| {
                    if let Some(value) = value {
                        values.push(value);
                    } else if let Some(send) = idle_send.take() {
                        send.send(()).context("report idle parser")?;
                    }
                    Ok(())
                });
                let _sent = finished_send.send((result, values));
            })
        };
        let idle = idle_receive.recv_timeout(Duration::from_secs(5));
        if idle.is_ok() {
            let producer = {
                let send = send.clone();
                let parser = parser.thread().clone();
                thread::spawn(move || {
                    send.try_send(42).expect("final event fits");
                    parser.unpark();
                })
            };
            producer
                .join()
                .expect("producer joins before done publication");
        }
        done.store(true, Ordering::Release);
        parser.thread().unpark();
        let finished = finished_receive.recv_timeout(Duration::from_secs(5));
        parser.join().expect("parser joins after completion");
        idle.context("parser did not become idle")?;
        let (result, values) = finished.context("parser did not finish")?;
        result?;
        assert_eq!(values, [42]);
        Ok(())
    }

    #[test]
    fn startup_failure_completion_finishes_an_empty_ring() -> Result<()> {
        let (_send, queue) = crossbeam_channel::bounded::<u8>(1);
        let done = AtomicBool::new(true);
        drain_ring(&queue, &done, |_| {
            bail!("empty completed ring must not consume")
        })
    }

    #[test]
    fn consumer_failure_is_propagated() {
        let (send, queue) = crossbeam_channel::bounded(1);
        send.try_send(1).expect("space available");
        let error = drain_ring(&queue, &AtomicBool::new(true), |_| bail!("output failed"))
            .expect_err("output error must escape");
        assert_eq!(error.to_string(), "output failed");
    }

    #[test]
    fn watchdog_disarms_only_after_explicit_completion() {
        let expired = Arc::new(AtomicBool::new(false));
        let observe = Arc::clone(&expired);
        let watchdog = ShutdownWatchdog::spawn(Duration::from_secs(5), move || {
            observe.store(true, Ordering::Relaxed);
        })
        .expect("watchdog starts");
        watchdog.disarm().expect("completion disarms watchdog");
        assert!(!expired.load(Ordering::Relaxed));
    }

    #[test]
    fn watchdog_reports_an_exceeded_shutdown_deadline() {
        let expired = Arc::new(AtomicBool::new(false));
        let observe = Arc::clone(&expired);
        let watchdog = ShutdownWatchdog::spawn(Duration::ZERO, move || {
            observe.store(true, Ordering::Relaxed);
        })
        .expect("watchdog starts");
        watchdog.worker.join().expect("watchdog finishes");
        assert!(expired.load(Ordering::Relaxed));
    }

    #[test]
    fn every_start_creates_a_new_output_without_replacing_the_base() {
        let directory = std::env::current_dir()
            .expect("workspace directory")
            .join("target");
        std::fs::create_dir_all(&directory).expect("test output directory exists");
        let base = directory.join(format!("output-test-{}", std::process::id()));
        let (first_file, first) = create_output(&base).expect("first capture output opens");
        let (second_file, second) = create_output(&base).expect("second capture output opens");
        assert_ne!(first, second);
        assert!(!base.exists());
        drop(first_file);
        drop(second_file);
        std::fs::remove_file(first).expect("remove first test capture");
        std::fs::remove_file(second).expect("remove second test capture");
    }

    #[test]
    fn output_limit_reserves_stop_and_keeps_existing_complete_frames() {
        let mut writer = FrameStreamWriter::new(Vec::new()).expect("START writes");
        assert!(write_with_limit(&mut writer, &[0x78, 1], 60).expect("first frame writes"));
        assert!(!write_with_limit(&mut writer, &[0x78, 1], 60).expect("limit check succeeds"));
        let bytes = writer.finish().expect("STOP fits in reserved bytes");
        assert_eq!(bytes.len(), 60);
        assert_eq!(&bytes[48..], &[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3]);
    }
}
