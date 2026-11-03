//! The binary writes nothing to stdout or stderr, whatever the arguments.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);

struct Outcome {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn drain(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// Runs the binary and kills it if it outlives [`DEADLINE`].
fn run(arguments: &[&str]) -> Result<Outcome, String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_etw-dnstap"))
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn failed: {error}"))?;
    let stdout = child.stdout.take().map(drain).ok_or("no stdout pipe")?;
    let stderr = child.stderr.take().map(drain).ok_or("no stderr pipe")?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= DEADLINE => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("still running after {DEADLINE:?}; killed"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(format!("wait failed: {error}")),
        }
    };
    let collect = |receiver: mpsc::Receiver<Vec<u8>>| {
        receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| format!("output not closed: {error}"))
    };
    Ok(Outcome {
        status,
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

#[test]
fn every_launch_from_a_console_is_silent_and_exits_with_one() {
    let cases: [&[&str]; 7] = [
        &[],
        &["--help"],
        &["-h"],
        &["--version"],
        &["-V"],
        &["--receiver", "not-an-address"],
        &["--receiver", "127.0.0.1:6000", "--plaintext"],
    ];
    for arguments in cases {
        let outcome = run(arguments).unwrap_or_else(|error| panic!("args {arguments:?}: {error}"));
        assert!(
            outcome.stdout.is_empty(),
            "args {arguments:?}: stdout: {}",
            String::from_utf8_lossy(&outcome.stdout)
        );
        assert!(
            outcome.stderr.is_empty(),
            "args {arguments:?}: stderr: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        assert_eq!(outcome.status.code(), Some(1), "args {arguments:?}");
    }
}
