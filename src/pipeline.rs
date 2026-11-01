//! Supervise ingestion and parsing, with bounded memory and explicit loss counts.

use std::{
    io,
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
    etw::{
        self, Capture, CaptureCounters, KeywordSelection, PoolManager, RawEtwEvent, SessionConfig,
    },
    network::NetworkSink,
    sink::{FrameSink, SinkStats},
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
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u16).range(0..=i64::from(crate::etw::MAX_JUMBO_CAPACITY)))]
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

    /// Seconds between counter snapshots in the log (queue depth, drops, ETW loss,
    /// receiver connection state), 0-86400; 0 disables the snapshots. The final
    /// report at shutdown is always written.
    #[arg(long, default_value = "60", value_parser = clap::value_parser!(u32).range(0..=86_400))]
    stats_interval_seconds: u32,

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
    check_receiver(address)
}

/// Shared by the command line and [`PipelineConfig::validate`].
fn check_receiver(address: SocketAddr) -> Result<SocketAddr, String> {
    if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
        return Err("receiver must have a unicast IP and a nonzero port".into());
    }
    Ok(address)
}

fn parse_etw_flush_ms(value: &str) -> Result<u32, String> {
    let milliseconds: u32 = value
        .parse()
        .map_err(|_| "etw-flush-ms must be an integer")?;
    check_etw_flush_ms(milliseconds)
}

fn check_etw_flush_ms(milliseconds: u32) -> Result<u32, String> {
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

/// Runs one flush, counts it, and records its duration. Returns whether it
/// succeeded.
fn flush_etw(capture: &impl EventProducer, counters: &CaptureCounters) -> bool {
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
    let succeeded = result.is_ok();
    record_etw_flush(counters, result);
    succeeded
}

fn parse_capacity(value: &str) -> Result<usize, String> {
    let capacity: usize = value.parse().map_err(|_| "capacity must be an integer")?;
    check_capacity(capacity)
}

fn check_capacity(capacity: usize) -> Result<usize, String> {
    if !(crate::etw::MIN_CAPACITY..=crate::etw::MAX_CAPACITY).contains(&capacity) {
        return Err(format!(
            "capacity must be between {} and {} events",
            crate::etw::MIN_CAPACITY,
            crate::etw::MAX_CAPACITY
        ));
    }
    Ok(capacity)
}

fn check_jumbo_capacity(capacity: u16) -> Result<u16, String> {
    if capacity > crate::etw::MAX_JUMBO_CAPACITY {
        return Err(format!(
            "jumbo capacity must be at most {} (zero disables it)",
            crate::etw::MAX_JUMBO_CAPACITY
        ));
    }
    Ok(capacity)
}

fn parse_session_name(value: &str) -> Result<String, String> {
    check_session_name(value)?;
    Ok(value.into())
}

fn check_session_name(value: &str) -> Result<(), String> {
    if value.is_empty() || value.encode_utf16().count() > 200 || value.contains('\0') {
        return Err("session name must contain 1..=200 UTF-16 units and no NUL".into());
    }
    Ok(())
}

fn parse_identity(value: &str) -> Result<String, String> {
    check_identity(value)?;
    Ok(value.into())
}

fn check_identity(value: &str) -> Result<(), String> {
    if value.len() > 1024 {
        return Err("identity must be at most 1024 UTF-8 bytes".into());
    }
    Ok(())
}

/// Diagnostic-only settings, folded into one value so the pipeline signatures
/// carry no per-feature parameters. It is empty unless `diag-timing` is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DiagContext {
    /// `timeBeginPeriod` request held for the whole run.
    #[cfg(feature = "diag-timing")]
    pub(crate) timer_resolution_ms: Option<u32>,
    /// Priority applied to the parser and ETW consumer threads.
    #[cfg(feature = "diag-timing")]
    pub(crate) thread_priority: Option<etw::ThreadPriority>,
}

/// Validated runtime settings. The command line is the only front end: it fills
/// this through [`CaptureArgs`] (`TryFrom`). Tests may build one directly.
/// [`PipelineConfig::validate`] repeats the clap bounds as defence in depth, so a
/// config built any other way is held to the same rules.
#[derive(Clone, Debug)]
pub(crate) struct PipelineConfig {
    pub(crate) receiver: SocketAddr,
    pub(crate) capacity: usize,
    pub(crate) jumbo_capacity: u16,
    pub(crate) duration: Option<Duration>,
    pub(crate) session_name: String,
    pub(crate) identity: String,
    pub(crate) session: SessionConfig,
    /// Explicit ETW flush interval in milliseconds; zero disables it.
    pub(crate) etw_flush_ms: u32,
    /// Seconds between counter snapshots; zero disables them.
    pub(crate) stats_interval_seconds: u32,
    pub(crate) diag: DiagContext,
}

impl PipelineConfig {
    /// Re-checks every bound the command line enforces, so a config built
    /// without clap is held to the same rules.
    ///
    /// # Errors
    /// Returns the first violated bound.
    pub(crate) fn validate(&self) -> Result<()> {
        check_receiver(self.receiver).map_err(anyhow::Error::msg)?;
        check_capacity(self.capacity).map_err(anyhow::Error::msg)?;
        check_jumbo_capacity(self.jumbo_capacity).map_err(anyhow::Error::msg)?;
        check_session_name(&self.session_name).map_err(anyhow::Error::msg)?;
        check_identity(&self.identity).map_err(anyhow::Error::msg)?;
        check_etw_flush_ms(self.etw_flush_ms).map_err(anyhow::Error::msg)?;
        SessionConfig::new(
            self.session.buffer_kib,
            self.session.maximum_buffers,
            self.session.flush_seconds,
        )?;
        Ok(())
    }

