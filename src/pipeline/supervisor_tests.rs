//! Schedules, the rate monitor, and the supervisor and pipeline runs against fakes.

use super::*;
use crate::{
    etw::{TargetChange, Win32Status},
    pipeline::{config::parse_args_from, test_support::config_from},
    spool::test_support::capture_logs_at,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::atomic::AtomicU64,
};

#[test]
fn schedule_disabled_is_never_due() {
    let start = Instant::now();
    let mut schedule = Schedule::new(None, start);
    assert!(!schedule.poll(start));
    assert!(!schedule.poll(start + Duration::from_secs(60)));
}

#[test]
fn schedule_is_due_exactly_at_interval() {
    let ms = Duration::from_millis;
    let start = Instant::now();
    let mut schedule = Schedule::new(Some(ms(100)), start);
    assert!(!schedule.poll(start));
    assert!(!schedule.poll(start + ms(99)));
    assert!(schedule.poll(start + ms(100)));
    // The deadline advances from the poll time.
    assert!(!schedule.poll(start + ms(199)));
    assert!(schedule.poll(start + ms(200)));
    assert!(schedule.poll(start + ms(5000)));
    assert!(!schedule.poll(start + ms(5001)));
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
            failed_responses: true,
        }
    }

    fn is_finished(&self) -> bool {
        false
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
        |target, _stats| crate::network::Connection::connect(target),
        |_start| {
            Ok(FakeProducer {
                stopped: producer_stopped,
            })
        },
        &stop,
        Lifecycle {
            progress: || Ok(()),
            running: || Ok(()),
            stopping: || Ok(()),
        },
        |_name, _sources| None,
    )?;
    receiver.join().expect("receiver completes")?;
    assert!(stopped.load(Ordering::SeqCst), "producer was stopped");
    Ok(())
}

/// Runs the pipeline with the production sink factory, [`build_sink`], against a
/// loopback receiver that completes the handshake and the STOP/FINISH exchange.
fn run_with_production_sink(extra: &[&str]) -> Result<Arc<SinkStats>> {
    run_with_sink_checks(extra, fsinfo::check_spool_dir)
}

/// [`run_with_production_sink`] with the spool folder check replaced.
fn run_with_sink_checks(extra: &[&str], check: SpoolCheck) -> Result<Arc<SinkStats>> {
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
    let address_text = address.to_string();
    let mut arguments = vec!["etw-dnstap", "--receiver", &address_text, "--plaintext"];
    arguments.extend_from_slice(extra);
    let config = PipelineConfig::try_from(parse_args_from(arguments)?)?;
    let stats = Arc::new(SinkStats::default());
    let factory_stats = Arc::clone(&stats);
    // Shutdown is already requested, so the supervisor exits at once.
    let stop = AtomicBool::new(true);
    run_pipeline(
        &config,
        |target, _stats| {
            build_sink_with(
                &config,
                target,
                factory_stats,
                check,
                fsinfo::available_bytes,
            )
        },
        |_start| {
            Ok(FakeProducer {
                stopped: Arc::new(AtomicBool::new(false)),
            })
        },
        &stop,
        Lifecycle {
            progress: || Ok(()),
            running: || Ok(()),
            stopping: || Ok(()),
        },
        |_name, _sources| None,
    )?;
    receiver.join().expect("receiver completes")?;
    Ok(stats)
}

