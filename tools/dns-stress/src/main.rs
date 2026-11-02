//! Pipelined laboratory DNS load generator; workers never share packet counters.

mod dns;
mod pacing;
mod stats;

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, ErrorKind, Write},
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, ValueEnum};
use dns::{Name, parse_response};
use pacing::Grid;
use serde::Serialize;
use stats::{Counts, Histogram, Latency};

/// Longest idle sleep, which bounds how late a response can be noticed.
const IDLE_POLL_CAP: Duration = Duration::from_millis(1);
/// Most sends in one loop pass, so expiry, receives, and metrics cannot starve.
const SEND_BATCH: usize = 64;
/// Most receives in one loop pass.
const RECEIVE_BATCH: usize = 256;
/// Query IDs per socket generation.
const ID_SPACE: usize = 65536;

#[derive(Clone, Copy, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Authoritative,
    Recursive,
}

#[derive(Clone, Debug, Parser, Serialize)]
#[command(version, about = "Pipelined UDP DNS laboratory load generator (A/IN)")]
struct Options {
    /// Explicit numeric server address and port; no default target.
    #[arg(long)]
    server: SocketAddr,
    /// UTF-8 file containing one ASCII FQDN per line; # starts a comment.
    #[arg(long)]
    names: PathBuf,
    /// Authoritative sends RD=0 and expects AA; recursive sends RD=1 and expects RA.
    #[arg(long, value_enum)]
    mode: Mode,
    /// Number of independent UDP worker threads (1..64).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=64))]
    threads: u16,
    /// Measured sending interval in seconds (1..3600), followed by timeout drain.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    duration: u64,
    /// Maximum outstanding requests per worker (1..8192).
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u16).range(1..=8192))]
    inflight: u16,
    /// Response deadline in milliseconds (1..60000).
    #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u64).range(1..=60000))]
    timeout_ms: u64,
    /// Optional global queries/second ceiling; evenly divided across workers.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=1_000_000_000))]
    qps: Option<u64>,
    /// Directory for newly created summary.json and metrics.jsonl.
    #[arg(long)]
    output_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Unused,
    Pending,
    Completed,
    TimedOut,
}

#[derive(Clone, Debug)]
struct Entry {
    name: usize,
    state: State,
    sent: Instant,
}

#[derive(Debug)]
struct Worker {
    id: u16,
    socket: UdpSocket,
    retired: Vec<UdpSocket>,
    entries: Vec<Entry>,
    deadlines: VecDeque<u16>,
    next_id: usize,
    outstanding: usize,
    counts: Counts,
    histogram: Histogram,
    rotation_wait_started: Option<Instant>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct Metric {
    schema_version: u8,
    worker: u16,
    elapsed_ns: u64,
    final_sample: bool,
    outstanding: usize,
    counts: Counts,
    latency: Latency,
}

#[derive(Debug, Serialize)]
struct WorkerSummary {
    worker: u16,
    counts: Counts,
    outstanding_final: usize,
    latency: Latency,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct Summary {
    schema_version: u8,
    start_unix_ms: u64,
    sending_end_unix_ms: u64,
    end_unix_ms: u64,
    options: Options,
    names_loaded: usize,
    measured_send_duration_ns: u64,
    elapsed_with_drain_ns: u64,
    load_model: &'static str,
    latency_scope: &'static str,
    peak_outstanding_semantics: &'static str,
    retired_socket_packets_unobserved: bool,
    counts: Counts,
    outstanding_final: usize,
    latency: Latency,
    latency_histogram_bins: Vec<u64>,
    workers: Vec<WorkerSummary>,
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn open_socket(server: SocketAddr) -> Result<UdpSocket> {
    let bind = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).context("bind UDP worker socket")?;
    socket
        .connect(server)
        .context("connect UDP worker socket")?;
    socket
        .set_nonblocking(true)
        .context("set UDP socket nonblocking")?;
    Ok(socket)
}

impl Worker {
    fn new(id: u16, options: &Options) -> Result<Self> {
        let now = Instant::now();
        Ok(Self {
            id,
            socket: open_socket(options.server)?,
            retired: Vec::new(),
            entries: vec![
                Entry {
                    name: 0,
                    state: State::Unused,
                    sent: now
                };
                ID_SPACE
            ],
            deadlines: VecDeque::with_capacity(ID_SPACE),
            next_id: 0,
            outstanding: 0,
            counts: Counts {
                sockets_used: 1,
                ..Counts::default()
            },
            histogram: Histogram::new(),
            rotation_wait_started: None,
            error: None,
        })
    }

