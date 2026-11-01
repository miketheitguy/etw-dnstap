//! Resolves the process that is allowed to emit DNS Server events.
//!
//! The DNS Server role runs as the Windows service `DNS`. Its process ID is read
//! from the Service Control Manager, and the ETW session and the callback both
//! admit only records from that process.

use std::{mem::size_of, ptr::null};

use anyhow::{Result, bail};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, WAIT_OBJECT_0},
    System::{
        Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
            SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS,
            SERVICE_STATUS_PROCESS,
        },
        Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            WaitForSingleObject,
        },
    },
};

/// The service whose process emits the DNS Server provider's events.
pub(super) const DNS_SERVICE: &str = "DNS";

/// Closes an SCM handle on drop.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: Each wrapper owns one handle returned by OpenSCManagerW or
        // OpenServiceW and closes it exactly once, after its last use.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

/// An open handle to the DNS Server process. While it is open the kernel cannot
/// reuse the process ID, so the PID filter stays bound to the process it was
/// built for even between supervisor polls.
pub(super) struct ProcessHandle(HANDLE);

// SAFETY: The handle is an owned kernel object handle with no thread affinity;
// waiting on it with a zero timeout is thread-safe, and it is closed only in Drop.
unsafe impl Send for ProcessHandle {}
// SAFETY: Only `WaitForSingleObject` is called through a shared reference, and
// the kernel serializes access to the process object.
unsafe impl Sync for ProcessHandle {}

impl ProcessHandle {
    /// Opens `pid` with the minimum rights to wait on it and query its state.
    ///
    /// # Errors
    /// Fails if the process cannot be opened (for example access denied or gone).
    pub(super) fn open(pid: u32) -> Result<Self> {
        // SAFETY: No pointers are passed; the returned handle, if any, is owned here.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(os_error(&format!("OpenProcess({pid})")));
        }
        Ok(Self(handle))
    }

    /// True once the process has terminated. A failed wait is inconclusive.
    pub(super) fn has_exited(&self) -> bool {
        // SAFETY: `self.0` is an open process handle with SYNCHRONIZE access; a
        // zero timeout polls without blocking.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: OpenProcess transferred this handle; it is closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn os_error(operation: &str) -> anyhow::Error {
    // SAFETY: Read immediately after the failing API call on this thread.
    let code = unsafe { GetLastError() };
    let hint = if code == ERROR_ACCESS_DENIED {
        // The restricted token passes a second access check against its
        // restricting SIDs, which include Everyone and the service SID.
        "; the restricted service token needs SC_MANAGER_CONNECT and \
         SERVICE_QUERY_STATUS on the DNS service for Everyone or NT SERVICE\\EtwDnstap"
    } else {
        ""
    };
    anyhow::anyhow!(
        "{operation}: {} (code {code}){hint}",
        std::io::Error::from_raw_os_error(code.cast_signed())
    )
}

/// Returns the process ID of the running service `name`.
///
/// # Errors
/// Fails if the service cannot be opened or queried, or is not running (a
/// stopped service reports process ID 0).
pub(super) fn service_process_id(name: &str) -> Result<u32> {
    running_pid(name, service_process_id_raw(name)?)
}

/// Returns the process ID the SCM reports for service `name`, which is 0 while
/// the service is stopped.
///
/// # Errors
/// Fails if the service cannot be opened or queried.
pub(super) fn service_process_id_raw(name: &str) -> Result<u32> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: Null machine and database names select the local active database.
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(os_error("OpenSCManagerW"));
    }
    let manager = ScHandle(manager);
    // SAFETY: `manager` is open and `wide` is NUL-terminated and outlives the call.
    let service = unsafe { OpenServiceW(manager.0, wide.as_ptr(), SERVICE_QUERY_STATUS) };
    if service.is_null() {
        return Err(os_error(&format!("OpenServiceW({name})")));
    }
    let service = ScHandle(service);
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "SERVICE_STATUS_PROCESS is 36 bytes"
    )]
    let size = size_of::<SERVICE_STATUS_PROCESS>() as u32;
    // SAFETY: `status` is a properly aligned, writable SERVICE_STATUS_PROCESS of
    // `size` bytes, as SC_STATUS_PROCESS_INFO requires; `service` is open.
    let success = unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&raw mut status).cast(),
            size,
            &raw mut needed,
        )
    };
    if success == 0 {
        return Err(os_error(&format!("QueryServiceStatusEx({name})")));
    }
    Ok(status.dwProcessId)
}

/// Rejects process ID 0, which the SCM reports for a service that is not running.
fn running_pid(name: &str, pid: u32) -> Result<u32> {
    if pid == 0 {
        bail!("service '{name}' is not running (process ID 0); start it before EtwDnstap");
    }
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_service_has_no_usable_pid() {
        assert!(running_pid("DNS", 0).is_err());
        assert_eq!(running_pid("DNS", 4242).expect("running"), 4242);
    }

    #[test]
    fn process_handle_observes_exit_of_a_child() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 60 127.0.0.1 >nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn child");
        let handle = ProcessHandle::open(child.id()).expect("open child");
        assert!(!handle.has_exited());
        child.kill().expect("kill child");
        child.wait().expect("reap child");
        assert!(handle.has_exited());
    }

    #[test]
    fn missing_service_is_an_error() {
        assert!(service_process_id("EtwDnstapNoSuchService").is_err());
    }
}
