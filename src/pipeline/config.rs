//! Command-line arguments, defaults, and the validated runtime settings.

use std::{net::SocketAddr, time::Duration};

use anyhow::{Result, bail};
use clap::Parser;

use crate::etw::SessionConfig;

/// Fixed default so a restart finds and reclaims a session leaked by a crash.
const DEFAULT_SESSION_NAME: &str = "EtwDnstap";

/// Default channel capacity; `CaptureArgs` uses the same value.
pub(super) const DEFAULT_CAPACITY: usize = 24_576;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Windows service for DNS Server ETW to a dnstap TCP receiver"
)]
pub(crate) struct CaptureArgs {
    /// Frame Streams TCP receiver as an IP:PORT. Uses TLS 1.2 or 1.3 unless
    /// `--plaintext` is set. The receiver certificate is verified with the Windows
    /// system certificate store. Revocation is checked for that certificate only.
    /// An unobtainable revocation status does not fail the connection.
    #[arg(long, value_parser = parse_receiver)]
    receiver: SocketAddr,

    /// Send unencrypted Frame Streams (trusted networks only).
    #[arg(long)]
    plaintext: bool,

    /// Name that the receiver certificate must match, also sent as the TLS SNI.
    /// Default: the receiver IP address.
    #[arg(long, conflicts_with = "plaintext", value_parser = parse_tls_server_name)]
    tls_server_name: Option<String>,

    /// Event channel capacity, 1024–262144; full channels drop the newest event.
    #[arg(long, default_value = "24576", value_parser = parse_capacity)]
    pub(super) capacity: usize,

    /// Concurrent records over 2 KiB; zero disables their capture (maximum 256).
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u16).range(0..=i64::from(crate::etw::MAX_JUMBO_CAPACITY)))]
    jumbo_capacity: u16,

    /// Stop after this many seconds; otherwise capture until SCM requests STOP.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=604_800))]
    duration: Option<u64>,

    /// ETW session name. A leaked session of the same name and shape is reclaimed
    /// at startup.
    #[arg(long, default_value = DEFAULT_SESSION_NAME, value_parser = parse_session_name)]
    session_name: String,

    /// Optional dnstap producer identity, at most 1024 UTF-8 bytes.
    #[arg(long, default_value = "etw-dnstap", value_parser = parse_identity)]
    identity: String,

    /// ETW buffer size in KiB, 4–128, must be a multiple of 4.
    #[arg(long, default_value = "16", value_parser = clap::value_parser!(u32).range(4..=128))]
    etw_buffer_kib: u32,

    /// Requested minimum ETW buffers, reserved when the session starts, 0–4096 and
    /// at most `--etw-max-buffers`. 0 lets ETW choose. ETW raises the value to at
    /// least two per logical processor.
    #[arg(long, default_value = "0", value_parser = clap::value_parser!(u32).range(0..=4096))]
    etw_min_buffers: u32,

    /// Requested maximum ETW buffers, 4–4096. ETW may reserve more (at least two buffers
    /// per logical processor, and raises the maximum to at least that).
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u32).range(4..=4096))]
    etw_max_buffers: u32,

    /// ETW real-time flush interval in seconds, 1–60. ETW delivers real-time
    /// events about once per interval, so this bounds delivery latency. Longer
    /// intervals use less CPU and make each delivered batch larger.
    #[arg(long, default_value = "1", value_parser = clap::value_parser!(u32).range(1..=60))]
    etw_flush_seconds: u32,

    /// Seconds between counter snapshots in the log (queue depth, drops, ETW loss,
    /// receiver connection state), 0-86400; 0 disables the snapshots. The final
    /// report at shutdown is always written.
    #[arg(long, default_value = "60", value_parser = clap::value_parser!(u32).range(0..=86_400))]
    stats_interval_seconds: u32,

    /// Diagnostic only: request this system timer resolution in milliseconds
    /// (`timeBeginPeriod`) for the whole run.
    #[cfg(feature = "diag-timing")]
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=15))]
    diag_timer_resolution_ms: Option<u32>,

    /// Diagnostic only: raise the priority of the `dnstap-parser` and
    /// `etw-consumer` threads.
    #[cfg(feature = "diag-timing")]
    #[arg(long, value_enum)]
    diag_thread_priority: Option<crate::etw::ThreadPriority>,
}

fn parse_receiver(value: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| "receiver must be a numeric IP:PORT")?;
    check_receiver(address)
}

fn parse_tls_server_name(value: &str) -> Result<String, String> {
    crate::network::parse_server_name(value).map(|_| value.to_owned())
}

/// Shared by the command line and [`PipelineConfig::validate`].
fn check_receiver(address: SocketAddr) -> Result<SocketAddr, String> {
    if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
        return Err("receiver must have a unicast IP and a nonzero port".into());
    }
    Ok(address)
}

fn parse_capacity(value: &str) -> Result<usize, String> {
    let capacity: usize = value.parse().map_err(|_| "capacity must be an integer")?;
    check_capacity(capacity)
}

fn check_capacity(capacity: usize) -> Result<usize, String> {
    if !(crate::etw::MIN_CAPACITY..=crate::etw::MAX_CAPACITY).contains(&capacity) {
        return Err(format!(
            "capacity must be between {} and {} events",
            crate::etw::MIN_CAPACITY,
            crate::etw::MAX_CAPACITY
        ));
    }
    Ok(capacity)
}

