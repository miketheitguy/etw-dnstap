use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::*;

/// Units of 100 ns in one second.
const SECOND: u64 = 10_000_000;

fn totals(etw: u64, parser: u64, sender: u64, link: u64) -> [u64; STAGE_COUNT] {
    let mut totals = [0; STAGE_COUNT];
    totals[ETW_CONSUMER] = etw;
    totals[PARSER] = parser;
    totals[SENDER] = sender;
    totals[LINK] = link;
    totals
}

#[test]
fn stage_indexes_are_distinct_and_inside_the_array() {
    let mut indexes = [ETW_CONSUMER, PARSER, SENDER, LINK];
    indexes.sort_unstable();
    assert_eq!(indexes, [0, 1, 2, 3]);
    assert_eq!(STAGE_COUNT, 4);
}

#[test]
fn one_busy_core_is_one_hundred_percent() {
    let start = Instant::now();
    let mut sampler = CpuSampler::new(start, totals(0, 0, 0, 0));
    let stages = sampler.sample(
        start + Duration::from_secs(1),
        totals(SECOND, SECOND / 2, SECOND / 10, 0),
    );
    assert_eq!(stages[ETW_CONSUMER].percent(), 100.0);
    assert_eq!(stages[PARSER].percent(), 50.0);
    assert_eq!(stages[SENDER].percent(), 10.0);
    assert_eq!(stages[LINK].percent(), 0.0);
    for stage in stages {
        assert_eq!(stage.elapsed_100ns, SECOND);
    }
}

#[test]
fn the_interval_runs_from_the_previous_sample() {
    let start = Instant::now();
    let mut sampler = CpuSampler::new(start, totals(5 * SECOND, 0, 0, 0));
    let first = sampler.sample(start + Duration::from_secs(10), totals(6 * SECOND, 0, 0, 0));
    assert_eq!(first[ETW_CONSUMER].percent(), 10.0);
    // The second interval is 2 s long and the stage used 1 s of it.
    let second = sampler.sample(start + Duration::from_secs(12), totals(7 * SECOND, 0, 0, 0));
    assert_eq!(second[ETW_CONSUMER].used_100ns, SECOND);
    assert_eq!(second[ETW_CONSUMER].elapsed_100ns, 2 * SECOND);
    assert_eq!(second[ETW_CONSUMER].percent(), 50.0);
}

#[test]
fn a_thread_that_ended_reads_zero_after_its_last_interval() {
    let start = Instant::now();
    let mut sampler = CpuSampler::new(start, totals(0, 0, 0, 0));
    // The thread ran for 0.4 s of the first second, then ended: its total stays.
    let running = sampler.sample(
        start + Duration::from_secs(1),
        totals(0, 0, 0, SECOND / 5 * 2),
    );
    assert_eq!(running[LINK].percent(), 40.0);
    let ended = sampler.sample(
        start + Duration::from_secs(2),
        totals(0, 0, 0, SECOND / 5 * 2),
    );
    assert_eq!(ended[LINK].used_100ns, 0);
    assert_eq!(ended[LINK].percent(), 0.0);
    // The cumulative time still reports what the thread used.
    assert_eq!(ended[LINK].total_ms, 400);
}

#[test]
fn a_zero_interval_reads_zero_and_does_not_divide_by_zero() {
    let start = Instant::now();
    let mut sampler = CpuSampler::new(start, totals(0, 0, 0, 0));
    let stages = sampler.sample(start, totals(SECOND, 0, 0, 0));
    assert_eq!(stages[ETW_CONSUMER].elapsed_100ns, 0);
    assert_eq!(stages[ETW_CONSUMER].percent(), 0.0);
    // The time is not lost: the next interval counts from the new total.
    let next = sampler.sample(start + Duration::from_secs(1), totals(SECOND, 0, 0, 0));
    assert_eq!(next[ETW_CONSUMER].used_100ns, 0);
}

#[test]
fn a_clock_that_went_back_is_an_empty_interval() {
    let start = Instant::now() + Duration::from_secs(10);
    let mut sampler = CpuSampler::new(start, totals(0, 0, 0, 0));
    let stages = sampler.sample(Instant::now(), totals(SECOND, 0, 0, 0));
    assert_eq!(stages[ETW_CONSUMER].elapsed_100ns, 0);
    assert_eq!(stages[ETW_CONSUMER].percent(), 0.0);
}

