//! Supervise ingestion and parsing, with bounded memory and explicit loss counts.
//!
//! The pipeline is split by responsibility:
//!
//! - `config`: the command line ([`CaptureArgs`]), defaults, and the validated
//!   runtime settings.
//! - `watchdog`: the startup and shutdown watchdogs and their time budgets.
//! - `supervisor`: [`run_capture`], which starts the sink, the parser thread, and
//!   the capture, supervises them until shutdown, then stops and reports.
//! - `alerts`: the loss and risk checks that the supervisor runs on its stats tick and
//!   that write the Event Log warnings with IDs 10 to 28.
//! - `parser`: the parser thread, which drains the event channel, encodes each
//!   record, and writes and flushes the sink.

mod alerts;
mod config;
mod parser;
mod supervisor;
#[cfg(test)]
mod test_support;
mod watchdog;

pub(crate) use config::{CaptureArgs, parse_args};
pub(crate) use parser::ParseCounters;
// Reached by the parse and encode benchmark and the schema tests only.
#[cfg(any(test, feature = "bench"))]
pub(crate) use parser::parse_and_encode;
pub(crate) use supervisor::run_capture;
pub(crate) use watchdog::{
    SHUTDOWN_TIMEOUT, STARTUP_BASE_BUDGET, ShutdownWatchdog, startup_budget,
};
