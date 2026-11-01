//! Bidirectional Frame Streams TCP sender with whole-operation I/O deadlines and
//! automatic reconnection.
//!
//! [`Connection`] is one Frame Streams session. Any error on it is final for that
//! session. [`NetworkSink`] owns the current session and, when it fails,
//! reconnects with bounded exponential backoff and a fresh handshake while ETW
//! capture keeps running. Frames that arrive while disconnected, and frames that
//! were still buffered when the connection failed, are discarded and counted in
//! `disconnected_dropped`; they are never resent, so the receiver sees each frame
//! at most once and never a duplicate.

use std::{
    io::{self, BufWriter, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    framestream::{CONTENT_TYPE, FrameStreamWriter},
    sink::{FrameSink, SinkStats},
};

#[allow(unsafe_code)]
mod sockopt;

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_ACCEPT: u32 = 1;
const CONTROL_READY: u32 = 4;
const CONTROL_FINISH: u32 = 5;
const CONTENT_TYPE_FIELD: u32 = 1;
const MAX_CONTROL_LENGTH: usize = 512;
// Holds many typical frames per send; larger frames bypass the buffer.
const WRITE_BUFFER_BYTES: usize = 64 * 1024;
/// Kernel send buffer, so a briefly slow receiver stalls the parser less.
const SEND_BUFFER_BYTES: u32 = 1024 * 1024;
/// A socket write at least this long counts as blocked.
const SLOW_WRITE: Duration = Duration::from_millis(5);

/// One Frame Streams session over TCP. Any error ends the session; the caller
/// decides whether to reconnect.
pub(crate) struct Connection {
    writer: FrameStreamWriter<BufWriter<DeadlineTcp>>,
}

impl Connection {
    pub(crate) fn connect(address: SocketAddr) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
        stream.set_nodelay(true)?;
        if let Err(error) = sockopt::set_send_buffer(&stream, SEND_BUFFER_BYTES) {
            // A smaller buffer only costs some slack; it is never a reason to refuse.
            tracing::debug!(%error, "could not raise SO_SNDBUF");
        }
        let mut transport = DeadlineTcp::new(stream, IO_TIMEOUT);
        write_control(&mut transport, CONTROL_READY, true)?;
        read_control(&mut transport, CONTROL_ACCEPT, true)?;
        let mut writer =
            FrameStreamWriter::new(BufWriter::with_capacity(WRITE_BUFFER_BYTES, transport))?;
        // Publish START before the service reports readiness, using the same
        // total handshake deadline as READY and ACCEPT.
        writer.flush()?;
        Ok(Self { writer })
    }

    /// Starts timing socket writes into `stats`.
    fn attach_stats(&mut self, stats: Arc<SinkStats>) {
        self.writer.get_mut().get_mut().stats = Some(stats);
    }

    /// The next socket operation starts a new deadline window; the clock is read
    /// only if that operation actually reaches the socket.
    fn reset_deadline(&mut self) {
        self.writer.get_mut().get_mut().rearm();
    }
}

impl FrameSink for Connection {
    fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        self.reset_deadline();
        self.writer.write_frame(payload)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.reset_deadline();
        self.writer.flush()
    }

    fn finish(mut self) -> Result<()> {
        self.reset_deadline();
        finish_stream(self.writer)
    }
}

/// Reconnect delay policy: exponential from `initial` to `max`, with jitter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BackoffPolicy {
    pub(crate) initial: Duration,
    pub(crate) max: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(250),
            max: Duration::from_secs(30),
        }
    }
}

/// Backoff state. The delay doubles after each failure up to `max`, and resets
/// after a success.
#[derive(Debug)]
struct Backoff {
    policy: BackoffPolicy,
    current: Duration,
    random: u64,
}

impl Backoff {
    fn new(policy: BackoffPolicy, seed: u64) -> Self {
        Self {
            policy,
            current: policy.initial,
            // Scramble the seed (splitmix64) so nearby seeds diverge at once;
            // xorshift must not start at zero.
            random: splitmix64(seed) | 1,
        }
    }

    fn next_random(&mut self) -> u32 {
        let mut x = self.random;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.random = x;
        u32::try_from(x >> 32).unwrap_or(u32::MAX)
    }

    /// The delay to wait now, then doubles the base. "Equal jitter": the result
    /// is uniform in `[base/2, base]`, so many instances do not retry in step.
    fn next_delay(&mut self) -> Duration {
        let random = self.next_random();
        let delay = jittered(self.current, random);
        self.current = self.current.saturating_mul(2).min(self.policy.max);
        delay
    }