#[test]
fn production_sink_runs_and_stops_without_a_spool() -> Result<()> {
    let stats = run_with_production_sink(&["--no-spool"])?;
    assert!(!stats.spool_available.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn production_sink_continues_without_a_spool_when_the_folder_is_missing() -> Result<()> {
    let base = crate::spool::test_support::TestDir::new("missing-spool");
    let missing = base.path().join("absent");
    let missing_text = missing.to_string_lossy().into_owned();
    // The start must not fail: the pipeline runs and stops through the queued sink.
    let stats = run_with_production_sink(&["--spool-dir", &missing_text])?;
    assert!(!stats.spool_available.load(Ordering::Relaxed));
    assert_eq!(stats.spool_segments.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn an_unusable_spool_folder_logs_one_warning_and_returns_no_spool() {
    let base = crate::spool::test_support::TestDir::new("unusable-spool");
    let settings = SpoolSettings {
        dir: base.path().join("absent"),
        max_mib: 64,
        min_free_mib: 0,
    };
    let stats = Arc::new(SinkStats::default());
    let mut spool = None;
    let lines = capture_logs_at(tracing::Level::INFO, || {
        spool = open_spool(
            &settings,
            &stats,
            fsinfo::check_spool_dir,
            fsinfo::available_bytes,
        );
    });
    assert!(spool.is_none());
    assert!(!stats.spool_available.load(Ordering::Relaxed));
    let text = lines.text();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("does not exist"), "{text}");
    assert!(text.contains("without a spool"), "{text}");
}

#[test]
fn a_relative_spool_folder_is_refused() {
    let settings = SpoolSettings {
        dir: PathBuf::from("relative-spool"),
        max_mib: 64,
        min_free_mib: 0,
    };
    let stats = Arc::new(SinkStats::default());
    assert!(
        open_spool(
            &settings,
            &stats,
            fsinfo::check_spool_dir,
            fsinfo::available_bytes
        )
        .is_none()
    );
}

#[test]
fn network_and_device_spool_folders_are_refused_by_the_real_check() {
    let stats = Arc::new(SinkStats::default());
    for dir in [
        r"\\host\share\spool",
        r"\\?\UNC\host\share\spool",
        r"\\.\C:\spool",
        r"\\?\GLOBALROOT\Device\Spool",
    ] {
        let settings = SpoolSettings {
            dir: PathBuf::from(dir),
            max_mib: 64,
            min_free_mib: 0,
        };
        let spool = open_spool(
            &settings,
            &stats,
            fsinfo::check_spool_dir,
            fsinfo::available_bytes,
        );
        assert!(spool.is_none(), "{dir}");
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is the folder check's function pointer type"
)]
fn accept_any_folder(_dir: &Path) -> Result<(), SpoolDirError> {
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is the free-space probe's function pointer type"
)]
fn plenty_of_space(_dir: &Path) -> io::Result<u64> {
    Ok(1 << 40)
}

#[test]
fn a_trusted_folder_opens_the_spool_and_reports_leftovers() {
    use crate::spool::test_support::{TestDir, numbered, start_bytes};
    let dir = TestDir::new("leftover-spool");
    let mut segment = start_bytes();
    segment.extend_from_slice(&numbered(0..3, 10).0);
    segment.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3]);
    std::fs::write(dir.path().join(format!("seg-{:020}.fstrm", 1)), &segment)
        .expect("leftover is written");
    let settings = SpoolSettings {
        dir: dir.path().to_path_buf(),
        max_mib: 64,
        min_free_mib: 0,
    };
    let stats = Arc::new(SinkStats::default());
    let mut spool = None;
    let lines = capture_logs_at(tracing::Level::INFO, || {
        spool = open_spool(&settings, &stats, accept_any_folder, plenty_of_space);
    });
    assert!(spool.is_some_and(|spool| spool.has_backlog()));
    assert!(stats.spool_available.load(Ordering::Relaxed));
    assert_eq!(stats.spool_segments.load(Ordering::Relaxed), 1);
    assert_eq!(
        stats.spool_bytes.load(Ordering::Relaxed),
        segment.len() as u64
    );
    let text = lines.text();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("spool in use"), "{text}");
    assert!(text.contains("leftover_segments=1"), "{text}");
}

#[test]
fn pipeline_runs_with_a_spool_in_use() -> Result<()> {
    let dir = crate::spool::test_support::TestDir::new("pipeline-spool");
    let dir_text = dir.path().to_string_lossy().into_owned();
    let stats = run_with_sink_checks(&["--spool-dir", &dir_text], accept_any_folder)?;
    assert!(stats.spool_available.load(Ordering::Relaxed));
    Ok(())
}

/// A receiver for the loopback tests: accepts one connection, completes the
/// handshake, collects data frames until STOP, and acknowledges it.
fn collecting_receiver(listener: TcpListener) -> thread::JoinHandle<std::io::Result<Vec<Vec<u8>>>> {
    thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        stream.set_write_timeout(Some(Duration::from_secs(20)))?;
        let mut control = [0; 42];
        stream.read_exact(&mut control)?;
        control[8..12].copy_from_slice(&1_u32.to_be_bytes());
        stream.write_all(&control)?;
        stream.read_exact(&mut control)?;
        let mut frames = Vec::new();
        loop {
            let mut length = [0; 4];
            stream.read_exact(&mut length)?;
            let length = u32::from_be_bytes(length) as usize;
            if length == 0 {
                // A control frame: its length, then its body (STOP here).
                let mut control_length = [0; 4];
                stream.read_exact(&mut control_length)?;
                let mut body = vec![0; u32::from_be_bytes(control_length) as usize];
                stream.read_exact(&mut body)?;
                stream.write_all(&[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 5])?;
                return Ok(frames);
            }
            let mut payload = vec![0; length];
            stream.read_exact(&mut payload)?;
            frames.push(payload);
        }
    })
}

