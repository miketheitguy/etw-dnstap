//! A scripted transport for the sender tests, so the link thread runs the same
//! code as in production against a socket whose every answer the test decides.
//!
//! [`Script`] is the state a test shares with the link thread: how the current
//! session's socket answers a write ([`Mode`]), whether a reconnect may happen,
//! and a record of every byte the socket accepted, per session. The receiver
//! view of a session is the plaintext of those bytes: for a TLS session the
//! script runs the server side of the connection and decrypts what the socket
//! took, so a record cut short by a failure yields nothing.
//!
//! With a manual clock, [`Wire::wait_writable`] returns at once and moves the
//! clock forward by the timeout, so the stall timer runs without real waiting
//! and the result does not depend on how busy the machine is. With the real
//! clock it waits on a condition variable until the test changes the script.

use std::{
    io::{self, Read, Write},
    mem,
    sync::{Arc, Condvar, Mutex, MutexGuard, atomic::Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use rustls::{ClientConnection, ServerConnection};

use super::link::{Clock, LinkSettings};
use crate::{
    network::{
        Moved, TLS_FEED_BYTES, Uplink, Wire, plain_offer, test_support::TlsKit, tls_drain,
        tls_offer,
    },
    sink::SinkStats,
};

/// How the socket of the current session answers a write.
#[derive(Clone, Debug)]
pub(super) enum Mode {
    /// Takes up to `limit` bytes per call.
    Open { limit: usize },
    /// Never writable: every call would block.
    Blocked,
    /// Takes one byte, then would block, in turn.
    Alternate { took_one: bool },
    /// Takes one byte each `period` of clock time, and would block otherwise.
    Paced { period: Duration, next: Duration },
    /// Takes up to `remaining` bytes in all (at most `per_call` per call), then
    /// behaves as `then`.
    Budget {
        remaining: usize,
        per_call: usize,
        then: Box<Mode>,
    },
    /// Every call fails with this error kind.
    Fail(io::ErrorKind),
}

impl Mode {
    pub(super) const fn open() -> Self {
        Self::Open { limit: usize::MAX }
    }

    /// Takes `bytes` bytes, then fails with a reset.
    pub(super) fn then_reset(bytes: usize) -> Self {
        Self::Budget {
            remaining: bytes,
            per_call: usize::MAX,
            then: Box::new(Self::Fail(io::ErrorKind::ConnectionReset)),
        }
    }

    /// Takes `bytes` bytes, then would block.
    pub(super) fn then_block(bytes: usize) -> Self {
        Self::Budget {
            remaining: bytes,
            per_call: usize::MAX,
            then: Box::new(Self::Blocked),
        }
    }

    /// The answer to a write of `length` bytes, at clock time `clock`.
    fn apply(&mut self, length: usize, clock: Duration) -> io::Result<usize> {
        match self {
            Self::Open { limit } => Ok(length.min(*limit)),
            Self::Blocked => Err(io::ErrorKind::WouldBlock.into()),
            Self::Alternate { took_one } => {
                *took_one = !*took_one;
                if *took_one {
                    Ok(1)
                } else {
                    Err(io::ErrorKind::WouldBlock.into())
                }
            }
            Self::Paced { period, next } => {
                if clock >= *next {
                    *next = clock + *period;
                    Ok(1)
                } else {
                    Err(io::ErrorKind::WouldBlock.into())
                }
            }
            Self::Budget {
                remaining,
                per_call,
                then,
            } => {
                if *remaining == 0 {
                    let next = mem::replace(&mut **then, Self::Blocked);
                    *self = next;
                    return self.apply(length, clock);
                }
                let taken = length.min(*remaining).min(*per_call);
                *remaining -= taken;
                Ok(taken)
            }
            Self::Fail(kind) => Err((*kind).into()),
        }
    }
}

/// What one session's socket took, and what a receiver would make of it.
struct SessionLog {
    /// The bytes the socket accepted.
    wire: Vec<u8>,
    /// The receiver's view: the bytes themselves, or their decryption.
    plain: Vec<u8>,
    /// The server side of a TLS session.
    server: Option<ServerConnection>,
}

impl SessionLog {
    fn accept(&mut self, bytes: &[u8]) {
        self.wire.extend_from_slice(bytes);
        let Some(server) = self.server.as_mut() else {
            self.plain.extend_from_slice(bytes);
            return;
        };
        let mut rest = bytes;
        while !rest.is_empty() {
            match server.read_tls(&mut rest) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if server.process_new_packets().is_err() {
                return;
            }
            let mut buffer = [0_u8; 4096];
            while let Ok(count @ 1..) = server.reader().read(&mut buffer) {
                self.plain.extend_from_slice(&buffer[..count]);
            }
        }
    }
}

struct Inner {
    sessions: Vec<SessionLog>,
    mode: Mode,
    /// The mode of the next session.
    next_mode: Mode,
    connect_allowed: bool,
    elapsed: Duration,
    waits: u64,
    /// The clock time of the last byte the socket accepted.
    last_accept: Duration,
    /// The clock time at which the link ended its session.
    lost_at: Option<Duration>,
    /// The errors the link ended sessions with, as text.
    lost: Vec<String>,
    /// `finish` ran, with `clean`.
    finished: Option<bool>,
    finish_delay: Duration,
}

/// The state a test shares with the scripted uplink.
pub(super) struct Script {
    inner: Mutex<Inner>,
    changed: Condvar,
    manual: bool,
    base: Instant,
}

impl Script {
    /// A script whose first session is open. With `manual`, the clock moves only
    /// when the link waits or the test advances it.
    pub(super) fn new(manual: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                sessions: Vec::new(),
                mode: Mode::open(),
                next_mode: Mode::open(),
                connect_allowed: true,
                elapsed: Duration::ZERO,
                waits: 0,
                last_accept: Duration::ZERO,
                lost_at: None,
                lost: Vec::new(),
                finished: None,
                finish_delay: Duration::ZERO,
            }),
            changed: Condvar::new(),
            manual,
            base: Instant::now(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("script lock")
    }

    /// Link settings that read this script's clock.
    pub(super) fn settings(self: &Arc<Self>, stall: Duration) -> LinkSettings {
        let mut settings = LinkSettings::new(stall);
        if self.manual {
            let script = Arc::clone(self);
            let clock: Clock = Arc::new(move || script.base + script.elapsed());
            settings.clock = clock;
        }
        settings
    }

    /// Changes how the current session's socket answers, and wakes the link.
    /// Before the first session it sets the mode that session starts in.
    pub(super) fn set_mode(&self, mode: Mode) {
        let mut inner = self.lock();
        if inner.sessions.is_empty() {
            inner.next_mode = mode.clone();
        }
        inner.mode = mode;
        drop(inner);
        self.changed.notify_all();
    }

    /// Sets the mode that the next session starts in.
    pub(super) fn set_next_mode(&self, mode: Mode) {
        self.lock().next_mode = mode;
    }

    /// Allows or refuses a reconnect. The link ends a session with refused
    /// reconnects until the test allows one.
    pub(super) fn allow_connect(&self, allow: bool) {
        self.lock().connect_allowed = allow;
        self.changed.notify_all();
    }

    pub(super) fn set_finish_delay(&self, delay: Duration) {
        self.lock().finish_delay = delay;
    }

    /// Moves the manual clock forward.
    pub(super) fn advance(&self, by: Duration) {
        self.lock().elapsed += by;
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.lock().elapsed
    }

    pub(super) fn sessions(&self) -> usize {
        self.lock().sessions.len()
    }

    /// The bytes a receiver would read from session `index`: plaintext.
    pub(super) fn plain(&self, index: usize) -> Vec<u8> {
        self.lock()
            .sessions
            .get(index)
            .map_or_else(Vec::new, |session| session.plain.clone())
    }

    /// Bytes the socket of session `index` accepted.
    pub(super) fn wire_len(&self, index: usize) -> usize {
        self.lock()
            .sessions
            .get(index)
            .map_or(0, |session| session.wire.len())
    }

    /// The complete frames of every session, in session order, as sequence
    /// numbers (the first four payload bytes).
    pub(super) fn frames(&self) -> Vec<u32> {
        let inner = self.lock();
        inner
            .sessions
            .iter()
            .flat_map(|session| complete_frames(&session.plain))
            .collect()
    }

    /// The complete frames of session `index`.
    pub(super) fn session_frames(&self, index: usize) -> Vec<u32> {
        complete_frames(&self.plain(index))
    }

    /// Bytes of an incomplete frame at the end of session `index`.
    pub(super) fn trailing_partial(&self, index: usize) -> usize {
        trailing_partial(&self.plain(index))
    }

    /// `Some(clean)` once the link has ended the stream.
    pub(super) fn finished(&self) -> Option<bool> {
        self.lock().finished
    }

    /// Readiness waits the link has made.
    pub(super) fn waits(&self) -> u64 {
        self.lock().waits
    }

    /// The clock time of the last byte the socket accepted.
    pub(super) fn last_accept(&self) -> Duration {
        self.lock().last_accept
    }

    /// The clock time at which the link ended its session, if it did.
    pub(super) fn lost_at(&self) -> Option<Duration> {
        self.lock().lost_at
    }

    fn socket_write(&self, bytes: &[u8]) -> io::Result<usize> {
        let mut inner = self.lock();
        let clock = inner.elapsed;
        let taken = inner.mode.apply(bytes.len(), clock)?;
        if taken > 0 {
            inner.last_accept = clock;
            if let Some(session) = inner.sessions.last_mut() {
                session.accept(&bytes[..taken]);
            }
        }
        Ok(taken)
    }

    fn wait_writable(&self, timeout: Duration) {
        let mut inner = self.lock();
        inner.waits += 1;
        if self.manual {
            inner.elapsed += timeout;
            return;
        }
        // A real wait ends early when the test changes the script.
        let _unused = self
            .changed
            .wait_timeout(inner, timeout)
            .expect("script lock");
    }

    fn begin_session(&self, server: Option<ServerConnection>) {
        let mut inner = self.lock();
        inner.mode = inner.next_mode.clone();
        inner.lost_at = None;
        inner.sessions.push(SessionLog {
            wire: Vec::new(),
            plain: Vec::new(),
            server,
        });
    }
}

/// The socket of the current session, as a `Write`.
struct ScriptSocket<'a>(&'a Script);

