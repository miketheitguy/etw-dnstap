//! The watchdog disarm and expiry paths and the startup budget.

use super::*;
use crate::pipeline::config::parse_args_from;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn watchdog_disarms_only_after_explicit_completion() {
    let expired = Arc::new(AtomicBool::new(false));
    let observe = Arc::clone(&expired);
    let watchdog = ShutdownWatchdog::spawn(Duration::from_secs(5), move || {
        observe.store(true, Ordering::Relaxed);
    })
    .expect("watchdog starts");
    watchdog.disarm().expect("completion disarms watchdog");
    assert!(!expired.load(Ordering::Relaxed));
}

#[test]
fn watchdog_reports_an_exceeded_shutdown_deadline() {
    let expired = Arc::new(AtomicBool::new(false));
    let observe = Arc::clone(&expired);
    let watchdog = ShutdownWatchdog::spawn(Duration::ZERO, move || {
        observe.store(true, Ordering::Relaxed);
    })
    .expect("watchdog starts");
    watchdog.worker.join().expect("watchdog finishes");
    assert!(expired.load(Ordering::Relaxed));
}

#[test]
fn startup_budget_scales_with_capacity() {
    let budget = |capacity: &str| {
        startup_budget(
            &parse_args_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:6000",
                "--capacity",
                capacity,
            ])
            .expect("valid arguments"),
        )
    };
    assert_eq!(budget("8192"), STARTUP_BASE_BUDGET + Duration::from_secs(1));
    assert_eq!(
        budget("16384"),
        STARTUP_BASE_BUDGET + Duration::from_secs(1)
    );
    assert_eq!(
        budget("16385"),
        STARTUP_BASE_BUDGET + Duration::from_secs(2)
    );
    assert_eq!(
        budget("65536"),
        STARTUP_BASE_BUDGET + Duration::from_secs(4)
    );
    // The default capacity.
    assert_eq!(
        budget("131072"),
        STARTUP_BASE_BUDGET + Duration::from_secs(8)
    );
    assert_eq!(budget("131072"), Duration::from_secs(18));
    assert_eq!(
        budget("262144"),
        STARTUP_BASE_BUDGET + Duration::from_secs(16)
    );
    assert_eq!(budget("262144"), Duration::from_secs(26));
    assert_eq!(
        budget("524288"),
        STARTUP_BASE_BUDGET + Duration::from_secs(32)
    );
    assert_eq!(budget("524288"), Duration::from_secs(42));
    assert!(budget("524288") > budget("262144"));
    assert!(budget("262144") > budget("1024"));
    // One budgeted second per progress report, so the service refreshes its
    // SCM checkpoint inside every second of the allocation budget.
    assert_eq!(PoolManager::PROGRESS_INTERVAL, 16_384);
}
