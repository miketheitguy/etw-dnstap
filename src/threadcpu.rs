//! CPU time of the four pipeline threads, as a percentage of one logical
//! processor and as a cumulative total.
//!
//! The pipeline has four threads that can each saturate a core: the ETW
//! consumer (intake), the parser, the sender (router), and the link (socket
//! writes). Each stage owns a [`ThreadCpu`]. The code that spawns a thread
//! hands its `JoinHandle` to [`ThreadCpu::register`]; the supervisor reads the
//! totals once for each publication tick and turns the differences into
//! percentages with [`CpuSampler`].
//!
//! The pipeline threads run no code of this module and never wait for the
//! sampler. The sampler calls `GetThreadTimes` on a handle that this module
//! owns.
//!
//! # Handles
//!
//! `register` duplicates the thread's handle with the standard library
//! (`BorrowedHandle::try_clone_to_owned`, which calls `DuplicateHandle`), so no
//! `unsafe` code duplicates or closes a handle and the duplicate closes itself
//! on drop. The duplicate keeps the thread's kernel object alive after the
//! thread ends and after `JoinHandle::join`, so the total stays readable and a
//! thread that ended has an unchanged total. The duplicate has the access
//! rights of the handle `CreateThread` returned, more than
//! `THREAD_QUERY_LIMITED_INFORMATION`; it never leaves the process.
//!
//! A stage whose thread starts again calls `register` again. The previous
//! thread's final time is added to the stage's retired time and its handle is
//! closed, so the total keeps growing across restarts.

use std::{
    os::windows::io::{AsHandle, AsRawHandle, OwnedHandle},
    sync::{Mutex, MutexGuard, PoisonError},
    thread::JoinHandle,
    time::Instant,
};

use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetThreadTimes};

/// Index of the ETW consumer thread (`etw-consumer`) in a [`StageCpu`] array.
pub(crate) const ETW_CONSUMER: usize = 0;
/// Index of the parser thread (`dnstap-parser`).
pub(crate) const PARSER: usize = 1;
/// Index of the sender thread (`dnstap-sender`, the router).
pub(crate) const SENDER: usize = 2;
/// Index of the link thread (`dnstap-link`, the socket writes).
pub(crate) const LINK: usize = 3;
/// The number of measured stages.
pub(crate) const STAGE_COUNT: usize = 4;

/// Units of 100 ns in one millisecond.
const UNITS_PER_MILLISECOND: u64 = 10_000;

/// The CPU time of one stage's thread.
#[derive(Default)]
pub(crate) struct ThreadCpu {
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    /// The current thread, or the last one after it ended.
    handle: Option<OwnedHandle>,
    /// CPU time of earlier threads of this stage, in units of 100 ns.
    retired_100ns: u64,
}

impl ThreadCpu {
    /// Starts measuring `thread`. Call it once after spawning. A failure to
    /// duplicate the handle is logged at debug level and leaves the stage
    /// unmeasured (its total stays at the earlier value).
    pub(crate) fn register<T>(&self, thread: &JoinHandle<T>) {
        match thread.as_handle().try_clone_to_owned() {
            Ok(handle) => self.install(handle),
            Err(error) => tracing::debug!(%error, "cannot measure the CPU time of a thread"),
        }
    }

    /// Makes `handle` the measured thread. The previous thread's final time
    /// moves to the retired total, and its handle closes.
    fn install(&self, handle: OwnedHandle) {
        let mut slot = self.lock();
        if let Some(previous) = slot.handle.replace(handle) {
            let final_time = thread_time_100ns(&previous).unwrap_or(0);
            slot.retired_100ns = slot.retired_100ns.saturating_add(final_time);
        }
    }

    /// Kernel plus user time of every thread of this stage, in units of 100 ns.
    /// Zero before a thread registers. A thread that ended keeps its final
    /// time. Takes a lock that only registration also takes, for the length of
    /// one system call.
    pub(crate) fn cumulative_100ns(&self) -> u64 {
        let slot = self.lock();
        let current = slot.handle.as_ref().and_then(thread_time_100ns);
        slot.retired_100ns.saturating_add(current.unwrap_or(0))
    }

    fn lock(&self) -> MutexGuard<'_, Slot> {
        // The slot holds plain data, so a poisoned lock is still consistent.
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Kernel plus user time of the thread, or `None` if the call fails.
fn thread_time_100ns(thread: &OwnedHandle) -> Option<u64> {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: `thread` is an open thread handle that this process owns and that
    // cannot close during the call, and the four pointers refer to distinct
    // locals that outlive the call. A handle without the access right makes the
    // call fail, not misbehave.
    let succeeded = unsafe {
        GetThreadTimes(
            thread.as_raw_handle(),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    (succeeded != 0).then(|| filetime_100ns(kernel).saturating_add(filetime_100ns(user)))
}

const fn filetime_100ns(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

/// One stage over one sampling interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StageCpu {
    /// CPU time the stage used in the interval, in units of 100 ns.
    pub(crate) used_100ns: u64,
    /// Length of the interval, in units of 100 ns.
    pub(crate) elapsed_100ns: u64,
    /// CPU time the stage has used since it started, in milliseconds.
    pub(crate) total_ms: u64,
}

impl StageCpu {
    /// CPU time over the interval as a percentage of one logical processor:
    /// 100 is one core fully busy, as in `\Process\% Processor Time`. Zero for
    /// an empty interval. The operating system accounts thread time in clock
    /// ticks, so a short interval can read a point or two high or low.
    #[must_use]
    pub(crate) fn percent(self) -> f64 {
        if self.elapsed_100ns == 0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a percentage; the times stay far below 2^52 units of 100 ns per interval"
        )]
        let percent = self.used_100ns as f64 / self.elapsed_100ns as f64 * 100.0;
        percent
    }
}

/// Turns successive cumulative totals into [`StageCpu`] intervals. Each log or
/// publication schedule has its own sampler, so each interval runs from that
/// schedule's previous sample.
pub(crate) struct CpuSampler {
    at: Instant,
    totals_100ns: [u64; STAGE_COUNT],
}

impl CpuSampler {
    /// Starts an interval at `now` with the stages' current totals.
    pub(crate) const fn new(now: Instant, totals_100ns: [u64; STAGE_COUNT]) -> Self {
        Self {
            at: now,
            totals_100ns,
        }
    }

    /// Ends the interval at `now`, with the stages' totals, and starts the
    /// next. A total that went down (a thread whose time could not be read) is
    /// treated as no CPU use, and a clock that went back as an empty interval.
    pub(crate) fn sample(
        &mut self,
        now: Instant,
        totals_100ns: [u64; STAGE_COUNT],
    ) -> [StageCpu; STAGE_COUNT] {
        let elapsed = now.saturating_duration_since(self.at).as_nanos() / 100;
        let elapsed_100ns = u64::try_from(elapsed).unwrap_or(u64::MAX);
        let previous = std::mem::replace(&mut self.totals_100ns, totals_100ns);
        self.at = self.at.max(now);
        std::array::from_fn(|stage| StageCpu {
            used_100ns: totals_100ns[stage].saturating_sub(previous[stage]),
            elapsed_100ns,
            total_ms: totals_100ns[stage] / UNITS_PER_MILLISECOND,
        })
    }
}

#[cfg(test)]
#[path = "threadcpu_tests.rs"]
mod threadcpu_tests;
