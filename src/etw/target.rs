//! Resolves the process that is allowed to emit DNS Server events.
//!
//! The DNS Server role runs as the Windows service `DNS`. Its process ID is read
//! from the Service Control Manager, and the ETW session and the callback both
//! admit only records from that process.

use std::{mem::size_of, ptr::null};

use anyhow::{Result, bail};
use windows_sys::Win32::{
    Foundation::GetLastError,
    System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
        SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_STATUS_PROCESS,
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

fn os_error(operation: &str) -> anyhow::Error {
    // SAFETY: Read immediately after the failing API call on this thread.
    let code = unsafe { GetLastError() };
    anyhow::anyhow!(
        "{operation}: {} (code {code})",
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
    fn missing_service_is_an_error() {
        assert!(service_process_id("EtwDnstapNoSuchService").is_err());
    }
}
