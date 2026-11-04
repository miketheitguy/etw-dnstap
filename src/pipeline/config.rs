//! Command-line arguments, defaults, and the validated runtime settings.

use std::{
    ffi::OsString,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Result, bail};
use clap::Parser;

use crate::{etw::SessionConfig, spool::fsinfo};

/// Fixed default so a restart finds and reclaims a session leaked by a crash.
const DEFAULT_SESSION_NAME: &str = "EtwDnstap";

/// Default channel capacity; `CaptureArgs` uses the same value.
pub(super) const DEFAULT_CAPACITY: usize = 65_536;

/// Where `ProgramData` is when the environment variable is unset or not a plain
/// absolute path on a local drive letter.
const FALLBACK_PROGRAM_DATA: &str = r"C:\ProgramData";
/// Bounds of `--spool-max-mib` and `--spool-min-free-mib`, in MiB. `CaptureArgs`
/// and [`PipelineConfig::validate`] both use them.
const SPOOL_MAX_MIB_RANGE: std::ops::RangeInclusive<u64> = 64..=1_048_576;
const SPOOL_MIN_FREE_MIB_RANGE: std::ops::RangeInclusive<u64> = 0..=1_048_576;
const BYTES_PER_MIB: u64 = 1024 * 1024;
/// Bounds of `--receiver-stall-seconds`. The shortest is the I/O timeout of the
/// handshakes, so a handshake within its own deadline is never taken for a
/// stall. `CaptureArgs` and [`PipelineConfig::validate`] both use them. The
/// bounds are `i64` because clap's range check for a `u32` takes `i64` bounds.
const RECEIVER_STALL_SECONDS_RANGE: std::ops::RangeInclusive<i64> = 2..=300;
const _: () = assert!(
    RECEIVER_STALL_SECONDS_RANGE.start().cast_unsigned()
        >= crate::network::MIN_STALL_TIMEOUT.as_secs()
);

#[derive(Debug, Parser)]
#[command(
    about = "Windows service for DNS Server ETW to a dnstap TCP receiver",
    disable_help_flag = true,
    disable_version_flag = true
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

    /// Name that the receiver certificate must match. A DNS name is also sent as
    /// the TLS SNI. An IP address is not, because rustls sends no SNI for one.
    /// Default: the receiver IP address, so no SNI.
    #[arg(long, conflicts_with = "plaintext", value_parser = parse_tls_server_name)]
    tls_server_name: Option<String>,

    /// Event channel capacity, 1024–262144; full channels drop the newest event.
    #[arg(long, default_value = "65536", value_parser = parse_capacity)]
    pub(super) capacity: usize,

    /// Concurrent records over 2 KiB; zero disables their capture (maximum 256).
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u16).range(0..=i64::from(crate::etw::MAX_JUMBO_CAPACITY)))]
    jumbo_capacity: u16,

    /// Stop after this many seconds; otherwise capture until SCM requests STOP.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=604_800))]
    duration: Option<u64>,

    /// ETW session name: 1 to 128 ASCII letters, digits, underscore, dot, or hyphen,
    /// not starting with a hyphen. A leaked session of the same name and shape is
    /// reclaimed at startup.
    #[arg(long, default_value = DEFAULT_SESSION_NAME, value_parser = parse_session_name)]
    session_name: String,

    /// Optional dnstap producer identity, at most 1024 UTF-8 bytes.
    #[arg(long, default_value = "etw-dnstap", value_parser = parse_identity)]
    identity: String,

    /// ETW buffer size in KiB, 4–128, must be a multiple of 4.
    #[arg(long, default_value = "128", value_parser = clap::value_parser!(u32).range(4..=128))]
    etw_buffer_kib: u32,

    /// Requested minimum ETW buffers, reserved when the session starts, 0–4096 and
    /// at most `--etw-max-buffers`. 0 lets ETW choose. ETW raises the value to at
    /// least two per logical processor, so the default of 12 matters only below
    /// six logical processors.
    #[arg(long, default_value = "12", value_parser = clap::value_parser!(u32).range(0..=4096))]
    etw_min_buffers: u32,

    /// Requested maximum ETW buffers, 4–4096. ETW may reserve more (at least two buffers
    /// per logical processor, and raises the maximum to at least that).
    #[arg(long, default_value = "128", value_parser = clap::value_parser!(u32).range(4..=4096))]
    etw_max_buffers: u32,

    /// ETW real-time flush interval in seconds, 1–60. ETW delivers real-time
    /// events about once per interval, so this bounds delivery latency. Longer
    /// intervals use less CPU and make each delivered batch larger.
    #[arg(long, default_value = "1", value_parser = clap::value_parser!(u32).range(1..=60))]
    etw_flush_seconds: u32,

    /// Spool folder for frames that cannot be sent while the receiver is down.
    /// Must be an absolute path. Default: `EtwDnstap\spool` under the
    /// `ProgramData` folder. The service never creates the folder; it uses it
    /// only when it exists and is trusted, and otherwise runs without a spool.
    #[arg(long, conflicts_with = "no_spool", value_parser = parse_spool_dir)]
    spool_dir: Option<PathBuf>,

    /// Run without a spool: frames that arrive while the receiver is down are
    /// dropped and counted.
    #[arg(long)]
    no_spool: bool,

    /// Cap on the bytes of spool segment files, in MiB, 64-1048576. At the cap
    /// the spool deletes its oldest segment.
    #[arg(long, default_value = "4096", conflicts_with = "no_spool", value_parser = clap::value_parser!(u64).range(SPOOL_MAX_MIB_RANGE))]
    spool_max_mib: u64,

    /// Free space to keep on the spool volume, in MiB, 0-1048576. Below it the
    /// spool deletes its oldest segment.
    #[arg(long, default_value = "2048", conflicts_with = "no_spool", value_parser = clap::value_parser!(u64).range(SPOOL_MIN_FREE_MIB_RANGE))]
    spool_min_free_mib: u64,

    /// Seconds the receiver may accept no bytes, while the service has data to
    /// send, before the service ends the session and reconnects, 2-300. A
    /// receiver that is slow or paused for less than this keeps its session, and
    /// the service spools the data it cannot send (at 50,000 queries per second,
    /// the default of 15 seconds spools about 155 MB). An idle connection never
    /// stalls.
    #[arg(long, default_value = "15", value_parser = clap::value_parser!(u32).range(RECEIVER_STALL_SECONDS_RANGE))]
    receiver_stall_seconds: u32,

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

