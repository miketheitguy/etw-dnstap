//! Supervise ingestion and parsing, with bounded memory and explicit loss counts.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossbeam_channel::Receiver;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::{
    dnstap::Encoder,
    etw::{self, Capture, CaptureCounters, PoolManager, RawEtwEvent, SessionConfig},
    network::NetworkSink,
};

/// Fixed default so a restart finds and reclaims a session leaked by a crash.
const DEFAULT_SESSION_NAME: &str = "EtwDnstap";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Windows service for DNS Server ETW to a dnstap TCP receiver"
)]
pub(crate) struct CaptureArgs {
    /// Plaintext Frame Streams TCP receiver as an IP:PORT (trusted networks only).
    #[arg(long, value_parser = parse_receiver)]
    receiver: SocketAddr,

    /// Event channel capacity, 1024–262144; full channels drop the newest event.
    #[arg(long, default_value = "8192", value_parser = parse_capacity)]
    capacity: usize,

    /// Concurrent records over 2 KiB; zero disables their capture (maximum 256).
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u16).range(0..=256))]
    jumbo_capacity: u16,

    /// Stop after this many seconds; otherwise capture until SCM requests STOP.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=604_800))]
    duration: Option<u64>,

    /// ETW session name. A leaked session of the same name and shape is reclaimed
    /// at startup.
    #[arg(long, default_value = DEFAULT_SESSION_NAME, value_parser = parse_session_name)]
    session_name: String,

    /// Optional dnstap producer identity, at most 1024 UTF-8 bytes.
    #[arg(long, default_value = "etw-dnstap", value_parser = parse_identity)]
    identity: String,

    /// ETW buffer size in KiB, 4–128, must be a multiple of 4.
    #[arg(long, default_value = "16", value_parser = clap::value_parser!(u32).range(4..=128))]
    etw_buffer_kib: u32,

    /// Requested maximum ETW buffers, 4–4096. ETW may reserve more (at least two buffers
    /// per logical processor, and raises the maximum to at least that).
    #[arg(long, default_value = "12", value_parser = clap::value_parser!(u32).range(4..=4096))]
    etw_max_buffers: u32,

    /// ETW real-time flush interval in seconds, 1–60. This is ETW's own flush
    /// timer. With `--etw-flush-ms` enabled the explicit flush
    /// bounds delivery latency to about that interval; this timer is the bound
    /// with the default `--etw-flush-ms 0`. Longer intervals use less CPU.
    #[arg(long, default_value = "1", value_parser = clap::value_parser!(u32).range(1..=60))]
    etw_flush_seconds: u32,

    /// Interval in milliseconds at which the service asks ETW to flush its session
    /// buffers, 20–1000, or 0 to disable (the default). ETW otherwise delivers
    /// real-time events about once per `--etw-flush-seconds`, even when its buffers
    /// are full, which at moderate rates arrives as one large batch that can
    /// overflow the event channel. The defaults suit about 1,000 queries/s or less;
    /// for higher rates use `--etw-flush-ms 50 --etw-max-buffers 256
    /// --etw-buffer-kib 64`.
    #[arg(long, default_value = "0", value_parser = parse_etw_flush_ms)]
    etw_flush_ms: u32,

    /// Diagnostic only: request this system timer resolution in milliseconds
    /// (`timeBeginPeriod`) for the whole run.
    #[cfg(feature = "diag-timing")]
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=15))]
    diag_timer_resolution_ms: Option<u32>,

    /// Diagnostic only: raise the priority of the `dnstap-parser` and
    /// `etw-consumer` threads.
    #[cfg(feature = "diag-timing")]
    #[arg(long, value_enum)]
    diag_thread_priority: Option<etw::ThreadPriority>,
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

fn parse_etw_flush_ms(value: &str) -> Result<u32, String> {
    let milliseconds: u32 = value
        .parse()
        .map_err(|_| "etw-flush-ms must be an integer")?;
    if milliseconds != 0 && !(20..=1000).contains(&milliseconds) {
        return Err("etw-flush-ms must be 0 (disabled) or 20-1000".into());
    }
    Ok(milliseconds)
}

/// Schedules periodic ETW flushes on the supervisor thread.
struct FlushSchedule {
    interval: Option<Duration>,
    next: Instant,
}

impl FlushSchedule {
    /// `None` disables flushing.
    fn new(interval: Option<Duration>, now: Instant) -> Self {
        Self {
            interval,
            next: now + interval.unwrap_or_default(),
        }
    }

    /// True when a flush is due; advances `next` to `now + interval`.
    fn poll(&mut self, now: Instant) -> bool {
        match self.interval {
            Some(interval) if now >= self.next => {
                self.next = now + interval;
                true
            }
            _ => false,
        }
    }

    /// How long the supervisor may park: the time until the next flush, capped
    /// at `cap` and at least 1 ms; `cap` when flushing is disabled.
    fn park_for(&self, now: Instant, cap: Duration) -> Duration {
        if self.interval.is_none() {
            return cap;
        }
        self.next
            .saturating_duration_since(now)
            .max(Duration::from_millis(1))
            .min(cap)
    }
}

/// Longest single flush the supervisor tolerates silently.
const SLOW_ETW_FLUSH: Duration = Duration::from_secs(1);

