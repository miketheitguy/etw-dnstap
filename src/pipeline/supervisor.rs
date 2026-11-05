//! Runs the pipeline: starts the sink, parser, and capture, supervises them
//! until shutdown, and stops and reports in order.

use std::{
    io,
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use super::{
    alerts::{Alerts, Limits, Readings, SpoolLimits},
    config::{CaptureArgs, DEFAULT_CAPACITY, DiagContext, PipelineConfig, SpoolSettings},
    parser::{ParseCounters, join_parser, spawn_parser},
    watchdog::{CAPTURE_START_TIMEOUT, ShutdownWatchdog},
};
use crate::{
    etw::{
        Capture, CaptureCounters, EventProducer, KeywordSelection, PoolManager, RawEtwEvent,
        SessionConfig,
    },
    network::{NetworkSink, ReceiverConfig},
    perfcounters::{CounterSink, CounterSources, PerfPublisher, snapshot, stage_cpu_totals},
    sender::QueuedSink,
    sink::{FrameSink, SinkStats},
    spool::{
        Spool, SpoolConfig,
        fsinfo::{self, SpoolDirError},
        segment_bytes_for,
    },
    threadcpu::{CpuSampler, ETW_CONSUMER, LINK, PARSER, SENDER, STAGE_COUNT, StageCpu},
};

/// Schedules a periodic task on the supervisor thread.
struct Schedule {
    interval: Option<Duration>,
    next: Instant,
}

impl Schedule {
    /// `None` disables the task.
    fn new(interval: Option<Duration>, now: Instant) -> Self {
        Self {
            interval,
            next: now + interval.unwrap_or_default(),
        }
    }

    /// True when the task is due; advances `next` to `now + interval`.
    fn poll(&mut self, now: Instant) -> bool {
        match self.interval {
            Some(interval) if now >= self.next => {
                self.next = now + interval;
                true
            }
            _ => false,
        }
    }
}

/// Everything an event producer needs to start delivering into the pipeline.
struct ProducerStart<'a> {
    pub(crate) session_name: &'a str,
    /// Bounded channel to the parser thread; the producer owns the only sender,
    /// so dropping it ends the stream.
    pub(crate) sender: crossbeam_channel::Sender<RawEtwEvent>,
    pub(crate) pool: &'static PoolManager,
    pub(crate) counters: Arc<CaptureCounters>,
    pub(crate) session: SessionConfig,
    /// The parser thread. The ETW producer wakes it once per delivered buffer
    /// that enqueued events; fakes may ignore it.
    pub(crate) parser: thread::Thread,
    /// Read only by diagnostic producers.
    #[cfg(feature = "diag-timing")]
    pub(crate) diag: DiagContext,
}

/// Callbacks the pipeline fires as it changes state. The service layer maps
/// them to SCM status reports; tests pass no-op closures.
struct Lifecycle<Progress, Running, Stopping> {
    /// Called from the pool allocation once per
    /// [`PoolManager::PROGRESS_INTERVAL`] allocated slots.
    progress: Progress,
    /// Called once capture is delivering events.
    running: Running,
    /// Called when shutdown begins.
    stopping: Stopping,
}

/// Runs capture until `stop` is set or the pipeline ends. `on_progress` is
/// called from the pool allocation once per [`PoolManager::PROGRESS_INTERVAL`]
/// slots, `on_running` once capture is delivering events, and `on_stopping`
/// when shutdown begins. The service layer maps them to SCM status reports.
pub(crate) fn run_capture(
    args: CaptureArgs,
    stop: &AtomicBool,
    on_progress: impl FnMut() -> Result<()>,
    on_running: impl FnOnce() -> Result<()>,
    on_stopping: impl FnOnce() -> Result<()>,
) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(crate::service::EventLogWriter)
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize diagnostics: {error}"))?;

    let config = PipelineConfig::try_from(args)?;
    run_pipeline(
        &config,
        |address, stats| build_sink(&config, address, stats),
        |start| {
            Capture::start(
                start.session_name,
                start.sender,
                start.pool,
                start.counters,
                start.session,
                start.parser,
                #[cfg(feature = "diag-timing")]
                start.diag.thread_priority,
            )
        },
        stop,
        Lifecycle {
            progress: on_progress,
            running: on_running,
            stopping: on_stopping,
        },
        PerfPublisher::start_or_warn,
    )
}