/// Longest accepted session name, in characters. The MSI and the provisioning
/// scripts apply the same limit, so a name that starts here is one they accept.
const MAX_SESSION_NAME_CHARS: usize = 128;

/// The session-name rule, as the error messages state it.
const SESSION_NAME_RULE: &str =
    "1 to 128 ASCII letters, digits, underscore, dot, or hyphen, not starting with a hyphen";

/// Accepts only `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}$`: ASCII letters, digits,
/// underscore, dot, and hyphen, and no hyphen first. The name is also the ETW
/// session name and the input of the session GUID, and it appears in the ACL
/// provisioning command, so the rule leaves nothing to quote or escape. A
/// leading hyphen is refused because the MSI passes the name as a separate
/// argument (`--session-name "[SESSION_NAME]"`), where the parser would read
/// it as an option.
fn check_session_name(value: &str) -> Result<(), String> {
    if let Some((position, character)) = value
        .chars()
        .enumerate()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    {
        return Err(format!(
            "session name must be {SESSION_NAME_RULE}; {character:?} at position {} is not \
             allowed",
            position + 1
        ));
    }
    if value.starts_with('-') {
        return Err(format!(
            "session name must be {SESSION_NAME_RULE}; it starts with a hyphen"
        ));
    }
    // Every character is ASCII here, so the byte length is the character count.
    let length = value.len();
    if length == 0 || length > MAX_SESSION_NAME_CHARS {
        return Err(format!(
            "session name must be {SESSION_NAME_RULE}; it has {length} characters"
        ));
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

fn parse_spool_dir(value: &str) -> Result<PathBuf, String> {
    let dir = PathBuf::from(value);
    check_spool_dir_path(&dir)?;
    Ok(dir)
}

/// Shared by the command line and [`PipelineConfig::validate`]. The folder must
/// be an absolute path on a local drive letter with no `.` or `..` component;
/// network and device paths are refused. Only the shape is checked here; the
/// startup path checks the folder itself.
fn check_spool_dir_path(dir: &Path) -> Result<(), String> {
    fsinfo::check_path_shape(dir).map_err(|reason| format!("spool-dir: {reason}"))
}

fn check_receiver_stall_seconds(seconds: u32) -> Result<u32, String> {
    if !RECEIVER_STALL_SECONDS_RANGE.contains(&i64::from(seconds)) {
        return Err(format!(
            "receiver-stall-seconds must be between {} and {}",
            RECEIVER_STALL_SECONDS_RANGE.start(),
            RECEIVER_STALL_SECONDS_RANGE.end()
        ));
    }
    Ok(seconds)
}

fn check_spool_sizes(max_mib: u64, min_free_mib: u64) -> Result<(), String> {
    if !SPOOL_MAX_MIB_RANGE.contains(&max_mib) {
        return Err(format!(
            "spool-max-mib must be between {} and {}",
            SPOOL_MAX_MIB_RANGE.start(),
            SPOOL_MAX_MIB_RANGE.end()
        ));
    }
    if !SPOOL_MIN_FREE_MIB_RANGE.contains(&min_free_mib) {
        return Err(format!(
            "spool-min-free-mib must be between {} and {}",
            SPOOL_MIN_FREE_MIB_RANGE.start(),
            SPOOL_MIN_FREE_MIB_RANGE.end()
        ));
    }
    Ok(())
}

/// The default spool folder, `<ProgramData>\EtwDnstap\spool`. `program_data` is
/// the value of the `ProgramData` environment variable; an unset value, a
/// relative one, or one that is not a plain path on a local drive letter (a
/// UNC or device path) falls back to `C:\ProgramData`. Taking the value as a parameter lets
/// tests check the result without touching the real folder.
pub(super) fn default_spool_dir(program_data: Option<OsString>) -> PathBuf {
    program_data
        .map(PathBuf::from)
        .filter(|base| fsinfo::check_path_shape(base).is_ok())
        .unwrap_or_else(|| PathBuf::from(FALLBACK_PROGRAM_DATA))
        .join("EtwDnstap")
        .join("spool")
}

/// The spool settings of a run that uses a spool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpoolSettings {
    /// The spool folder; absolute. The startup path checks that it is trusted.
    pub(crate) dir: PathBuf,
    /// Cap on segment bytes, in MiB.
    pub(crate) max_mib: u64,
    /// Free space to keep on the volume, in MiB.
    pub(crate) min_free_mib: u64,
}

impl SpoolSettings {
    /// The cap in bytes. The range check keeps the product far below `u64::MAX`.
    pub(crate) const fn max_bytes(&self) -> u64 {
        self.max_mib.saturating_mul(BYTES_PER_MIB)
    }

    /// The free-space floor in bytes.
    pub(crate) const fn min_free_bytes(&self) -> u64 {
        self.min_free_mib.saturating_mul(BYTES_PER_MIB)
    }
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
    /// Seconds the receiver may accept no bytes, with data waiting, before the
    /// session is ended.
    pub(crate) receiver_stall_seconds: u32,
    /// Seconds between counter snapshots; zero disables them.
    pub(crate) stats_interval_seconds: u32,
    /// `None` runs without a spool (`--no-spool`).
    pub(crate) spool: Option<SpoolSettings>,
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
        check_receiver_stall_seconds(self.receiver_stall_seconds).map_err(anyhow::Error::msg)?;
        if let Some(spool) = &self.spool {
            check_spool_dir_path(&spool.dir).map_err(anyhow::Error::msg)?;
            check_spool_sizes(spool.max_mib, spool.min_free_mib).map_err(anyhow::Error::msg)?;
        }
        SessionConfig::new(
            self.session.buffer_kib,
            self.session.minimum_buffers,
            self.session.maximum_buffers,
            self.session.flush_seconds,
        )?;
        Ok(())
    }

    /// How long the receiver may accept no bytes, while data waits, before the
    /// link ends the session.
    pub(crate) fn receiver_stall(&self) -> Duration {
        Duration::from_secs(u64::from(self.receiver_stall_seconds))
    }

    /// The counter snapshot interval, or `None` when disabled.
    pub(super) fn stats_interval(&self) -> Option<Duration> {
        (self.stats_interval_seconds != 0)
            .then(|| Duration::from_secs(u64::from(self.stats_interval_seconds)))
    }

    /// True when the ETW buffer and flush settings are the defaults.
    pub(super) fn uses_default_etw_settings(&self) -> bool {
        self.session == SessionConfig::default()
    }
}

impl TryFrom<CaptureArgs> for PipelineConfig {
    type Error = anyhow::Error;

    fn try_from(args: CaptureArgs) -> Result<Self> {
        let spool = (!args.no_spool).then(|| SpoolSettings {
            dir: args
                .spool_dir
                .unwrap_or_else(|| default_spool_dir(std::env::var_os("ProgramData"))),
            max_mib: args.spool_max_mib,
            min_free_mib: args.spool_min_free_mib,
        });
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
            receiver_stall_seconds: args.receiver_stall_seconds,
            stats_interval_seconds: args.stats_interval_seconds,
            spool,
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
/// to report argument errors. The help and version flags are disabled, so they
/// are unknown arguments like any other.
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