    /// The periodic ETW flush interval, or `None` when disabled.
    fn etw_flush_interval(&self) -> Option<Duration> {
        (self.etw_flush_ms != 0).then(|| Duration::from_millis(u64::from(self.etw_flush_ms)))
    }

    /// The counter snapshot interval, or `None` when disabled.
    fn stats_interval(&self) -> Option<Duration> {
        (self.stats_interval_seconds != 0)
            .then(|| Duration::from_secs(u64::from(self.stats_interval_seconds)))
    }

    /// True when the ETW buffer and flush settings are the low-rate defaults.
    fn uses_default_etw_settings(&self) -> bool {
        self.etw_flush_ms == 0 && self.session == SessionConfig::default()
    }
}

impl TryFrom<CaptureArgs> for PipelineConfig {
    type Error = anyhow::Error;

    fn try_from(args: CaptureArgs) -> Result<Self> {
        let config = Self {
            receiver: args.receiver,
            capacity: args.capacity,
            jumbo_capacity: args.jumbo_capacity,
            duration: args.duration.map(Duration::from_secs),
            session_name: args.session_name,
            identity: args.identity,
            session: SessionConfig::new(
                args.etw_buffer_kib,
                args.etw_max_buffers,
                args.etw_flush_seconds,
            )?,
            etw_flush_ms: args.etw_flush_ms,
            stats_interval_seconds: args.stats_interval_seconds,
            diag: DiagContext {
                #[cfg(feature = "diag-timing")]
                timer_resolution_ms: args.diag_timer_resolution_ms,
                #[cfg(feature = "diag-timing")]
                thread_priority: args.diag_thread_priority,
            },
        };
        config.validate()?;
        Ok(config)
    }
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
    /// Events rejected because their schema failed to load (first failure and
    /// every cached rejection). Also counted in `rejected`.
    schema_rejected: AtomicU64,
    /// Deepest the event channel was seen: sampled by the parser on the first
    /// event after each idle wake and every 256 events, so a lower bound.
    queue_depth_max: AtomicU64,
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

/// Startup budget: the base timeout plus one second per 16384 channel slots,
/// because allocating and touching the event pool (about 2.3 KiB per slot, up to
/// roughly 640 MB) dominates startup at large `--capacity`.
pub(crate) fn startup_budget(args: &CaptureArgs) -> Duration {
    let slots = u64::try_from(args.capacity).unwrap_or(u64::MAX);
    SHUTDOWN_TIMEOUT + Duration::from_secs(slots.div_ceil(16_384))
}

/// Records the abort in the Event Log (best effort, bounded) and terminates.
fn fail_stop(phase: &str, timeout: Duration) {
    crate::service::report_watchdog_expiry(&format!(
        "EtwDnstap watchdog: the {phase} deadline of {} s expired; aborting the process so \
         the Service Control Manager can apply its recovery actions",
        timeout.as_secs()
    ));
    std::process::abort();
}

/// Holds the shutdown deadline until all native callbacks and network writes end.
pub(crate) struct ShutdownWatchdog {
    disarm: Sender<()>,
    worker: thread::JoinHandle<()>,
}

impl ShutdownWatchdog {
    pub(crate) fn arm() -> Self {
        Self::arm_for("shutdown", SHUTDOWN_TIMEOUT)
    }