/// Counts one flush outcome. Only the first failure is logged; failures never
/// stop capture.
fn record_etw_flush(counters: &CaptureCounters, result: Result<(), u32>) {
    match result {
        Ok(()) => {
            counters.etw_flushes.fetch_add(1, Ordering::Relaxed);
        }
        Err(status) => {
            if counters.etw_flush_errors.fetch_add(1, Ordering::Relaxed) == 0 {
                tracing::warn!(
                    code = status,
                    "ETW session flush failed: {}; capture continues, further failures counted",
                    std::io::Error::from_raw_os_error(status.cast_signed())
                );
            }
        }
    }
}

/// Runs one flush, counts it, and records its duration.
fn flush_etw(capture: &Capture, counters: &CaptureCounters) {
    let flush_started = Instant::now();
    let result = capture.flush();
    let elapsed = flush_started.elapsed();
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    counters
        .etw_flush_max_us
        .fetch_max(micros, Ordering::Relaxed);
    if elapsed > SLOW_ETW_FLUSH && !counters.etw_flush_slow_warned.swap(true, Ordering::Relaxed) {
        tracing::warn!("ETW session flush took {} ms", elapsed.as_millis());
    }
    record_etw_flush(counters, result);
}

fn parse_capacity(value: &str) -> Result<usize, String> {
    let capacity: usize = value.parse().map_err(|_| "capacity must be an integer")?;
    if !(crate::etw::MIN_CAPACITY..=crate::etw::MAX_CAPACITY).contains(&capacity) {
        return Err(format!(
            "capacity must be between {} and {} events",
            crate::etw::MIN_CAPACITY,
            crate::etw::MAX_CAPACITY
        ));
    }
    Ok(capacity)
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

/// Holds a `timeBeginPeriod` request and withdraws it on drop, so every exit
/// path from `run_capture` pairs the calls.
#[cfg(feature = "diag-timing")]
struct TimerResolution {
    milliseconds: u32,
    end: fn(u32) -> u32,
}

#[cfg(feature = "diag-timing")]
impl TimerResolution {
    fn begin(milliseconds: u32) -> Result<Self> {
        Self::begin_with(milliseconds, etw::time_begin_period, etw::time_end_period)
    }

    fn begin_with(milliseconds: u32, begin: fn(u32) -> u32, end: fn(u32) -> u32) -> Result<Self> {
        let result = begin(milliseconds);
        if result != etw::TIME_NO_ERROR {
            bail!("timeBeginPeriod({milliseconds}) failed with MMRESULT {result}");
        }
        Ok(Self { milliseconds, end })
    }
}

#[cfg(feature = "diag-timing")]
impl Drop for TimerResolution {
    fn drop(&mut self) {
        (self.end)(self.milliseconds);
    }
}

#[derive(Default)]
struct ParseCounters {
    dequeued: AtomicU64,
    encoded: AtomicU64,
    ignored: AtomicU64,
    rejected: AtomicU64,
    output_failed_events: AtomicU64,
    output_errors: AtomicU64,
    #[cfg(feature = "diag-timing")]
    timing: DiagTiming,
}

/// Parser-thread wait attribution, compiled only for the diagnostic build. Only
/// the parser thread writes these; the final report reads them after the join.
#[cfg(feature = "diag-timing")]
#[derive(Default)]
struct DiagTiming {
    parks: AtomicU64,
    backoff_parks: AtomicU64,
    park_ns: AtomicU64,
    wake_len_max: AtomicU64,
    wake_len_0: AtomicU64,
    wake_len_lt_1k: AtomicU64,
    wake_len_lt_4k: AtomicU64,
    wake_len_lt_8k: AtomicU64,
    wake_len_ge_8k_or_full: AtomicU64,
    max_run: AtomicU64,
    write_ns: AtomicU64,
    writes_over_1ms: AtomicU64,
    flush_ns: AtomicU64,
    flushes_over_1ms: AtomicU64,
}

#[cfg(feature = "diag-timing")]
impl DiagTiming {
    const SLOW_CALL: Duration = Duration::from_millis(1);

    fn nanos(elapsed: Duration) -> u64 {
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    }

    fn record_park(&self, elapsed: Duration, requested: Duration) {
        self.parks.fetch_add(1, Ordering::Relaxed);
        if requested > IDLE_POLL_MIN {
            self.backoff_parks.fetch_add(1, Ordering::Relaxed);
        }
        self.park_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
    }

    /// Records the queue length first seen after a park. "Full" is `len >=
    /// capacity` for a bounded queue.
    fn record_wake(&self, len: usize, capacity: Option<usize>) {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        self.wake_len_max.fetch_max(len, Ordering::Relaxed);
        let full =
            capacity.is_some_and(|capacity| len >= u64::try_from(capacity).unwrap_or(u64::MAX));
        let bucket = if len == 0 {
            &self.wake_len_0
        } else if len >= 8192 || full {
            &self.wake_len_ge_8k_or_full
        } else if len < 1024 {
            &self.wake_len_lt_1k
        } else if len < 4096 {
            &self.wake_len_lt_4k
        } else {
            &self.wake_len_lt_8k
        };
        bucket.fetch_add(1, Ordering::Relaxed);
    }

    fn record_run(&self, events: u64) {
        self.max_run.fetch_max(events, Ordering::Relaxed);
    }

    fn record_write(&self, elapsed: Duration) {
        self.write_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
        if elapsed > Self::SLOW_CALL {
            self.writes_over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_flush(&self, elapsed: Duration) {
        self.flush_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
        if elapsed > Self::SLOW_CALL {
            self.flushes_over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }
}

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Holds the shutdown deadline until all native callbacks and network writes end.
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

/// Parses the process command line without exiting, so the caller decides how
/// to report help, version, and argument errors.
pub(crate) fn parse_args() -> Result<CaptureArgs, clap::Error> {
    parse_args_from(std::env::args_os())
}

fn parse_args_from<I, T>(arguments: I) -> Result<CaptureArgs, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    CaptureArgs::try_parse_from(arguments)
}

pub(crate) fn run_capture(
    args: CaptureArgs,
    stop: &AtomicBool,
    on_running: impl FnOnce() -> Result<()>,
    on_stopping: impl FnOnce() -> Result<()>,
) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr.and(crate::service::EventLogWriter))
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize diagnostics: {error}"))?;

    // Declared before any worker thread so it drops after both are joined.
    #[cfg(feature = "diag-timing")]
    let (_timer_resolution, thread_priority) = begin_diag_levers(&args)?;
    let session_config = SessionConfig::new(
        args.etw_buffer_kib,
        args.etw_max_buffers,
        args.etw_flush_seconds,
    )?;
    let etw_flush_ms = args.etw_flush_ms;
    let etw_flush_interval =
        (etw_flush_ms != 0).then(|| Duration::from_millis(u64::from(etw_flush_ms)));
    let session_name = args.session_name;
    let writer =
        NetworkSink::connect(args.receiver).context("connect/negotiation with dnstap receiver")?;
    let (sender, queue) = crossbeam_channel::bounded(args.capacity);
    let pool = PoolManager::new(args.capacity, usize::from(args.jumbo_capacity))?;
    let producer_done = Arc::new(AtomicBool::new(false));
    let capture_counts = Arc::new(CaptureCounters::default());
    let parse_counts = Arc::new(ParseCounters::default());

    let parser = spawn_parser(
        queue,
        Arc::clone(&producer_done),
        Arc::clone(&parse_counts),
        args.identity,
        writer,
        #[cfg(feature = "diag-timing")]
        thread_priority,
    )?;

    let startup_watchdog = ShutdownWatchdog::arm();
    let capture = Capture::start(
        &session_name,
        sender,
        pool,
        Arc::clone(&capture_counts),
        session_config,
        #[cfg(feature = "diag-timing")]
        thread_priority,
    );
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
    let running_result = on_running();
    let keywords = capture.keywords();
    log_capture_started(
        &session_name,
        args.receiver,
        args.capacity,
        session_config,
        etw_flush_ms,
        keywords,
    );
    #[cfg(feature = "diag-timing")]
    log_diag_settings(args.diag_timer_resolution_ms, thread_priority);
    startup_watchdog.disarm()?;
    let started = Instant::now();
    let duration = args.duration.map(Duration::from_secs);
    let schedule = FlushSchedule::new(etw_flush_interval, Instant::now());
    let unexpected_exit = supervise(
        stop,
        duration.map(|limit| started + limit),
        running_result.is_err(),
        &parser,
        &capture,
        &capture_counts,
        schedule,
    );

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
    report_counts(&capture_counts, &parse_counts);
    watchdog.disarm()?;
    combine_results(capture_result, parser_result)?;
    running_result?;
    stopping_result?;
    if unexpected_exit {
        bail!("a capture worker exited before shutdown was requested");
    }
    Ok(())
}

/// Supervises the workers until shutdown. Returns whether a worker exited
/// before shutdown was requested.
fn supervise(
    stop: &AtomicBool,
    deadline: Option<Instant>,
    startup_failed: bool,
    parser: &thread::JoinHandle<Result<()>>,
    capture: &Capture,
    counters: &CaptureCounters,
    mut schedule: FlushSchedule,
) -> bool {
    loop {
        if startup_failed {
            break false;
        }
        if stop.load(Ordering::Relaxed) || deadline.is_some_and(|limit| Instant::now() >= limit) {
            break false;
        }
        if parser.is_finished() || capture.is_finished() {
            break true;
        }
        // Flushing here, not in the ETW callback, keeps the callback wake-free
        // (docs/ring-wakeup.md). The park ends at the next flush deadline, so the
        // cadence is the setting plus at most one timer tick (about 15.6 ms).
        if schedule.poll(Instant::now()) {
            flush_etw(capture, counters);
        }
        thread::park_timeout(schedule.park_for(Instant::now(), Duration::from_millis(50)));
    }
}

fn log_capture_started(
    session: &str,
    receiver: SocketAddr,
    capacity: usize,
    session_config: SessionConfig,
    etw_flush_ms: u32,
    keywords: etw::KeywordSelection,
) {
    tracing::info!(
        session,
        receiver = %receiver,
        capacity,
        etw_buffer_kib = session_config.buffer_kib,
        etw_max_buffers = session_config.maximum_buffers,
        etw_flush_seconds = session_config.flush_seconds,
        etw_flush_ms,
        match_any_keyword = format_args!("{:#x}", keywords.match_any),
        match_all_keyword = format_args!("{:#x}", keywords.match_all),
        keyword_fallback = keywords.fallback,
        "capture started"
    );
}

/// Applies the process-wide diagnostic lever; the guard must outlive the workers.
#[cfg(feature = "diag-timing")]
fn begin_diag_levers(
    args: &CaptureArgs,
) -> Result<(Option<TimerResolution>, Option<etw::ThreadPriority>)> {
    let timer = args
        .diag_timer_resolution_ms
        .map(TimerResolution::begin)
        .transpose()?;
    Ok((timer, args.diag_thread_priority))
}

/// Records which diagnostic levers a run applied.
#[cfg(feature = "diag-timing")]
fn log_diag_settings(timer_resolution_ms: Option<u32>, priority: Option<etw::ThreadPriority>) {
    tracing::info!(
        diag_timer_resolution_ms =
            timer_resolution_ms.map_or_else(|| "default".to_owned(), |ms| ms.to_string()),
        diag_thread_priority = priority.map_or("normal", etw::ThreadPriority::as_str),
        "diagnostic settings applied"
    );
}

fn spawn_parser(
    queue: Receiver<RawEtwEvent>,
    producer_done: Arc<AtomicBool>,
    counters: Arc<ParseCounters>,
    identity: String,
    writer: NetworkSink,
    #[cfg(feature = "diag-timing")] priority: Option<etw::ThreadPriority>,
) -> Result<thread::JoinHandle<Result<()>>> {
    thread::Builder::new()
        .name("dnstap-parser".into())
        .spawn(move || {
            parse_loop(
                &queue,
                &producer_done,
                &counters,
                &identity,
                writer,
                #[cfg(feature = "diag-timing")]
                priority,
            )
        })
        .context("start dnstap parser thread")
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

fn parse_loop(
    queue: &Receiver<RawEtwEvent>,
    producer_done: &AtomicBool,
    counters: &ParseCounters,
    identity: &str,
    mut writer: NetworkSink,
    #[cfg(feature = "diag-timing")] priority: Option<etw::ThreadPriority>,
) -> Result<()> {
    #[cfg(feature = "diag-timing")]
    if let Some(priority) = priority {
        etw::set_current_thread_priority(priority);
    }
    let mut parser = etw::Parser::new();
    let mut encoder = Encoder::new(identity.as_bytes())?;
    let mut last_flush = Instant::now();
    let consume = |event: Option<RawEtwEvent>| {
        if let Some(event) = event {
            counters.dequeued.fetch_add(1, Ordering::Relaxed);
            // Parsing consumes and releases the pooled record before network I/O;
            // the returned payload borrows only reusable encoder storage.
            let result = parser.parse(event, &mut encoder);
            match result {
                Ok(Some(payload)) => {
                    #[cfg(feature = "diag-timing")]
                    let write_started = Instant::now();
                    let sent = writer.write_frame(payload);
                    #[cfg(feature = "diag-timing")]
                    counters.timing.record_write(write_started.elapsed());
                    sent.context("send dnstap packet").inspect_err(|_error| {
                        counters
                            .output_failed_events
                            .fetch_add(1, Ordering::Relaxed);
                        counters.output_errors.fetch_add(1, Ordering::Relaxed);
                    })?;
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
            #[cfg(feature = "diag-timing")]
            let flush_started = Instant::now();
            let flushed = writer.flush();
            #[cfg(feature = "diag-timing")]
            counters.timing.record_flush(flush_started.elapsed());
            flushed
                .context("flush dnstap TCP sender")
                .inspect_err(|_error| {
                    counters.output_errors.fetch_add(1, Ordering::Relaxed);
                })?;
            last_flush = Instant::now();
        }
        Ok(())
    };
    #[cfg(not(feature = "diag-timing"))]
    drain_ring(queue, producer_done, consume)?;
    #[cfg(feature = "diag-timing")]
    drain_ring_inner(queue, producer_done, Some(&counters.timing), consume)?;
    writer
        .finish()
        .context("finish dnstap STOP/FINISH exchange")
        .inspect_err(|_error| {
            counters.output_errors.fetch_add(1, Ordering::Relaxed);
        })
}

// The ETW callback must stay wake-free; see docs/ring-wakeup.md. The parser
// therefore polls with a bounded backoff: each empty poll doubles the park from
// 2 ms to at most 20 ms, and any received event resets it to 2 ms. Windows'
// default ~15.6 ms timer granularity stretches these requests, so a 20 ms park
// can last about 31 ms. Nominal idle wake-ups fall from about 500/s to about
// 50/s (about 37/s measured). Shutdown is unaffected because completion is
// followed by unpark, which ends any park.
const IDLE_POLL_MIN: Duration = Duration::from_millis(2);
const IDLE_POLL_MAX: Duration = Duration::from_millis(20);

/// Resets the idle poll interval after an event and otherwise doubles it, clamped
/// to `IDLE_POLL_MAX`.
fn idle_poll_after(received: bool, current: Duration) -> Duration {
    if received {
        IDLE_POLL_MIN
    } else {
        current.saturating_mul(2).min(IDLE_POLL_MAX)
    }
}

// The completion flag is separate from the queue: after observing completion,
// retry receive so a final send between the previous receive and flag load drains.
#[cfg(any(test, not(feature = "diag-timing")))]
fn drain_ring<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    #[cfg(feature = "diag-timing")]
    {
        drain_ring_inner(queue, producer_done, None, consume)
    }
    #[cfg(not(feature = "diag-timing"))]
    {
        drain_ring_inner(queue, producer_done, consume)
    }
}

fn drain_ring_inner<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    #[cfg(feature = "diag-timing")] diag: Option<&DiagTiming>,
    mut consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    let mut idle_poll = IDLE_POLL_MIN;
    #[cfg(feature = "diag-timing")]
    let (mut run, mut just_parked) = (0_u64, false);
    loop {
        #[cfg(feature = "diag-timing")]
        if let Some(diag) = diag
            && just_parked
        {
            just_parked = false;
            diag.record_wake(queue.len(), queue.capacity());
        }
        if let Ok(event) = queue.try_recv() {
            idle_poll = idle_poll_after(true, idle_poll);
            #[cfg(feature = "diag-timing")]
            {
                run += 1;
            }
            consume(Some(event))?;
        } else if producer_done.load(Ordering::Acquire) {
            while let Ok(event) = queue.try_recv() {
                #[cfg(feature = "diag-timing")]
                {
                    run += 1;
                }
                consume(Some(event))?;
            }
            #[cfg(feature = "diag-timing")]
            if let Some(diag) = diag {
                diag.record_run(run);
            }
            return Ok(());
        } else {
            consume(None)?;
            #[cfg(feature = "diag-timing")]
            let parked = Instant::now();
            thread::park_timeout(idle_poll);
            #[cfg(feature = "diag-timing")]
            if let Some(diag) = diag {
                diag.record_park(parked.elapsed(), idle_poll);
                diag.record_run(run);
                run = 0;
                just_parked = true;
            }
            idle_poll = idle_poll_after(false, idle_poll);
        }
    }
}

fn report_counts(capture: &CaptureCounters, parser: &ParseCounters) {
    tracing::info!(
        received = capture.received.load(Ordering::Relaxed),
        dequeued = parser.dequeued.load(Ordering::Relaxed),
        encoded = parser.encoded.load(Ordering::Relaxed),
        ignored = parser.ignored.load(Ordering::Relaxed),
        ring_dropped = capture.dropped.load(Ordering::Relaxed),
        intake_dropped = capture.intake_dropped.load(Ordering::Relaxed),
        pool_exhausted = capture.pool_exhausted.load(Ordering::Relaxed),
        receiver_disconnected = capture.receiver_disconnected.load(Ordering::Relaxed),
        etw_lost = capture.etw_lost.load(Ordering::Relaxed),
        etw_buffers_lost = capture.etw_buffers_lost.load(Ordering::Relaxed),
        callback_errors = capture.callback_errors.load(Ordering::Relaxed),
        rejected = parser.rejected.load(Ordering::Relaxed),
        output_failed_events = parser.output_failed_events.load(Ordering::Relaxed),
        output_errors = parser.output_errors.load(Ordering::Relaxed),
        etw_stats_valid = capture.stats_valid.load(Ordering::Relaxed),
        etw_stats_errors = capture.stats_errors.load(Ordering::Relaxed),
        stale_sessions_reclaimed = capture.stale_sessions_reclaimed.load(Ordering::Relaxed),
        etw_flushes = capture.etw_flushes.load(Ordering::Relaxed),
        etw_flush_errors = capture.etw_flush_errors.load(Ordering::Relaxed),
        etw_flush_max_us = capture.etw_flush_max_us.load(Ordering::Relaxed),
        "capture stopped"
    );
    #[cfg(feature = "diag-timing")]
    report_timing(&parser.timing);
}

#[cfg(feature = "diag-timing")]
fn report_timing(timing: &DiagTiming) {
    let parks = timing.parks.load(Ordering::Relaxed);
    let park_ns = timing.park_ns.load(Ordering::Relaxed);
    tracing::info!(
        parks,
        backoff_parks = timing.backoff_parks.load(Ordering::Relaxed),
        park_ns,
        mean_park_ns = park_ns.checked_div(parks).unwrap_or(0),
        wake_len_max = timing.wake_len_max.load(Ordering::Relaxed),
        wake_len_0 = timing.wake_len_0.load(Ordering::Relaxed),
        wake_len_lt_1k = timing.wake_len_lt_1k.load(Ordering::Relaxed),
        wake_len_lt_4k = timing.wake_len_lt_4k.load(Ordering::Relaxed),
        wake_len_lt_8k = timing.wake_len_lt_8k.load(Ordering::Relaxed),
        wake_len_ge_8k_or_full = timing.wake_len_ge_8k_or_full.load(Ordering::Relaxed),
        max_run = timing.max_run.load(Ordering::Relaxed),
        write_ns = timing.write_ns.load(Ordering::Relaxed),
        writes_over_1ms = timing.writes_over_1ms.load(Ordering::Relaxed),
        flush_ns = timing.flush_ns.load(Ordering::Relaxed),
        flushes_over_1ms = timing.flushes_over_1ms.load(Ordering::Relaxed),
        "parser timing"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn etw_flush_ms_validation() {
        let parse = |value: &str| {
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:5000",
                &format!("--etw-flush-ms={value}"),
            ])
        };
        for ok in ["0", "20", "50", "1000"] {
            assert!(parse(ok).is_ok(), "{ok}");
        }
        for bad in ["1", "19", "1001", "-1", "x"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn flush_schedule_disabled_is_never_due_and_parks_cap() {
        let start = Instant::now();
        let mut schedule = FlushSchedule::new(None, start);
        let cap = Duration::from_millis(50);
        assert!(!schedule.poll(start + Duration::from_secs(60)));
        assert_eq!(schedule.park_for(start, cap), cap);
    }

    #[test]
    fn flush_schedule_is_due_exactly_at_interval() {
        let ms = Duration::from_millis;
        let start = Instant::now();
        let mut schedule = FlushSchedule::new(Some(ms(100)), start);
        assert!(!schedule.poll(start));
        assert!(!schedule.poll(start + ms(99)));
        assert!(schedule.poll(start + ms(100)));
        // The deadline advances from the poll time.
        assert!(!schedule.poll(start + ms(199)));
        assert!(schedule.poll(start + ms(200)));
        assert!(schedule.poll(start + ms(5000)));
        assert!(!schedule.poll(start + ms(5001)));
    }

    #[test]
    fn flush_schedule_park_for_is_bounded() {
        let ms = Duration::from_millis;
        let start = Instant::now();
        let schedule = FlushSchedule::new(Some(ms(100)), start);
        let cap = ms(50);
        assert_eq!(schedule.park_for(start + ms(70), cap), ms(30));
        assert_eq!(schedule.park_for(start, cap), cap);
        assert_eq!(schedule.park_for(start + ms(100), cap), ms(1));
        assert_eq!(schedule.park_for(start + ms(500), cap), ms(1));
    }

    #[test]
    fn record_etw_flush_counts_outcomes_separately() {
        let counters = CaptureCounters::default();
        for _ in 0..3 {
            record_etw_flush(&counters, Ok(()));
        }
        assert_eq!(counters.etw_flushes.load(Ordering::Relaxed), 3);
        assert_eq!(counters.etw_flush_errors.load(Ordering::Relaxed), 0);
        for _ in 0..2 {
            record_etw_flush(&counters, Err(5));
        }
        assert_eq!(counters.etw_flushes.load(Ordering::Relaxed), 3);
        assert_eq!(counters.etw_flush_errors.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn argument_parsing_returns_errors_instead_of_exiting() {
        let invalid = parse_args_from(["etw-dnstap", "--receiver", "not-an-address"]);
        assert!(invalid.is_err());
        let missing = parse_args_from(["etw-dnstap"]);
        assert_eq!(
            missing.err().map(|error| error.kind()),
            Some(clap::error::ErrorKind::MissingRequiredArgument)
        );
        let help = parse_args_from(["etw-dnstap", "--help"]);
        assert_eq!(
            help.err().map(|error| error.kind()),
            Some(clap::error::ErrorKind::DisplayHelp)
        );
        let valid = parse_args_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"]);
        assert!(valid.is_ok());
    }

    #[test]
    fn completed_parser_finishes_tcp_and_reports_receiver_failure() -> Result<()> {
        for acknowledge_finish in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let address = listener.local_addr()?;
            let receiver = thread::spawn(move || -> std::io::Result<()> {
                let (mut stream, _) = listener.accept()?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                let mut control = [0; 42];
                stream.read_exact(&mut control)?;
                assert_eq!(&control[8..12], &4_u32.to_be_bytes(), "READY");
                control[8..12].copy_from_slice(&1_u32.to_be_bytes());
                stream.write_all(&control)?;
                stream.read_exact(&mut control)?;
                assert_eq!(&control[8..12], &2_u32.to_be_bytes(), "START");
                let mut stop = [0; 12];
                stream.read_exact(&mut stop)?;
                assert_eq!(stop, [0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3]);
                if acknowledge_finish {
                    stop[8..12].copy_from_slice(&5_u32.to_be_bytes());
                    stream.write_all(&stop)?;
                }
                Ok(())
            });
            let writer = NetworkSink::connect(address)?;
            let (_send, queue) = crossbeam_channel::bounded(1);
            let counters = ParseCounters::default();
            let result = parse_loop(
                &queue,
                &AtomicBool::new(true),
                &counters,
                "test",
                writer,
                #[cfg(feature = "diag-timing")]
                None,
            );
            receiver.join().expect("receiver completes")?;
            assert_eq!(result.is_ok(), acknowledge_finish);
            assert_eq!(
                counters.output_errors.load(Ordering::Relaxed),
                u64::from(!acknowledge_finish)
            );
            assert_eq!(counters.encoded.load(Ordering::Relaxed), 0);
            assert_eq!(counters.output_failed_events.load(Ordering::Relaxed), 0);
            if let Err(error) = result {
                assert!(error.to_string().contains("STOP/FINISH"));
            }
        }
        Ok(())
    }

    #[test]
    fn validates_memory_and_cli_bounds() {
        for rejected in ["0", "-1", "1023", "262145"] {
            assert!(parse_capacity(rejected).is_err(), "{rejected}");
        }
        assert_eq!(parse_capacity("262144"), Ok(262_144));
        assert_eq!(parse_capacity("65536"), Ok(65_536));
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
        assert!(
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:6000",
                "--duration",
                "0"
            ])
            .is_err()
        );
    }

    #[test]
    fn etw_session_flags_default_and_enforce_ranges() {
        let base = ["etw-dnstap", "--receiver", "127.0.0.1:6000"];
        let parse = |extra: &[&str]| {
            let mut arguments = base.to_vec();
            arguments.extend_from_slice(extra);
            CaptureArgs::try_parse_from(arguments)
        };

        let defaults = parse(&[]).expect("default ETW arguments");
        assert_eq!(defaults.etw_buffer_kib, 16);
        assert_eq!(defaults.etw_max_buffers, 12);
        assert_eq!(defaults.etw_flush_seconds, 1);
        assert_eq!(defaults.etw_flush_ms, 0);

        for (flag, values) in [
            ("--etw-buffer-kib", &["0", "3", "132", "256", "1024"][..]),
            ("--etw-max-buffers", &["0", "3", "4097"][..]),
            ("--etw-flush-seconds", &["0", "61", "4294967296"][..]),
        ] {
            for value in values {
                assert!(parse(&[flag, value]).is_err(), "{flag} {value}");
            }
        }
        for (flag, value) in [
            ("--etw-buffer-kib", "4"),
            ("--etw-buffer-kib", "128"),
            ("--etw-max-buffers", "4"),
            ("--etw-max-buffers", "4096"),
            ("--etw-flush-seconds", "1"),
            ("--etw-flush-seconds", "60"),
        ] {
            assert!(parse(&[flag, value]).is_ok(), "{flag} {value}");
        }
    }

    #[test]
    fn session_name_defaults_and_can_be_overridden() {
        let base = ["etw-dnstap", "--receiver", "127.0.0.1:6000"];
        let args = CaptureArgs::try_parse_from(base).expect("default arguments");
        assert_eq!(args.session_name, DEFAULT_SESSION_NAME);
        assert_eq!(args.session_name, "EtwDnstap");
        let args =
            CaptureArgs::try_parse_from([base[0], base[1], base[2], "--session-name", "Mine"])
                .expect("explicit session name");
        assert_eq!(args.session_name, "Mine");
    }

    #[test]
    fn requires_a_numeric_receiver_and_rejects_removed_options() {
        assert_eq!(
            CaptureArgs::try_parse_from(["etw-dnstap"])
                .expect_err("receiver is required")
                .kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        for option in [
            "--output",
            "-o",
            "--max-output-bytes",
            "--metrics",
            "--foreground",
        ] {
            assert_eq!(
                CaptureArgs::try_parse_from([
                    "etw-dnstap",
                    "--receiver",
                    "127.0.0.1:6000",
                    option,
                    "anything"
                ])
                .expect_err("unsupported options must fail")
                .kind(),
                clap::error::ErrorKind::UnknownArgument
            );
        }
        for receiver in ["127.0.0.1:6000", "[::1]:6000"] {
            let args = CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", receiver])
                .expect("numeric unicast receiver is valid");
            assert_eq!(args.receiver.to_string(), receiver);
        }
        for invalid in [
            "collector:6000",
            "127.0.0.1:0",
            "0.0.0.0:6000",
            "224.0.0.1:6000",
            "[::]:6000",
            "[ff02::1]:6000",
        ] {
            assert!(CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", invalid]).is_err());
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
    fn idle_poll_backoff_doubles_and_clamps() {
        assert_eq!(
            idle_poll_after(false, Duration::from_millis(2)),
            Duration::from_millis(4)
        );
        assert_eq!(
            idle_poll_after(false, Duration::from_millis(4)),
            Duration::from_millis(8)
        );
        assert_eq!(
            idle_poll_after(false, Duration::from_millis(8)),
            Duration::from_millis(16)
        );
        assert_eq!(
            idle_poll_after(false, Duration::from_millis(16)),
            IDLE_POLL_MAX
        );
        assert_eq!(idle_poll_after(false, IDLE_POLL_MAX), IDLE_POLL_MAX);
    }

    #[test]
    fn idle_poll_resets_to_minimum_after_an_event() {
        assert_eq!(idle_poll_after(true, IDLE_POLL_MAX), IDLE_POLL_MIN);
        assert_eq!(
            idle_poll_after(true, Duration::from_millis(8)),
            IDLE_POLL_MIN
        );
        assert_eq!(idle_poll_after(true, IDLE_POLL_MIN), IDLE_POLL_MIN);
    }

    #[test]
    fn consumer_failure_is_propagated() {
        let (send, queue) = crossbeam_channel::bounded(1);
        send.try_send(1).expect("space available");
        let error = drain_ring(&queue, &AtomicBool::new(true), |_| bail!("output failed"))
            .expect_err("output error must escape");
        assert_eq!(error.to_string(), "output failed");
    }

    #[cfg(feature = "diag-timing")]
    #[test]
    fn diag_flags_parse_and_default_to_unset() {
        let parse = |extra: &[&str]| {
            let mut arguments = vec!["etw-dnstap", "--receiver", "127.0.0.1:6000"];
            arguments.extend_from_slice(extra);
            CaptureArgs::try_parse_from(arguments)
        };
        let absent = parse(&[]).expect("defaults parse");
        assert_eq!(absent.diag_timer_resolution_ms, None);
        assert_eq!(absent.diag_thread_priority, None);
        for valid in ["1", "15"] {
            let args = parse(&["--diag-timer-resolution-ms", valid]).expect("in range");
            assert_eq!(args.diag_timer_resolution_ms, valid.parse().ok());
        }
        for invalid in ["0", "16", "-1", "x"] {
            assert!(parse(&["--diag-timer-resolution-ms", invalid]).is_err());
        }
        assert_eq!(
            parse(&["--diag-thread-priority", "above-normal"])
                .expect("above-normal")
                .diag_thread_priority,
            Some(etw::ThreadPriority::AboveNormal)
        );
        assert_eq!(
            parse(&["--diag-thread-priority", "highest"])
                .expect("highest")
                .diag_thread_priority,
            Some(etw::ThreadPriority::Highest)
        );
        assert!(parse(&["--diag-thread-priority", "realtime"]).is_err());
    }

    #[cfg(feature = "diag-timing")]
    #[test]
    fn timer_resolution_guard_pairs_begin_and_end() {
        use std::sync::atomic::AtomicU32;
        static BEGUN: AtomicU32 = AtomicU32::new(0);
        static ENDED: AtomicU32 = AtomicU32::new(0);
        let guard = TimerResolution::begin_with(
            7,
            |ms| {
                BEGUN.store(ms, Ordering::SeqCst);
                0
            },
            |ms| {
                ENDED.store(ms, Ordering::SeqCst);
                0
            },
        )
        .expect("begin succeeds");
        assert_eq!(BEGUN.load(Ordering::SeqCst), 7);
        assert_eq!(ENDED.load(Ordering::SeqCst), 0);
        drop(guard);
        assert_eq!(ENDED.load(Ordering::SeqCst), 7);
        // A failed begin yields an error and no guard, so no end call occurs.
        assert!(TimerResolution::begin_with(3, |_| 97, |_| panic!("unpaired end")).is_err());
    }

    #[cfg(feature = "diag-timing")]
    #[test]
    fn diag_timing_counts_parks_runs_and_wake_lengths() {
        let (send, queue) = crossbeam_channel::bounded(16);
        for value in 0..5 {
            send.try_send(value).expect("space available");
        }
        let done = AtomicBool::new(false);
        let timing = DiagTiming::default();
        let mut idle_calls = 0;
        drain_ring_inner(&queue, &done, Some(&timing), |value| {
            if value.is_none() {
                idle_calls += 1;
                if idle_calls == 1 {
                    for value in 0..3 {
                        send.try_send(value).expect("space available");
                    }
                } else {
                    done.store(true, Ordering::Release);
                }
            }
            Ok(())
        })
        .expect("drain succeeds");
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        assert_eq!(load(&timing.parks), 2);
        assert_eq!(load(&timing.backoff_parks), 0);
        assert!(load(&timing.park_ns) > 0);
        assert_eq!(load(&timing.max_run), 5);
        assert_eq!(load(&timing.wake_len_max), 3);
        assert_eq!(load(&timing.wake_len_0), 1);
        assert_eq!(load(&timing.wake_len_lt_1k), 1);
        let histogram = load(&timing.wake_len_0)
            + load(&timing.wake_len_lt_1k)
            + load(&timing.wake_len_lt_4k)
            + load(&timing.wake_len_lt_8k)
            + load(&timing.wake_len_ge_8k_or_full);
        assert_eq!(histogram, load(&timing.parks));
    }

    #[cfg(feature = "diag-timing")]
    #[test]
    fn idle_backoff_counts_and_event_resets_it() {
        let (send, queue) = crossbeam_channel::bounded(4);
        let done = AtomicBool::new(false);
        let timing = DiagTiming::default();
        let mut idle_calls = 0;
        let mut before_event = 0;
        drain_ring_inner(&queue, &done, Some(&timing), |value| {
            if value.is_none() {
                idle_calls += 1;
                match idle_calls {
                    3 => {
                        before_event = timing.backoff_parks.load(Ordering::Relaxed);
                        send.try_send(1).expect("space available");
                    }
                    5 => done.store(true, Ordering::Release),
                    _ => {}
                }
            }
            Ok(())
        })
        .expect("drain succeeds");
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        // Parks 1 and 2 requested 2 ms and 4 ms, so one backoff park precedes the
        // event. Park 3 (8 ms) backs off; the event resets the poll, so park 4
        // (2 ms) is not counted; park 5 (4 ms) is.
        assert_eq!(before_event, 1);
        assert_eq!(load(&timing.parks), 5);
        assert_eq!(load(&timing.backoff_parks), 3);
    }

    #[cfg(feature = "diag-timing")]
    #[test]
    fn diag_timing_buckets_and_slow_calls() {
        let timing = DiagTiming::default();
        for (len, capacity) in [
            (0, Some(4)),
            (3, Some(4)),
            (4, Some(4)),
            (1023, Some(16_384)),
            (1024, Some(16_384)),
            (4095, None),
            (4096, None),
            (8191, None),
            (8192, None),
        ] {
            timing.record_wake(len, capacity);
        }
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        assert_eq!(load(&timing.wake_len_0), 1);
        assert_eq!(load(&timing.wake_len_lt_1k), 2);
        assert_eq!(load(&timing.wake_len_lt_4k), 2);
        assert_eq!(load(&timing.wake_len_lt_8k), 2);
        assert_eq!(load(&timing.wake_len_ge_8k_or_full), 2);
        assert_eq!(load(&timing.wake_len_max), 8192);
        timing.record_write(Duration::from_millis(1));
        timing.record_write(Duration::from_millis(2));
        timing.record_flush(Duration::from_micros(10));
        timing.record_flush(Duration::from_millis(3));
        assert_eq!(load(&timing.writes_over_1ms), 1);
        assert_eq!(load(&timing.write_ns), 3_000_000);
        assert_eq!(load(&timing.flushes_over_1ms), 1);
        assert_eq!(load(&timing.flush_ns), 3_010_000);
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
}