fn check_jumbo_capacity(capacity: u16) -> Result<u16, String> {
    if capacity > crate::etw::MAX_JUMBO_CAPACITY {
        return Err(format!(
            "jumbo capacity must be at most {} (zero disables it)",
            crate::etw::MAX_JUMBO_CAPACITY
        ));
    }
    Ok(capacity)
}

fn parse_session_name(value: &str) -> Result<String, String> {
    check_session_name(value)?;
    Ok(value.into())
}

fn check_session_name(value: &str) -> Result<(), String> {
    if value.is_empty() || value.encode_utf16().count() > 200 || value.contains('\0') {
        return Err("session name must contain 1..=200 UTF-16 units and no NUL".into());
    }
    Ok(())
}

fn parse_identity(value: &str) -> Result<String, String> {
    check_identity(value)?;
    Ok(value.into())
}

fn check_identity(value: &str) -> Result<(), String> {
    if value.len() > 1024 {
        return Err("identity must be at most 1024 UTF-8 bytes".into());
    }
    Ok(())
}

/// Diagnostic-only settings, folded into one value so the pipeline signatures
/// carry no per-feature parameters. It is empty unless `diag-timing` is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DiagContext {
    /// `timeBeginPeriod` request held for the whole run.
    #[cfg(feature = "diag-timing")]
    pub(crate) timer_resolution_ms: Option<u32>,
    /// Priority applied to the parser and ETW consumer threads.
    #[cfg(feature = "diag-timing")]
    pub(crate) thread_priority: Option<crate::etw::ThreadPriority>,
}

/// Validated runtime settings. The command line is the only front end: it fills
/// this through [`CaptureArgs`] (`TryFrom`). Tests may build one directly.
/// [`PipelineConfig::validate`] repeats the clap bounds as defence in depth, so a
/// config built any other way is held to the same rules.
#[derive(Clone, Debug)]
pub(crate) struct PipelineConfig {
    pub(crate) receiver: SocketAddr,
    /// True to connect with TLS.
    pub(crate) tls: bool,
    /// Certificate name override; not valid with plaintext.
    pub(crate) tls_server_name: Option<String>,
    pub(crate) capacity: usize,
    pub(crate) jumbo_capacity: u16,
    pub(crate) duration: Option<Duration>,
    pub(crate) session_name: String,
    pub(crate) identity: String,
    pub(crate) session: SessionConfig,
    /// Seconds between counter snapshots; zero disables them.
    pub(crate) stats_interval_seconds: u32,
    pub(crate) diag: DiagContext,
}

impl PipelineConfig {
    /// Re-checks every bound the command line enforces, so a config built
    /// without clap is held to the same rules.
    ///
    /// # Errors
    /// Returns the first violated bound.
    pub(crate) fn validate(&self) -> Result<()> {
        check_receiver(self.receiver).map_err(anyhow::Error::msg)?;
        if let Some(name) = &self.tls_server_name {
            if !self.tls {
                bail!("tls-server-name cannot be used with plaintext");
            }
            parse_tls_server_name(name).map_err(anyhow::Error::msg)?;
        }
        check_capacity(self.capacity).map_err(anyhow::Error::msg)?;
        check_jumbo_capacity(self.jumbo_capacity).map_err(anyhow::Error::msg)?;
        check_session_name(&self.session_name).map_err(anyhow::Error::msg)?;
        check_identity(&self.identity).map_err(anyhow::Error::msg)?;
        SessionConfig::new(
            self.session.buffer_kib,
            self.session.minimum_buffers,
            self.session.maximum_buffers,
            self.session.flush_seconds,
        )?;
        Ok(())
    }

    /// The counter snapshot interval, or `None` when disabled.
    pub(super) fn stats_interval(&self) -> Option<Duration> {
        (self.stats_interval_seconds != 0)
            .then(|| Duration::from_secs(u64::from(self.stats_interval_seconds)))
    }

    /// True when the ETW buffer and flush settings are the low-rate defaults.
    pub(super) fn uses_default_etw_settings(&self) -> bool {
        self.session == SessionConfig::default()
    }
}

impl TryFrom<CaptureArgs> for PipelineConfig {
    type Error = anyhow::Error;

    fn try_from(args: CaptureArgs) -> Result<Self> {
        let config = Self {
            receiver: args.receiver,
            tls: !args.plaintext,
            tls_server_name: args.tls_server_name,
            capacity: args.capacity,
            jumbo_capacity: args.jumbo_capacity,
            duration: args.duration.map(Duration::from_secs),
            session_name: args.session_name,
            identity: args.identity,
            session: SessionConfig::new(
                args.etw_buffer_kib,
                args.etw_min_buffers,
                args.etw_max_buffers,
                args.etw_flush_seconds,
            )?,
            stats_interval_seconds: args.stats_interval_seconds,
            diag: DiagContext {
                #[cfg(feature = "diag-timing")]
                timer_resolution_ms: args.diag_timer_resolution_ms,
                #[cfg(feature = "diag-timing")]
                thread_priority: args.diag_thread_priority,
            },
        };
        config.validate()?;
        Ok(config)
    }
}

/// Parses the process command line without exiting, so the caller decides how
/// to report help, version, and argument errors.
pub(crate) fn parse_args() -> Result<CaptureArgs, clap::Error> {
    parse_args_from(std::env::args_os())
}

pub(super) fn parse_args_from<I, T>(arguments: I) -> Result<CaptureArgs, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    CaptureArgs::try_parse_from(arguments)
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod cli_tests;

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