    /// Arms a watchdog for `phase` ("startup" or "shutdown") with `timeout`. The
    /// Event Log source is opened now, so the expiry path never has to.
    pub(crate) fn arm_for(phase: &'static str, timeout: Duration) -> Self {
        crate::service::preopen_event_source();
        match Self::spawn(timeout, move || fail_stop(phase, timeout)) {
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

/// Everything an event producer needs to start delivering into the pipeline.
struct ProducerStart<'a> {
    pub(crate) session_name: &'a str,
    /// Bounded channel to the parser thread; the producer owns the only sender,
    /// so dropping it ends the stream.
    pub(crate) sender: crossbeam_channel::Sender<RawEtwEvent>,
    pub(crate) pool: Arc<PoolManager>,
    pub(crate) counters: Arc<CaptureCounters>,
    pub(crate) session: SessionConfig,
    /// Read only by diagnostic producers.
    #[cfg(feature = "diag-timing")]
    pub(crate) diag: DiagContext,
}

/// The source of [`RawEtwEvent`]s the pipeline supervises. [`Capture`] is the
/// production implementation; tests supply fakes.
trait EventProducer {
    /// The keyword masks the producer subscribed with, for the startup log.
    fn keywords(&self) -> KeywordSelection;
    /// True once the producer stopped delivering on its own.
    fn is_finished(&self) -> bool;
    /// Asks the source to deliver buffered events now; the error is a Win32 status.
    ///
    /// # Errors
    /// Returns the native status when the request fails.
    fn flush(&self) -> Result<(), u32>;
    /// Stops delivery and waits until no producer thread can still send.
    ///
    /// # Errors
    /// Returns an error if the source could not be stopped cleanly.
    fn stop(&mut self) -> Result<()>;
    /// Refreshes the source's own loss counters into the shared counters.
    ///
    /// # Errors
    /// Returns the native status when the query fails.
    fn query_statistics(&self) -> Result<(), u32> {
        Ok(())
    }
    /// A reason capture can no longer be trusted (for example the process the
    /// source filters on was replaced), or `None`.
    fn target_change(&self) -> Option<String> {
        None
    }
}

impl EventProducer for Capture {
    fn keywords(&self) -> KeywordSelection {
        Capture::keywords(self)
    }

    fn is_finished(&self) -> bool {
        Capture::is_finished(self)
    }

    fn flush(&self) -> Result<(), u32> {
        Capture::flush(self)
    }

    fn stop(&mut self) -> Result<()> {
        Capture::stop(self)
    }

    fn query_statistics(&self) -> Result<(), u32> {
        Capture::query_statistics(self)
    }

    fn target_change(&self) -> Option<String> {
        Capture::target_change(self)
    }
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

    let config = PipelineConfig::try_from(args)?;
    run_pipeline(
        &config,
        NetworkSink::start,
        |start| {
            Capture::start(
                start.session_name,
                start.sender,
                start.pool,
                start.counters,
                start.session,
                #[cfg(feature = "diag-timing")]
                start.diag.thread_priority,
            )
        },
        stop,
        on_running,
        on_stopping,
    )
}

/// Runs the pipeline against any sink and producer. `connect` opens the sink
/// before capture starts (it may return a sink that reconnects on its own and
/// reports through the shared [`SinkStats`]), `start` begins event delivery, and
/// the call returns after both workers have stopped and the counters have been
/// reported.
fn run_pipeline<S, P>(
    config: &PipelineConfig,
    connect: impl FnOnce(SocketAddr, Arc<SinkStats>) -> io::Result<S>,
    start: impl FnOnce(ProducerStart<'_>) -> Result<P>,
    stop: &AtomicBool,
    on_running: impl FnOnce() -> Result<()>,
    on_stopping: impl FnOnce() -> Result<()>,
) -> Result<()>
where
    S: FrameSink + Send + 'static,
    P: EventProducer,
{
    // Declared before any worker thread so it drops after both are joined.
    let _diag_levers = begin_diag_levers(config.diag)?;
    let sink_stats = Arc::new(SinkStats::default());
    let writer = connect(config.receiver, Arc::clone(&sink_stats))
        .context("connect/negotiation with dnstap receiver")?;
    let (sender, queue) = crossbeam_channel::bounded(config.capacity);
    let pool = PoolManager::new(config.capacity, usize::from(config.jumbo_capacity))?;
    let producer_done = Arc::new(AtomicBool::new(false));
    let capture_counts = Arc::new(CaptureCounters::default());
    let parse_counts = Arc::new(ParseCounters::default());

    let parser = spawn_parser(
        queue,
        Arc::clone(&producer_done),
        Arc::clone(&parse_counts),
        config.identity.clone(),
        writer,
        config.diag,
    )?;

    let startup_watchdog = ShutdownWatchdog::arm_for("startup", SHUTDOWN_TIMEOUT);
    let producer = start(ProducerStart {
        session_name: &config.session_name,
        sender,
        pool,
        counters: Arc::clone(&capture_counts),
        session: config.session,
        #[cfg(feature = "diag-timing")]
        diag: config.diag,
    });
    let mut producer = match producer {
        Ok(producer) => producer,
        Err(error) => {
            startup_watchdog.disarm()?;
            let watchdog = ShutdownWatchdog::arm();
            producer_done.store(true, Ordering::Release);
            parser.thread().unpark();
            let parser_result = join_parser(parser);
            report_counts(&capture_counts, &parse_counts, &sink_stats);
            watchdog.disarm()?;
            return combine_results(Err(error), parser_result);
        }
    };
    let running_result = on_running();
    log_capture_started(config, producer.keywords());
    log_diag_settings(config.diag);
    startup_watchdog.disarm()?;
    let started = Instant::now();
    let mut monitor = Monitor::new(
        config,
        &capture_counts,
        &parse_counts,
        &sink_stats,
        Instant::now(),
    );
    let unexpected_exit = supervise(
        stop,
        config.duration.map(|limit| started + limit),
        running_result.is_err(),
        &parser,
        &producer,
        &mut monitor,
    );

    let watchdog = ShutdownWatchdog::arm();
    let stopping_result = on_stopping();
    // stop joins the producer's threads, so none can enqueue after this publication.
    let capture_result = producer.stop();
    // A failed stop may retain the handle for Drop to retry. Keep that retry
    // inside the deadline as well, while native resources are still retained.
    drop(producer);
    producer_done.store(true, Ordering::Release);
    parser.thread().unpark();
    let parser_result = join_parser(parser);
    report_counts(&capture_counts, &parse_counts, &sink_stats);
    watchdog.disarm()?;
    combine_results(capture_result, parser_result)?;
    running_result?;
    stopping_result?;
    if let Some(reason) = unexpected_exit {
        bail!("{reason}");
    }
    Ok(())
}

/// Received-rate threshold above which the low-rate ETW defaults are inadequate.
const LOW_RATE_WARN_EVENTS_PER_SEC: f64 = 1500.0;
/// How often the supervisor re-reads the DNS Server service process ID.
const TARGET_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Rate window when counter snapshots are disabled.
const DEFAULT_RATE_WINDOW: Duration = Duration::from_secs(60);

/// Measures the received-event rate over fixed windows.
struct RateMonitor {
    window: Duration,
    window_start: Instant,
    received_at_start: u64,
}

impl RateMonitor {
    fn new(window: Duration, now: Instant, received: u64) -> Self {
        Self {
            window,
            window_start: now,
            received_at_start: received,
        }
    }

    /// At the end of each window returns the average events per second over it
    /// and starts the next window.
    fn poll(&mut self, now: Instant, received: u64) -> Option<f64> {
        let elapsed = now.saturating_duration_since(self.window_start);
        if elapsed < self.window {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "an approximate rate; counts stay far below 2^52"
        )]
        let delta = received.saturating_sub(self.received_at_start) as f64;
        self.window_start = now;
        self.received_at_start = received;
        Some(delta / elapsed.as_secs_f64())
    }
}

/// True when the default ETW settings are in use and the rate exceeds what they
/// were sized for.
fn low_rate_defaults_exceeded(defaults_in_use: bool, events_per_second: f64) -> bool {
    defaults_in_use && events_per_second > LOW_RATE_WARN_EVENTS_PER_SEC
}

/// Everything the supervisor schedules and reads while capture runs.
struct Monitor<'a> {
    capture: &'a CaptureCounters,
    parse: &'a ParseCounters,
    sink: &'a SinkStats,
    flush: FlushSchedule,
    stats: FlushSchedule,
    target: FlushSchedule,
    rate: RateMonitor,
    defaults_in_use: bool,
}

impl<'a> Monitor<'a> {
    fn new(
        config: &PipelineConfig,
        capture: &'a CaptureCounters,
        parse: &'a ParseCounters,
        sink: &'a SinkStats,
        now: Instant,
    ) -> Self {
        Self {
            capture,
            parse,
            sink,
            flush: FlushSchedule::new(config.etw_flush_interval(), now),
            stats: FlushSchedule::new(config.stats_interval(), now),
            target: FlushSchedule::new(Some(TARGET_CHECK_INTERVAL), now),
            rate: RateMonitor::new(
                config.stats_interval().unwrap_or(DEFAULT_RATE_WINDOW),
                now,
                capture.received.load(Ordering::Relaxed),
            ),
            defaults_in_use: config.uses_default_etw_settings(),
        }
    }
}

/// Supervises the workers until shutdown. Returns the reason when a worker
/// exited, or capture became untrustworthy, before shutdown was requested.
fn supervise(
    stop: &AtomicBool,
    deadline: Option<Instant>,
    startup_failed: bool,
    parser: &thread::JoinHandle<Result<()>>,
    capture: &impl EventProducer,
    monitor: &mut Monitor<'_>,
) -> Option<String> {
    loop {
        if startup_failed {
            break None;
        }
        if stop.load(Ordering::Relaxed) || deadline.is_some_and(|limit| Instant::now() >= limit) {
            break None;
        }
        if parser.is_finished() || capture.is_finished() {
            break Some("a capture worker exited before shutdown was requested".to_owned());
        }
        let now = Instant::now();
        // Flushing here, not in the ETW callback, keeps the callback wake-free
        // (docs/history/ring-wakeup.md). The park ends at the next flush deadline, so the
        // cadence is the setting plus at most one timer tick (about 15.6 ms).
        if monitor.flush.poll(now) && flush_etw(capture, monitor.capture) {
            // The batch just delivered is queued; wake the parser instead of
            // letting it finish a backed-off idle park first.
            parser.thread().unpark();
        }
        if monitor.target.poll(now)
            && let Some(reason) = capture.target_change()
        {
            // The PID filter and callback check now match no process. Exiting
            // with a failure lets SCM recovery restart the service, which
            // resolves the new PID at startup.
            tracing::error!(
                "{reason}; stopping so the service can be restarted and re-resolve the DNS \
                 Server process"
            );
            break Some(reason);
        }
        if let Some(rate) = monitor
            .rate
            .poll(now, monitor.capture.received.load(Ordering::Relaxed))
            && low_rate_defaults_exceeded(monitor.defaults_in_use, rate)
        {
            tracing::warn!(
                events_per_second = rate.round(),
                "received rate is above what the default low-rate ETW settings are sized \
                 for (about 1,500 events/s) and events may be dropped; restart with \
                 --etw-flush-ms 50 --etw-max-buffers 256 --etw-buffer-kib 64"
            );
        }
        if monitor.stats.poll(now) {
            if let Err(status) = capture.query_statistics() {
                tracing::debug!(code = status, "periodic ETW session query failed");
            }
            report_snapshot(monitor.capture, monitor.parse, monitor.sink);
        }
        thread::park_timeout(
            monitor
                .flush
                .park_for(Instant::now(), Duration::from_millis(50)),
        );
    }
}

fn log_capture_started(config: &PipelineConfig, keywords: KeywordSelection) {
    tracing::info!(
        session = config.session_name.as_str(),
        session_guid = crate::etw::session_guid_text(&config.session_name),
        receiver = %config.receiver,
        capacity = config.capacity,
        etw_buffer_kib = config.session.buffer_kib,
        etw_max_buffers = config.session.maximum_buffers,
        etw_flush_seconds = config.session.flush_seconds,
        etw_flush_ms = config.etw_flush_ms,
        match_any_keyword = format_args!("{:#x}", keywords.match_any),
        match_all_keyword = format_args!("{:#x}", keywords.match_all),
        keyword_fallback = keywords.fallback,
        "capture started"
    );
}

/// Process-wide diagnostic levers; dropping it withdraws them, so it must
/// outlive the workers. Empty unless `diag-timing` is enabled.
struct DiagLevers {
    #[cfg(feature = "diag-timing")]
    _timer: Option<TimerResolution>,
}

#[cfg_attr(
    not(feature = "diag-timing"),
    expect(
        clippy::unnecessary_wraps,
        reason = "Starting the diagnostic timer can fail; the normal build has no timer."
    )
)]
fn begin_diag_levers(diag: DiagContext) -> Result<DiagLevers> {
    #[cfg(not(feature = "diag-timing"))]
    let _ = diag;
    Ok(DiagLevers {
        #[cfg(feature = "diag-timing")]
        _timer: diag
            .timer_resolution_ms
            .map(TimerResolution::begin)
            .transpose()?,
    })
}