#[test]
fn frames_are_spooled_while_the_receiver_is_down_and_delivered_after() -> Result<()> {
    let dir = crate::spool::test_support::TestDir::new("outage-spool");
    let dir_text = dir.path().to_string_lossy().into_owned();
    // A port that was just released refuses connections.
    let address = TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    let address_text = address.to_string();
    let config = PipelineConfig::try_from(parse_args_from([
        "etw-dnstap",
        "--receiver",
        &address_text,
        "--plaintext",
        "--spool-dir",
        &dir_text,
        "--spool-min-free-mib",
        "0",
    ])?)?;
    let stats = Arc::new(SinkStats::default());
    let mut sink = build_sink_with(
        &config,
        address,
        Arc::clone(&stats),
        accept_any_folder,
        plenty_of_space,
    )?;
    assert!(stats.spool_available.load(Ordering::Relaxed));
    let payloads: Vec<Vec<u8>> = (0..3_u8).map(|n| vec![n; 20]).collect();
    for payload in &payloads {
        sink.write_frame(payload)?;
    }
    sink.flush()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while stats.spooled.load(Ordering::Relaxed) < 3 {
        assert!(Instant::now() < deadline, "frames were not spooled");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(stats.disconnected_dropped.load(Ordering::Relaxed), 0);

    // The receiver comes up; the sender reconnects and replays in order.
    let collector = collecting_receiver(TcpListener::bind(address)?);
    while stats.replayed.load(Ordering::Relaxed) < 3 {
        assert!(Instant::now() < deadline, "frames were not replayed");
        thread::sleep(Duration::from_millis(5));
    }
    sink.finish()?;
    let delivered = collector.join().expect("receiver completes")?;
    assert_eq!(delivered, payloads);
    assert_eq!(stats.disconnected_dropped.load(Ordering::Relaxed), 0);
    assert_eq!(stats.spool_dropped.load(Ordering::Relaxed), 0);
    assert_eq!(stats.queue_dropped.load(Ordering::Relaxed), 0);
    Ok(())
}

/// A producer whose session query stores fixed buffer counts or fails.
struct QueryProducer {
    counters: Arc<CaptureCounters>,
    status: Result<(), Win32Status>,
}

impl EventProducer for QueryProducer {
    fn keywords(&self) -> KeywordSelection {
        KeywordSelection {
            match_any: 0,
            match_all: 0,
            fallback: true,
            failed_responses: true,
        }
    }

    fn is_finished(&self) -> bool {
        false
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn query_statistics(&self) -> Result<(), Win32Status> {
        if self.status.is_ok() {
            self.counters
                .etw_buffers_minimum
                .store(12, Ordering::Relaxed);
            self.counters
                .etw_buffers_maximum
                .store(40, Ordering::Relaxed);
            self.counters
                .etw_buffers_allocated
                .store(14, Ordering::Relaxed);
        }
        self.status
    }
}

fn logged_by_first_query(status: Result<(), Win32Status>) -> String {
    let counters = Arc::new(CaptureCounters::default());
    let producer = QueryProducer {
        counters: Arc::clone(&counters),
        status,
    };
    let lines = capture_logs_at(tracing::Level::INFO, || {
        query_and_log_buffers_applied(&producer, &counters);
    });
    lines.text()
}

#[test]
fn applied_buffer_counts_are_logged_only_after_a_successful_query() {
    let text = logged_by_first_query(Ok(()));
    assert!(text.contains("ETW buffers applied"), "{text}");
    assert!(text.contains("etw_min_buffers_applied=12"), "{text}");
    assert!(text.contains("etw_max_buffers_applied=40"), "{text}");
    assert!(text.contains("etw_buffers_allocated=14"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");

    let failed_text = logged_by_first_query(Err(Win32Status(5)));
    assert!(failed_text.is_empty(), "{failed_text}");
}

#[test]
fn capture_started_logs_the_package_version() {
    let config = PipelineConfig::try_from(
        parse_args_from(["etw-dnstap", "--receiver", "127.0.0.1:6000", "--plaintext"])
            .expect("args"),
    )
    .expect("config");
    let keywords = KeywordSelection {
        match_any: 1,
        match_all: 0,
        fallback: false,
        failed_responses: true,
    };
    let lines = capture_logs_at(tracing::Level::INFO, || {
        log_capture_started(&config, keywords);
    });
    let text = lines.text();
    assert!(text.contains("capture started"), "{text}");
    let expected = format!("version=\"{}\"", env!("CARGO_PKG_VERSION"));
    assert!(text.contains(&expected), "{text}");
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

#[test]
fn rate_warning_needs_defaults_and_a_rate_above_the_validated_one() {
    assert!(default_rate_exceeded(true, 22_001.0));
    assert!(!default_rate_exceeded(true, 22_000.0));
    assert!(!default_rate_exceeded(true, 20_000.0));
    assert!(!default_rate_exceeded(false, 500_000.0));
}

#[test]
fn rate_warning_text_states_the_validated_rate_and_the_threshold() {
    assert!(
        RATE_WARNING.contains("validated at about 20,000 events/s (10,000 queries per second)")
    );
    assert!(RATE_WARNING.contains("above 22,000 events/s"));
    assert!(RATE_WARNING.contains("Recommended settings for higher rates"));
    assert!(RATE_WARNING.contains("--etw-buffer-kib 128 --etw-max-buffers 128"));
    // The text states the figure the condition uses.
    assert!((DEFAULTS_VALIDATED_EVENTS_PER_SEC - 22_000.0).abs() < f64::EPSILON);
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

/// A producer scripted for supervisor tests.
struct ScriptedProducer {
    moved: bool,
}

impl EventProducer for ScriptedProducer {
    fn keywords(&self) -> KeywordSelection {
        KeywordSelection {
            match_any: 0,
            match_all: 0,
            fallback: true,
            failed_responses: true,
        }
    }

    fn is_finished(&self) -> bool {
        false
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn target_change(&self) -> Option<TargetChange> {
        self.moved
            .then_some(TargetChange::Restarted { old: 1, new: 2 })
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
    monitor.target = Schedule::new(Some(Duration::from_millis(10)), Instant::now());
    let done = Arc::new(AtomicBool::new(false));
    let parser_done = Arc::clone(&done);
    let parser = thread::spawn(move || {
        while !parser_done.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(10));
        }
        Ok(())
    });
    let producer = ScriptedProducer { moved: true };
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
    let mut progress_reports = 0;
    run_pipeline(
        &config,
        NetworkSink::start,
        |_start| {
            Ok(FakeProducer {
                stopped: producer_stopped,
            })
        },
        &stop,
        Lifecycle {
            progress: || {
                progress_reports += 1;
                Ok(())
            },
            running: || Ok(()),
            stopping: || Ok(()),
        },
        |_name, _sources| None,
    )?;
    // The default pool (131,072 + 2 regular and 64 jumbo slots) crosses the
    // progress interval exactly eight times.
    assert_eq!(
        progress_reports,
        (config.capacity + 2 + usize::from(config.jumbo_capacity)) / PoolManager::PROGRESS_INTERVAL
    );
    assert_eq!(progress_reports, 8);
    assert!(stopped.load(Ordering::SeqCst), "producer was stopped");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "prompt shutdown"
    );
    Ok(())
}

/// Counts the ETW queries that the supervisor runs.
struct QueryCountingProducer {
    queries: Arc<AtomicU64>,
}

impl EventProducer for QueryCountingProducer {
    fn keywords(&self) -> KeywordSelection {
        KeywordSelection {
            match_any: 0,
            match_all: 0,
            fallback: true,
            failed_responses: true,
        }
    }

    fn is_finished(&self) -> bool {
        false
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn query_statistics(&self) -> Result<(), Win32Status> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Keeps every snapshot that the supervisor publishes.
struct RecordingSink {
    published: Arc<std::sync::Mutex<Vec<crate::perfcounters::CounterValues>>>,
}

impl CounterSink for RecordingSink {
    fn publish(&self, values: &crate::perfcounters::CounterValues) {
        self.published.lock().expect("lock").push(*values);
    }
}

/// Supervises for `run_for` and returns how many ETW queries ran and the
/// snapshots published, with the publisher on or off.
fn supervise_for_counters(
    publish: bool,
    stats_too: bool,
    run_for: Duration,
) -> (u64, Vec<crate::perfcounters::CounterValues>) {
    let config = config_from(&[]);
    let (capture, parse, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    capture.received.store(5, Ordering::Relaxed);
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queries = Arc::new(AtomicU64::new(0));
    let mut monitor = Monitor::new(&config, &capture, &parse, &sink, Instant::now());
    if publish {
        monitor.start_publishing(
            Box::new(RecordingSink {
                published: Arc::clone(&published),
            }),
            Instant::now(),
        );
        let now = Instant::now();
        monitor.publish = Schedule::new(Some(Duration::from_millis(10)), now);
        if stats_too {
            // Due in the same iteration as the publish tick.
            monitor.stats = Schedule::new(Some(Duration::from_millis(10)), now);
        }
    }
    let done = Arc::new(AtomicBool::new(false));
    let parser_done = Arc::clone(&done);
    let parser = thread::spawn(move || {
        while !parser_done.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(10));
        }
        Ok(())
    });
    let producer = QueryCountingProducer {
        queries: Arc::clone(&queries),
    };
    let reason = supervise(
        &AtomicBool::new(false),
        Some(Instant::now() + run_for),
        false,
        &parser,
        &producer,
        &mut monitor,
    );
    assert_eq!(reason, None);
    done.store(true, Ordering::Relaxed);
    parser.join().expect("parser stub exits").expect("ok");
    drop(monitor);
    let published = published.lock().expect("lock").clone();
    (queries.load(Ordering::SeqCst), published)
}

#[test]
fn supervisor_queries_etw_and_publishes_a_snapshot_on_each_tick() {
    let (queries, published) = supervise_for_counters(true, false, Duration::from_millis(300));
    assert!(published.len() >= 2, "{} snapshots", published.len());
    assert_eq!(queries, u64::try_from(published.len()).expect("small"));
    let last = published.last().expect("a snapshot");
    assert_eq!(last.channel_capacity, 131_072);
    assert_eq!(last.channel_depth, 5);
}

#[test]
fn supervisor_runs_one_etw_query_per_iteration_when_both_ticks_are_due() {
    let (queries, published) = supervise_for_counters(true, true, Duration::from_millis(300));
    assert!(published.len() >= 2, "{} snapshots", published.len());
    assert_eq!(queries, u64::try_from(published.len()).expect("small"));
}

#[test]
fn supervisor_does_no_counter_work_without_a_publisher() {
    let (queries, published) = supervise_for_counters(false, false, Duration::from_millis(200));
    assert_eq!(queries, 0);
    assert_eq!(published.len(), 0);
}

#[test]
fn the_output_report_carries_the_slow_receiver_signals() {
    let (capture, parser, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    sink.write_max_us.store(9_000_000, Ordering::Relaxed);
    sink.write_max_recent_us.store(2_500, Ordering::Relaxed);
    sink.router_wait_max_recent_us
        .store(48_000, Ordering::Relaxed);
    sink.router_pass_max_recent_us.store(310, Ordering::Relaxed);
    sink.bytes_sent.store(123_456_789, Ordering::Relaxed);
    sink.send_queue_depth_max.store(77, Ordering::Relaxed);
    sink.spool_oldest_age_ms.store(93_250, Ordering::Relaxed);
    sink.link_saturated.store(true, Ordering::Relaxed);
    let logs = capture_logs_at(tracing::Level::INFO, || {
        report_snapshot(
            &capture,
            &parser,
            &sink,
            &[StageCpu::default(); STAGE_COUNT],
        );
        report_counts(&capture, &parser, &sink);
    });
    let text = logs.text();
    for field in [
        "write_max_us=9000000",
        "write_max_recent_us=2500",
        "router_wait_max_recent_us=48000",
        "router_pass_max_recent_us=310",
        "bytes_sent=123456789",
        "send_queue_depth_max=77",
        "spool_oldest_age_ms=93250",
        "link_saturated=true",
    ] {
        // Once on `output stats` and once on `output stopped`.
        assert_eq!(logs.count(field), 2, "{field}: {text}");
    }
}

/// Supervises for `run_for` with the loss and risk check due every 10 ms, a
/// healthy sink, and `prepare` applied to the counters after the baseline is
/// taken. Returns the log text of the supervisor thread and the ETW queries
/// that ran. With `stopping`, shutdown is requested before supervision starts.
fn supervise_with_alerts(
    extra_arguments: &[&str],
    stopping: bool,
    prepare: impl FnOnce(&CaptureCounters, &ParseCounters, &SinkStats),
    free_space: FreeSpace,
    run_for: Duration,
) -> (String, u64) {
    let config = config_from(extra_arguments);
    let (capture, parse, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    let mut monitor = Monitor::new(&config, &capture, &parse, &sink, Instant::now());
    monitor.alerts_tick = Schedule::new(Some(Duration::from_millis(10)), Instant::now());
    monitor.free_space = free_space;
    sink.connected.store(true, Ordering::Relaxed);
    sink.spool_available.store(true, Ordering::Relaxed);
    prepare(&capture, &parse, &sink);
    let done = Arc::new(AtomicBool::new(false));
    let parser_done = Arc::clone(&done);
    let parser = thread::spawn(move || {
        while !parser_done.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(10));
        }
        Ok(())
    });
    let queries = Arc::new(AtomicU64::new(0));
    let producer = QueryCountingProducer {
        queries: Arc::clone(&queries),
    };
    let stop = AtomicBool::new(stopping);
    let lines = capture_logs_at(tracing::Level::INFO, || {
        let reason = supervise(
            &stop,
            Some(Instant::now() + run_for),
            false,
            &parser,
            &producer,
            &mut monitor,
        );
        assert_eq!(reason, None);
    });
    done.store(true, Ordering::Relaxed);
    parser.join().expect("parser stub exits").expect("ok");
    (lines.text(), queries.load(Ordering::SeqCst))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is the free-space probe's function pointer type"
)]
fn plenty_of_room(_dir: &Path) -> io::Result<u64> {
    Ok(u64::MAX)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is the free-space probe's function pointer type"
)]
fn almost_no_room(_dir: &Path) -> io::Result<u64> {
    Ok(1024 * 1024)
}

const QUIET_FOR: Duration = Duration::from_millis(300);

#[test]
fn a_loss_is_written_once_when_its_counter_rises_and_not_on_later_ticks() {
    // About thirty ticks run; only the first sees a change.
    let (text, queries) = supervise_with_alerts(
        &[],
        false,
        |capture, _parse, _sink| capture.dropped.store(9, Ordering::Relaxed),
        plenty_of_room,
        QUIET_FOR,
    );
    assert_eq!(text.matches("etw_dnstap::event::11").count(), 1, "{text}");
    assert!(text.contains("ring_dropped_increase=9"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
    // The default stats interval is not due, so these queries are the loss
    // check's: it reads fresh ETW counters.
    assert!(queries >= 2, "{queries} queries");
}

#[test]
fn flat_counters_write_nothing_on_any_tick() {
    let (text, queries) = supervise_with_alerts(
        &[],
        false,
        |_capture, _parse, _sink| {},
        plenty_of_room,
        QUIET_FOR,
    );
    assert_eq!(text, "");
    assert!(queries >= 2, "the check ran: {queries} queries");
}

#[test]
fn each_cause_is_written_at_most_once_in_a_tick() {
    let (text, _queries) = supervise_with_alerts(
        &[],
        false,
        |capture, parse, sink| {
            capture.etw_lost.store(1, Ordering::Relaxed);
            capture.etw_buffers_lost.store(1, Ordering::Relaxed);
            capture.dropped.store(1, Ordering::Relaxed);
            capture.pool_exhausted.store(2, Ordering::Relaxed);
            capture.pool_exhausted_jumbo.store(1, Ordering::Relaxed);
            sink.disconnected_dropped.store(1, Ordering::Relaxed);
            sink.queue_dropped.store(1, Ordering::Relaxed);
            sink.spool_dropped.store(1, Ordering::Relaxed);
            sink.spool_dropped_bytes.store(1, Ordering::Relaxed);
            parse.rejected.store(1, Ordering::Relaxed);
            parse.schema_rejected.store(1, Ordering::Relaxed);
            capture.callback_errors.store(1, Ordering::Relaxed);
        },
        plenty_of_room,
        QUIET_FOR,
    );
    for id in [10, 11, 12, 13, 14, 15, 16] {
        let target = format!("etw_dnstap::event::{id}");
        assert_eq!(text.matches(&target).count(), 1, "{target}: {text}");
    }
    assert_eq!(text.lines().count(), 7, "{text}");
}

#[test]
fn nothing_is_written_once_shutdown_has_begun() {
    let (text, queries) = supervise_with_alerts(
        &[],
        true,
        |capture, parse, sink| {
            capture.dropped.store(9, Ordering::Relaxed);
            capture.etw_lost.store(9, Ordering::Relaxed);
            parse
                .queue_depth_interval_max
                .store(u64::MAX, Ordering::Relaxed);
            sink.connected.store(false, Ordering::Relaxed);
            sink.spool_available.store(false, Ordering::Relaxed);
        },
        almost_no_room,
        QUIET_FOR,
    );
    assert_eq!(text, "");
    assert_eq!(queries, 0, "no check ran");
}

#[test]
fn the_check_runs_at_the_stats_interval_or_the_rate_window_when_snapshots_are_off() {
    let tick_of = |arguments: &[&str]| {
        let config = config_from(arguments);
        let (capture, parse, sink) = (
            CaptureCounters::default(),
            ParseCounters::default(),
            SinkStats::default(),
        );
        Monitor::new(&config, &capture, &parse, &sink, Instant::now())
            .alerts_tick
            .interval
    };
    assert_eq!(tick_of(&[]), Some(Duration::from_secs(60)));
    assert_eq!(
        tick_of(&["--stats-interval-seconds", "15"]),
        Some(Duration::from_secs(15))
    );
    assert_eq!(
        tick_of(&["--stats-interval-seconds", "0"]),
        Some(DEFAULT_RATE_WINDOW)
    );
}

#[test]
fn the_check_still_runs_with_snapshots_off() {
    let (text, queries) = supervise_with_alerts(
        &["--stats-interval-seconds", "0"],
        false,
        |capture, _parse, _sink| capture.etw_lost.store(3, Ordering::Relaxed),
        plenty_of_room,
        QUIET_FOR,
    );
    assert_eq!(text.matches("etw_dnstap::event::10").count(), 1, "{text}");
    assert!(!text.contains("capture stats"), "{text}");
    assert!(queries >= 1, "{queries}");
}

#[test]
fn the_channel_risk_uses_the_deepest_sample_of_the_interval() {
    let (text, _queries) = supervise_with_alerts(
        &[],
        false,
        |_capture, parse, _sink| {
            // 80 percent of the default capacity of 131,072 events.
            parse
                .queue_depth_interval_max
                .store(104_858, Ordering::Relaxed);
        },
        plenty_of_room,
        QUIET_FOR,
    );
    // Raised on the first tick. The tick takes the peak, so the next one reads
    // 0 and clears the risk.
    assert_eq!(text.matches("etw_dnstap::event::20").count(), 1, "{text}");
    assert_eq!(
        text.matches("the event channel is no longer near full")
            .count(),
        1,
        "{text}"
    );
}

#[test]
fn the_spool_risk_reads_the_free_space_of_the_spool_volume() {
    let (text, _queries) = supervise_with_alerts(
        &[],
        false,
        |_capture, _parse, _sink| {},
        almost_no_room,
        QUIET_FOR,
    );
    assert_eq!(text.matches("etw_dnstap::event::25").count(), 1, "{text}");
    assert!(text.contains("free_mib=1"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}

#[test]
fn the_receiver_down_risk_is_written_once_when_nothing_takes_the_frames() {
    let (text, _queries) = supervise_with_alerts(
        &["--no-spool"],
        false,
        |_capture, _parse, sink| {
            sink.connected.store(false, Ordering::Relaxed);
            sink.spool_available.store(false, Ordering::Relaxed);
        },
        plenty_of_room,
        QUIET_FOR,
    );
    assert_eq!(text.matches("etw_dnstap::event::24").count(), 1, "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}

#[test]
fn no_spool_logs_the_disabled_warning_once_at_start() -> Result<()> {
    let mut run = None;
    let lines = capture_logs_at(tracing::Level::INFO, || {
        run = Some(run_with_production_sink(&["--no-spool"]));
    });
    run.expect("ran")?;
    let text = lines.text();
    assert_eq!(text.matches("etw_dnstap::event::26").count(), 1, "{text}");
    assert!(text.contains("--no-spool"), "{text}");
    assert!(text.contains("receiver is down"), "{text}");
    // Nothing else of the loss and risk kinds is written, at start or at stop.
    assert_eq!(text.matches("etw_dnstap::event::").count(), 1, "{text}");
    Ok(())
}

#[test]
fn a_spool_that_cannot_be_used_uses_the_spool_disabled_id() {
    let base = crate::spool::test_support::TestDir::new("disabled-id");
    let settings = SpoolSettings {
        dir: base.path().join("absent"),
        max_mib: 64,
        min_free_mib: 0,
    };
    let stats = Arc::new(SinkStats::default());
    let lines = capture_logs_at(tracing::Level::INFO, || {
        let _none = open_spool(
            &settings,
            &stats,
            fsinfo::check_spool_dir,
            fsinfo::available_bytes,
        );
    });
    assert_eq!(lines.count("etw_dnstap::event::26"), 1, "{}", lines.text());
}

fn stage_cpu_with_percent(percent_tenths: u64) -> StageCpu {
    // An interval of one second and the CPU time that makes the percentage.
    StageCpu {
        used_100ns: percent_tenths * 10_000,
        elapsed_100ns: 10_000_000,
        total_ms: 0,
    }
}

#[test]
fn the_periodic_reports_carry_the_stage_cpu_percentages() {
    let (capture, parser, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    let mut cpu = [StageCpu::default(); STAGE_COUNT];
    cpu[ETW_CONSUMER] = stage_cpu_with_percent(1_234);
    cpu[PARSER] = stage_cpu_with_percent(1_000);
    cpu[SENDER] = stage_cpu_with_percent(55);
    cpu[LINK] = stage_cpu_with_percent(0);
    let logs = capture_logs_at(tracing::Level::INFO, || {
        report_snapshot(&capture, &parser, &sink, &cpu);
    });
    let text = logs.text();
    let capture_line = text
        .lines()
        .find(|line| line.contains("capture stats"))
        .expect("a capture stats line");
    let output_line = text
        .lines()
        .find(|line| line.contains("output stats"))
        .expect("an output stats line");
    // One decimal, always: 123.4 percent of one core is not a rounding error.
    assert!(capture_line.contains("cpu_etw_pct=123.4"), "{capture_line}");
    assert!(
        capture_line.contains("cpu_parser_pct=100.0"),
        "{capture_line}"
    );
    assert!(output_line.contains("cpu_sender_pct=5.5"), "{output_line}");
    assert!(output_line.contains("cpu_link_pct=0.0"), "{output_line}");
    // Each stage appears on one line only.
    for field in [
        "cpu_etw_pct",
        "cpu_parser_pct",
        "cpu_sender_pct",
        "cpu_link_pct",
    ] {
        assert_eq!(logs.count(field), 1, "{field}: {text}");
    }
}

#[test]
fn the_stop_summaries_carry_no_stage_cpu() {
    let (capture, parser, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    let logs = capture_logs_at(tracing::Level::INFO, || {
        report_counts(&capture, &parser, &sink);
    });
    assert_eq!(logs.count("cpu_"), 0, "{}", logs.text());
}

#[test]
fn publishing_counters_carries_the_cpu_of_the_registered_threads() {
    let config = config_from(&[]);
    let (capture, parse, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut monitor = Monitor::new(&config, &capture, &parse, &sink, Instant::now());
    monitor.start_publishing(
        Box::new(RecordingSink {
            published: Arc::clone(&published),
        }),
        Instant::now(),
    );
    // The link stage burns CPU for 300 ms; the other stages have no thread.
    let link = thread::spawn(|| {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            std::hint::black_box(0u64);
        }
    });
    sink.cpu_link.register(&link);
    link.join().expect("the link stand-in ends");
    monitor.publish_counters(Instant::now());
    let values = *published.lock().expect("lock").last().expect("a snapshot");
    let link_cpu = values.stage_cpu[LINK];
    assert!(link_cpu.used_100ns > 1_000_000, "{link_cpu:?}");
    assert!(link_cpu.elapsed_100ns >= 3_000_000, "{link_cpu:?}");
    assert!(link_cpu.total_ms >= 100, "{link_cpu:?}");
    assert!(link_cpu.percent() > 30.0, "{link_cpu:?}");
    for stage in [ETW_CONSUMER, PARSER, SENDER] {
        assert_eq!(values.stage_cpu[stage].used_100ns, 0, "stage {stage}");
        assert_eq!(values.stage_cpu[stage].total_ms, 0, "stage {stage}");
    }

    // The next tick measures from this one: the ended thread uses no more CPU.
    monitor.publish_counters(Instant::now() + Duration::from_secs(1));
    let later = *published.lock().expect("lock").last().expect("a snapshot");
    assert_eq!(later.stage_cpu[LINK].used_100ns, 0);
    assert_eq!(later.stage_cpu[LINK].percent(), 0.0);
    assert_eq!(later.stage_cpu[LINK].total_ms, link_cpu.total_ms);
}

#[test]
fn loss_counted_before_supervision_starts_is_reported_by_the_first_check() {
    let config = config_from(&[]);
    let (capture, parse, sink) = (
        CaptureCounters::default(),
        ParseCounters::default(),
        SinkStats::default(),
    );
    capture.dropped.store(7, Ordering::Relaxed);
    let mut monitor = Monitor::new(&config, &capture, &parse, &sink, Instant::now());
    let lines = capture_logs_at(tracing::Level::INFO, || {
        monitor.check_alerts(Instant::now());
    });
    let text = lines.text();
    assert_eq!(text.matches("etw_dnstap::event::11").count(), 1, "{text}");
    assert_eq!(
        text.matches("etw_dnstap::event::28").count(),
        0,
        "a startup ETW error alone raises no risk: {text}"
    );
}