    fn rotation_wait_us(&self, now: Instant, end: Instant) -> u64 {
        self.rotation_wait_started.map_or(0, |started| {
            u64::try_from(now.min(end).saturating_duration_since(started).as_micros())
                .unwrap_or(u64::MAX)
        })
    }

    fn finish_rotation_wait(&mut self, now: Instant, end: Instant) {
        self.counts.rotation_wait_us += self.rotation_wait_us(now, end);
        self.rotation_wait_started = None;
    }

    fn metric(&self, start: Instant, end: Instant, final_sample: bool) -> Metric {
        let now = Instant::now();
        let mut counts = self.counts.clone();
        counts.rotation_wait_us += self.rotation_wait_us(now, end);
        Metric {
            schema_version: 1,
            worker: self.id,
            elapsed_ns: nanos(now.saturating_duration_since(start)),
            final_sample,
            outstanding: self.outstanding,
            counts,
            latency: self.histogram.snapshot(),
        }
    }

    fn receive(
        &mut self,
        packet: &[u8],
        names: &[Name],
        mode: Mode,
        now: Instant,
        timeout: Duration,
    ) {
        self.counts.bytes_received += packet.len() as u64;
        let Ok(response) = parse_response(packet) else {
            self.counts.malformed += 1;
            return;
        };
        let entry = &mut self.entries[usize::from(response.id)];
        if entry.state == State::Unused {
            self.counts.unsolicited += 1;
            return;
        }
        if response.question[..response.question_len] != names[entry.name].wire {
            self.counts.mismatched += 1;
            return;
        }
        if entry.state == State::Pending && now.duration_since(entry.sent) >= timeout {
            entry.state = State::TimedOut;
            self.outstanding -= 1;
            self.counts.timeouts += 1;
        }
        match entry.state {
            State::Completed => {
                self.counts.duplicate += 1;
                return;
            }
            State::TimedOut => {
                self.counts.late += 1;
                return;
            }
            State::Unused => return,
            State::Pending => {}
        }
        entry.state = State::Completed;
        self.outstanding -= 1;
        self.counts.completed += 1;
        self.histogram.record(nanos(now.duration_since(entry.sent)));
        let rcode = usize::from(response.flags & 15);
        self.counts.rcode[rcode] += 1;
        let aa_missing = matches!(mode, Mode::Authoritative) && response.flags & 0x0400 == 0;
        let ra_missing = matches!(mode, Mode::Recursive) && response.flags & 0x0080 == 0;
        let rd_mismatch = (response.flags & 0x0100 != 0) != matches!(mode, Mode::Recursive);
        let truncated = response.flags & 0x0200 != 0;
        self.counts.aa_missing += u64::from(aa_missing);
        self.counts.ra_missing += u64::from(ra_missing);
        self.counts.rd_mismatch += u64::from(rd_mismatch);
        self.counts.truncated += u64::from(truncated);
        self.counts.noerror_without_answers += u64::from(rcode == 0 && response.answers == 0);
        self.counts.successful += u64::from(
            rcode == 0
                && response.answers > 0
                && !aa_missing
                && !ra_missing
                && !rd_mismatch
                && !truncated,
        );
    }

    fn expire(&mut self, now: Instant, timeout: Duration) {
        while let Some(id) = self.deadlines.front().copied() {
            let entry = &mut self.entries[usize::from(id)];
            if entry.state == State::Pending {
                if now.duration_since(entry.sent) < timeout {
                    break;
                }
                entry.state = State::TimedOut;
                self.outstanding -= 1;
                self.counts.timeouts += 1;
            }
            self.deadlines.pop_front();
        }
    }

    fn rotate(&mut self, options: &Options) -> Result<()> {
        ensure!(self.outstanding == 0, "rotation with outstanding requests");
        ensure!(
            self.counts.sockets_used < 8192 / usize::from(options.threads),
            "socket safety budget exhausted; refusing endpoint/ID reuse"
        );
        // Keep every previous endpoint bound for the whole run. A delayed packet
        // therefore cannot be assigned to a new generation of the same ID.
        let replacement = open_socket(options.server)?;
        self.retired
            .push(std::mem::replace(&mut self.socket, replacement));
        self.entries
            .iter_mut()
            .for_each(|entry| entry.state = State::Unused);
        self.deadlines.clear();
        self.next_id = 0;
        self.counts.sockets_used += 1;
        self.counts.rotation_count += 1;
        Ok(())
    }