impl Write for ScriptSocket<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.socket_write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Frames in `bytes` that are complete, as sequence numbers.
pub(super) fn complete_frames(bytes: &[u8]) -> Vec<u32> {
    let mut numbers = Vec::new();
    let mut at = 0;
    while let Some(header) = bytes.get(at..).and_then(|rest| rest.first_chunk::<4>()) {
        let length = u32::from_be_bytes(*header) as usize;
        let Some(frame) = bytes.get(at + 4..at + 4 + length) else {
            break;
        };
        numbers.push(u32::from_be_bytes(
            frame[..4].try_into().expect("numbered payload"),
        ));
        at += 4 + length;
    }
    numbers
}

/// The bytes of an incomplete frame at the end of `bytes`.
pub(super) fn trailing_partial(bytes: &[u8]) -> usize {
    let mut at = 0;
    while let Some(header) = bytes.get(at..).and_then(|rest| rest.first_chunk::<4>()) {
        let end = at + 4 + u32::from_be_bytes(*header) as usize;
        if end > bytes.len() {
            break;
        }
        at = end;
    }
    bytes.len() - at
}

/// One session's data path.
enum ScriptWire {
    Plain(Arc<Script>),
    Tls(Arc<Script>, Box<ClientConnection>),
}

impl Wire for ScriptWire {
    fn piece_limit(&self) -> Option<usize> {
        match self {
            Self::Plain(_) => None,
            Self::Tls(..) => Some(TLS_FEED_BYTES),
        }
    }

