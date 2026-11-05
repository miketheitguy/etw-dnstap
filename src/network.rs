//! Bidirectional Frame Streams TCP sender, optionally over TLS 1.2 or 1.3, with
//! whole-operation I/O deadlines for the handshakes and automatic reconnection.
//!
//! [`Connection`] is one Frame Streams session. Any error on it is final for that
//! session. [`NetworkSink`] owns the current session and, when it fails,
//! reconnects with bounded exponential backoff and a fresh handshake while ETW
//! capture keeps running.
//!
//! The session has two modes. The connect, the TLS and Frame Streams handshakes,
//! and the closing STOP/FINISH exchange are blocking and run under
//! [`IO_TIMEOUT`]. The data path is the opposite: the link thread
//! (`crate::sender`) puts the socket in non-blocking mode and drives it through
//! the [`Uplink`] and [`Wire`] traits, so no data send has a socket timeout and a
//! send never ends in an indeterminate state. The link thread decides which
//! frames count in `disconnected_dropped` and which go back to the router, from
//! the exact byte counts the wire reports.
//!
//! In test builds [`NetworkSink`] also implements the blocking [`FrameSink`], for
//! the parser tests and the reconnect tests that write frames one at a time. That
//! path counts frames buffered when the connection fails in
//! `disconnected_dropped` and never resends them. The service does not have it.
//!
//! A failure of the closing STOP/FINISH exchange never fails the service and is
//! not reported: see [`Uplink::close`] for [`NetworkSink`].

use std::{
    io::{self, BufWriter, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rustls::{
    ClientConfig, ClientConnection, StreamOwned,
    pki_types::{IpAddr as TlsIpAddr, ServerName},
};
use rustls_platform_verifier::BuilderVerifierExt;

#[cfg(test)]
use crate::sink::FrameSink;
use crate::{
    framestream::{CONTENT_TYPE, FrameStreamWriter},
    sink::SinkStats,
};

#[allow(unsafe_code)]
mod sockopt;
mod wire;

#[cfg(any(test, feature = "bench"))]
pub(crate) use self::wire::plain_offer;
pub(crate) use self::wire::{Moved, Uplink, Wire};
#[cfg(test)]
pub(crate) use self::wire::{TLS_FEED_BYTES, tls_drain, tls_offer};

/// The deadline of one blocking operation: the connect, each handshake, and the
/// STOP/FINISH exchange. A data send never has it.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// The shortest stall timeout the link accepts: it is never shorter than
/// [`IO_TIMEOUT`], so a handshake that is still within its own deadline is not
/// mistaken for a stall.
pub(crate) const MIN_STALL_TIMEOUT: Duration = IO_TIMEOUT;

const CONTROL_ACCEPT: u32 = 1;
const CONTROL_READY: u32 = 4;
const CONTROL_FINISH: u32 = 5;
const CONTENT_TYPE_FIELD: u32 = 1;
const MAX_CONTROL_LENGTH: usize = 512;
// Holds many typical frames per send; larger frames bypass the buffer.
const WRITE_BUFFER_BYTES: usize = 64 * 1024;
/// A write, or a readiness wait for one block, of at least this long counts as
/// blocked in `slow_writes`.
pub(crate) const SLOW_WRITE: Duration = Duration::from_millis(5);

/// TLS settings for one receiver: the client configuration and the name that
/// the receiver certificate must match. It is sent as SNI only when it is a DNS
/// name: rustls sends no SNI for an IP address.
#[derive(Clone)]
pub(crate) struct TlsConfig {
    client: Arc<ClientConfig>,
    server_name: ServerName<'static>,
}

impl TlsConfig {
    /// TLS 1.2 and 1.3 only, with the receiver certificate verified by the
    /// Windows system store (end-entity revocation only, soft-fail) and no client certificate.
    fn system(server_name: ServerName<'static>) -> io::Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
            .map_err(io::Error::other)?
            .with_platform_verifier()
            .map_err(io::Error::other)?
            .with_no_client_auth();
        Ok(Self {
            client: Arc::new(client),
            server_name,
        })
    }
}

/// Parses a `--tls-server-name` value: a DNS name or an IP address.
pub(crate) fn parse_server_name(value: &str) -> Result<ServerName<'static>, String> {
    ServerName::try_from(value.to_owned())
        .map_err(|_| "TLS server name must be a valid DNS name or IP address".into())
}

/// Where and how to connect: the receiver address and optional TLS settings.
#[derive(Clone)]
pub(crate) struct ReceiverConfig {
    address: SocketAddr,
    tls: Option<TlsConfig>,
}

impl ReceiverConfig {
    /// TLS with system-store verification. Without `server_name` the
    /// certificate must name the receiver IP address.
    ///
    /// # Errors
    /// Returns an error if the name is invalid or TLS cannot be set up.
    pub(crate) fn system_tls(address: SocketAddr, server_name: Option<&str>) -> io::Result<Self> {
        let name = match server_name {
            Some(value) => parse_server_name(value)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?,
            None => ServerName::IpAddress(TlsIpAddr::from(address.ip())),
        };
        Ok(Self {
            address,
            tls: Some(TlsConfig::system(name)?),
        })
    }
}

impl From<SocketAddr> for ReceiverConfig {
    /// A plaintext receiver.
    fn from(address: SocketAddr) -> Self {
        Self { address, tls: None }
    }
}

/// The byte stream of one session: plain TCP or TLS over it. `DeadlineTcp`
/// stays the only owner of the I/O deadline.
enum Transport {
    Plain(DeadlineTcp),
    Tls(Box<StreamOwned<ClientConnection, DeadlineTcp>>),
}

impl Transport {
    fn deadline(&self) -> &DeadlineTcp {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => &tls.sock,
        }
    }

    fn deadline_mut(&mut self) -> &mut DeadlineTcp {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => &mut tls.sock,
        }
    }
}

impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(bytes),
            Self::Tls(tls) => tls.read(bytes),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(bytes),
            Self::Tls(tls) => tls.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

/// The message of a TLS setup or handshake failure. It is the payload of the
/// `io::Error`, so the reconnect log can tell a TLS failure from a Frame Streams
/// protocol failure of the same kind. It displays as the message alone.
#[derive(Debug)]
struct TlsFailure(String);

impl std::fmt::Display for TlsFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TlsFailure {}

/// Runs the TLS handshake to completion under the shared connect deadline.
/// rustls reports protocol and certificate failures as `InvalidData`; the kind
/// of every other failure is kept.
fn complete_tls_handshake(tls: &mut StreamOwned<ClientConnection, DeadlineTcp>) -> io::Result<()> {
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(|error| {
            io::Error::new(error.kind(), TlsFailure(format!("TLS handshake: {error}")))
        })?;
    }
    Ok(())
}

/// The cause of a failed connection attempt, for the reconnect log. The first
/// failure of each class in an outage is always logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureClass {
    /// The TCP connection failed or was reset (receiver down or unreachable).
    Connect,
    /// A connect, handshake, or I/O deadline expired.
    Timeout,
    /// TLS setup or the TLS handshake failed, including a certificate the
    /// client does not trust or that does not match the server name, and a TLS
    /// alert that arrives after the client's side of the handshake (a TLS 1.3
    /// receiver that requires a client certificate sends it then).
    Tls,
    /// The receiver answered but did not complete the Frame Streams handshake
    /// (for example, it does not accept the dnstap content type).
    Protocol,
}

impl FailureClass {
    /// The class of `error`. A TLS failure is either the `TlsFailure` payload
    /// of `complete_tls_handshake` and TLS setup, or the `rustls::Error`
    /// payload that rustls gives an `InvalidData` error when it rejects a
    /// record or receives an alert during a later read or write of the
    /// Frame Streams handshake. The code's own Frame Streams errors carry a
    /// text payload, so they stay `Protocol`.
    fn of(error: &io::Error) -> Self {
        let tls = error
            .get_ref()
            .is_some_and(|inner| inner.is::<TlsFailure>() || inner.is::<rustls::Error>());
        match error.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Self::Timeout,
            _ if tls => Self::Tls,
            io::ErrorKind::InvalidData => Self::Protocol,
            _ => Self::Connect,
        }
    }

    /// The value of the `cause` log field.
    const fn name(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::Protocol => "protocol",
        }
    }

    /// This class's bit in a `SeenClasses` set.
    const fn bit(self) -> u8 {
        match self {
            Self::Connect => 1,
            Self::Timeout => 2,
            Self::Tls => 4,
            Self::Protocol => 8,
        }
    }
}

