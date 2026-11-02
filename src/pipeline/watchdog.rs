//! The startup and shutdown watchdogs and the time budgets they enforce.

use std::{
    sync::mpsc::{self, Sender},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};

use super::config::CaptureArgs;
use crate::etw::PoolManager;

pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Startup budget before `--capacity` is known: the identity checks, the
/// argument hand-over, the receiver connection, and the capture start.
pub(crate) const STARTUP_BASE_BUDGET: Duration = Duration::from_secs(10);

/// Startup budget: the base budget plus one second per
/// [`PoolManager::PROGRESS_INTERVAL`] channel slots, because allocating and
/// touching the event pool (about 2.1 KiB per slot, up to roughly 560 MB)
/// dominates startup at large `--capacity`. The pool reports progress once per
/// interval, so the service refreshes its SCM checkpoint at least once per
/// budgeted second of allocation.
pub(crate) fn startup_budget(args: &CaptureArgs) -> Duration {
    let slots = u64::try_from(args.capacity).unwrap_or(u64::MAX);
    let interval = u64::try_from(PoolManager::PROGRESS_INTERVAL).unwrap_or(u64::MAX);
    STARTUP_BASE_BUDGET + Duration::from_secs(slots.div_ceil(interval))
}

/// Records the abort in the Event Log (best effort, bounded) and terminates.
fn fail_stop(phase: &str, timeout: Duration) {
    crate::service::report_watchdog_expiry(&format!(
        "EtwDnstap watchdog: the {phase} deadline of {} s expired; aborting the process so \
         the Service Control Manager can apply its recovery actions",
        timeout.as_secs()
    ));
    std::process::abort();
}

/// Holds the shutdown deadline until all native callbacks and network writes end.
pub(crate) struct ShutdownWatchdog {
    disarm: Sender<()>,
    worker: thread::JoinHandle<()>,
}

impl ShutdownWatchdog {
    pub(crate) fn arm() -> Self {
        Self::arm_for("shutdown", SHUTDOWN_TIMEOUT)
    }

    /// Arms a watchdog for `phase` ("startup" or "shutdown") with `timeout`. The
    /// Event Log source is opened now, so the expiry path never has to.
    pub(crate) fn arm_for(phase: &'static str, timeout: Duration) -> Self {
        crate::service::preopen_event_source();
        match Self::spawn(timeout, move || fail_stop(phase, timeout)) {
            Ok(watchdog) => watchdog,
            // Failure reporting can itself block; the fail-stop path performs
            // no logging or cleanup before process termination.
            Err(_) => std::process::abort(),
        }
    }

    fn spawn(
        timeout: Duration,
        on_expiry: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (disarm, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("shutdown-watchdog".into())
            .spawn(move || {
                // Disconnection also fails closed: only explicit completion
                // disarms the watchdog, never dropping a native worker handle.
                if receive.recv_timeout(timeout).is_err() {
                    on_expiry();
                }
            })?;
        Ok(Self { disarm, worker })
    }

    pub(crate) fn disarm(self) -> Result<()> {
        self.disarm.send(()).context("disarm shutdown watchdog")?;
        self.worker
            .join()
            .map_err(|_| anyhow::anyhow!("shutdown watchdog panicked"))
    }
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod watchdog_tests;
