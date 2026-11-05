//! Send Microsoft DNS Server ETW packet data to a dnstap TCP receiver.

#[cfg(not(target_os = "windows"))]
compile_error!("etw-dnstap requires Windows; build for x86_64-pc-windows-msvc");

mod dnstap;
// `unsafe` is allowed only in the `etw` submodules, `service`, `perfcounters`, and `threadcpu`,
// which call native APIs.
mod etw;
mod event;
mod event_ids;
mod framestream;
mod network;
#[allow(unsafe_code)]
mod perfcounters;
mod pipeline;
mod sender;
#[allow(unsafe_code)]
mod service;
mod sink;
mod spool;
#[allow(unsafe_code)]
mod threadcpu;

/// Entry points for the `fuzz/` crate. Not part of the supported interface.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    pub use crate::etw::fuzzing::{FuzzField, compile_and_decode, decode_reference};
    pub use crate::spool::fuzzing::spool_segment;

    /// Reads one Frame Streams control frame and validates it, as the handshake
    /// does. Returns whether the frame was accepted.
    #[must_use]
    pub fn read_control(bytes: &[u8], expected: u32, require_content: bool) -> bool {
        crate::network::fuzz_read_control(bytes, expected, require_content)
    }
}

/// Entry points for `benches/hot_path.rs`. Not part of the supported interface.
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench {
    pub use crate::etw::bench::{
        Copied, Decoded, Pipeline, Plan, Pool, Record, copy_record, private_bytes,
    };
    pub use crate::network::bench::SocketWriter;
}

/// Runs capture through the Windows Service Control Manager dispatcher.
///
/// Installs the panic hook first, so a panic is recorded in the Event Log and
/// not on stderr. The binary has no help or version output; every argument
/// error is passed to the service layer, which reports it through the Event Log
/// and SCM, or returns it to a console user.
///
/// # Errors
/// Returns an error if capture, output, or worker initialization or shutdown fails.
pub fn run() -> anyhow::Result<()> {
    service::install_panic_hook();
    match pipeline::parse_args() {
        Ok(args) => service::run(Ok(args)),
        Err(error) => {
            let rendered = error.render().to_string();
            service::run(Err(strip_clap_error_prefix(&rendered).to_owned()))
        }
    }
}

/// Removes clap's leading `error: ` label once, because the service layer adds its
/// own label and the text would otherwise repeat it.
fn strip_clap_error_prefix(rendered: &str) -> &str {
    rendered.strip_prefix("error: ").unwrap_or(rendered)
}

#[cfg(test)]
mod tests {
    use super::strip_clap_error_prefix;

    #[test]
    fn strips_only_one_leading_error_label() {
        assert_eq!(
            strip_clap_error_prefix("error: missing --receiver"),
            "missing --receiver"
        );
        assert_eq!(
            strip_clap_error_prefix("error: error: nested"),
            "error: nested"
        );
        assert_eq!(strip_clap_error_prefix("Error: kept"), "Error: kept");
        assert_eq!(strip_clap_error_prefix("usage: kept"), "usage: kept");
        assert_eq!(strip_clap_error_prefix(""), "");
    }
}