/// The failure classes already seen since the last established session, so
/// that the reconnect log reports each cause once per outage.
///
/// Reset: the set is emptied when a session is established (at startup or on
/// a reconnect), so each later outage reports each of its causes once again.
/// It is not emptied when a session is lost; the line
/// `dnstap receiver connection lost` marks the start of each outage. A failed
/// connection at startup adds its class, because the startup warning already
/// reported it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SeenClasses(u8);

impl SeenClasses {
    /// Adds `class` and returns true if it was not in the set before.
    const fn insert(&mut self, class: FailureClass) -> bool {
        let new = self.0 & class.bit() == 0;
        self.0 |= class.bit();
        new
    }

    const fn clear(&mut self) {
        self.0 = 0;
    }
}

/// Whether a failed reconnect attempt is logged. `prior_failures` is the
/// `reconnect_failures` count before this failure, and `new_class` is true when
/// its class was not yet seen in this outage. The first four failures and then
/// every 60th are logged, and so is the first failure of each class in an
/// outage. Causes that alternate (for example `connect` and `timeout`) add at
/// most one line per class, not one line per attempt.
const fn reconnect_log_due(prior_failures: u64, new_class: bool) -> bool {
    prior_failures < 4 || prior_failures.is_multiple_of(60) || new_class
}

/// Logs, at error level, that the receiver refused a reconnect attempt with
/// `InvalidData` before any session was established, and names what to check.
fn log_first_rejection(
    receiver: SocketAddr,
    class: FailureClass,
    error: &io::Error,
    failures: u64,
) {
    if class == FailureClass::Tls {
        tracing::error!(
            %receiver,
            %error,
            cause = class.name(),
            failures,
            "dnstap receiver failed the TLS handshake before the first session; check that \
            the Windows trust store trusts the receiver certificate, that \
            --tls-server-name (or the receiver IP address when it is not set) matches the \
            certificate, and that the receiver does not require a client certificate (this \
            client sends none); retrying with backoff"
        );
    } else {
        tracing::error!(
            %receiver,
            %error,
            cause = class.name(),
            failures,
            "dnstap receiver refused the Frame Streams handshake before the first session; \
            check that the receiver accepts the dnstap content type; retrying with backoff"
        );
    }
}

/// One Frame Streams session over TCP. Any error ends the session; the caller
/// decides whether to reconnect.
pub(crate) struct Connection {
    writer: FrameStreamWriter<BufWriter<Transport>>,
    /// True while the socket is in non-blocking mode for the link thread. The
    /// session is blocking again for STOP/FINISH.
    link_mode: bool,
}

impl Connection {
    pub(crate) fn connect(config: impl Into<ReceiverConfig>) -> io::Result<Self> {
        let config = config.into();
        let stream = TcpStream::connect_timeout(&config.address, IO_TIMEOUT)?;
        stream.set_nodelay(true)?;
        // SO_SNDBUF is deliberately not set. Windows turns dynamic send buffering
        // (autotuning) off for a socket that sets it, which caps throughput at the
        // buffer size per round trip. The kernel buffer therefore grows with the link.
        let tcp = DeadlineTcp::new(stream, IO_TIMEOUT);
        let mut transport = match config.tls {
            None => Transport::Plain(tcp),
            Some(tls) => {
                let connection = ClientConnection::new(tls.client, tls.server_name)
                    .map_err(|error| io::Error::other(TlsFailure(error.to_string())))?;
                let mut tls_stream = StreamOwned::new(connection, tcp);
                complete_tls_handshake(&mut tls_stream)?;
                tracing::debug!(
                    version = ?tls_stream.conn.protocol_version(),
                    "TLS handshake complete"
                );
                Transport::Tls(Box::new(tls_stream))
            }
        };
        write_control(&mut transport, CONTROL_READY, true)?;
        read_control(&mut transport, CONTROL_ACCEPT, true)?;
        let mut writer =
            FrameStreamWriter::new(BufWriter::with_capacity(WRITE_BUFFER_BYTES, transport))?;
        // Publish START before the service reports readiness, using the same
        // total handshake deadline as READY and ACCEPT.
        writer.flush()?;
        Ok(Self {
            writer,
            link_mode: false,
        })
    }