    fn offer(&mut self, bytes: &[u8]) -> io::Result<Moved> {
        match self {
            Self::Plain(script) => plain_offer(&mut ScriptSocket(script), bytes),
            Self::Tls(_, connection) => tls_offer(connection, bytes),
        }
    }

    fn pending(&self) -> bool {
        match self {
            Self::Plain(_) => false,
            Self::Tls(_, connection) => connection.wants_write(),
        }
    }

    fn drain(&mut self) -> io::Result<Moved> {
        match self {
            Self::Plain(_) => Ok(Moved::NONE),
            Self::Tls(script, connection) => tls_drain(connection, &mut ScriptSocket(script)),
        }
    }

    fn wait_writable(&mut self, timeout: Duration) -> io::Result<()> {
        match self {
            Self::Plain(script) | Self::Tls(script, _) => script.wait_writable(timeout),
        }
        Ok(())
    }
}

/// An [`Uplink`] over a [`Script`].
pub(super) struct ScriptUplink {
    script: Arc<Script>,
    stats: Arc<SinkStats>,
    tls: Option<Arc<TlsKit>>,
    wire: Option<ScriptWire>,
    started: u64,
    /// The first session cannot enter link mode: the first `poll` loses it.
    fail_link_mode: bool,
}

impl ScriptUplink {
    /// An uplink that is connected now, if `connected`, and otherwise connects
    /// when the script allows it.
    pub(super) fn new(
        script: &Arc<Script>,
        stats: &Arc<SinkStats>,
        tls: Option<Arc<TlsKit>>,
        connected: bool,
    ) -> Self {
        let mut uplink = Self {
            script: Arc::clone(script),
            stats: Arc::clone(stats),
            tls,
            wire: None,
            started: 0,
            fail_link_mode: false,
        };
        if connected {
            uplink.start_session();
        }
        uplink
    }