/// The connection settings for `address` from the validated `config`.
fn receiver_config(config: &PipelineConfig, address: SocketAddr) -> io::Result<ReceiverConfig> {
    if config.tls {
        ReceiverConfig::system_tls(address, config.tls_server_name.as_deref())
    } else {
        Ok(address.into())
    }
}

/// Builds the sink the parser writes to: the receiver link, the optional spool,
/// and the queued sink that runs the `dnstap-sender` thread over both.
///
/// The order is fixed. The link starts first and a protocol failure there is
/// fatal, as before. The spool comes second and never fails the start: any
/// problem with it is one warning, and the run continues without a spool.
fn build_sink(
    config: &PipelineConfig,
    address: SocketAddr,
    stats: Arc<SinkStats>,
) -> io::Result<QueuedSink> {
    build_sink_with(
        config,
        address,
        stats,
        fsinfo::check_spool_dir,
        fsinfo::available_bytes,
    )
}

/// The folder trust check [`open_spool`] runs before it opens the spool.
type SpoolCheck = fn(&Path) -> Result<(), SpoolDirError>;
/// The free-space probe the spool uses for its floor.
type FreeSpace = fn(&Path) -> io::Result<u64>;

/// [`build_sink`] with the folder check and the free-space probe as parameters,
/// so tests can run the whole wiring on a temp folder. Production passes
/// [`fsinfo::check_spool_dir`] and [`fsinfo::available_bytes`].
fn build_sink_with(
    config: &PipelineConfig,
    address: SocketAddr,
    stats: Arc<SinkStats>,
    check: SpoolCheck,
    free_space: FreeSpace,
) -> io::Result<QueuedSink> {
    let link = NetworkSink::start(receiver_config(config, address)?, Arc::clone(&stats))?;
    let spool = if let Some(settings) = &config.spool {
        open_spool(settings, &stats, check, free_space)
    } else {
        warn_spool_disabled();
        None
    };
    QueuedSink::start(link, spool, stats, config.receiver_stall())
}

/// Logs once, at start, that `--no-spool` is set: any receiver outage loses
/// frames. Event ID 26.
fn warn_spool_disabled() {
    tracing::warn!(
        target: crate::event_ids::RISK_SPOOL_DISABLED,
        "spool is disabled (--no-spool); frames are dropped and counted while the receiver is down, and when the send queue is full"
    );
}

/// Checks the spool folder with `check` and opens the spool. Returns `None` after
/// logging one warning that names the reason, so the caller runs without a
/// spool. Never logs frame data.
fn open_spool(
    settings: &SpoolSettings,
    stats: &Arc<SinkStats>,
    check: SpoolCheck,
    free_space: FreeSpace,
) -> Option<Spool> {
    let dir = settings.dir.as_path();
    if let Err(reason) = check(dir) {
        tracing::warn!(
            target: crate::event_ids::RISK_SPOOL_DISABLED,
            spool_dir = %dir.display(),
            %reason,
            "spool folder is not usable; capture continues without a spool, so frames are \
             dropped and counted while the receiver is down"
        );
        return None;
    }
    let config = SpoolConfig {
        dir: settings.dir.clone(),
        max_bytes: settings.max_bytes(),
        min_free_bytes: settings.min_free_bytes(),
    };
    match Spool::open(config, Arc::clone(stats), free_space) {
        Ok(spool) => {
            tracing::info!(
                spool_dir = %dir.display(),
                spool_max_mib = settings.max_mib,
                spool_min_free_mib = settings.min_free_mib,
                leftover_segments = stats.spool_segments.load(Ordering::Relaxed),
                leftover_bytes = stats.spool_bytes.load(Ordering::Relaxed),
                "spool in use"
            );
            Some(spool)
        }
        Err(error) => {
            tracing::warn!(
                target: crate::event_ids::RISK_SPOOL_DISABLED,
                spool_dir = %dir.display(),
                %error,
                "spool could not be opened; capture continues without a spool, so frames \
                 are dropped and counted while the receiver is down"
            );
            None
        }
    }
}