/// Records which diagnostic levers a run applied.
#[cfg(feature = "diag-timing")]
fn log_diag_settings(diag: DiagContext) {
    tracing::info!(
        diag_timer_resolution_ms = diag
            .timer_resolution_ms
            .map_or_else(|| "default".to_owned(), |ms| ms.to_string()),
        diag_thread_priority = diag
            .thread_priority
            .map_or("normal", etw::ThreadPriority::as_str),
        "diagnostic settings applied"
    );
}

#[cfg(not(feature = "diag-timing"))]
fn log_diag_settings(_diag: DiagContext) {}

/// Applies the configured diagnostic priority to the calling thread.
#[cfg(feature = "diag-timing")]
fn apply_thread_priority(diag: DiagContext) {
    if let Some(priority) = diag.thread_priority {
        etw::set_current_thread_priority(priority);
    }
}

#[cfg(not(feature = "diag-timing"))]
fn apply_thread_priority(_diag: DiagContext) {}

fn spawn_parser<S>(
    queue: Receiver<RawEtwEvent>,
    producer_done: Arc<AtomicBool>,
    counters: Arc<ParseCounters>,
    identity: String,
    writer: S,
    diag: DiagContext,
) -> Result<thread::JoinHandle<Result<()>>>
where
    S: FrameSink + Send + 'static,
{
    thread::Builder::new()
        .name("dnstap-parser".into())
        .spawn(move || parse_loop(&queue, &producer_done, &counters, &identity, writer, diag))
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

fn parse_loop<S: FrameSink>(
    queue: &Receiver<RawEtwEvent>,
    producer_done: &AtomicBool,
    counters: &ParseCounters,
    identity: &str,
    mut writer: S,
    diag: DiagContext,
) -> Result<()> {
    apply_thread_priority(diag);
    let mut parser = etw::Parser::new();
    let mut encoder = Encoder::new(identity.as_bytes())?;
    let mut last_flush = Instant::now();
    // Bytes may sit in the sink's buffer since the last flush.
    let mut dirty = false;
    // True after an idle poll, so the next event samples the queue depth.
    let mut sample_depth = true;
    let mut events_since_clock = 0u32;
    let consume = |event: Option<RawEtwEvent>| {
        let idle = event.is_none();
        if idle {
            sample_depth = true;
        }
        if let Some(event) = event {
            let dequeued = counters.dequeued.fetch_add(1, Ordering::Relaxed);
            if sample_depth || dequeued.is_multiple_of(256) {
                sample_depth = false;
                let depth = u64::try_from(queue.len()).unwrap_or(u64::MAX);
                counters
                    .queue_depth_max
                    .fetch_max(depth.saturating_add(1), Ordering::Relaxed);
            }
            // The decoded view borrows the pooled record. Encoding copies the packet
            // into encoder storage, so the record is released before network I/O and
            // the returned payload borrows only reusable encoder storage.
            let result = parse_and_encode(&mut parser, event, &mut encoder);
            if result.is_err() {
                counters
                    .schema_rejected
                    .store(parser.schema_rejected_events(), Ordering::Relaxed);
            }
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
                    dirty = true;
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
        // Read the clock only when it can matter: on every idle poll that has
        // unflushed bytes (so a trickle is not held for a second), and once per
        // CLOCK_CHECK_EVENTS events while busy.
        let flush_due = if idle {
            dirty && last_flush.elapsed() >= IDLE_FLUSH_AFTER
        } else {
            events_since_clock += 1;
            if events_since_clock >= CLOCK_CHECK_EVENTS {
                events_since_clock = 0;
                dirty && last_flush.elapsed() >= BUSY_FLUSH_AFTER
            } else {
                false
            }
        };
        if flush_due {
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
            dirty = false;
        }
        Ok(())
    };
    #[cfg(not(feature = "diag-timing"))]
    drain_queue(queue, producer_done, consume)?;
    #[cfg(feature = "diag-timing")]
    drain_queue_inner(queue, producer_done, Some(&counters.timing), consume)?;
    writer
        .finish()
        .context("finish dnstap STOP/FINISH exchange")
        .inspect_err(|_error| {
            counters.output_errors.fetch_add(1, Ordering::Relaxed);
        })
}

/// Decodes one record and encodes it to dnstap bytes, the production step of the
/// parser thread. The record (and its pool lease) is released before this
/// returns, so the bytes borrow only the encoder.
pub(crate) fn parse_and_encode<'e>(
    parser: &mut etw::Parser,
    event: RawEtwEvent,
    encoder: &'e mut Encoder,
) -> Result<Option<&'e [u8]>> {
    let result = match parser.parse(&event) {
        Ok(Some(decoded)) => encoder.encode(&decoded).map(Some),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    };
    drop(event);
    result
}