    /// Sleeps instead of spinning, so pacing does not steal CPU from a DNS Server
    /// sharing the host. The wake-up is the next send (when one is possible) or
    /// the deadline, capped so responses are still polled promptly.
    fn idle(&self, options: &Options, next_send: Instant, deadline: Instant) {
        let can_send = self.send_room(options) != 0;
        let wake = if can_send {
            next_send.min(deadline)
        } else {
            deadline
        };
        thread::sleep(
            wake.saturating_duration_since(Instant::now())
                .min(IDLE_POLL_CAP),
        );
    }

    /// Sends that the in-flight window and the ID space permit, at most one batch.
    fn send_room(&self, options: &Options) -> usize {
        SEND_BATCH
            .min(usize::from(options.inflight).saturating_sub(self.outstanding))
            .min(ID_SPACE - self.next_id)
    }

    /// Sends `names[name]` with the next query ID. Returns false when the
    /// socket did not accept the complete datagram; the caller ends the batch.
    fn send_next(&mut self, query: &mut Vec<u8>, names: &[Name], name: usize) -> Result<bool> {
        let id = u16::try_from(self.next_id)?;
        query.clear();
        query.extend_from_slice(&names[name].query);
        query[..2].copy_from_slice(&id.to_be_bytes());
        let sent_at = Instant::now();
        match self.socket.send(query) {
            Ok(size) if size == query.len() => {
                self.entries[self.next_id] = Entry {
                    name,
                    state: State::Pending,
                    sent: sent_at,
                };
                self.deadlines.push_back(id);
                self.next_id += 1;
                if self.next_id == ID_SPACE {
                    self.rotation_wait_started = Some(Instant::now());
                }
                self.outstanding += 1;
                self.counts.sent += 1;
                self.counts.bytes_sent += size as u64;
                self.counts.peak_outstanding = self.counts.peak_outstanding.max(self.outstanding);
                Ok(true)
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(false),
            Ok(_) | Err(_) => {
                self.counts.send_errors += 1;
                Ok(false)
            }
        }
    }

    // Send/receive batches bound each loop so expiry and measurement cannot starve.
    fn run_loop(
        &mut self,
        options: &Options,
        names: &[Name],
        start: Instant,
        sender: &SyncSender<Metric>,
    ) -> Result<()> {
        let end = start + Duration::from_secs(options.duration);
        let timeout = Duration::from_millis(options.timeout_ms);
        // Paced workers send on an absolute grid from the shared start. A late
        // wake-up sends every due slot, so sleep overshoot does not lower the rate.
        let grid = options.qps.map(|qps| Grid::new(options.threads, qps));
        let mut next_slot = 0_u64;
        let mut next_metric = start + Duration::from_secs(1);
        let mut next_name = usize::from(self.id) % names.len();
        let mut query = Vec::with_capacity(271);
        let mut buffer = vec![0; 65535];
        thread::sleep(start.saturating_duration_since(Instant::now()));
        loop {
            let now = Instant::now();
            self.expire(now, timeout);
            if self.outstanding == 0 || now >= end {
                self.finish_rotation_wait(now, end);
            }
            if now >= end && self.outstanding == 0 {
                break;
            }
            if now >= next_metric {
                sender
                    .send(self.metric(start, end, false))
                    .context("metrics writer stopped")?;
                next_metric += Duration::from_secs(1);
            }
            if self.next_id == ID_SPACE && self.outstanding == 0 && now < end {
                self.rotate(options)?;
            }
            let now = Instant::now();
            let room = if now < end {
                self.send_room(options)
            } else {
                0
            };
            let batch = match grid {
                Some(grid) => {
                    let due = pacing::due(
                        grid,
                        nanos(now.saturating_duration_since(start)),
                        next_slot,
                        room,
                    );
                    next_slot = due.slot;
                    self.counts.pacing_slots_skipped += due.skipped;
                    due.count
                }
                None => room,
            };
            let mut progress = false;
            for _ in 0..batch {
                if Instant::now() >= end {
                    break;
                }
                if !self.send_next(&mut query, names, next_name)? {
                    break;
                }
                next_name = (next_name + 1) % names.len();
                // Advance on the grid, not from the send time.
                next_slot += 1;
                progress = true;
            }
            for _ in 0..RECEIVE_BATCH {
                match self.socket.recv(&mut buffer) {
                    Ok(size) => {
                        self.receive(
                            &buffer[..size],
                            names,
                            options.mode,
                            Instant::now(),
                            timeout,
                        );
                        progress = true;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        self.counts.receive_errors += 1;
                        break;
                    }
                }
            }
            if !progress {
                let next_send = grid.map_or(start, |grid| {
                    start
                        .checked_add(Duration::from_nanos(grid.slot_offset_ns(next_slot)))
                        .unwrap_or(end)
                });
                self.idle(options, next_send, next_metric.min(end));
            }
        }
        Ok(())
    }
}

fn load_names(options: &Options) -> Result<Vec<Name>> {
    let file = File::open(&options.names).context("open names file")?;
    ensure!(
        file.metadata()?.len() <= 16 * 1024 * 1024,
        "names file exceeds 16 MiB"
    );
    let mut names = Vec::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let name = line.split('#').next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        names.push(
            Name::parse(name, matches!(options.mode, Mode::Recursive))
                .with_context(|| format!("names line {}", line_number + 1))?,
        );
        ensure!(names.len() <= 100_000, "names file exceeds 100000 entries");
    }
    ensure!(!names.is_empty(), "names file contains no names");
    Ok(names)
}

fn create_output(path: PathBuf) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .context("create output file (existing results are never overwritten)")?,
    ))
}

