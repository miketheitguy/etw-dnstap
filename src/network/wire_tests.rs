//! Tests for the byte rules of the data path: plaintext writes report exactly
//! what the socket took, and TLS records stay whole when the socket takes a byte
//! at a time or fails in the middle of one.

use std::io::{self, Read, Write};

use rustls::{ClientConnection, ServerConnection};

use super::*;
use crate::network::test_support::TlsKit;

/// A socket that answers each write from a script.
struct Socket {
    /// The answer to each write, in order, repeating from the start.
    answers: Vec<Answer>,
    calls: usize,
    taken: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Answer {
    /// Takes at most this many bytes.
    Take(usize),
    WouldBlock,
    Interrupted,
    Fail(io::ErrorKind),
}

impl Socket {
    fn new(answers: &[Answer]) -> Self {
        Self {
            answers: answers.to_vec(),
            calls: 0,
            taken: Vec::new(),
        }
    }
}

impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let answer = self.answers[self.calls % self.answers.len()];
        self.calls += 1;
        match answer {
            Answer::Take(limit) => {
                let count = bytes.len().min(limit);
                self.taken.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            Answer::WouldBlock => Err(io::ErrorKind::WouldBlock.into()),
            Answer::Interrupted => Err(io::ErrorKind::Interrupted.into()),
            Answer::Fail(kind) => Err(kind.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn plaintext_offers_report_exactly_what_the_socket_took() {
    let mut socket = Socket::new(&[Answer::Take(3)]);
    assert_eq!(
        plain_offer(&mut socket, b"abcdefgh").expect("offer"),
        Moved {
            taken: 3,
            written: 3
        }
    );
    assert_eq!(socket.taken, b"abc");
}

#[test]
fn a_plaintext_socket_that_would_block_or_was_interrupted_took_nothing() {
    for answer in [Answer::WouldBlock, Answer::Interrupted] {
        let mut socket = Socket::new(&[answer]);
        assert_eq!(
            plain_offer(&mut socket, b"abc").expect("not an error"),
            Moved::NONE
        );
        assert_eq!(socket.taken, Vec::<u8>::new());
    }
}

#[test]
fn a_plaintext_socket_error_ends_the_session() {
    let mut socket = Socket::new(&[Answer::Fail(io::ErrorKind::ConnectionReset)]);
    let error = plain_offer(&mut socket, b"abc").expect_err("a reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}

/// Plaintext that the server side of `connection` has read so far, after being
/// given `ciphertext`.
fn server_reads(server: &mut ServerConnection, ciphertext: &[u8]) -> Vec<u8> {
    let mut rest = ciphertext;
    let mut plain = Vec::new();
    while !rest.is_empty() {
        server.read_tls(&mut rest).expect("records are read");
        server.process_new_packets().expect("records are valid");
        let mut buffer = [0_u8; 4096];
        while let Ok(count @ 1..) = server.reader().read(&mut buffer) {
            plain.extend_from_slice(&buffer[..count]);
        }
    }
    plain
}

/// The lengths of the TLS records in `ciphertext`, by their 5-byte headers.
fn record_lengths(ciphertext: &[u8]) -> Vec<usize> {
    let mut lengths = Vec::new();
    let mut at = 0;
    while at + 5 <= ciphertext.len() {
        let length = usize::from(u16::from_be_bytes([ciphertext[at + 3], ciphertext[at + 4]]));
        lengths.push(length);
        at += 5 + length;
    }
    assert_eq!(at, ciphertext.len(), "whole records only");
    lengths
}

fn sessions() -> Vec<(&'static str, ClientConnection, ServerConnection)> {
    let tls12 = TlsKit::with_versions(&[&rustls::version::TLS12]).pair();
    let tls13 = TlsKit::with_versions(&[&rustls::version::TLS13]).pair();
    vec![("TLS 1.2", tls12.0, tls12.1), ("TLS 1.3", tls13.0, tls13.1)]
}

#[test]
fn a_piece_of_the_feed_size_is_one_tls_record() {
    // The design relies on the default fragment size: one piece, one record. A
    // rustls change that alters it must fail here and in the tests of the
    // unconfirmed piece.
    assert_eq!(TLS_FEED_BYTES, 16 * 1024);
    for (version, mut client, _server) in sessions() {
        let piece = vec![0x5a; TLS_FEED_BYTES];
        let moved = tls_offer(&mut client, &piece).expect("offer");
        assert_eq!(moved.taken, piece.len(), "{version}");
        assert_eq!(moved.written, 0, "nothing reaches the socket by offering");
        assert!(client.wants_write());
        let mut socket = Socket::new(&[Answer::Take(usize::MAX)]);
        let drained = tls_drain(&mut client, &mut socket).expect("drain");
        assert_eq!(drained.written, socket.taken.len());
        assert!(!client.wants_write());
        assert_eq!(record_lengths(&socket.taken).len(), 1, "{version}");
    }
}

#[test]
fn tls_records_stay_whole_when_the_socket_takes_one_byte_at_a_time() {
    for (version, mut client, mut server) in sessions() {
        let plain: Vec<u8> = (0..5_000_u32).map(|n| (n % 251) as u8).collect();
        let moved = tls_offer(&mut client, &plain).expect("offer");
        assert_eq!(moved.taken, plain.len());
        // One byte, then would block, in turn: every drain call makes progress of
        // one byte at most, and the record is never cut.
        let mut socket = Socket::new(&[Answer::Take(1), Answer::WouldBlock]);
        let mut written = 0;
        let mut calls = 0;
        while client.wants_write() {
            let drained = tls_drain(&mut client, &mut socket).expect("drain");
            assert!(drained.written <= 1, "{version}");
            written += drained.written;
            calls += 1;
            assert!(calls < 100_000, "{version}: the drain made no progress");
            // Until the last byte, the server has no complete record.
            if client.wants_write() {
                let seen = server_reads(
                    &mut server,
                    &socket.taken[written - drained.written..written],
                );
                assert!(seen.is_empty(), "{version}: a record was cut");
            }
        }
        assert_eq!(written, socket.taken.len());
        // The last byte completes the record.
        let last = server_reads(&mut server, &socket.taken[written - 1..]);
        assert_eq!(last, plain, "{version}");
    }
}

#[test]
fn a_tls_socket_that_would_block_keeps_the_records_inside_rustls() {
    let (version, mut client, _server) = sessions().remove(1);
    let moved = tls_offer(&mut client, &[1; 100]).expect("offer");
    assert_eq!(moved.taken, 100);
    let mut socket = Socket::new(&[Answer::WouldBlock]);
    assert_eq!(
        tls_drain(&mut client, &mut socket).expect("drain"),
        Moved::NONE
    );
    assert!(client.wants_write(), "{version}: nothing is lost");
    // The next drain, with the socket open again, writes it all.
    let mut open = Socket::new(&[Answer::Take(usize::MAX)]);
    let drained = tls_drain(&mut client, &mut open).expect("drain");
    assert!(drained.written > 100);
    assert!(!client.wants_write());
}

#[test]
fn a_tls_socket_that_fails_in_the_middle_of_a_record_gives_the_server_nothing() {
    for (version, mut client, mut server) in sessions() {
        let plain = vec![7_u8; 10_000];
        tls_offer(&mut client, &plain).expect("offer");
        // The socket takes 100 bytes of the record, then resets.
        let mut socket = Socket::new(&[
            Answer::Take(100),
            Answer::Fail(io::ErrorKind::ConnectionReset),
        ]);
        let error = tls_drain(&mut client, &mut socket).expect_err("the reset ends the drain");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset, "{version}");
        assert_eq!(socket.taken.len(), 100);
        assert!(
            server_reads(&mut server, &socket.taken).is_empty(),
            "{version}: a cut record yields no plaintext"
        );
    }
}

#[test]
fn rustls_takes_at_most_its_buffer_limit_of_a_large_offer() {
    // A frame of the maximum length is longer than a piece. rustls may take part
    // of it; the caller offers the rest after draining, so a partial take is a
    // defined result, not an error.
    let (_version, mut client, mut server) = sessions().remove(1);
    let frame = vec![9_u8; 70 * 1024];
    let mut sent = Vec::new();
    let mut socket = Socket::new(&[Answer::Take(usize::MAX)]);
    let mut offered = 0;
    let mut guard = 0;
    while offered < frame.len() {
        let moved = tls_offer(&mut client, &frame[offered..]).expect("offer");
        assert!(
            moved.taken > 0,
            "rustls takes some of an offer after a drain"
        );
        offered += moved.taken;
        tls_drain(&mut client, &mut socket).expect("drain");
        assert!(!client.wants_write());
        sent.extend_from_slice(&std::mem::take(&mut socket.taken));
        guard += 1;
        assert!(guard < 100);
    }
    assert_eq!(server_reads(&mut server, &sent), frame);
}
