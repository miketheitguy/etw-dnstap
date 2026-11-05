//! One locked page of its own, for the spool key (see [`super::protected`]).
//!
//! [`LockedPage`] is a page that `VirtualAlloc` commits for it alone, filled
//! with zeros. `VirtualLock` keeps the page in physical memory, so the memory
//! manager never writes it to the pagefile, and `WerRegisterExcludedMemoryBlock`
//! (best effort) leaves it out of Windows Error Reporting (WER) crash dumps. On
//! drop the whole page is overwritten with zeros that the compiler cannot
//! remove, and the page is unlocked, unregistered, and freed.
//!
//! The page holds raw bytes only. Its owner places values in it through
//! [`LockedPage::base`] and drops them before the page drops.
//!
//! Limits: SYSTEM, Administrators, and code in this process can read the page
//! while it exists. The page is in physical memory, so it is in a VM saved
//! state or memory checkpoint, in `hiberfil.sys`, and in a complete kernel
//! memory dump; the WER exclusion covers WER dumps only. A failed
//! `VirtualLock` is an error: the key is never kept in pageable memory.

use std::{
    fmt, io,
    ptr::{self, NonNull},
    sync::atomic::{Ordering, compiler_fence},
};

use windows_sys::Win32::System::{
    ErrorReporting::{WerRegisterExcludedMemoryBlock, WerUnregisterExcludedMemoryBlock},
    Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        VirtualLock, VirtualUnlock,
    },
};

/// Bytes in the allocation: one page on every Windows architecture.
pub(crate) const PAGE_BYTES: usize = PAGE_BYTES_U32 as usize;
/// [`PAGE_BYTES`] as the `u32` that `WerRegisterExcludedMemoryBlock` takes.
const PAGE_BYTES_U32: u32 = 4096;

/// Why a locked page could not be made. Each variant carries the Win32 error
/// code of the failed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LockError {
    /// `VirtualAlloc` failed.
    #[error("cannot allocate a page for the spool key (VirtualAlloc Win32 error {0})")]
    Alloc(u32),
    /// `VirtualLock` failed, for example because the process working set quota
    /// is used up (error 1453).
    #[error("cannot lock the spool key page in memory (VirtualLock Win32 error {0})")]
    Lock(u32),
}

/// A committed, locked, zero-filled page that is zeroed and freed on drop.
pub(crate) struct LockedPage {
    /// The start of the page.
    base: NonNull<u8>,
    /// The page is registered with Windows Error Reporting as excluded.
    excluded: bool,
}

// SAFETY: `LockedPage` owns its page as a `Box<[u8; PAGE_BYTES]>` would; no
// other value points into it. Sending it sends the page.
unsafe impl Send for LockedPage {}
// SAFETY: a shared `LockedPage` gives out only the raw base address. Every
// read or write through it is `unsafe`, and the owner (`protected.rs`)
// serializes them with a mutex.
unsafe impl Sync for LockedPage {}

impl LockedPage {
    /// A new committed, locked page. Its bytes are zero.
    ///
    /// # Errors
    /// [`LockError::Alloc`] or [`LockError::Lock`].
    pub(crate) fn new() -> Result<Self, LockError> {
        // SAFETY: a null address lets the system choose; the size and flags are
        // valid. The result is checked for null below.
        let page = unsafe {
            VirtualAlloc(
                ptr::null(),
                PAGE_BYTES,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        let Some(page) = NonNull::new(page) else {
            return Err(LockError::Alloc(last_error()));
        };
        // SAFETY: `page` is a committed region of `PAGE_BYTES` bytes.
        let locked = unsafe { VirtualLock(page.as_ptr(), PAGE_BYTES) };
        if locked == 0 {
            let code = last_error();
            // SAFETY: `page` is the base of the region `VirtualAlloc` returned;
            // size 0 with `MEM_RELEASE` frees all of it. Nothing points into it.
            unsafe { VirtualFree(page.as_ptr(), 0, MEM_RELEASE) };
            return Err(LockError::Lock(code));
        }
        // SAFETY: `page` and its length describe committed memory this value
        // owns until drop, where it is unregistered before it is freed.
        let excluded =
            unsafe { WerRegisterExcludedMemoryBlock(page.as_ptr(), PAGE_BYTES_U32) } >= 0;
        Ok(Self {
            base: page.cast(),
            excluded,
        })
    }

    /// The first byte of the page. The page is [`PAGE_BYTES`] bytes long,
    /// page-aligned, readable, and writable until this value drops.
    pub(crate) const fn base(&self) -> NonNull<u8> {
        self.base
    }

    /// Whether Windows Error Reporting accepted the page as excluded from dumps.
    #[cfg(test)]
    pub(crate) const fn is_excluded_from_dumps(&self) -> bool {
        self.excluded
    }
}

impl Drop for LockedPage {
    fn drop(&mut self) {
        let base = self.base.as_ptr();
        for at in 0..PAGE_BYTES {
            // SAFETY: `base + at` lies in the committed, writable page.
            let byte = unsafe { base.add(at) };
            // SAFETY: `byte` is a valid, aligned pointer into the page, and no
            // reference into the page exists any more.
            unsafe { ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
        // SAFETY: the page was locked in `new`. A failure leaves nothing to do.
        unsafe { VirtualUnlock(base.cast(), PAGE_BYTES) };
        if self.excluded {
            // SAFETY: the page was registered at this address in `new`.
            unsafe { WerUnregisterExcludedMemoryBlock(base.cast()) };
        }
        // SAFETY: `base` is the region `VirtualAlloc` returned, and nothing
        // points into it after this.
        unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) };
    }
}

impl fmt::Debug for LockedPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockedPage").finish_non_exhaustive()
    }
}

/// The calling thread's last Win32 error code.
pub(crate) fn last_error() -> u32 {
    io::Error::last_os_error()
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_aligned_zeroed_writable_and_excluded() {
        let page = LockedPage::new().expect("one page locks");
        let base = page.base().as_ptr();
        assert_eq!(base as usize % PAGE_BYTES, 0, "page-aligned");
        for at in [0, 1, PAGE_BYTES / 2, PAGE_BYTES - 1] {
            // SAFETY: `at` is inside the page.
            let byte = unsafe { base.add(at) };
            // SAFETY: `byte` is in the committed page, readable and writable.
            assert_eq!(unsafe { byte.read() }, 0, "zero-filled at {at}");
            // SAFETY: as above.
            unsafe { byte.write(7) };
            // SAFETY: as above.
            assert_eq!(unsafe { byte.read() }, 7);
        }
        // Best effort, but available on every supported Windows version.
        assert!(page.is_excluded_from_dumps());
    }

    #[test]
    fn lock_errors_name_the_call_and_the_code() {
        assert_eq!(
            LockError::Lock(1453).to_string(),
            "cannot lock the spool key page in memory (VirtualLock Win32 error 1453)"
        );
        assert!(LockError::Alloc(8).to_string().contains("VirtualAlloc"));
    }
}