    fn reset(&mut self) {
        self.current = self.policy.initial;
    }
}

/// `base/2 + base/2 * fraction`, where `fraction = random / 2^32`.
fn jittered(base: Duration, random: u32) -> Duration {
    let half = base / 2;
    half + half.mul_f64(f64::from(random) / 4_294_967_296.0)
}

fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

enum State {
    Connected(Connection),
    Disconnected {
        /// No attempt starts before this instant.
        next_attempt: Instant,
        /// The in-flight connect-and-handshake, run off the parser thread so a
        /// slow or blackholed receiver never stalls draining.
        attempt: Option<Receiver<io::Result<Connection>>>,
    },
}

/// Plaintext sender for an explicitly trusted receiver that survives receiver
/// restarts. See the module comment for the loss and no-resend rules.
///
/// Startup: the first connection is attempted synchronously. A network failure
/// (refused, timed out, reset) does not stop the service; it starts
/// disconnected and retries. A protocol failure (the receiver answers but does
/// not accept the dnstap content type) is a configuration error and is fatal.
pub(crate) struct NetworkSink {
    address: SocketAddr,
    stats: Arc<SinkStats>,
    state: State,
    backoff: Backoff,
    /// Frames accepted since the last successful flush. They may sit in the
    /// write buffer, so a failure counts them as dropped.
    unflushed_frames: u64,
}

fn clock_seed(address: SocketAddr) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or(0) ^ (u64::from(address.port()) << 40)
}

impl NetworkSink {
    /// Connects, or starts disconnected on a network failure.
    ///
    /// # Errors
    /// Returns the error of a protocol failure during the first handshake.
    pub(crate) fn start(address: SocketAddr, stats: Arc<SinkStats>) -> io::Result<Self> {
        Self::start_with(address, stats, BackoffPolicy::default())
    }

    pub(crate) fn start_with(
        address: SocketAddr,
        stats: Arc<SinkStats>,
        policy: BackoffPolicy,
    ) -> io::Result<Self> {
        let mut backoff = Backoff::new(policy, clock_seed(address));
        let initial = match Connection::connect(address) {
            Ok(mut connection) => {
                connection.attach_stats(Arc::clone(&stats));
                stats.connected.store(true, Ordering::Relaxed);
                State::Connected(connection)
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => return Err(error),
            Err(error) => {
                tracing::warn!(
                    receiver = %address,
                    %error,
                    "dnstap receiver unavailable at startup; capture is running, events are \
                     dropped and counted until it connects (retrying with backoff)"
                );
                State::Disconnected {
                    next_attempt: Instant::now() + backoff.next_delay(),
                    attempt: None,
                }
            }
        };
        Ok(Self {
            address,
            stats,
            state: initial,
            backoff,
            unflushed_frames: 0,
        })
    }

    fn drop_frames(&self, count: u64) {
        self.stats
            .disconnected_dropped
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Abandons the failed connection. The write buffer is discarded with it, so
    /// nothing from the failed stream is ever resent.
    fn connection_lost(&mut self, error: &io::Error, current_frame: u64) {
        self.stats.connected.store(false, Ordering::Relaxed);
        self.stats.disconnects.fetch_add(1, Ordering::Relaxed);
        self.drop_frames(self.unflushed_frames + current_frame);
        self.unflushed_frames = 0;
        let delay = self.backoff.next_delay();
        tracing::warn!(
            receiver = %self.address,
            %error,
            retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "dnstap receiver connection lost; events are dropped and counted until it \
             reconnects (buffered frames are discarded, never resent)"
        );
        self.state = State::Disconnected {
            next_attempt: Instant::now() + delay,
            attempt: None,
        };
    }

    /// Advances a disconnected sink: collects a finished attempt or starts one
    /// when the backoff has elapsed. Never blocks.
    fn poll_reconnect(&mut self, now: Instant) {
        let State::Disconnected {
            next_attempt,
            attempt,
        } = &mut self.state
        else {
            return;
        };
        if let Some(receiver) = attempt {
            match receiver.try_recv() {
                Ok(Ok(mut connection)) => {
                    connection.attach_stats(Arc::clone(&self.stats));
                    self.stats.connected.store(true, Ordering::Relaxed);
                    self.stats.reconnects.fetch_add(1, Ordering::Relaxed);
                    self.backoff.reset();
                    self.unflushed_frames = 0;
                    tracing::info!(receiver = %self.address, "dnstap receiver reconnected");
                    self.state = State::Connected(connection);
                }
                Ok(Err(error)) => {
                    let failures = self
                        .stats
                        .reconnect_failures
                        .fetch_add(1, Ordering::Relaxed);
                    // Bounded diagnostics; every failure stays counted.
                    if failures < 4 || failures.is_multiple_of(60) {
                        tracing::warn!(receiver = %self.address, %error, "dnstap reconnect failed");
                    }
                    *next_attempt = now + self.backoff.next_delay();
                    *attempt = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.stats
                        .reconnect_failures
                        .fetch_add(1, Ordering::Relaxed);
                    *next_attempt = now + self.backoff.next_delay();
                    *attempt = None;
                }
            }
            return;
        }
        if now < *next_attempt {
            return;
        }
        let (send, receive) = mpsc::channel();
        let address = self.address;
        let spawned = thread::Builder::new()
            .name("dnstap-reconnect".into())
            .spawn(move || {
                // The sink may be gone by the time this finishes; ignore that.
                let _ = send.send(Connection::connect(address));
            });
        if spawned.is_ok() {
            *attempt = Some(receive);
        } else {
            self.stats
                .reconnect_failures
                .fetch_add(1, Ordering::Relaxed);
            *next_attempt = now + self.backoff.next_delay();
        }
    }

    fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected(_))
    }
}