    /// Puts the socket in non-blocking mode for the link thread, which then
    /// writes through [`Wire`] and never through the frame writer. The write
    /// buffer must be empty, as it is after the handshake.
    fn enter_link_mode(&mut self) -> io::Result<()> {
        if self.link_mode {
            return Ok(());
        }
        if !self.writer.get_ref().buffer().is_empty() {
            return Err(io::Error::other("the session write buffer is not empty"));
        }
        self.writer
            .get_mut()
            .get_mut()
            .deadline_mut()
            .stream
            .set_nonblocking(true)?;
        self.link_mode = true;
        Ok(())
    }

    /// Puts the socket back in blocking mode for STOP/FINISH.
    fn leave_link_mode(&mut self) -> io::Result<()> {
        if !self.link_mode {
            return Ok(());
        }
        self.writer
            .get_mut()
            .get_mut()
            .deadline_mut()
            .stream
            .set_nonblocking(false)?;
        self.link_mode = false;
        Ok(())
    }

    /// The next socket operation starts a new deadline window; the clock is read
    /// only if that operation actually reaches the socket.
    fn reset_deadline(&mut self) {
        self.writer.get_mut().get_mut().deadline_mut().rearm();
    }

    /// Ends the stream: flushes, sends STOP, and waits for FINISH, all under one
    /// fresh deadline. The socket must be in blocking mode.
    #[cfg(test)]
    fn finish(self) -> Result<()> {
        self.finish_noting(&AtomicBool::new(false))
    }

    /// [`Self::finish`], and sets `stop_written` once the STOP frame is written
    /// and flushed, before the wait for FINISH.
    fn finish_noting(mut self, stop_written: &AtomicBool) -> Result<()> {
        self.reset_deadline();
        finish_stream_noting(self.writer, stop_written)
    }
}

impl Wire for Connection {
    fn piece_limit(&self) -> Option<usize> {
        match self.writer.get_ref().get_ref() {
            Transport::Plain(_) => None,
            Transport::Tls(_) => Some(wire::TLS_FEED_BYTES),
        }
    }

    fn offer(&mut self, bytes: &[u8]) -> io::Result<Moved> {
        match self.writer.get_mut().get_mut() {
            Transport::Plain(tcp) => wire::plain_offer(&mut &tcp.stream, bytes),
            Transport::Tls(tls) => wire::tls_offer(&mut tls.conn, bytes),
        }
    }

    fn pending(&self) -> bool {
        match self.writer.get_ref().get_ref() {
            Transport::Plain(_) => false,
            Transport::Tls(tls) => tls.conn.wants_write(),
        }
    }

    fn drain(&mut self) -> io::Result<Moved> {
        match self.writer.get_mut().get_mut() {
            Transport::Plain(_) => Ok(Moved::NONE),
            Transport::Tls(tls) => {
                let StreamOwned { conn, sock } = &mut **tls;
                wire::tls_drain(conn, &mut &sock.stream)
            }
        }
    }

    fn wait_writable(&mut self, timeout: Duration) -> io::Result<()> {
        sockopt::wait_writable(&self.writer.get_ref().get_ref().deadline().stream, timeout)
    }
}

