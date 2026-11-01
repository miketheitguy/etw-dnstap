//! A per-session named mutex that keeps a second instance from taking over a
//! running instance's ETW session.

use anyhow::{Result, bail};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
    System::Threading::CreateMutexW,
};

/// Holds the named mutex until dropped. The mutex is only an existence marker;
/// it is never waited on.
pub(super) struct InstanceLock(HANDLE);

// SAFETY: The handle is an owned kernel object handle with no thread affinity;
// it is only closed in Drop.
unsafe impl Send for InstanceLock {}

/// `Global\EtwDnstap-<session>`, with backslashes in the session name replaced
/// because a kernel object name cannot contain them.
pub(super) fn mutex_name(session: &str) -> Vec<u16> {
    format!("Global\\EtwDnstap-{}", session.replace('\\', "_"))
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

impl InstanceLock {
    /// Takes the lock for `session`.
    ///
    /// # Errors
    /// Fails if another instance already holds it or the mutex cannot be created.
    pub(super) fn acquire(session: &str) -> Result<Self> {
        let name = mutex_name(session);
        // SAFETY: Null attributes select the default security descriptor; `name`
        // is NUL-terminated and outlives the call.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        // SAFETY: Read immediately after CreateMutexW, before any other API call.
        let error = unsafe { GetLastError() };
        if handle.is_null() {
            if error == ERROR_ACCESS_DENIED {
                bail!(
                    "cannot create the instance lock for session '{session}' \
                     (CreateMutexW on Global\\EtwDnstap-{session}, access denied): either another \
                     process or account owns a mutex of that name, or the write-restricted \
                     service token cannot open an existing one because its DACL does not grant \
                     the service SID NT SERVICE\\EtwDnstap or Everyone; stop the other \
                     EtwDnstap, or see scripts\\provision\\README.md"
                );
            }
            bail!("CreateMutexW for the instance lock failed (code {error})");
        }
        let lock = Self(handle);
        if error == ERROR_ALREADY_EXISTS {
            // Dropping `lock` closes only this process's handle to the object.
            bail!(
                "another EtwDnstap instance is already running for ETW session '{session}'; \
                 use a different --session-name or stop the other instance"
            );
        }
        Ok(lock)
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // SAFETY: CreateMutexW transferred this handle; it is closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_of_the_same_session_is_refused() {
        let session = format!("lock-test-{}", std::process::id());
        let first = InstanceLock::acquire(&session).expect("first instance");
        assert!(InstanceLock::acquire(&session).is_err());
        drop(first);
        assert!(InstanceLock::acquire(&session).is_ok());
    }

    #[test]
    fn mutex_name_is_global_and_sanitized() {
        let name = String::from_utf16_lossy(&mutex_name("a\\b"));
        assert_eq!(name, "Global\\EtwDnstap-a_b\0");
    }
}
