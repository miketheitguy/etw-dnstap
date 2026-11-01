//! Diagnostic-only thread priority and timer resolution helpers.

use windows_sys::Win32::Foundation::GetLastError;

/// Diagnostic thread priority levels; compiled only for the diagnostic build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ThreadPriority {
    AboveNormal,
    Highest,
}

impl ThreadPriority {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AboveNormal => "above-normal",
            Self::Highest => "highest",
        }
    }
}

/// Sets the calling thread's priority. Failure is logged once and ignored,
/// because the setting is a diagnostic experiment, not a capture requirement.
pub(crate) fn set_current_thread_priority(priority: ThreadPriority) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_HIGHEST,
    };
    let native = match priority {
        ThreadPriority::AboveNormal => THREAD_PRIORITY_ABOVE_NORMAL,
        ThreadPriority::Highest => THREAD_PRIORITY_HIGHEST,
    };
    // SAFETY: GetCurrentThread returns a pseudo-handle valid only for the
    // calling thread, which is the thread being adjusted; it needs no closing.
    let succeeded = unsafe { SetThreadPriority(GetCurrentThread(), native) } != 0;
    if !succeeded {
        // SAFETY: GetLastError is read immediately after the failing API call.
        let error = unsafe { GetLastError() };
        tracing::warn!(
            priority = priority.as_str(),
            error = %std::io::Error::from_raw_os_error(error.cast_signed()),
            "SetThreadPriority failed; continuing at the default priority"
        );
    }
}

/// Raises the process timer resolution request to `milliseconds`. Returns the
/// `MMRESULT`, which is zero on success.
pub(crate) fn time_begin_period(milliseconds: u32) -> u32 {
    // SAFETY: timeBeginPeriod takes a plain integer and has no pointer arguments.
    unsafe { windows_sys::Win32::Media::timeBeginPeriod(milliseconds) }
}

/// Withdraws a request made by `time_begin_period` with the same value.
pub(crate) fn time_end_period(milliseconds: u32) -> u32 {
    // SAFETY: timeEndPeriod takes a plain integer and has no pointer arguments.
    unsafe { windows_sys::Win32::Media::timeEndPeriod(milliseconds) }
}

/// The success value of the `timeBeginPeriod` family.
pub(crate) const TIME_NO_ERROR: u32 = windows_sys::Win32::Media::TIMERR_NOERROR;
