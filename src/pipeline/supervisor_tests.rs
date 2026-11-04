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
    assert!(!default_rate_exceeded(false, 50_000.0));
}

#[test]
fn rate_warning_text_states_the_validated_rate_and_the_threshold() {
    assert!(RATE_WARNING.contains("validated at about 20,000 events/s"));
    assert!(RATE_WARNING.contains("above 22,000 events/s"));
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
    // The default pool (24,576 + 2 regular and 64 jumbo slots) crosses the
    // progress interval exactly once.
    assert_eq!(
        progress_reports,
        (config.capacity + 2 + usize::from(config.jumbo_capacity)) / PoolManager::PROGRESS_INTERVAL
    );
    assert_eq!(progress_reports, 1);
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
    assert_eq!(last.channel_capacity, 24_576);
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
    sink.bytes_sent.store(123_456_789, Ordering::Relaxed);
    sink.send_queue_depth_max.store(77, Ordering::Relaxed);
    sink.spool_oldest_age_ms.store(93_250, Ordering::Relaxed);
    sink.link_saturated.store(true, Ordering::Relaxed);
    let logs = capture_logs_at(tracing::Level::INFO, || {
        report_snapshot(&capture, &parser, &sink);
        report_counts(&capture, &parser, &sink);
    });
    let text = logs.text();
    for field in [
        "write_max_us=9000000",
        "write_max_recent_us=2500",
        "bytes_sent=123456789",
        "send_queue_depth_max=77",
        "spool_oldest_age_ms=93250",
        "link_saturated=true",
    ] {
        // Once on `output stats` and once on `output stopped`.
        assert_eq!(logs.count(field), 2, "{field}: {text}");
    }
}
