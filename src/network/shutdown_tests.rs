use super::test_support::handshake;
use super::*;
use std::{cell::RefCell, net::TcpListener, rc::Rc};

#[derive(Default)]
struct ScriptState {
    written: Vec<u8>,
    flushed: Vec<Vec<u8>>,
    read_calls: usize,
    write_calls: usize,
}

struct ScriptedIo {
    state: Rc<RefCell<ScriptState>>,
    incoming: io::Cursor<Vec<u8>>,
    reset_after_input: bool,
    fail_flush: Option<usize>,
    fail_write: Option<usize>,
}

impl Read for ScriptedIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.state.borrow_mut().read_calls += 1;
        if self.incoming.position() == self.incoming.get_ref().len() as u64
            && self.reset_after_input
        {
            return Err(io::Error::from_raw_os_error(10054));
        }
        // Short reads exercise read_exact's partial-header/body paths without timing.
        let length = bytes.len().min(1);
        self.incoming.read(&mut bytes[..length])
    }
}

impl Write for ScriptedIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.state.borrow_mut();
        state.write_calls += 1;
        if self.fail_write == Some(state.write_calls) {
            return Err(io::Error::from_raw_os_error(10054));
        }
        state.written.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let snapshot = state.written.clone();
        state.flushed.push(snapshot);
        if self.fail_flush == Some(state.flushed.len()) {
            return Err(io::Error::from_raw_os_error(10054));
        }
        Ok(())
    }
}

fn fixture(incoming: Vec<u8>) -> (ScriptedIo, Rc<RefCell<ScriptState>>) {
    let state = Rc::new(RefCell::new(ScriptState::default()));
    (
        ScriptedIo {
            state: Rc::clone(&state),
            incoming: io::Cursor::new(incoming),
            reset_after_input: false,
            fail_flush: None,
            fail_write: None,
        },
        state,
    )
}

fn buffered_writer(io: ScriptedIo) -> FrameStreamWriter<BufWriter<ScriptedIo>> {
    let mut writer = FrameStreamWriter::new(BufWriter::new(io)).expect("START buffers");
    writer.write_frame(&[0x78, 1]).expect("final data buffers");
    writer
}

fn finish_frame() -> Vec<u8> {
    vec![0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 5]
}

fn assert_reset(error: &anyhow::Error, phase: &str) {
    assert!(error.to_string().contains(phase), "{error:#}");
    let source = error
        .downcast_ref::<io::Error>()
        .expect("I/O source retained");
    assert_eq!(source.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(source.raw_os_error(), Some(10054));
    assert!(error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.raw_os_error() == Some(10054))
    }));
}

#[test]
fn buffered_final_data_is_flushed_before_stop_and_valid_finish_succeeds() {
    let (io, state) = fixture(finish_frame());
    let writer = buffered_writer(io);
    assert!(
        state.borrow().written.is_empty(),
        "fixture has buffered data"
    );
    finish_stream(writer).expect("complete FINISH succeeds");
    let state = state.borrow();
    assert_eq!(state.flushed.len(), 2);
    assert!(state.flushed[0].ends_with(&[0, 0, 0, 2, 0x78, 1]));
    let mut expected = state.flushed[0].clone();
    expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3]);
    assert_eq!(state.flushed[1], expected);
    assert_eq!(state.written, expected);
    assert_eq!(state.read_calls, 12);
}

#[test]
fn final_data_flush_and_stop_flush_resets_have_distinct_phases() {
    for (flush, phase) in [(1, "final buffered data flush"), (2, "STOP write/flush")] {
        let (mut io, state) = fixture(finish_frame());
        io.fail_flush = Some(flush);
        let error = finish_stream(buffered_writer(io)).expect_err("flush reset fails shutdown");
        assert_reset(&error, phase);
        let state = state.borrow();
        assert_eq!(state.read_calls, 0, "failed writes never await FINISH");
        if flush == 1 {
            assert!(state.written.ends_with(&[0, 0, 0, 2, 0x78, 1]));
        }
    }
}

#[test]
fn stop_write_reset_preserves_original_os_error() {
    let (mut io, state) = fixture(finish_frame());
    io.fail_write = Some(2);
    let error = finish_stream(buffered_writer(io)).expect_err("STOP write reset fails");
    assert_reset(&error, "STOP write/flush");
    assert_eq!(state.borrow().read_calls, 0);
}

#[test]
fn partial_finish_header_or_body_eof_and_reset_never_succeed() {
    let complete = finish_frame();
    for length in [0, 1, 7, 8, 9, 11] {
        for reset in [false, true] {
            let (mut io, _) = fixture(complete[..length].to_vec());
            io.reset_after_input = reset;
            let phase = if length < 8 {
                "FINISH header read"
            } else {
                "FINISH body read"
            };
            let error = finish_stream(buffered_writer(io)).expect_err("partial FINISH fails");
            if reset {
                assert_reset(&error, phase);
            } else {
                assert!(error.to_string().contains(phase), "{error:#}");
                assert_eq!(
                    error
                        .downcast_ref::<io::Error>()
                        .expect("EOF source retained")
                        .kind(),
                    io::ErrorKind::UnexpectedEof
                );
            }
        }
    }
}

#[test]
fn malformed_or_wrong_finish_is_rejected() {
    let mut data_frame = finish_frame();
    data_frame[3] = 1;
    let mut short_length = finish_frame();
    short_length[7] = 3;
    let mut oversized = finish_frame();
    oversized[4..8].copy_from_slice(&513_u32.to_be_bytes());
    let mut wrong_kind = finish_frame();
    wrong_kind[11] = 1;
    let mut fields = finish_frame();
    fields[7] = 5;
    fields.push(0);
    for incoming in [data_frame, short_length, oversized, wrong_kind, fields] {
        let (io, _) = fixture(incoming);
        let error = finish_stream(buffered_writer(io)).expect_err("invalid FINISH fails");
        assert!(error.to_string().contains("FINISH validation"), "{error:#}");
        assert_eq!(
            error
                .downcast_ref::<io::Error>()
                .expect("validation source retained")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn shutdown_deadline_bounds_wait_for_receiver_finish() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let (release, receive) = mpsc::channel();
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let mut stream = handshake(stream);
        read_control(&mut stream, 3, false).expect("STOP arrives");
        receive
            .recv_timeout(Duration::from_secs(5))
            .expect("sender releases receiver");
    });
    let sender = Connection::connect(address).expect("handshake succeeds");
    let started = Instant::now();
    let result = sender.finish();
    release.send(()).expect("release receiver");
    receiver.join().expect("receiver assertions pass");
    assert!(result.is_err(), "missing FINISH is a shutdown failure");
    assert!(started.elapsed() < Duration::from_secs(4));
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
