//! Bidirectional Frame Streams TCP sender with whole-operation I/O deadlines.

use std::{
    io::{self, BufWriter, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use crate::framestream::{CONTENT_TYPE, FrameStreamWriter};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_ACCEPT: u32 = 1;
const CONTROL_READY: u32 = 4;
const CONTROL_FINISH: u32 = 5;
const CONTENT_TYPE_FIELD: u32 = 1;
const MAX_CONTROL_LENGTH: usize = 512;

/// Plaintext sender for an explicitly trusted receiver. Connection loss is
/// fatal; packets are never ambiguously resent or silently reconnected.
pub(crate) struct NetworkSink {
    writer: FrameStreamWriter<BufWriter<DeadlineTcp>>,
}

impl NetworkSink {
    pub(crate) fn connect(address: SocketAddr) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
        stream.set_nodelay(true)?;
        let mut transport = DeadlineTcp::new(stream, IO_TIMEOUT);
        write_control(&mut transport, CONTROL_READY, true)?;
        read_control(&mut transport, CONTROL_ACCEPT, true)?;
        let mut writer = FrameStreamWriter::new(BufWriter::new(transport))?;
        // Publish START before the service reports readiness, using the same
        // total handshake deadline as READY and ACCEPT.
        writer.flush()?;
        Ok(Self { writer })
    }

    pub(crate) fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        self.reset_deadline();
        self.writer.write_frame(payload)
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.reset_deadline();
        self.writer.flush()
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.reset_deadline();
        let buffered = self.writer.finish()?;
        let mut transport = buffered
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        // STOP, flush, and FINISH share one deadline; each read cannot extend it.
        read_control(&mut transport, CONTROL_FINISH, false)
    }

    fn reset_deadline(&mut self) {
        self.writer.get_mut().get_mut().deadline = Instant::now() + IO_TIMEOUT;
    }
}

struct DeadlineTcp {
    stream: TcpStream,
    deadline: Instant,
}

impl DeadlineTcp {
    fn new(stream: TcpStream, timeout: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + timeout,
        }
    }

    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Frame Streams I/O deadline"))
    }
}

impl Read for DeadlineTcp {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}

impl Write for DeadlineTcp {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
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

fn read_control(reader: &mut impl Read, expected: u32, require_content: bool) -> io::Result<()> {
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    if header[..4] != [0; 4] {
        return Err(invalid_control("expected a Frame Streams control frame"));
    }
    let length = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    if !(4..=512).contains(&length) {
        return Err(invalid_control("control payload length outside 4..=512"));
    }
    let length =
        usize::try_from(length).map_err(|_| invalid_control("control length exceeds usize"))?;
    let mut storage = [0; MAX_CONTROL_LENGTH];
    reader.read_exact(&mut storage[..length])?;
    validate_control(&storage[..length], expected, require_content)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc, thread};

    #[test]
    fn round_trip_negotiates_and_waits_for_finish() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let address = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("client connects");
            let mut stream = DeadlineTcp::new(stream, Duration::from_secs(3));
            read_control(&mut stream, CONTROL_READY, true).expect("READY advertises dnstap");
            write_control(&mut stream, CONTROL_ACCEPT, true).expect("ACCEPT writes");
            read_control(&mut stream, 2, true).expect("START advertises dnstap");
            let mut data = [0; 6];
            stream
                .read_exact(&mut data)
                .expect("one complete data frame");
            assert_eq!(data, [0, 0, 0, 2, 0x78, 1]);
            read_control(&mut stream, 3, false).expect("STOP arrives");
            write_control(&mut stream, CONTROL_FINISH, false).expect("FINISH writes");
        });
        let mut sender = NetworkSink::connect(address).expect("handshake succeeds");
        sender.write_frame(&[0x78, 1]).expect("payload writes");
        sender.finish().expect("STOP/FINISH succeeds");
        server.join().expect("server assertions pass");
    }

    #[test]
    fn rejects_malformed_and_incompatible_controls_without_unbounded_allocations() {
        for bytes in [
            vec![0, 0, 0, 1, 0, 0, 0, 4],
            vec![0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff],
            vec![0, 0, 0, 0, 0, 0, 0, 3],
        ] {
            assert!(read_control(&mut bytes.as_slice(), CONTROL_ACCEPT, true).is_err());
        }
        assert!(validate_control(&[0, 0, 0, 1], CONTROL_ACCEPT, true).is_err());
        assert!(validate_control(&[0, 0, 0, 5, 1], CONTROL_FINISH, false).is_err());
        assert!(
            validate_control(
                &[0, 0, 0, 1, 0, 0, 0, 1, 0xff, 0xff, 0xff, 0xff],
                CONTROL_ACCEPT,
                true
            )
            .is_err()
        );
    }

    #[test]
    fn deadline_is_not_extended_by_partial_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let address = listener.local_addr().expect("listener address");
        let (release, receive) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client connects");
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .expect("server write bound");
            let mut accept = Vec::new();
            write_control(&mut accept, CONTROL_ACCEPT, true).expect("valid ACCEPT fixture");
            // Every byte arrives sooner than a per-call 100 ms socket timeout,
            // but the whole valid frame takes over a second. Only a shared
            // operation deadline rejects this trickle before it completes.
            for byte in accept {
                stream.write_all(&[byte]).expect("partial header writes");
                if receive.recv_timeout(Duration::from_millis(25)).is_ok() {
                    break;
                }
            }
        });
        let stream = TcpStream::connect(address).expect("loopback connects");
        let mut stream = DeadlineTcp::new(stream, Duration::from_millis(100));
        let started = Instant::now();
        assert!(read_control(&mut stream, CONTROL_ACCEPT, true).is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        release.send(()).expect("release server");
        server.join().expect("server exits");
    }
}