#[test]
fn a_total_that_went_down_is_no_use() {
    let start = Instant::now();
    let mut sampler = CpuSampler::new(start, totals(3 * SECOND, 0, 0, 0));
    let stages = sampler.sample(start + Duration::from_secs(1), totals(SECOND, 0, 0, 0));
    assert_eq!(stages[ETW_CONSUMER].used_100ns, 0);
    assert_eq!(stages[ETW_CONSUMER].total_ms, 1_000);
}

#[test]
fn a_busy_stage_can_read_slightly_over_one_hundred() {
    // Clock-tick accounting; the value is not clamped.
    let stage = StageCpu {
        used_100ns: SECOND + SECOND / 50,
        elapsed_100ns: SECOND,
        total_ms: 0,
    };
    assert!((stage.percent() - 102.0).abs() < 1e-9);
}

#[test]
fn cumulative_time_is_milliseconds() {
    let start = Instant::now();
    let mut sampler = CpuSampler::new(start, totals(0, 0, 0, 0));
    let stages = sampler.sample(
        start + Duration::from_secs(1),
        totals(12_345 * 10_000 + 9_999, 0, 0, 0),
    );
    assert_eq!(stages[ETW_CONSUMER].total_ms, 12_345);
}

/// Burns CPU on the calling thread until it has used about `target` of wall
/// time, which is enough CPU time for the clock-tick accounting to see.
fn spin_for(target: Duration) {
    let start = Instant::now();
    let mut value = 1u64;
    while start.elapsed() < target {
        value = std::hint::black_box(
            value
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1),
        );
    }
}

#[test]
fn an_unregistered_stage_reads_zero() {
    assert_eq!(ThreadCpu::default().cumulative_100ns(), 0);
}

#[test]
fn a_registered_thread_reports_its_cpu_time_and_keeps_it_after_it_ends() {
    let cpu = ThreadCpu::default();
    let worker = thread::spawn(|| spin_for(Duration::from_millis(300)));
    cpu.register(&worker);
    worker.join().expect("the spinning thread ends");
    let after_join = cpu.cumulative_100ns();
    // 300 ms of spinning is 3,000,000 units; accept anything above 100 ms.
    assert!(after_join > SECOND / 10, "{after_join}");
    // The handle outlives the thread and the join, and the total no longer moves.
    thread::sleep(Duration::from_millis(50));
    assert_eq!(cpu.cumulative_100ns(), after_join);
}

#[test]
fn an_idle_thread_uses_almost_no_cpu() {
    let cpu = ThreadCpu::default();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(10));
        }
    });
    cpu.register(&worker);
    thread::sleep(Duration::from_millis(300));
    let idle = cpu.cumulative_100ns();
    stop.store(true, Ordering::Relaxed);
    worker.join().expect("the sleeping thread ends");
    // Waking 30 times costs far less than the 300 ms of wall time.
    assert!(idle < SECOND / 10, "{idle}");
}

#[test]
fn a_restarted_stage_keeps_the_time_of_the_thread_before() {
    let cpu = ThreadCpu::default();
    let first = thread::spawn(|| spin_for(Duration::from_millis(300)));
    cpu.register(&first);
    first.join().expect("the first thread ends");
    let first_total = cpu.cumulative_100ns();
    assert!(first_total > SECOND / 10, "{first_total}");

    // A second thread replaces the first; the total does not fall back to the
    // new thread's time.
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let second = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(10));
        }
    });
    cpu.register(&second);
    let second_total = cpu.cumulative_100ns();
    assert!(
        second_total >= first_total,
        "{second_total} < {first_total}"
    );
    stop.store(true, Ordering::Relaxed);
    second.join().expect("the second thread ends");
}

#[test]
fn registering_a_replacement_closes_the_previous_handle() {
    let cpu = ThreadCpu::default();
    let first = thread::spawn(|| ());
    cpu.register(&first);
    let second = thread::spawn(|| ());
    cpu.register(&second);
    first.join().expect("ends");
    second.join().expect("ends");
    // One handle remains in the slot; the retired time holds the first thread's.
    let slot = cpu.lock();
    assert!(slot.handle.is_some());
}

#[test]
fn a_poisoned_lock_still_reads() {
    let cpu = Arc::new(ThreadCpu::default());
    let shared = Arc::clone(&cpu);
    let poisoner = thread::spawn(move || {
        let _slot = shared.slot.lock().expect("lock");
        panic!("poison the slot");
    });
    assert!(poisoner.join().is_err());
    assert_eq!(cpu.cumulative_100ns(), 0);
}