/// The blocking frame-at-a-time sink, for the parser and reconnect tests.
#[cfg(test)]
impl FrameSink for Connection {
    fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        self.reset_deadline();
        self.writer.write_frame(payload)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.reset_deadline();
        self.writer.flush()
    }

    fn finish(self) -> Result<()> {
        Connection::finish(self)
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
    const fn new(policy: BackoffPolicy, seed: u64) -> Self {
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

    const fn reset(&mut self) {
        self.current = self.policy.initial;
    }
}

/// `base/2 + base/2 * fraction`, where `fraction = random / 2^32`.
fn jittered(base: Duration, random: u32) -> Duration {
    let half = base / 2;
    half + half.mul_f64(f64::from(random) / 4_294_967_296.0)
}

const fn splitmix64(seed: u64) -> u64 {
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

/// Sender for an explicitly trusted receiver, plaintext or TLS, that survives
/// receiver restarts. See the module comment for the loss and no-resend rules.
///
/// Startup: the first connection is attempted synchronously. A network failure
/// (refused, timed out, reset) does not stop the service; it starts
/// disconnected and retries. A protocol failure (the receiver answers but does
/// not accept the dnstap content type, or the TLS handshake or certificate
/// check fails) is a configuration error and is fatal.
pub(crate) struct NetworkSink {
    config: ReceiverConfig,
    stats: Arc<SinkStats>,
    state: State,
    backoff: Backoff,
    /// Frames accepted since the last successful flush, on the blocking
    /// [`FrameSink`] path of the tests. They may sit in the write buffer, so a
    /// failure counts them as dropped.
    #[cfg(test)]
    unflushed_frames: u64,
    /// The failure classes seen since the last established session, so that
    /// the first failure of each cause in an outage is logged at once.
    seen_failures: SeenClasses,
    /// True once a session has been established.
    ever_connected: bool,
    /// True once a rejection before the first session has been logged.
    rejection_logged: bool,
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
    pub(crate) fn start(
        config: impl Into<ReceiverConfig>,
        stats: Arc<SinkStats>,
    ) -> io::Result<Self> {
        Self::start_with(config, stats, BackoffPolicy::default())
    }

    pub(crate) fn start_with(
        config: impl Into<ReceiverConfig>,
        stats: Arc<SinkStats>,
        policy: BackoffPolicy,
    ) -> io::Result<Self> {
        let config = config.into();
        let address = config.address;
        let mut backoff = Backoff::new(policy, clock_seed(address));
        let mut seen_failures = SeenClasses::default();
        let initial = match Connection::connect(config.clone()) {
            Ok(connection) => {
                stats.connected.store(true, Ordering::Relaxed);
                State::Connected(connection)
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => return Err(error),
            Err(error) => {
                seen_failures.insert(FailureClass::of(&error));
                tracing::warn!(
                    target: crate::event_ids::RISK_RECEIVER_DOWN,
                    receiver = %address,
                    %error,
                    "dnstap receiver unavailable at startup; capture is running, and frames go \
                    to the spool, or are dropped and counted when no spool is in use, \
                    until it connects (retrying with backoff)"
                );
                State::Disconnected {
                    next_attempt: Instant::now() + backoff.next_delay(),
                    attempt: None,
                }
            }
        };
        let ever_connected = matches!(initial, State::Connected(_));
        Ok(Self {
            config,
            stats,
            state: initial,
            backoff,
            #[cfg(test)]
            unflushed_frames: 0,
            seen_failures,
            ever_connected,
            rejection_logged: false,
        })
    }

    #[cfg(test)]
    fn drop_frames(&self, count: u64) {
        self.stats
            .disconnected_dropped
            .fetch_add(count, Ordering::Relaxed);
    }

    /// [`Self::connection_lost`] for the blocking [`FrameSink`] path of the
    /// tests: the frames written since the last flush, and `current_frame`, count
    /// as dropped.
    #[cfg(test)]
    fn connection_lost_counting(&mut self, error: &io::Error, current_frame: u64) {
        self.drop_frames(self.unflushed_frames + current_frame);
        self.unflushed_frames = 0;
        self.connection_lost(error);
    }

    /// Abandons the failed connection and starts the backoff. It counts no
    /// frames: the link thread knows exactly which were not accepted.
    fn connection_lost(&mut self, error: &io::Error) {
        self.stats.connected.store(false, Ordering::Relaxed);
        self.stats.disconnects.fetch_add(1, Ordering::Relaxed);
        let delay = self.backoff.next_delay();
        // Once shutdown has begun the service writes nothing; `receiver_disconnects`
        // and `receiver_connected` in `output stopped` show the loss.
        if !self.stats.is_stopping() {
            tracing::warn!(
                target: crate::event_ids::RISK_RECEIVER_DOWN,
                receiver = %self.config.address,
                %error,
                retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                "dnstap receiver connection lost; frames go to the spool, or are dropped \
                and counted when no spool is in use, until it reconnects (frames \
                already sent are never resent)"
            );
        }
        self.state = State::Disconnected {
            next_attempt: Instant::now() + delay,
            attempt: None,
        };
    }

    /// Advances a disconnected sink: collects a finished attempt or starts one
    /// when the backoff has elapsed. Never blocks. Does nothing while connected.
    pub(crate) fn poll(&mut self, now: Instant) {
        let State::Disconnected {
            next_attempt,
            attempt,
        } = &mut self.state
        else {
            return;
        };
        if let Some(receiver) = attempt {
            match receiver.try_recv() {
                Ok(Ok(connection)) => {
                    self.stats.connected.store(true, Ordering::Relaxed);
                    self.stats.reconnects.fetch_add(1, Ordering::Relaxed);
                    self.backoff.reset();
                    #[cfg(test)]
                    {
                        self.unflushed_frames = 0;
                    }
                    self.ever_connected = true;
                    // Each later outage reports each of its causes once again.
                    self.seen_failures.clear();
                    if !self.stats.is_stopping() {
                        tracing::info!(
                            receiver = %self.config.address,
                            "dnstap receiver reconnected"
                        );
                    }
                    self.state = State::Connected(connection);
                }
                Ok(Err(error)) => {
                    let failures = self
                        .stats
                        .reconnect_failures
                        .fetch_add(1, Ordering::Relaxed);
                    let class = FailureClass::of(&error);
                    let new_class = self.seen_failures.insert(class);
                    // Bounded diagnostics; every failure stays counted.
                    if self.stats.is_stopping() {
                        // Counted in `receiver_reconnect_failures`; not logged
                        // once shutdown has begun.
                    } else if error.kind() == io::ErrorKind::InvalidData
                        && !self.ever_connected
                        && !self.rejection_logged
                    {
                        // The receiver answers but refuses this client before any
                        // session: a configuration problem that retries alone do
                        // not fix. Logged once, whatever the failure count.
                        self.rejection_logged = true;
                        log_first_rejection(self.config.address, class, &error, failures + 1);
                    } else if reconnect_log_due(failures, new_class) {
                        tracing::warn!(
                            receiver = %self.config.address,
                            %error,
                            cause = class.name(),
                            failures = failures + 1,
                            "dnstap reconnect failed"
                        );
                    }
                    *next_attempt = now + self.backoff.next_delay();
                    *attempt = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    // The attempt thread ended without sending a result: it
                    // panicked. Counted as a failed attempt and retried.
                    let failures = self
                        .stats
                        .reconnect_failures
                        .fetch_add(1, Ordering::Relaxed);
                    if (failures < 4 || failures.is_multiple_of(60)) && !self.stats.is_stopping() {
                        tracing::error!(
                            receiver = %self.config.address,
                            failures = failures + 1,
                            "dnstap reconnect thread ended without a result; retrying with backoff"
                        );
                    }
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
        let config = self.config.clone();
        let spawned = thread::Builder::new()
            .name("dnstap-reconnect".into())
            .spawn(move || {
                // The sink may be gone by the time this finishes; ignore that.
                let _ = send.send(Connection::connect(config));
            });
        match spawned {
            Ok(_handle) => *attempt = Some(receive),
            Err(error) => {
                let failures = self
                    .stats
                    .reconnect_failures
                    .fetch_add(1, Ordering::Relaxed);
                if (failures < 4 || failures.is_multiple_of(60)) && !self.stats.is_stopping() {
                    tracing::warn!(
                        receiver = %self.config.address,
                        %error,
                        failures = failures + 1,
                        "could not start the dnstap reconnect thread; retrying with backoff"
                    );
                }
                *next_attempt = now + self.backoff.next_delay();
            }
        }
    }

    /// True while a Frame Streams session is established.
    pub(crate) const fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected(_))
    }
}

impl Uplink for NetworkSink {
    /// Advances the reconnect and puts a new session in non-blocking mode for
    /// the link thread.
    fn poll(&mut self, now: Instant) {
        Self::poll(self, now);
        let failed = match &mut self.state {
            State::Connected(connection) => connection.enter_link_mode().err(),
            State::Disconnected { .. } => None,
        };
        if let Some(error) = failed {
            self.connection_lost(&error);
        }
    }

    fn is_connected(&self) -> bool {
        Self::is_connected(self)
    }

    fn wire(&mut self) -> Option<&mut dyn Wire> {
        match &mut self.state {
            State::Connected(connection) if connection.link_mode => Some(connection),
            _ => None,
        }
    }

    fn lost(&mut self, error: &io::Error) {
        // The link thread has counted the frames; none is counted here.
        self.connection_lost(error);
    }

    /// Ends the stream. A failure of the closing exchange (the socket does not
    /// return to blocking mode, the final flush or the STOP write fails or times
    /// out, or FINISH does not arrive) never fails the service, and the service
    /// writes nothing about it. Every data frame was already accepted by the
    /// socket, handed back to the router, or counted, so the exchange only says
    /// goodbye. This returns `Ok`.
    fn close(self, clean: bool) -> Result<()> {
        match self.state {
            State::Connected(mut connection) => {
                if !clean {
                    // A data frame was cut: the connection closes with no STOP,
                    // and the service writes nothing about it.
                    return Ok(());
                }
                // A requested stop does not fail because the receiver would not
                // answer: the result of the exchange is dropped on purpose.
                let _closed = connection
                    .leave_link_mode()
                    .context("return the socket to blocking mode")
                    .and_then(|()| connection.finish_noting(&self.stats.stop_sent));
                Ok(())
            }
            // No session: no STOP can be sent. The service writes nothing; the
            // `output stopped` report shows `stop_sent=false`.
            State::Disconnected { .. } => Ok(()),
        }
    }
}

/// The blocking frame-at-a-time sink, for the parser and reconnect tests.
#[cfg(test)]
impl FrameSink for NetworkSink {
    fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        if !self.is_connected() {
            self.poll(Instant::now());
        }
        let State::Connected(connection) = &mut self.state else {
            self.drop_frames(1);
            return Ok(());
        };
        match connection.write_frame(payload) {
            Ok(()) => {
                self.unflushed_frames += 1;
                self.stats
                    .bytes_sent
                    .fetch_add(payload.len() as u64 + 4, Ordering::Relaxed);
            }
            Err(error) => self.connection_lost_counting(&error, 1),
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.is_connected() {
            self.poll(Instant::now());
        }
        if let State::Connected(connection) = &mut self.state {
            match connection.flush() {
                Ok(()) => self.unflushed_frames = 0,
                Err(error) => self.connection_lost_counting(&error, 0),
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
            State::Disconnected { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
fn finish_stream<W: Read + Write>(writer: FrameStreamWriter<BufWriter<W>>) -> Result<()> {
    finish_stream_noting(writer, &AtomicBool::new(false))
}

/// Runs the closing exchange and sets `stop_written` once the STOP frame is
/// written and flushed, even when FINISH then fails to arrive.
fn finish_stream_noting<W: Read + Write>(
    mut writer: FrameStreamWriter<BufWriter<W>>,
    stop_written: &AtomicBool,
) -> Result<()> {
    // Flush the last data frame separately so a failure is not reported as a
    // STOP or FINISH failure. NetworkSink resets the shared deadline once
    // before entering this helper; these phases do not extend it.
    writer.flush().context("final buffered data flush")?;
    let buffered = writer.finish().context("STOP write/flush")?;
    stop_written.store(true, Ordering::Relaxed);
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
pub(crate) mod test_support;

#[cfg(test)]
#[path = "network/shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
#[path = "network/closing_tests.rs"]
mod closing_tests;

#[cfg(test)]
#[path = "network/reconnect_tests.rs"]
mod reconnect_tests;

#[cfg(test)]
#[path = "network/handshake_tests.rs"]
mod handshake_tests;

#[cfg(test)]
#[path = "network/backpressure_tests.rs"]
mod backpressure_tests;

#[cfg(test)]
#[path = "network/tls_tests.rs"]
mod tls_tests;

/// The longest a socket timeout may exceed the time left to the deadline before
/// `DeadlineTcp` sets it again (the shorter of this and `timeout / 32`).
const TIMEOUT_SLACK: Duration = Duration::from_millis(5);

/// Whether the socket timeout must be set again to `wanted`. The timeout last
/// applied is kept when it is at least `wanted` (so a stalled call is never cut
/// short of the deadline) and exceeds it by at most `slack` (so it cannot run
/// past the deadline by more than that). Within one operation the time left only
/// shrinks, so a timeout applied once is kept for the first `slack` of the
/// window and the operation's later calls skip the socket option; across
/// operations the applied value tracks the largest `wanted` seen.
fn timeout_needs_update(applied: Option<Duration>, wanted: Duration, slack: Duration) -> bool {
    !applied.is_some_and(|applied| {
        applied
            .checked_sub(wanted)
            .is_some_and(|excess| excess <= slack)
    })
}

struct DeadlineTcp {
    stream: TcpStream,
    deadline: Instant,
    timeout: Duration,
    /// False until the first I/O of an operation, which then starts the clock.
    armed: bool,
    /// The socket read and write timeouts last set, so that a call whose time
    /// left is within `slack` of them skips the `setsockopt` system call.
    read_applied: Option<Duration>,
    write_applied: Option<Duration>,
    slack: Duration,
}

impl DeadlineTcp {
    fn new(stream: TcpStream, timeout: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + timeout,
            timeout,
            armed: true,
            read_applied: None,
            write_applied: None,
            slack: (timeout / 32).min(TIMEOUT_SLACK),
        }
    }

    /// Starts a new operation: the deadline restarts at its first socket call.
    const fn rearm(&mut self) {
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
}

impl Read for DeadlineTcp {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self.remaining()?;
        if timeout_needs_update(self.read_applied, remaining, self.slack) {
            self.stream.set_read_timeout(Some(remaining))?;
            self.read_applied = Some(remaining);
        }
        self.stream.read(bytes)
    }
}

impl Write for DeadlineTcp {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.remaining()?;
        if timeout_needs_update(self.write_applied, remaining, self.slack) {
            self.stream.set_write_timeout(Some(remaining))?;
            self.write_applied = Some(remaining);
        }
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.stream.flush()
    }
}

/// Socket-write entry point for `benches/hot_path.rs` (feature `bench`).
#[cfg(feature = "bench")]
pub(crate) mod bench {
    use std::{
        io::Read,
        net::{Shutdown, TcpListener, TcpStream},
        thread::{self, JoinHandle},
        time::Duration,
    };

    use super::{Moved, plain_offer, sockopt};

    /// A non-blocking socket connected over loopback to a thread that reads and
    /// discards everything, written the way the link thread writes a plaintext
    /// block: `plain_offer`, and a wait for writability when the socket takes
    /// nothing.
    pub struct SocketWriter {
        stream: TcpStream,
        receiver: Option<JoinHandle<()>>,
    }

    impl SocketWriter {
        /// # Panics
        /// Panics if the loopback connection cannot be made.
        #[must_use]
        pub fn new() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let address = listener.local_addr().expect("the listener has an address");
            let receiver = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("the writer connects");
                let mut sink = vec![0_u8; 256 * 1024];
                while stream.read(&mut sink).is_ok_and(|read| read != 0) {}
            });
            let stream = TcpStream::connect(address).expect("connect to the loopback listener");
            stream
                .set_nonblocking(true)
                .expect("the socket enters non-blocking mode");
            Self {
                stream,
                receiver: Some(receiver),
            }
        }

        /// Writes all of `bytes`, in as many non-blocking sends as the socket
        /// needs. Returns the bytes written.
        ///
        /// # Panics
        /// Panics if the write fails.
        #[inline(never)]
        pub fn write_block(&mut self, bytes: &[u8]) -> usize {
            let mut written = 0;
            while written < bytes.len() {
                let moved = plain_offer(&mut &self.stream, &bytes[written..])
                    .expect("the loopback write succeeds");
                if moved == Moved::NONE {
                    sockopt::wait_writable(&self.stream, Duration::from_millis(50))
                        .expect("the wait succeeds");
                }
                written += moved.written;
            }
            written
        }
    }

    impl Default for SocketWriter {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for SocketWriter {
        fn drop(&mut self) {
            // The receiver thread ends when the connection closes.
            let _ = self.stream.shutdown(Shutdown::Both);
            if let Some(receiver) = self.receiver.take() {
                let _ = receiver.join();
            }
        }
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