// This orchestration keeps the channel drain before joins explicit to avoid a
// bounded-channel shutdown deadlock; packet paths remain separate methods.
#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        options.server.port() != 0
            && !options.server.ip().is_unspecified()
            && !options.server.ip().is_multicast(),
        "server must be a unicast address with nonzero port"
    );
    let names = Arc::new(load_names(&options)?);
    let mut workers = (0..options.threads)
        .map(|id| Worker::new(id, &options))
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir_all(&options.output_dir).context("create output directory")?;
    let mut summary_file = create_output(options.output_dir.join("summary.json"))?;
    let mut metrics_file = create_output(options.output_dir.join("metrics.jsonl"))?;
    let start = Instant::now() + Duration::from_millis(250);
    let start_unix_ms = u64::try_from(
        (SystemTime::now() + start.duration_since(Instant::now()))
            .duration_since(UNIX_EPOCH)?
            .as_millis(),
    )?;
    let (sender, receiver) = mpsc::sync_channel(usize::from(options.threads) * 4);
    let mut handles = Vec::new();
    for mut worker in workers.drain(..) {
        let names = Arc::clone(&names);
        let options = options.clone();
        let sender = sender.clone();
        handles.push(
            thread::Builder::new()
                .name(format!("dns-load-{}", worker.id))
                .spawn(move || {
                    if let Err(error) = worker.run_loop(&options, &names, start, &sender) {
                        worker.error = Some(format!("{error:#}"));
                    }
                    let end = start + Duration::from_secs(options.duration);
                    worker.finish_rotation_wait(Instant::now(), end);
                    // A failed run preserves pending counts rather than fabricating timeouts.
                    let _ = sender.send(worker.metric(start, end, true));
                    worker
                })
                .context("spawn DNS worker")?,
        );
    }
    drop(sender);
    let mut write_error = None;
    for metric in receiver {
        if write_error.is_none() {
            let result = serde_json::to_writer(&mut metrics_file, &metric)
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    metrics_file.write_all(b"\n")?;
                    metrics_file.flush()?;
                    Ok(())
                });
            if let Err(error) = result {
                write_error = Some(error);
            }
        }
    }
    let mut counts = Counts::default();
    let mut histogram = Histogram::new();
    let mut results = Vec::new();
    let mut outstanding_final = 0;
    for handle in handles {
        let worker = handle
            .join()
            .map_err(|_| anyhow::anyhow!("DNS worker panicked"))?;
        counts.merge(&worker.counts);
        histogram.merge(&worker.histogram);
        outstanding_final += worker.outstanding;
        results.push(WorkerSummary {
            worker: worker.id,
            counts: worker.counts,
            outstanding_final: worker.outstanding,
            latency: worker.histogram.snapshot(),
            error: worker.error,
        });
    }
    let failed = results.iter().any(|worker| worker.error.is_some());
    let summary = Summary {
        schema_version: 1,
        start_unix_ms,
        sending_end_unix_ms: start_unix_ms + options.duration * 1000,
        end_unix_ms: u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?,
        names_loaded: names.len(),
        measured_send_duration_ns: options.duration * 1_000_000_000,
        elapsed_with_drain_ns: nanos(start.elapsed()),
        load_model: if options.qps.is_some() {
            "paced closed loop; absolute schedule; catch-up of at most 100 ms backlog"
        } else {
            "unpaced bounded closed loop"
        },
        latency_scope: "matched responses only, send syscall start to application receive; no coordinated-omission correction; timeouts excluded",
        peak_outstanding_semantics: "sum of per-worker peaks, upper bound on simultaneous peak",
        retired_socket_packets_unobserved: counts.sockets_used > usize::from(options.threads),
        counts,
        outstanding_final,
        latency: histogram.snapshot(),
        latency_histogram_bins: histogram.bins,
        workers: results,
        options,
    };
    serde_json::to_writer_pretty(&mut summary_file, &summary)?;
    summary_file.write_all(b"\n")?;
    summary_file.flush()?;
    println!(
        "sent={} completed={} successful={} timeouts={} outstanding={} summary={}",
        summary.counts.sent,
        summary.counts.completed,
        summary.counts.successful,
        summary.counts.timeouts,
        summary.outstanding_final,
        summary.options.output_dir.join("summary.json").display()
    );
    if let Some(error) = write_error {
        return Err(error.context("write metrics"));
    }
    if failed {
        bail!("one or more workers failed; inspect summary.json");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(server: SocketAddr) -> Options {
        Options {
            server,
            names: PathBuf::new(),
            mode: Mode::Authoritative,
            threads: 1,
            duration: 1,
            inflight: 8,
            timeout_ms: 2000,
            qps: Some(8),
            output_dir: PathBuf::new(),
        }
    }

    fn answer(name: &Name, id: u16) -> Vec<u8> {
        let mut packet = name.query.clone();
        packet[..2].copy_from_slice(&id.to_be_bytes());
        packet[2] |= 0x84;
        packet[7] = 1;
        packet.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 192, 0, 2, 53]);
        packet
    }

    #[test]
    fn matching_late_duplicates_and_timeout_are_distinct() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0")?;
        let options = options(server.local_addr()?);
        let names = vec![
            Name::parse("a.example", false)?,
            Name::parse("b.example", false)?,
        ];
        let mut worker = Worker::new(0, &options)?;
        let now = Instant::now();
        let timeout = Duration::from_millis(10);
        worker.entries[7] = Entry {
            name: 0,
            state: State::Pending,
            sent: now,
        };
        worker.outstanding = 1;
        worker.receive(
            &answer(&names[1], 7),
            &names,
            Mode::Authoritative,
            now,
            timeout,
        );
        assert_eq!(worker.outstanding, 1);
        assert_eq!(worker.counts.mismatched, 1);
        worker.receive(
            &answer(&names[0], 7),
            &names,
            Mode::Authoritative,
            now,
            timeout,
        );
        worker.receive(
            &answer(&names[0], 7),
            &names,
            Mode::Authoritative,
            now,
            timeout,
        );
        assert_eq!(worker.counts.completed, 1);
        assert_eq!(worker.counts.successful, 1);
        assert_eq!(worker.counts.duplicate, 1);
        worker.entries[8] = Entry {
            name: 0,
            state: State::Pending,
            sent: now,
        };
        worker.outstanding = 1;
        worker.receive(
            &answer(&names[0], 8),
            &names,
            Mode::Authoritative,
            now + timeout,
            timeout,
        );
        assert_eq!(worker.counts.timeouts, 1);
        assert_eq!(worker.counts.late, 1);
        assert_eq!(worker.outstanding, 0);
        worker.receive(
            &answer(&names[0], 9),
            &names,
            Mode::Authoritative,
            now,
            timeout,
        );
        assert_eq!(worker.counts.unsolicited, 1);
        Ok(())
    }

    #[test]
    fn wrap_retains_old_endpoint_and_resets_id_states() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0")?;
        let options = options(server.local_addr()?);
        let mut worker = Worker::new(0, &options)?;
        let old_endpoint = worker.socket.local_addr()?;
        worker.next_id = ID_SPACE;
        worker.entries[0].state = State::Completed;
        worker.rotate(&options)?;
        assert_eq!(worker.next_id, 0);
        assert_eq!(worker.entries[0].state, State::Unused);
        assert_eq!(worker.retired[0].local_addr()?, old_endpoint);
        assert_ne!(worker.socket.local_addr()?, old_endpoint);
        assert_eq!(worker.counts.sockets_used, 2);
        assert_eq!(worker.counts.rotation_count, 1);
        worker.outstanding = 1;
        assert!(worker.rotate(&options).is_err());
        Ok(())
    }

    #[test]
    fn rotation_wait_counts_active_window_and_merges() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0")?;
        let mut worker = Worker::new(0, &options(server.local_addr()?))?;
        let started = Instant::now();
        let end = started + Duration::from_millis(50);
        worker.rotation_wait_started = Some(started);
        assert_eq!(
            worker.rotation_wait_us(started + Duration::from_millis(20), end),
            20_000
        );
        worker.finish_rotation_wait(started + Duration::from_millis(100), end);
        assert_eq!(worker.counts.rotation_wait_us, 50_000);
        assert!(worker.rotation_wait_started.is_none());
        worker.finish_rotation_wait(started + Duration::from_secs(1), end);
        assert_eq!(worker.counts.rotation_wait_us, 50_000);
        let mut merged = Counts::default();
        worker.counts.rotation_count = 2;
        merged.merge(&worker.counts);
        merged.merge(&worker.counts);
        assert_eq!(merged.rotation_count, 4);
        assert_eq!(merged.rotation_wait_us, 100_000);
        Ok(())
    }

    #[test]
    fn local_mock_pipelines_and_matches_out_of_order_responses() -> Result<()> {
        let server = UdpSocket::bind("127.0.0.1:0")?;
        server.set_read_timeout(Some(Duration::from_secs(3)))?;
        let options = options(server.local_addr()?);
        let mock = thread::spawn(move || -> Result<()> {
            let mut requests = Vec::new();
            let mut buffer = [0; 512];
            // No response until all eight are outstanding: a sequential client
            // would time out here. Return in reverse order on the real socket.
            for _ in 0..8 {
                let (size, peer) = server.recv_from(&mut buffer)?;
                let mut packet = buffer[..size].to_vec();
                packet[2] |= 0x84;
                requests.push((packet, peer));
            }
            for (packet, peer) in requests.into_iter().rev() {
                server.send_to(&packet, peer)?;
            }
            Ok(())
        });
        let names = vec![Name::parse("pipeline.example", false)?];
        let mut worker = Worker::new(0, &options)?;
        let (sender, _receiver) = mpsc::sync_channel(8);
        worker.run_loop(&options, &names, Instant::now(), &sender)?;
        mock.join()
            .map_err(|_| anyhow::anyhow!("mock panicked"))??;
        assert_eq!(worker.counts.sent, 8);
        assert_eq!(worker.counts.completed, 8);
        assert_eq!(worker.counts.peak_outstanding, 8);
        assert_eq!(worker.counts.timeouts, 0);
        assert_eq!(worker.outstanding, 0);
        Ok(())
    }

    #[test]
    fn paced_run_recovers_sleep_overshoot() -> Result<()> {
        // 5,000 QPS on one worker is one slot every 200 us, shorter than the
        // usual idle sleep. A schedule relative to each send would lose the
        // overshoot of every sleep; the absolute grid sends each due slot.
        let server = UdpSocket::bind("127.0.0.1:0")?;
        let mut options = options(server.local_addr()?);
        options.qps = Some(5_000);
        options.inflight = 8192;
        options.timeout_ms = 1;
        let names = vec![Name::parse("paced.example", false)?];
        let mut worker = Worker::new(0, &options)?;
        let (sender, _receiver) = mpsc::sync_channel(8);
        worker.run_loop(&options, &names, Instant::now(), &sender)?;
        // Slots 0 through 4,999 are due before the one-second deadline. Allow
        // for a host stall that ends after the deadline.
        assert!(worker.counts.sent <= 5_000, "sent {}", worker.counts.sent);
        assert!(worker.counts.sent >= 4_750, "sent {}", worker.counts.sent);
        assert_eq!(worker.counts.sent, worker.counts.timeouts);
        assert_eq!(worker.counts.send_errors, 0);
        Ok(())
    }

    #[test]
    fn connected_socket_filters_unrelated_sender() -> Result<()> {
        let legitimate = UdpSocket::bind("127.0.0.1:0")?;
        let unrelated = UdpSocket::bind("127.0.0.1:0")?;
        let worker = Worker::new(0, &options(legitimate.local_addr()?))?;
        let mut packet = [0; 1];
        unrelated.send_to(&[1], worker.socket.local_addr()?)?;
        // Give the loopback packet time to arrive, then verify it cannot appear
        // as a response from the connected server endpoint.
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            worker
                .socket
                .recv(&mut packet)
                .err()
                .map(|error| error.kind()),
            Some(ErrorKind::WouldBlock)
        );
        Ok(())
    }
}