    /// Makes the first session fail to enter link mode, as `NetworkSink` does when
    /// `set_nonblocking` fails: the first `poll` loses it.
    pub(super) const fn failing_link_mode(mut self) -> Self {
        self.fail_link_mode = true;
        self
    }

    fn start_session(&mut self) {
        let wire = match &self.tls {
            None => {
                self.script.begin_session(None);
                ScriptWire::Plain(Arc::clone(&self.script))
            }
            Some(kit) => {
                let (client, server) = kit.pair();
                self.script.begin_session(Some(server));
                ScriptWire::Tls(Arc::clone(&self.script), Box::new(client))
            }
        };
        self.wire = Some(wire);
        self.stats.connected.store(true, Ordering::Relaxed);
        if self.started > 0 {
            self.stats.reconnects.fetch_add(1, Ordering::Relaxed);
        }
        self.started += 1;
    }
}

impl Uplink for ScriptUplink {
    fn poll(&mut self, _now: Instant) {
        if self.wire.is_none() && self.script.lock().connect_allowed {
            self.start_session();
        }
        if self.fail_link_mode && self.wire.is_some() {
            self.fail_link_mode = false;
            self.lost(&io::Error::other("the session could not enter link mode"));
        }
    }

    fn is_connected(&self) -> bool {
        self.wire.is_some()
    }

    fn wire(&mut self) -> Option<&mut dyn Wire> {
        self.wire.as_mut().map(|wire| wire as &mut dyn Wire)
    }

    fn lost(&mut self, error: &io::Error) {
        self.wire = None;
        self.stats.connected.store(false, Ordering::Relaxed);
        self.stats.disconnects.fetch_add(1, Ordering::Relaxed);
        let mut inner = self.script.lock();
        inner.lost_at = Some(inner.elapsed);
        inner.lost.push(error.to_string());
        // A test allows the reconnect when it wants one.
        inner.connect_allowed = false;
    }

    /// Records the close. The closing exchange, simulated by `finish_delay`, runs
    /// only for a clean close: an unclean one sends no STOP.
    fn close(self, clean: bool) -> Result<()> {
        let delay = {
            let mut inner = self.script.lock();
            inner.finished = Some(clean);
            inner.finish_delay
        };
        if clean && !delay.is_zero() {
            std::thread::sleep(delay);
        }
        Ok(())
    }
}