impl FrameSink for NetworkSink {
    fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        if !self.is_connected() {
            self.poll_reconnect(Instant::now());
        }
        let State::Connected(connection) = &mut self.state else {
            self.drop_frames(1);
            return Ok(());
        };
        match connection.write_frame(payload) {
            Ok(()) => self.unflushed_frames += 1,
            Err(error) => self.connection_lost(&error, 1),
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.is_connected() {
            self.poll_reconnect(Instant::now());
        }
        if let State::Connected(connection) = &mut self.state {
            match connection.flush() {
                Ok(()) => self.unflushed_frames = 0,
                Err(error) => self.connection_lost(&error, 0),
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<()> {
        match self.state {
            State::Connected(connection) => {
                let unflushed = self.unflushed_frames;
                connection.finish().inspect_err(|_error| {
                    self.stats
                        .disconnected_dropped
                        .fetch_add(unflushed, Ordering::Relaxed);
                })
            }
            // Nothing to flush and no one to tell; an in-flight attempt is
            // abandoned and its thread ends on its own within the I/O timeouts.
            State::Disconnected { .. } => {
                tracing::warn!(
                    receiver = %self.address,
                    "shutting down while the dnstap receiver is disconnected; no STOP sent"
                );
                Ok(())
            }
        }
    }
}

fn finish_stream<W: Read + Write>(mut writer: FrameStreamWriter<BufWriter<W>>) -> Result<()> {
    // Flush the last data frame separately so a failure is not reported as a
    // STOP or FINISH failure. NetworkSink resets the shared deadline once
    // before entering this helper; these phases do not extend it.
    writer.flush().context("final buffered data flush")?;
    let buffered = writer.finish().context("STOP write/flush")?;
    let mut transport = buffered
        .into_inner()
        .map_err(io::IntoInnerError::into_error)
        .context("transport extraction")?;
    read_finish(&mut transport)
}

fn read_finish(reader: &mut impl Read) -> Result<()> {
    let mut storage = [0; MAX_CONTROL_LENGTH];
    let frame = read_control_frame(reader, &mut storage).map_err(|(phase, error)| {
        let context = match phase {
            ControlPhase::HeaderRead => "FINISH header read",
            ControlPhase::BodyRead => "FINISH body read",
            ControlPhase::Validation => "FINISH validation",
        };
        anyhow::Error::new(error).context(context)
    })?;
    validate_control(frame, CONTROL_FINISH, false).context("FINISH validation")?;
    Ok(())
}

#[cfg(test)]
#[path = "network/test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "network/shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
#[path = "network/reconnect_tests.rs"]
mod reconnect_tests;

#[cfg(test)]
#[path = "network/handshake_tests.rs"]
mod handshake_tests;

#[cfg(test)]
#[path = "network/backpressure_tests.rs"]
mod backpressure_tests;

struct DeadlineTcp {
    stream: TcpStream,
    deadline: Instant,
    timeout: Duration,
    /// False until the first I/O of an operation, which then starts the clock.
    armed: bool,
    stats: Option<Arc<SinkStats>>,
}

impl DeadlineTcp {
    fn new(stream: TcpStream, timeout: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + timeout,
            timeout,
            armed: true,
            stats: None,
        }
    }

    /// Starts a new operation: the deadline restarts at its first socket call.
    fn rearm(&mut self) {
        self.armed = false;
    }

    fn remaining(&mut self) -> io::Result<Duration> {
        if !self.armed {
            self.deadline = Instant::now() + self.timeout;
            self.armed = true;
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Frame Streams I/O deadline"))
    }

    fn record_write(&self, elapsed: Duration) {
        if let Some(stats) = &self.stats {
            let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
            stats.write_max_us.fetch_max(micros, Ordering::Relaxed);
            if elapsed >= SLOW_WRITE {
                stats.slow_writes.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Read for DeadlineTcp {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self.remaining()?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}

impl Write for DeadlineTcp {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.remaining()?;
        self.stream.set_write_timeout(Some(remaining))?;
        let started = Instant::now();
        let written = self.stream.write(bytes);
        self.record_write(started.elapsed());
        written
    }

    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.stream.flush()
    }
}

fn invalid_control(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn write_control(writer: &mut impl Write, kind: u32, content_type: bool) -> io::Result<()> {
    let content_length = u32::try_from(CONTENT_TYPE.len())
        .map_err(|_| invalid_control("content type length exceeds u32"))?;
    let length = if content_type { 12 + content_length } else { 4 };
    writer.write_all(&0_u32.to_be_bytes())?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&kind.to_be_bytes())?;
    if content_type {
        writer.write_all(&CONTENT_TYPE_FIELD.to_be_bytes())?;
        writer.write_all(&content_length.to_be_bytes())?;
        writer.write_all(CONTENT_TYPE)?;
    }
    writer.flush()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlPhase {
    HeaderRead,
    BodyRead,
    Validation,
}

/// Reads one bounded control frame into `storage` and returns its payload.
/// Field validation is left to `validate_control`; this only checks framing.
fn read_control_frame<'a>(
    reader: &mut impl Read,
    storage: &'a mut [u8; MAX_CONTROL_LENGTH],
) -> Result<&'a [u8], (ControlPhase, io::Error)> {
    let mut header = [0; 8];
    reader
        .read_exact(&mut header)
        .map_err(|error| (ControlPhase::HeaderRead, error))?;
    if header[..4] != [0; 4] {
        return Err((
            ControlPhase::Validation,
            invalid_control("expected a Frame Streams control frame"),
        ));
    }
    let length = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    if !(4..=512).contains(&length) {
        return Err((
            ControlPhase::Validation,
            invalid_control("control payload length outside 4..=512"),
        ));
    }
    let length = usize::try_from(length).map_err(|_| {
        (
            ControlPhase::Validation,
            invalid_control("control length exceeds usize"),
        )
    })?;
    reader
        .read_exact(&mut storage[..length])
        .map_err(|error| (ControlPhase::BodyRead, error))?;
    Ok(&storage[..length])
}

fn read_control(reader: &mut impl Read, expected: u32, require_content: bool) -> io::Result<()> {
    let mut storage = [0; MAX_CONTROL_LENGTH];
    let frame = read_control_frame(reader, &mut storage).map_err(|(_, error)| error)?;
    validate_control(frame, expected, require_content)
}

fn take_u32(cursor: &mut &[u8]) -> io::Result<u32> {
    let (bytes, remainder) = cursor
        .split_first_chunk::<4>()
        .ok_or_else(|| invalid_control("truncated control field header"))?;
    *cursor = remainder;
    Ok(u32::from_be_bytes(*bytes))
}

fn validate_control(mut cursor: &[u8], expected: u32, require_content: bool) -> io::Result<()> {
    if take_u32(&mut cursor)? != expected {
        return Err(invalid_control("unexpected Frame Streams control type"));
    }
    if !require_content && !cursor.is_empty() {
        return Err(invalid_control("FINISH cannot contain fields"));
    }
    let mut matched = false;
    while !cursor.is_empty() {
        let kind = take_u32(&mut cursor)?;
        let length = usize::try_from(take_u32(&mut cursor)?)
            .map_err(|_| invalid_control("field length exceeds usize"))?;
        let (field, remainder) = cursor
            .split_at_checked(length)
            .ok_or_else(|| invalid_control("control field exceeds payload"))?;
        cursor = remainder;
        if kind == CONTENT_TYPE_FIELD {
            if length > 256 {
                return Err(invalid_control("content type exceeds 256 bytes"));
            }
            matched |= field == CONTENT_TYPE;
        }
    }
    if require_content && !matched {
        return Err(invalid_control(
            "receiver did not accept dnstap content type",
        ));
    }
    Ok(())
}

/// Fuzzing entry point: reads a control frame from `bytes` and validates it.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_read_control(mut bytes: &[u8], expected: u32, require_content: bool) -> bool {
    read_control(&mut bytes, expected, require_content).is_ok()
}
