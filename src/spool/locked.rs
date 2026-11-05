//! One value in a locked page of its own, for the spool key schedule.
//!
//! [`Locked`] moves a value into a page that `VirtualAlloc` commits for it alone.
//! `VirtualLock` keeps the page in physical memory, so the memory manager never
//! writes it to the pagefile, and `WerRegisterExcludedMemoryBlock` (best effort)
//! leaves it out of Windows Error Reporting (WER) crash dumps. On drop the
//! value is dropped in place, the whole page is overwritten with zeros that the
//! compiler cannot remove, and the page is unlocked, unregistered, and freed.
//!
//! Limits: the value is built on the caller's stack and copied into the page,
//! so stack residue of it can remain. SYSTEM, Administrators, and code in this
//! process can read the page while it exists. The page is in physical memory, so
//! it is in a VM saved state or memory checkpoint, in `hiberfil.sys`, and in a
//! complete kernel memory dump; the WER exclusion covers WER dumps only. A
//! failed `VirtualLock` is an error: the value is never kept in pageable memory.

use std::{
    fmt, io,
    marker::PhantomData,
    mem,
    ops::Deref,
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
const PAGE_BYTES: usize = PAGE_BYTES_U32 as usize;
/// [`PAGE_BYTES`] as the `u32` that `WerRegisterExcludedMemoryBlock` takes.
const PAGE_BYTES_U32: u32 = 4096;

/// Why a value could not be put in a locked page. Each variant carries the
/// Win32 error code of the failed call.
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

/// A `T` alone in a committed, locked page that is zeroed and freed on drop.
pub(crate) struct Locked<T> {
    /// The start of the page, which holds the `T`.
    value: NonNull<T>,
    /// The page is registered with Windows Error Reporting as excluded.
    excluded: bool,
    _owns: PhantomData<T>,
}

// SAFETY: `Locked<T>` owns its `T` as a `Box<T>` would; the page is private to
// it. Sending it sends the `T`.
unsafe impl<T: Send> Send for Locked<T> {}
// SAFETY: `Locked<T>` hands out only `&T`, so sharing it shares a `&T`.
unsafe impl<T: Sync> Sync for Locked<T> {}

impl<T> Locked<T> {
    /// Moves `value` into a new locked page.
    ///
    /// # Errors
    /// [`LockError::Alloc`] or [`LockError::Lock`]. `value` is dropped then.
    pub(crate) fn new(value: T) -> Result<Self, LockError> {
        const {
            assert!(mem::size_of::<T>() <= PAGE_BYTES);
            assert!(mem::align_of::<T>() <= PAGE_BYTES);
        }
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
        let slot = page.cast::<T>();
        // SAFETY: the page is writable and holds no value. It is page-aligned,
        // and the const assertions above show a `T` fits and its alignment
        // divides the page alignment.
        unsafe { slot.as_ptr().write(value) };
        Ok(Self {
            value: slot,
            excluded,
            _owns: PhantomData,
        })
    }

    /// Whether Windows Error Reporting accepted the page as excluded from dumps.
    #[cfg(test)]
    pub(crate) const fn is_excluded_from_dumps(&self) -> bool {
        self.excluded
    }
}

impl<T> Deref for Locked<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: `new` wrote a valid `T` there, and it lives until drop.
        unsafe { self.value.as_ref() }
    }
}

impl<T> Drop for Locked<T> {
    fn drop(&mut self) {
        // SAFETY: the `T` is valid and is dropped only here, once.
        unsafe { ptr::drop_in_place(self.value.as_ptr()) };
        let base = self.value.as_ptr().cast::<u8>();
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

impl<T> fmt::Debug for Locked<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Locked").finish_non_exhaustive()
    }
}

/// The calling thread's last Win32 error code.
fn last_error() -> u32 {
    io::Error::last_os_error()
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn a_value_is_stored_page_aligned_and_read_back() {
        let locked = Locked::new([7_u8; 300]).expect("one page locks");
        assert_eq!(locked[0], 7);
        assert_eq!(locked[299], 7);
        let address = ptr::from_ref::<[u8; 300]>(&locked) as usize;
        assert_eq!(address % PAGE_BYTES, 0, "the value is at the page start");
        // Best effort, but available on every supported Windows version.
        assert!(locked.is_excluded_from_dumps());
    }

    #[test]
    fn drop_runs_the_value_drop_once() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Counted;
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        let locked = Locked::new(Counted).expect("one page locks");
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        drop(locked);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
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