// The ETW callback must stay wake-free; see docs/history/ring-wakeup.md. The parser
// therefore polls with a bounded backoff: each empty poll doubles the park from
// 2 ms to at most 20 ms, and any received event resets it to 2 ms. Windows'
// default ~15.6 ms timer granularity stretches these requests, so a 20 ms park
// can last about 31 ms. Nominal idle wake-ups fall from about 500/s to about
// 50/s (about 37/s measured). Shutdown is unaffected because completion is
// followed by unpark, which ends any park.
const IDLE_POLL_MIN: Duration = Duration::from_millis(2);
/// With bytes pending, an idle parser flushes if the last flush is older than this.
const IDLE_FLUSH_AFTER: Duration = Duration::from_millis(100);
/// While busy the sink's buffer fills and flushes itself; this bounds the wait
/// for a partly full buffer.
const BUSY_FLUSH_AFTER: Duration = Duration::from_secs(1);
/// While busy, the flush clock is read once per this many events.
const CLOCK_CHECK_EVENTS: u32 = 64;
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
fn drain_queue<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    #[cfg(feature = "diag-timing")]
    {
        drain_queue_inner(queue, producer_done, None, consume)
    }
    #[cfg(not(feature = "diag-timing"))]
    {
        drain_queue_inner(queue, producer_done, consume)
    }
}

