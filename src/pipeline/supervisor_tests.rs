//! Schedules, the rate monitor, and the supervisor and pipeline runs against fakes.

use super::*;
use crate::{
    etw::{TargetChange, Win32Status},
    pipeline::{config::parse_args_from, test_support::config_from},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
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
        |address, _stats| crate::network::Connection::connect(address),
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

/// Collects formatted log lines for the current thread's subscriber.
#[derive(Clone, Default)]
struct LogLines(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for LogLines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogLines {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn logged_by_first_query(status: Result<(), Win32Status>) -> String {
    let lines = LogLines::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(lines.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let counters = Arc::new(CaptureCounters::default());
    let producer = QueryProducer {
        counters: Arc::clone(&counters),
        status,
    };
    tracing::subscriber::with_default(subscriber, || {
        query_and_log_buffers_applied(&producer, &counters);
    });
    let bytes = lines.0.lock().expect("log buffer").clone();
    String::from_utf8(bytes).expect("log text")
}

#[test]
fn applied_buffer_counts_are_logged_only_after_a_successful_query() {
    let text = logged_by_first_query(Ok(()));
    assert!(text.contains("ETW buffers applied"), "{text}");
    assert!(text.contains("etw_min_buffers_applied=12"), "{text}");
    assert!(text.contains("etw_max_buffers_applied=40"), "{text}");
    assert!(text.contains("etw_buffers_allocated=14"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");

    let text = logged_by_first_query(Err(Win32Status(5)));
    assert!(text.is_empty(), "{text}");
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
