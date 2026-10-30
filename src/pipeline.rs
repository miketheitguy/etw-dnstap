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

use crate::{
    dnstap::Encoder,
    etw::{self, Capture, CaptureCounters, PoolManager, RawEtwEvent},
    network::NetworkSink,
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Windows service for DNS Server ETW to a dnstap TCP receiver"
)]
pub(crate) struct CaptureArgs {
    /// Plaintext Frame Streams TCP receiver as an IP:PORT (trusted networks only).
    #[arg(long, value_parser = parse_receiver)]
    receiver: SocketAddr,

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

fn parse_capacity(value: &str) -> Result<usize, String> {
    let capacity: usize = value.parse().map_err(|_| "capacity must be an integer")?;
    if !(1..=16_384).contains(&capacity) {
        return Err("capacity must be between 1 and 16384 events".into());
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

#[derive(Default)]
struct ParseCounters {
    dequeued: AtomicU64,
    encoded: AtomicU64,
    ignored: AtomicU64,
    rejected: AtomicU64,
    output_failed_events: AtomicU64,
    output_errors: AtomicU64,
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

pub(crate) fn parse_args() -> CaptureArgs {
    CaptureArgs::parse()
}

pub(crate) fn run_capture(
    args: CaptureArgs,
    stop: &AtomicBool,
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
    let writer =
        NetworkSink::connect(args.receiver).context("connect/negotiation with dnstap receiver")?;
    let (sender, queue) = crossbeam_channel::bounded(args.capacity);
    let pool = PoolManager::new(args.capacity, usize::from(args.jumbo_capacity))?;
    let producer_done = Arc::new(AtomicBool::new(false));
    let capture_counts = Arc::new(CaptureCounters::default());
    let parse_counts = Arc::new(ParseCounters::default());

    let parser = {
        let queue = queue.clone();
        let producer_done = Arc::clone(&producer_done);
        let counters = Arc::clone(&parse_counts);
        thread::Builder::new()
            .name("dnstap-parser".into())
            .spawn(move || parse_loop(&queue, &producer_done, &counters, &args.identity, writer))
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
    let running_result = on_running();
    tracing::info!(
        session = session_name,
        receiver = %args.receiver,
        capacity = args.capacity,
        "capture started"
    );
    startup_watchdog.disarm()?;
    let started = Instant::now();
    let duration = args.duration.map(Duration::from_secs);
    let unexpected_exit = loop {
        if running_result.is_err() {
            break false;
        }
        if stop.load(Ordering::Relaxed) || duration.is_some_and(|limit| started.elapsed() >= limit)
        {
            break false;
        }
        if parser.is_finished() || capture.is_finished() {
            break true;
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
) -> Result<()> {
    let mut parser = etw::Parser::new();
    let mut encoder = Encoder::new(identity.as_bytes())?;
    let mut packet = Vec::new();
    let mut last_flush = Instant::now();
    drain_ring(queue, producer_done, |event| {
        if let Some(event) = event {
            counters.dequeued.fetch_add(1, Ordering::Relaxed);
            let result = parser.parse(event, &mut packet).and_then(|event| {
                event
                    .map(|event| encoder.encode(event, &mut packet))
                    .transpose()
                    .map_err(Into::into)
            });
            match result {
                Ok(Some(payload)) => {
                    writer
                        .write_frame(payload)
                        .context("send dnstap packet")
                        .inspect_err(|_error| {
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
            writer
                .flush()
                .context("flush dnstap TCP sender")
                .inspect_err(|_error| {
                    counters.output_errors.fetch_add(1, Ordering::Relaxed);
                })?;
            last_flush = Instant::now();
        }
        Ok(())
    })?;
    writer
        .finish()
        .context("finish dnstap STOP/FINISH exchange")
        .inspect_err(|_error| {
            counters.output_errors.fetch_add(1, Ordering::Relaxed);
        })
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
        "capture stopped"
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
            let result = parse_loop(&queue, &AtomicBool::new(true), &counters, "test", writer);
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
}