/// Runs the pipeline against any sink and producer. `connect` opens the sink
/// before capture starts (it may return a sink that reconnects on its own and
/// reports through the shared [`SinkStats`]), `start` begins event delivery, and
/// the call returns after both workers have stopped and the counters have been
/// reported. `lifecycle` carries the state callbacks (see [`Lifecycle`]). `perf`
/// starts the performance counter publisher for the named instance, or returns
/// `None` when counters are unavailable.
fn run_pipeline<S, P>(
    config: &PipelineConfig,
    connect: impl FnOnce(SocketAddr, Arc<SinkStats>) -> io::Result<S>,
    start: impl FnOnce(ProducerStart<'_>) -> Result<P>,
    stop: &AtomicBool,
    lifecycle: Lifecycle<
        impl FnMut() -> Result<()>,
        impl FnOnce() -> Result<()>,
        impl FnOnce() -> Result<()>,
    >,
    perf: impl FnOnce(&str, CounterSources) -> Option<Box<dyn CounterSink>>,
) -> Result<()>
where
    S: FrameSink + Send + 'static,
    P: EventProducer,
{
    let Lifecycle {
        progress: on_progress,
        running: on_running,
        stopping: on_stopping,
    } = lifecycle;
    // Declared before any worker thread so it drops after both are joined.
    let _diag_levers = begin_diag_levers(config.diag)?;
    let sink_stats = Arc::new(SinkStats::default());
    let writer = connect(config.receiver, Arc::clone(&sink_stats))
        .context("connect/negotiation with dnstap receiver")?;
    let (sender, queue) = crossbeam_channel::bounded(config.capacity);
    let pool = PoolManager::with_progress(
        config.capacity,
        usize::from(config.jumbo_capacity),
        on_progress,
    )?;
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

    let startup_watchdog = ShutdownWatchdog::arm_for("startup", CAPTURE_START_TIMEOUT);
    let producer = start(ProducerStart {
        session_name: &config.session_name,
        sender,
        pool,
        counters: Arc::clone(&capture_counts),
        session: config.session,
        parser: parser.thread().clone(),
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
    // Started while the startup deadline still holds; capture is already running.
    // Query first so the first snapshot has the ETW maximum.
    query_and_log_buffers_applied(&producer, &capture_counts);
    let publisher = perf(
        &config.session_name,
        CounterSources {
            capture: Arc::clone(&capture_counts),
            parse: Arc::clone(&parse_counts),
            sink: Arc::clone(&sink_stats),
            capacity: config.capacity,
        },
    );
    startup_watchdog.disarm()?;
    let started = Instant::now();
    let mut monitor = Monitor::new(
        config,
        &capture_counts,
        &parse_counts,
        &sink_stats,
        Instant::now(),
    );
    if let Some(publisher) = publisher {
        monitor.start_publishing(publisher, Instant::now());
    }
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
    // Both workers are gone; withdraw the counter instance.
    drop(monitor);
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

/// Received rate above which the default buffer settings are untested. The
/// defaults are sized for about 20,000 events/s (10,000 queries/s); the 10%
/// margin covers ETW's once-per-second delivery batches within a rate window.
const DEFAULTS_VALIDATED_EVENTS_PER_SEC: f64 = 22_000.0;
/// The warning logged once when the rate passes
/// [`DEFAULTS_VALIDATED_EVENTS_PER_SEC`]. It names both figures: the validated
/// rate and the threshold above it.
const RATE_WARNING: &str = "received rate is above 22,000 events/s; the default buffer settings \
    were validated at about 20,000 events/s (10,000 queries per second); if ring_dropped or \
    etw_lost increase, use the settings for your rate in the \"Recommended settings for higher \
    rates\" section of docs/guide/tuning.md, for example --etw-buffer-kib 128 --etw-max-buffers \
    128 for 50,000 queries per second";
/// How often the supervisor re-reads the DNS Server service process ID.
const TARGET_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Rate window when counter snapshots are disabled.
const DEFAULT_RATE_WINDOW: Duration = Duration::from_secs(60);
/// How often the supervisor queries ETW and publishes the performance counters.
const PUBLISH_INTERVAL: Duration = Duration::from_secs(1);

/// Measures the received-event rate over fixed windows.
struct RateMonitor {
    window: Duration,
    window_start: Instant,
    received_at_start: u64,
}

impl RateMonitor {
    const fn new(window: Duration, now: Instant, received: u64) -> Self {
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

/// True when the default buffer settings are in use and the rate exceeds what
/// they were validated for.
fn default_rate_exceeded(defaults_in_use: bool, events_per_second: f64) -> bool {
    defaults_in_use && events_per_second > DEFAULTS_VALIDATED_EVENTS_PER_SEC
}

/// Everything the supervisor schedules and reads while capture runs.
struct Monitor<'a> {
    capture: &'a CaptureCounters,
    parse: &'a ParseCounters,
    sink: &'a SinkStats,
    stats: Schedule,
    /// When the loss and risk check runs: the stats interval, or the rate
    /// window when snapshots are disabled.
    alerts_tick: Schedule,
    target: Schedule,
    /// Disabled until a counter publisher starts.
    publish: Schedule,
    rate: RateMonitor,
    /// The ETW settings and the channel capacity are all defaults.
    defaults_in_use: bool,
    /// The rate warning was logged; it is logged at most once per run.
    rate_warned: bool,
    /// Channel capacity, the base of the channel fill counter.
    capacity: usize,
    perf: Option<Box<dyn CounterSink>>,
    /// The loss and risk checks.
    alerts: Alerts,
    /// The spool folder, whose volume the spool risk measures; `None` with
    /// `--no-spool`.
    spool_dir: Option<std::path::PathBuf>,
    /// The free-space probe of the spool volume.
    free_space: FreeSpace,
    /// Pipeline thread CPU use between publication ticks.
    publish_cpu: CpuSampler,
    /// Pipeline thread CPU use between periodic stats logs.
    stats_cpu: CpuSampler,
}

impl<'a> Monitor<'a> {
    fn new(
        config: &PipelineConfig,
        capture: &'a CaptureCounters,
        parse: &'a ParseCounters,
        sink: &'a SinkStats,
        now: Instant,
    ) -> Self {
        let limits = Limits {
            capacity: config.capacity,
            jumbo_capacity: config.jumbo_capacity,
            spool: config.spool.as_ref().map(|spool| SpoolLimits {
                max: spool.max_bytes(),
                min_free: spool.min_free_bytes(),
                segment: segment_bytes_for(spool.max_bytes()),
            }),
        };
        // A zero baseline: the first check covers everything since capture
        // started, including loss counted before supervision began.
        let baseline = Readings::default();
        Self {
            capture,
            parse,
            sink,
            stats: Schedule::new(config.stats_interval(), now),
            alerts_tick: Schedule::new(
                Some(config.stats_interval().unwrap_or(DEFAULT_RATE_WINDOW)),
                now,
            ),
            alerts: Alerts::new(limits, now, baseline),
            spool_dir: config.spool.as_ref().map(|spool| spool.dir.clone()),
            free_space: fsinfo::available_bytes,
            target: Schedule::new(Some(TARGET_CHECK_INTERVAL), now),
            publish: Schedule::new(None, now),
            rate: RateMonitor::new(
                config.stats_interval().unwrap_or(DEFAULT_RATE_WINDOW),
                now,
                capture.received.load(Ordering::Relaxed),
            ),
            defaults_in_use: config.uses_default_etw_settings()
                && config.capacity == DEFAULT_CAPACITY,
            rate_warned: false,
            capacity: config.capacity,
            perf: None,
            publish_cpu: CpuSampler::new(now, stage_cpu_totals(capture, parse, sink)),
            stats_cpu: CpuSampler::new(now, stage_cpu_totals(capture, parse, sink)),
        }
    }

    /// Publishes counters every [`PUBLISH_INTERVAL`] to `perf` until the
    /// monitor is dropped.
    fn start_publishing(&mut self, perf: Box<dyn CounterSink>, now: Instant) {
        self.perf = Some(perf);
        self.publish = Schedule::new(Some(PUBLISH_INTERVAL), now);
        self.publish_cpu =
            CpuSampler::new(now, stage_cpu_totals(self.capture, self.parse, self.sink));
    }

    /// Reads the counters and writes the loss and risk events that the change
    /// since the previous check calls for. At most one event per ID.
    fn check_alerts(&mut self, now: Instant) {
        let volume_free = self
            .spool_dir
            .as_deref()
            .and_then(|dir| (self.free_space)(dir).ok());
        let readings = Readings::read(self.capture, self.parse, self.sink, volume_free);
        for notice in self.alerts.check(now, &readings) {
            notice.emit();
        }
    }

    /// Publishes one snapshot of the counters, if a publisher is running.
    fn publish_counters(&mut self, now: Instant) {
        if let Some(perf) = &self.perf {
            let mut values = snapshot(self.capture, self.parse, self.sink, self.capacity);
            values.stage_cpu = self
                .publish_cpu
                .sample(now, stage_cpu_totals(self.capture, self.parse, self.sink));
            perf.publish(&values);
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
        if monitor.target.poll(now)
            && let Some(change) = capture.target_change()
        {
            // The PID filter and callback check now match no process. Exiting
            // with a failure lets SCM recovery restart the service, which
            // resolves the new PID at startup.
            tracing::error!(
                "{change}; stopping so the service can be restarted and re-resolve the DNS \
                 Server process"
            );
            break Some(change.to_string());
        }
        if let Some(rate) = monitor
            .rate
            .poll(now, monitor.capture.received.load(Ordering::Relaxed))
            && !monitor.rate_warned
            && default_rate_exceeded(monitor.defaults_in_use, rate)
        {
            monitor.rate_warned = true;
            tracing::warn!(events_per_second = rate.round(), "{RATE_WARNING}");
        }
        // One ETW query per iteration at most, shared by all three ticks.
        let mut queried = false;
        let stats_due = monitor.stats.poll(now);
        let alerts_due = monitor.alerts_tick.poll(now);
        if stats_due || alerts_due {
            query_session(capture);
            queried = true;
        }
        if stats_due {
            let cpu = monitor.stats_cpu.sample(
                now,
                stage_cpu_totals(monitor.capture, monitor.parse, monitor.sink),
            );
            report_snapshot(monitor.capture, monitor.parse, monitor.sink, &cpu);
        }
        // Nothing but the two stop reports is written once shutdown begins. The
        // loop head checks `stop`; this closes the gap within one iteration.
        if alerts_due && !stop.load(Ordering::Relaxed) {
            monitor.check_alerts(now);
        }
        if monitor.publish.poll(now) {
            if !queried {
                query_session(capture);
            }
            monitor.publish_counters(now);
        }
        thread::park_timeout(Duration::from_millis(50));
    }
}

/// Refreshes the ETW gauges and returns whether the query succeeded. A failed
/// query leaves the last values in place.
fn query_session(capture: &impl EventProducer) -> bool {
    if let Err(status) = capture.query_statistics() {
        tracing::debug!(%status, "ETW session query failed");
        return false;
    }
    true
}

/// Queries the session once and, only if the query succeeded, logs the buffer
/// counts that ETW applied. They can differ from `etw_min_buffers` and
/// `etw_max_buffers` in `capture started`.
fn query_and_log_buffers_applied(capture: &impl EventProducer, counters: &CaptureCounters) {
    if !query_session(capture) {
        return;
    }
    tracing::info!(
        etw_min_buffers_applied = counters.etw_buffers_minimum.load(Ordering::Relaxed),
        etw_max_buffers_applied = counters.etw_buffers_maximum.load(Ordering::Relaxed),
        etw_buffers_allocated = counters.etw_buffers_allocated.load(Ordering::Relaxed),
        "ETW buffers applied"
    );
}

fn log_capture_started(config: &PipelineConfig, keywords: KeywordSelection) {
    tracing::info!(
        session = config.session_name.as_str(),
        session_guid = crate::etw::session_guid_text(&config.session_name),
        receiver = %config.receiver,
        tls = config.tls,
        tls_server_name = config.tls_server_name.as_deref(),
        capacity = config.capacity,
        etw_buffer_kib = config.session.buffer_kib,
        etw_min_buffers = config.session.minimum_buffers,
        etw_max_buffers = config.session.maximum_buffers,
        etw_flush_seconds = config.session.flush_seconds,
        receiver_stall_seconds = config.receiver_stall_seconds,
        match_any_keyword = format_args!("{:#x}", keywords.match_any),
        match_all_keyword = format_args!("{:#x}", keywords.match_all),
        keyword_fallback = keywords.fallback,
        failed_responses_captured = keywords.failed_responses,
        version = env!("CARGO_PKG_VERSION"),
        "capture started"
    );
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
        Self::begin_with(
            milliseconds,
            crate::etw::time_begin_period,
            crate::etw::time_end_period,
        )
    }

    fn begin_with(milliseconds: u32, begin: fn(u32) -> u32, end: fn(u32) -> u32) -> Result<Self> {
        let result = begin(milliseconds);
        if result != crate::etw::TIME_NO_ERROR {
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

/// Process-wide diagnostic levers; dropping it withdraws them, so it must
/// outlive the workers. Empty unless `diag-timing` is enabled.
struct DiagLevers {
    #[cfg(feature = "diag-timing")]
    _timer: Option<TimerResolution>,
}

#[cfg(feature = "diag-timing")]
fn begin_diag_levers(diag: DiagContext) -> Result<DiagLevers> {
    Ok(DiagLevers {
        _timer: diag
            .timer_resolution_ms
            .map(TimerResolution::begin)
            .transpose()?,
    })
}

#[cfg(not(feature = "diag-timing"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Starting the diagnostic timer can fail; the normal build has no timer."
)]
const fn begin_diag_levers(_diag: DiagContext) -> Result<DiagLevers> {
    Ok(DiagLevers {})
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
            .map_or("normal", crate::etw::ThreadPriority::as_str),
        "diagnostic settings applied"
    );
}

#[cfg(not(feature = "diag-timing"))]
const fn log_diag_settings(_diag: DiagContext) {}

fn combine_results(capture: Result<()>, parser: Result<()>) -> Result<()> {
    match (capture, parser) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(capture), Err(parser)) => {
            Err(capture.context(format!("parser also failed: {parser:#}")))
        }
    }
}

/// Emits the two counter events. A macro because `tracing` needs a literal
/// message, and the final report and the periodic snapshot share every field.
///
/// Counter relationships: `ring_dropped` (channel full) is a subset of
/// `intake_dropped`, which also includes pool exhaustion and a closed channel;
/// never add them. `pool_exhausted_regular + pool_exhausted_jumbo` is likewise
/// part of `intake_dropped`.
///
/// The sink counters count frames that left the parser, and they are not part of
/// `intake_dropped`. Each frame of this run that is lost is in at most one of
/// three loss counters: `disconnected_dropped` (no spool was configured or
/// usable while the receiver was down, or the connection failed with a live frame
/// in flight and no spool to take it back), `spool_dropped` (the spool evicted,
/// refused, or could not read the frame), and `queue_dropped` (the send queue
/// was full). `replayed` counts frames from the spool that the connection fully
/// accepted, when it accepted them: a frame in a replay batch that a failed
/// connection did not take stays in the spool, is read again, and counts then.
/// `spooled` counts frames appended to the spool; those not yet replayed or
/// dropped are still in it. Leftover frames from an earlier run appear in `replayed` and in
/// `spool_dropped_bytes`, but never in `encoded`, and `spool_dropped_bytes` has
/// no fixed ratio to `spool_dropped` because a leftover segment's frame count is
/// unknown. One case is in none of these counters: frames already in the send
/// queue when the sender thread stops abnormally (the parser counts only the
/// failed write in `output_failed_events`). `spool_bytes`, `spool_segments`,
/// `write_max_recent_us`, `router_wait_max_recent_us`,
/// `router_pass_max_recent_us`, `send_queue_depth_max`, and `spool_oldest_age_ms`
/// are gauges, and `spool_available` and `link_saturated` are flags. The sender
/// thread sets the four peaks, `spool_oldest_age_ms`, and `link_saturated` on
/// its own one-second clock, so the interval of this log does not change them. `bytes_sent` is a total.
///
/// The two brace groups at the end add fields to the capture line and the output
/// line; the stop summaries pass empty groups.
macro_rules! emit_counts {
    ($capture_message:literal, $output_message:literal, $capture:expr, $parser:expr, $sink:expr) => {
        emit_counts!(
            $capture_message,
            $output_message,
            $capture,
            $parser,
            $sink,
            {},
            {}
        )
    };
    (
        $capture_message:literal,
        $output_message:literal,
        $capture:expr,
        $parser:expr,
        $sink:expr,
        {$($capture_extra:tt)*},
        {$($output_extra:tt)*}
    ) => {{
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
            etw_buffers_delivered = capture.etw_buffers_delivered.load(Ordering::Relaxed),
            parser_wake_requests = capture.parser_wake_requests.load(Ordering::Relaxed),
            $($capture_extra)*
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
            queue_dropped = sink.queue_dropped.load(Ordering::Relaxed),
            spooled = sink.spooled.load(Ordering::Relaxed),
            replayed = sink.replayed.load(Ordering::Relaxed),
            spool_dropped = sink.spool_dropped.load(Ordering::Relaxed),
            spool_dropped_bytes = sink.spool_dropped_bytes.load(Ordering::Relaxed),
            spool_errors = sink.spool_errors.load(Ordering::Relaxed),
            spool_bytes = sink.spool_bytes.load(Ordering::Relaxed),
            spool_segments = sink.spool_segments.load(Ordering::Relaxed),
            spool_available = sink.spool_available.load(Ordering::Relaxed),
            slow_writes = sink.slow_writes.load(Ordering::Relaxed),
            write_max_us = sink.write_max_us.load(Ordering::Relaxed),
            write_max_recent_us = sink.write_max_recent_us.load(Ordering::Relaxed),
            router_wait_max_recent_us = sink.router_wait_max_recent_us.load(Ordering::Relaxed),
            router_pass_max_recent_us = sink.router_pass_max_recent_us.load(Ordering::Relaxed),
            queue_depth_max = parser.queue_depth_max.load(Ordering::Relaxed),
            send_queue_depth_max = sink.send_queue_depth_max.load(Ordering::Relaxed),
            bytes_sent = sink.bytes_sent.load(Ordering::Relaxed),
            spool_oldest_age_ms = sink.spool_oldest_age_ms.load(Ordering::Relaxed),
            link_saturated = sink.link_saturated.load(Ordering::Relaxed),
            $($output_extra)*
            $output_message
        );
    }};
}

/// The periodic counter snapshot. `cpu` is the CPU use of the four pipeline
/// threads since the previous snapshot, as a percentage of one logical processor
/// (see [`StageCpu::percent`]). The intake and parser threads are on the capture
/// line, and the sender and link threads on the output line.
fn report_snapshot(
    capture: &CaptureCounters,
    parser: &ParseCounters,
    sink: &SinkStats,
    cpu: &[StageCpu; STAGE_COUNT],
) {
    emit_counts!(
        "capture stats",
        "output stats",
        capture,
        parser,
        sink,
        {
            cpu_etw_pct = format_args!("{:.1}", cpu[ETW_CONSUMER].percent()),
            cpu_parser_pct = format_args!("{:.1}", cpu[PARSER].percent()),
        },
        {
            cpu_sender_pct = format_args!("{:.1}", cpu[SENDER].percent()),
            cpu_link_pct = format_args!("{:.1}", cpu[LINK].percent()),
        }
    );
}

fn report_counts(capture: &CaptureCounters, parser: &ParseCounters, sink: &SinkStats) {
    emit_counts!("capture stopped", "output stopped", capture, parser, sink);
    parser.timing.report();
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod supervisor_tests;