fn drain_queue_inner<T>(
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

/// Emits the two counter events. A macro because `tracing` needs a literal
/// message, and the final report and the periodic snapshot share every field.
///
/// Counter relationships: `ring_dropped` (channel full) is a subset of
/// `intake_dropped`, which also includes pool exhaustion and a closed channel;
/// never add them. `pool_exhausted_regular + pool_exhausted_jumbo` is likewise
/// part of `intake_dropped`. `disconnected_dropped` counts frames lost after the
/// parser, in the sink, and is not part of `intake_dropped`.
macro_rules! emit_counts {
    ($capture_message:literal, $output_message:literal, $capture:expr, $parser:expr, $sink:expr) => {{
        let (capture, parser, sink): (&CaptureCounters, &ParseCounters, &SinkStats) =
            ($capture, $parser, $sink);
        let pool_exhausted = capture.pool_exhausted.load(Ordering::Relaxed);
        let pool_exhausted_jumbo = capture.pool_exhausted_jumbo.load(Ordering::Relaxed);
        tracing::info!(
            received = capture.received.load(Ordering::Relaxed),
            dequeued = parser.dequeued.load(Ordering::Relaxed),
            encoded = parser.encoded.load(Ordering::Relaxed),
            ignored = parser.ignored.load(Ordering::Relaxed),
            ring_dropped = capture.dropped.load(Ordering::Relaxed),
            intake_dropped = capture.intake_dropped.load(Ordering::Relaxed),
            pool_exhausted_regular = pool_exhausted.saturating_sub(pool_exhausted_jumbo),
            pool_exhausted_jumbo,
            foreign_pid_rejected = capture.foreign_pid_rejected.load(Ordering::Relaxed),
            receiver_disconnected = capture.receiver_disconnected.load(Ordering::Relaxed),
            etw_lost = capture.etw_lost.load(Ordering::Relaxed),
            etw_buffers_lost = capture.etw_buffers_lost.load(Ordering::Relaxed),
            callback_errors = capture.callback_errors.load(Ordering::Relaxed),
            rejected = parser.rejected.load(Ordering::Relaxed),
            schema_rejected = parser.schema_rejected.load(Ordering::Relaxed),
            etw_stats_valid = capture.stats_valid.load(Ordering::Relaxed),
            etw_stats_errors = capture.stats_errors.load(Ordering::Relaxed),
            stale_sessions_reclaimed = capture.stale_sessions_reclaimed.load(Ordering::Relaxed),
            etw_flushes = capture.etw_flushes.load(Ordering::Relaxed),
            etw_flush_errors = capture.etw_flush_errors.load(Ordering::Relaxed),
            etw_flush_max_us = capture.etw_flush_max_us.load(Ordering::Relaxed),
            $capture_message
        );
        tracing::info!(
            output_failed_events = parser.output_failed_events.load(Ordering::Relaxed),
            output_errors = parser.output_errors.load(Ordering::Relaxed),
            receiver_connected = sink.connected.load(Ordering::Relaxed),
            receiver_disconnects = sink.disconnects.load(Ordering::Relaxed),
            receiver_reconnects = sink.reconnects.load(Ordering::Relaxed),
            receiver_reconnect_failures = sink.reconnect_failures.load(Ordering::Relaxed),
            disconnected_dropped = sink.disconnected_dropped.load(Ordering::Relaxed),
            slow_writes = sink.slow_writes.load(Ordering::Relaxed),
            write_max_us = sink.write_max_us.load(Ordering::Relaxed),
            queue_depth_max = parser.queue_depth_max.load(Ordering::Relaxed),
            $output_message
        );
    }};
}

/// The periodic counter snapshot.
fn report_snapshot(capture: &CaptureCounters, parser: &ParseCounters, sink: &SinkStats) {
    emit_counts!("capture stats", "output stats", capture, parser, sink);
}

fn report_counts(capture: &CaptureCounters, parser: &ParseCounters, sink: &SinkStats) {
    emit_counts!("capture stopped", "output stopped", capture, parser, sink);
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
#[path = "pipeline/cli_tests.rs"]
mod cli_tests;

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
            let writer = crate::network::Connection::connect(address)?;
            let (_send, queue) = crossbeam_channel::bounded(1);
            let counters = ParseCounters::default();
            let result = parse_loop(
                &queue,
                &AtomicBool::new(true),
                &counters,
                "test",
                writer,
                DiagContext::default(),
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

    /// A producer that delivers nothing and records how the pipeline drove it.
    struct FakeProducer {
        stopped: Arc<AtomicBool>,
    }

    impl EventProducer for FakeProducer {
        fn keywords(&self) -> KeywordSelection {
            KeywordSelection {
                match_any: 0,
                match_all: 0,
                fallback: true,
            }
        }

        fn is_finished(&self) -> bool {
            false
        }

        fn flush(&self) -> Result<(), u32> {
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn pipeline_runs_a_fake_producer_against_a_loopback_receiver() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let receiver = thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            let mut control = [0; 42];
            stream.read_exact(&mut control)?;
            control[8..12].copy_from_slice(&1_u32.to_be_bytes());
            stream.write_all(&control)?;
            stream.read_exact(&mut control)?;
            let mut stop = [0; 12];
            stream.read_exact(&mut stop)?;
            stop[8..12].copy_from_slice(&5_u32.to_be_bytes());
            stream.write_all(&stop)
        });
        let config = PipelineConfig::try_from(parse_args_from([
            "etw-dnstap",
            "--receiver",
            &address.to_string(),
        ])?)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let producer_stopped = Arc::clone(&stopped);
        // Shutdown is already requested, so the supervisor exits at once.
        let stop = AtomicBool::new(true);
        run_pipeline(
            &config,
            |address, _stats| crate::network::Connection::connect(address),
            |_start| {
                Ok(FakeProducer {
                    stopped: producer_stopped,
                })
            },
            &stop,
            || Ok(()),
            || Ok(()),
        )?;
        receiver.join().expect("receiver completes")?;
        assert!(stopped.load(Ordering::SeqCst), "producer was stopped");
        Ok(())
    }

    #[test]
    fn pipeline_config_validation_matches_the_command_line() -> Result<()> {
        let valid = PipelineConfig::try_from(parse_args_from([
            "etw-dnstap",
            "--receiver",
            "127.0.0.1:6000",
        ])?)?;
        assert_eq!(valid.capacity, 8192);
        assert_eq!(valid.etw_flush_interval(), None);
        let mut bad = valid.clone();
        bad.capacity = 1;
        assert!(bad.validate().is_err());
        let mut bad = valid.clone();
        bad.etw_flush_ms = 5;
        assert!(bad.validate().is_err());
        let mut bad = valid;
        bad.receiver = "0.0.0.0:6000".parse()?;
        assert!(bad.validate().is_err());
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
        drain_queue(&queue, &done, |value| {
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
        drain_queue(&queue, &done, |value| {
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
                let result = drain_queue(&queue, &done, |value| {
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
    fn startup_failure_completion_finishes_an_empty_queue() -> Result<()> {
        let (_send, queue) = crossbeam_channel::bounded::<u8>(1);
        let done = AtomicBool::new(true);
        drain_queue(&queue, &done, |_| {
            bail!("empty completed queue must not consume")
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
        let error = drain_queue(&queue, &AtomicBool::new(true), |_| bail!("output failed"))
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
        drain_queue_inner(&queue, &done, Some(&timing), |value| {
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
        drain_queue_inner(&queue, &done, Some(&timing), |value| {
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

    fn config_from(extra: &[&str]) -> PipelineConfig {
        let mut arguments = vec!["etw-dnstap", "--receiver", "127.0.0.1:6000"];
        arguments.extend_from_slice(extra);
        PipelineConfig::try_from(parse_args_from(arguments).expect("valid arguments"))
            .expect("valid configuration")
    }

    #[test]
    fn stats_interval_flag_defaults_to_60_and_zero_disables() {
        assert_eq!(
            config_from(&[]).stats_interval(),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            config_from(&["--stats-interval-seconds", "5"]).stats_interval(),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            config_from(&["--stats-interval-seconds=0"]).stats_interval(),
            None
        );
        for bad in ["-1", "86401", "x", ""] {
            assert!(
                parse_args_from([
                    "etw-dnstap",
                    "--receiver",
                    "127.0.0.1:6000",
                    &format!("--stats-interval-seconds={bad}"),
                ])
                .is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn default_etw_settings_are_detected() {
        assert!(config_from(&[]).uses_default_etw_settings());
        assert!(!config_from(&["--etw-flush-ms", "50"]).uses_default_etw_settings());
        assert!(!config_from(&["--etw-max-buffers", "256"]).uses_default_etw_settings());
        assert!(!config_from(&["--etw-buffer-kib", "64"]).uses_default_etw_settings());
        assert!(!config_from(&["--etw-flush-seconds", "2"]).uses_default_etw_settings());
    }

    #[test]
    fn low_rate_warning_needs_defaults_and_a_high_rate() {
        assert!(low_rate_defaults_exceeded(true, 1501.0));
        assert!(!low_rate_defaults_exceeded(true, 1500.0));
        assert!(!low_rate_defaults_exceeded(true, 10.0));
        assert!(!low_rate_defaults_exceeded(false, 50_000.0));
    }

    #[test]
    fn rate_monitor_reports_once_per_window() {
        let start = Instant::now();
        let mut rate = RateMonitor::new(Duration::from_secs(10), start, 100);
        assert_eq!(rate.poll(start + Duration::from_secs(9), 99_999), None);
        let measured = rate
            .poll(start + Duration::from_secs(10), 20_100)
            .expect("window complete");
        assert!((measured - 2000.0).abs() < 0.001, "{measured}");
        // The next window starts from the previous reading.
        assert_eq!(rate.poll(start + Duration::from_secs(15), 20_200), None);
        let slow = rate
            .poll(start + Duration::from_secs(20), 20_200)
            .expect("second window");
        assert!((slow - 10.0).abs() < 0.001, "{slow}");
    }

    #[test]
    fn startup_budget_scales_with_capacity() {
        let budget = |capacity: &str| {
            startup_budget(
                &parse_args_from([
                    "etw-dnstap",
                    "--receiver",
                    "127.0.0.1:6000",
                    "--capacity",
                    capacity,
                ])
                .expect("valid arguments"),
            )
        };
        assert_eq!(budget("8192"), SHUTDOWN_TIMEOUT + Duration::from_secs(1));
        assert_eq!(budget("262144"), SHUTDOWN_TIMEOUT + Duration::from_secs(16));
        assert!(budget("262144") > budget("1024"));
    }

    /// A producer scripted for supervisor tests.
    struct ScriptedProducer {
        flushes: Arc<std::sync::atomic::AtomicU32>,
        moved: bool,
    }

    impl EventProducer for ScriptedProducer {
        fn keywords(&self) -> KeywordSelection {
            KeywordSelection {
                match_any: 0,
                match_all: 0,
                fallback: true,
            }
        }

        fn is_finished(&self) -> bool {
            false
        }

        fn flush(&self) -> Result<(), u32> {
            self.flushes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            Ok(())
        }

        fn target_change(&self) -> Option<String> {
            self.moved
                .then(|| "the DNS Server service restarted (process ID 1 -> 2)".to_owned())
        }
    }

    #[test]
    fn supervisor_ends_capture_when_the_dns_process_changes() {
        let config = config_from(&[]);
        let (capture, parse, sink) = (
            CaptureCounters::default(),
            ParseCounters::default(),
            SinkStats::default(),
        );
        let mut monitor = Monitor::new(&config, &capture, &parse, &sink, Instant::now());
        monitor.target = FlushSchedule::new(Some(Duration::from_millis(10)), Instant::now());
        let done = Arc::new(AtomicBool::new(false));
        let parser_done = Arc::clone(&done);
        let parser = thread::spawn(move || {
            while !parser_done.load(Ordering::Relaxed) {
                thread::park_timeout(Duration::from_millis(10));
            }
            Ok(())
        });
        let producer = ScriptedProducer {
            flushes: Arc::default(),
            moved: true,
        };
        let reason = supervise(
            &AtomicBool::new(false),
            None,
            false,
            &parser,
            &producer,
            &mut monitor,
        )
        .expect("a changed PID ends supervision");
        assert!(reason.contains("restarted"), "{reason}");
        done.store(true, Ordering::Relaxed);
        parser.join().expect("parser stub exits").expect("ok");
    }

    #[test]
    fn supervisor_wakes_the_parser_after_each_successful_flush() {
        let config = config_from(&["--etw-flush-ms", "20"]);
        let (capture, parse, sink) = (
            CaptureCounters::default(),
            ParseCounters::default(),
            SinkStats::default(),
        );
        let mut monitor = Monitor::new(&config, &capture, &parse, &sink, Instant::now());
        let done = Arc::new(AtomicBool::new(false));
        let wakes = Arc::new(AtomicU64::new(0));
        let (parser_done, parser_wakes) = (Arc::clone(&done), Arc::clone(&wakes));
        let parser = thread::spawn(move || {
            // A long park: only an explicit unpark ends it early.
            while !parser_done.load(Ordering::Relaxed) {
                thread::park_timeout(Duration::from_secs(30));
                parser_wakes.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        });
        let flushes = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let producer = ScriptedProducer {
            flushes: Arc::clone(&flushes),
            moved: false,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(400));
                stop.store(true, Ordering::Relaxed);
            })
        };
        assert!(supervise(&stop, None, false, &parser, &producer, &mut monitor).is_none());
        stopper.join().expect("stopper exits");
        done.store(true, Ordering::Relaxed);
        parser.thread().unpark();
        parser.join().expect("parser stub exits").expect("ok");
        let flush_count = u64::from(flushes.load(std::sync::atomic::Ordering::Relaxed));
        assert!(flush_count >= 3, "flushed {flush_count} times");
        // One wake per flush, plus the final one that observes `done`.
        assert!(wakes.load(Ordering::Relaxed) >= flush_count.min(3));
    }

    #[test]
    fn pipeline_starts_and_stops_promptly_while_the_receiver_is_down() -> Result<()> {
        // A port that was just released refuses connections.
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            listener.local_addr()?
        };
        let config = PipelineConfig::try_from(parse_args_from([
            "etw-dnstap",
            "--receiver",
            &address.to_string(),
        ])?)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let producer_stopped = Arc::clone(&stopped);
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        run_pipeline(
            &config,
            NetworkSink::start,
            |_start| {
                Ok(FakeProducer {
                    stopped: producer_stopped,
                })
            },
            &stop,
            || Ok(()),
            || Ok(()),
        )?;
        assert!(stopped.load(Ordering::SeqCst), "producer was stopped");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "prompt shutdown"
        );
        Ok(())
    }
}
