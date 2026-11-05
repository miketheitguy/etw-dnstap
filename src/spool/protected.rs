//! The spool AES key, encrypted in RAM between uses (option A of
//! `docs/plans/spool-encryption-plan.md`).
//!
//! A [`SegmentKey`] owns one [`LockedPage`] (never written to the pagefile,
//! left out of WER crash dumps, zeroed on drop) with two areas:
//!
//! - **The 16 key bytes.** They are encrypted in place with
//!   `CryptProtectMemory(CRYPTPROTECTMEMORY_SAME_PROCESS)` at all times, except
//!   for the few instructions in [`SegmentKey::with_aead`] that build the key
//!   schedule from them.
//! - **A scratch area** where [`SegmentKey::with_aead`] builds the `ring`
//!   AES-128-GCM key for one seal or one open. It is all zeros between uses.
//!
//! [`SegmentKey::with_aead`] is the only way to use the key. Under a mutex (the
//! scratch area is shared), it calls `CryptUnprotectMemory`, checks the result
//! against the key's check value, builds the key in the scratch area, calls
//! `CryptProtectMemory` on the 16 bytes at once, runs the closure, then drops
//! the key in place and overwrites the scratch area with zeros. A guard does
//! the re-protect and the zeroing in `Drop`, so a panic in the closure cannot
//! leave plaintext, and the mutex is recovered after one.
//!
//! Failures:
//!
//! - `CryptUnprotectMemory` fails: the bytes are still encrypted, so nothing
//!   is zeroed. The use fails with [`CryptError::MemoryProtection`] and a later
//!   use tries again.
//! - `CryptUnprotectMemory` succeeds but the bytes do not match the check value
//!   (a SHA-256 of the key taken when it was loaded): the key would seal
//!   records that can never be read, so it is zeroed and lost
//!   ([`KeyLoss::Check`]).
//! - `CryptProtectMemory` fails: it is tried once more; if it fails again the
//!   16 bytes are zeroed and the key is lost ([`KeyLoss::Protect`]).
//!
//! A lost key stays lost for the run: every later use returns
//! [`CryptError::KeyZeroed`], and [`SegmentKey::loss`] says why. The spool then
//! suspends itself and keeps its files; the next start unwraps the key again.
//!
//! The key reaches the page without passing through any other buffer:
//! `NCryptDecrypt` (load) and `BCryptGenRandom` (first start) write into
//! [`KeyBuffer::bytes_mut`], which is the key area of the page, and
//! [`KeyBuffer::protect`] encrypts it in place and turns the page into the
//! [`SegmentKey`].
//!
//! Residue: `ring` returns the key schedule by value, so it is built on the
//! stack and then moved into the scratch area, and a seal or open keeps
//! cipher state on the stack. Every use runs in an out-of-line frame below
//! [`SegmentKey::with_aead`]; when it returns (or unwinds), `with_aead`
//! overwrites [`SCRUB_BYTES`] of the stack below its own frame with zeros,
//! which covers those frames. Register contents and any stack deeper than that
//! are not cleared.

use std::{
    fmt,
    hint::black_box,
    mem::{self, MaybeUninit},
    ptr,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering, compiler_fence},
    },
};

use ring::{
    aead::{AES_128_GCM, LessSafeKey, UnboundKey},
    digest::{self, SHA256, SHA256_OUTPUT_LEN},
};
use windows_sys::Win32::Security::Cryptography::{
    CRYPTPROTECTMEMORY_BLOCK_SIZE, CRYPTPROTECTMEMORY_SAME_PROCESS, CryptProtectMemory,
    CryptUnprotectMemory,
};

use super::{
    crypt::{CryptError, KEY_ID_LEN, KEY_LEN},
    locked::{LockedPage, PAGE_BYTES, last_error},
};

/// [`KEY_LEN`] as the `u32` that `CryptProtectMemory` takes.
const KEY_LEN_U32: u32 = 16;

const _: () = {
    assert!(KEY_LEN_U32 as usize == KEY_LEN);
    // `CryptProtectMemory` works on whole blocks of this size.
    assert!(CRYPTPROTECTMEMORY_BLOCK_SIZE == 16);
    assert!(KEY_LEN.is_multiple_of(CRYPTPROTECTMEMORY_BLOCK_SIZE as usize));
};

/// The layout of the key page.
#[repr(C)]
struct Slots {
    /// The AES key, encrypted with `CryptProtectMemory` except inside
    /// [`SegmentKey::with_aead`] (and in a [`KeyBuffer`], before
    /// [`KeyBuffer::protect`]).
    raw: [u8; KEY_LEN],
    /// Where [`SegmentKey::with_aead`] builds the key schedule. All zeros
    /// between uses.
    scratch: MaybeUninit<LessSafeKey>,
}

/// Offset of [`Slots::raw`] in the page.
const RAW_AT: usize = mem::offset_of!(Slots, raw);
/// Offset of [`Slots::scratch`] in the page.
const SCRATCH_AT: usize = mem::offset_of!(Slots, scratch);
/// Bytes in the scratch area.
const SCRATCH_LEN: usize = mem::size_of::<LessSafeKey>();

const _: () = {
    assert!(mem::size_of::<Slots>() <= PAGE_BYTES);
    assert!(mem::align_of::<Slots>() <= PAGE_BYTES);
    assert!(RAW_AT == 0);
    assert!(SCRATCH_AT >= RAW_AT + KEY_LEN);
    assert!(SCRATCH_AT.is_multiple_of(mem::align_of::<LessSafeKey>()));
    // The scratch area is zeroed in `u64` words.
    assert!(SCRATCH_AT.is_multiple_of(mem::align_of::<u64>()));
    assert!(SCRATCH_LEN.is_multiple_of(mem::size_of::<u64>()));
};

/// Bytes of stack below [`SegmentKey::with_aead`] that are overwritten with
/// zeros after each use, to clear what `ring` left there. A release build
/// uses about 5.4 KiB for the key setup and one seal or open of 64 KiB.
pub(crate) const SCRUB_BYTES: usize = 16 * 1024;

/// Prefix of the data hashed for the key check value.
const CHECK_DOMAIN: &[u8] = b"etw-dnstap spool key check v1";

/// The Win32 code the test hooks report for a failure they inject
/// (`ERROR_INVALID_PARAMETER`).
#[cfg(test)]
pub(crate) const TEST_FAILURE_CODE: u32 = 87;

/// Why a key was zeroed for the rest of the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyLoss {
    /// `CryptProtectMemory` failed twice in a row, with this Win32 error.
    Protect {
        /// The Win32 error code of the second failure.
        code: u32,
    },
    /// `CryptUnprotectMemory` returned bytes that do not match the key check
    /// value.
    Check,
}

impl fmt::Display for KeyLoss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protect { code } => write!(f, "CryptProtectMemory, Win32 error {code}"),
            Self::Check => {
                f.write_str("CryptUnprotectMemory returned bytes that fail the key check")
            }
        }
    }
}

/// The AES-128 key and the id that names it in segment headers.
///
/// The 16 key bytes are kept encrypted with `CryptProtectMemory` in a locked
/// page of their own; see the module documentation. The type is not `Clone`:
/// writers and readers share one key through an `Arc<SegmentKey>`. `Debug`
/// shows the key id only.
pub(crate) struct SegmentKey {
    page: LockedPage,
    /// Serializes every use of the page.
    state: Mutex<State>,
    id: [u8; KEY_ID_LEN],
    /// SHA-256 of [`CHECK_DOMAIN`] and the key bytes, taken before they were
    /// first encrypted. It shows that a decrypt gave the key back unchanged.
    check: [u8; SHA256_OUTPUT_LEN],
    /// Set with `State::lost`, so a caller can see a lost key without the
    /// mutex.
    lost: AtomicBool,
}

/// What the mutex of a [`SegmentKey`] guards besides the page.
#[derive(Default)]
struct State {
    /// Why the key bytes were zeroed; `None` while the key is usable.
    lost: Option<KeyLoss>,
    /// Test hook: the next this many `CryptUnprotectMemory` calls fail.
    #[cfg(test)]
    fail_unprotect: u32,
    /// Test hook: the next this many `CryptProtectMemory` calls fail.
    #[cfg(test)]
    fail_protect: u32,
}

impl SegmentKey {
    /// A key from known bytes, for tests and the fuzz targets. The caller owns
    /// (and zeroes, if it matters) its copy of `key`.
    ///
    /// # Errors
    /// [`CryptError::Memory`] if the locked page cannot be made,
    /// [`CryptError::MemoryProtection`] if `CryptProtectMemory` fails.
    #[cfg(any(test, feature = "fuzzing"))]
    pub(crate) fn from_bytes(
        key: &[u8; KEY_LEN],
        id: [u8; KEY_ID_LEN],
    ) -> Result<Self, CryptError> {
        let mut buffer = KeyBuffer::new()?;
        buffer.bytes_mut().copy_from_slice(key);
        buffer.protect(id)
    }

    /// The key id written to and checked in segment headers.
    pub(crate) const fn id(&self) -> [u8; KEY_ID_LEN] {
        self.id
    }

    /// True once a failure has zeroed the key for the rest of the run. Reads
    /// an atomic flag; it does not wait for a use in progress.
    pub(crate) fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    /// Why the key was zeroed, or `None` while it is usable.
    pub(crate) fn loss(&self) -> Option<KeyLoss> {
        self.lock().lost
    }

    /// Runs `f` with the AES-128-GCM key, built for this call only.
    ///
    /// The key bytes are in the clear only while they are checked and the
    /// schedule is built; they are encrypted again before `f` runs. After `f`
    /// returns or panics, the schedule is dropped, its scratch area overwritten
    /// with zeros, and the stack below this call scrubbed. Calls from several
    /// threads take turns.
    ///
    /// # Errors
    /// `f` does not run then.
    /// - [`CryptError::MemoryProtection`] for `CryptUnprotectMemory`: nothing
    ///   changed, a later call may succeed.
    /// - [`CryptError::MemoryProtection`] for `CryptProtectMemory` (after one
    ///   retry) or [`CryptError::KeyCheck`]: the key is now zeroed.
    /// - [`CryptError::KeyZeroed`] once the key is zeroed.
    /// - [`CryptError::BadKey`] if `ring` rejects the key (it cannot for 16
    ///   bytes).
    pub(crate) fn with_aead<R>(&self, f: impl FnOnce(&LessSafeKey) -> R) -> Result<R, CryptError> {
        scrubbed(|| {
            let mut state = self.lock();
            let mut open = Unprotected::begin(self, &mut state)?;
            open.build()?;
            open.protect()?;
            let result = f(open.schedule());
            drop(open);
            Ok(result)
        })
    }

    /// Whether the key bytes equal `candidate`, compared in constant time.
    ///
    /// # Errors
    /// As for [`Self::with_aead`].
    pub(crate) fn matches(&self, candidate: &[u8; KEY_LEN]) -> Result<bool, CryptError> {
        scrubbed(|| {
            let mut state = self.lock();
            let open = Unprotected::begin(self, &mut state)?;
            let equal = constant_time_eq(open.raw(), candidate);
            open.finish()?;
            Ok(equal)
        })
    }

    /// The mutex, recovered after a panic. The guard of the panicking use put
    /// the page back in order while it unwound (key bytes encrypted or zeroed,
    /// scratch area zeroed), so the poison carries no broken state.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The address of the key page, so tests can show that every user shares
    /// the one key.
    #[cfg(test)]
    pub(crate) fn page_address(&self) -> usize {
        self.page.base().as_ptr() as usize
    }

    /// The key area and the scratch area as they are now, between uses.
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> PageSnapshot {
        let _state = self.lock();
        let base = self.page.base().as_ptr();
        // SAFETY: the mutex is held, so no use is in progress; the page is
        // readable for `PAGE_BYTES` bytes and every byte of it is initialized
        // (zero-filled at allocation, then written as bytes or zeroed).
        let page = unsafe { std::slice::from_raw_parts(base, PAGE_BYTES) };
        let mut raw = [0; KEY_LEN];
        raw.copy_from_slice(&page[RAW_AT..RAW_AT + KEY_LEN]);
        PageSnapshot {
            raw,
            scratch: page[SCRATCH_AT..SCRATCH_AT + SCRATCH_LEN].to_vec(),
        }
    }

    /// The scratch area's address: where [`Self::with_aead`] builds the key.
    #[cfg(test)]
    pub(crate) fn scratch_address(&self) -> usize {
        self.page_address() + SCRATCH_AT
    }

    /// A copy of the key bytes in the clear, for tests that compare keys.
    ///
    /// # Errors
    /// As for [`Self::with_aead`].
    #[cfg(test)]
    pub(crate) fn plaintext_for_test(&self) -> Result<[u8; KEY_LEN], CryptError> {
        let mut state = self.lock();
        let open = Unprotected::begin(self, &mut state)?;
        let copy = *open.raw();
        open.finish()?;
        Ok(copy)
    }

    /// Zeroes the key as two failed `CryptProtectMemory` calls do, with
    /// [`TEST_FAILURE_CODE`].
    #[cfg(test)]
    pub(crate) fn zero_for_test(&self) {
        lose(
            self,
            &mut self.lock(),
            KeyLoss::Protect {
                code: TEST_FAILURE_CODE,
            },
        );
    }

    /// Flips one bit of the encrypted key bytes, as a decrypt that went wrong
    /// would leave them.
    #[cfg(test)]
    pub(crate) fn tamper_for_test(&self) {
        let _state = self.lock();
        let first = raw_ptr(&self.page).cast::<u8>();
        // SAFETY: `first` is the first byte of the key area; the mutex is held.
        let byte = unsafe { first.read() };
        // SAFETY: as above.
        unsafe { first.write(byte ^ 1) };
    }

    /// Makes the next `times` `CryptUnprotectMemory` calls fail with
    /// [`TEST_FAILURE_CODE`], without calling it.
    #[cfg(test)]
    pub(crate) fn fail_unprotect_for_test(&self, times: u32) {
        self.lock().fail_unprotect = times;
    }

    /// Makes the next `times` `CryptProtectMemory` calls of a use fail with
    /// [`TEST_FAILURE_CODE`], without calling it.
    #[cfg(test)]
    pub(crate) fn fail_protect_for_test(&self, times: u32) {
        self.lock().fail_protect = times;
    }
}

impl fmt::Debug for SegmentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SegmentKey")
            .field("id", &format_args!("{}", Hex(&self.id)))
            .field("key", &format_args!("<redacted>"))
            .finish_non_exhaustive()
    }
}

/// The two areas of a key page, read by tests between uses.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PageSnapshot {
    /// The key area: the key bytes as `CryptProtectMemory` left them.
    pub(crate) raw: [u8; KEY_LEN],
    /// The scratch area.
    pub(crate) scratch: Vec<u8>,
}

/// 16 key bytes in the clear in a new locked page, on their way to becoming a
/// [`SegmentKey`]. `NCryptDecrypt` or `BCryptGenRandom` writes into
/// [`Self::bytes_mut`]; [`Self::protect`] encrypts the bytes in place. If the
/// buffer drops instead, the page is zeroed and freed.
pub(crate) struct KeyBuffer {
    page: LockedPage,
}

impl KeyBuffer {
    /// A new locked page whose key bytes are zero.
    ///
    /// # Errors
    /// [`CryptError::Memory`] if the page cannot be made or locked.
    pub(crate) fn new() -> Result<Self, CryptError> {
        Ok(Self {
            page: LockedPage::new().map_err(CryptError::Memory)?,
        })
    }

    /// The key bytes, in the clear, in the page.
    pub(crate) fn bytes(&self) -> &[u8; KEY_LEN] {
        // SAFETY: the key area is `KEY_LEN` initialized bytes at the start of
        // the page (alignment 1); the page lives as long as `self`.
        unsafe { &*raw_ptr(&self.page) }
    }

    /// The key bytes, in the clear, in the page, for the call that fills them.
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8; KEY_LEN] {
        // SAFETY: as in `bytes`; `&mut self` makes the reference exclusive.
        unsafe { &mut *raw_ptr(&self.page) }
    }

    /// The address of the key bytes, so tests can show that they are not
    /// copied on the way to the [`SegmentKey`].
    #[cfg(test)]
    pub(crate) fn address(&self) -> usize {
        self.page.base().as_ptr() as usize
    }

    /// Takes the key check value, encrypts the key bytes in place with
    /// `CryptProtectMemory` (tried twice), and makes the page the
    /// [`SegmentKey`] named `id`. The bytes are not copied.
    ///
    /// # Errors
    /// [`CryptError::MemoryProtection`] if `CryptProtectMemory` fails twice;
    /// the page is zeroed and freed then.
    pub(crate) fn protect(self, id: [u8; KEY_ID_LEN]) -> Result<SegmentKey, CryptError> {
        let check = scrubbed(|| key_check(self.bytes()));
        protect_raw(&self.page).or_else(|_| protect_raw(&self.page))?;
        Ok(SegmentKey {
            page: self.page,
            state: Mutex::new(State::default()),
            lost: AtomicBool::new(false),
            id,
            check,
        })
    }
}

impl fmt::Debug for KeyBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyBuffer").finish_non_exhaustive()
    }
}

/// One use of a key page with its key bytes in the clear. Made only with the
/// key's mutex held. `Drop` encrypts the key bytes again (or zeroes them),
/// drops the schedule, and zeroes the scratch area, also during a panic.
struct Unprotected<'a> {
    key: &'a SegmentKey,
    state: &'a mut State,
    /// The key bytes are in the clear.
    raw_plain: bool,
    /// The scratch area holds a key schedule.
    built: bool,
}

impl<'a> Unprotected<'a> {
    /// Decrypts the key bytes in place and checks them against the key's check
    /// value.
    fn begin(key: &'a SegmentKey, state: &'a mut State) -> Result<Self, CryptError> {
        if state.lost.is_some() {
            return Err(CryptError::KeyZeroed);
        }
        #[cfg(test)]
        if state.fail_unprotect > 0 {
            state.fail_unprotect -= 1;
            return Err(CryptError::MemoryProtection {
                call: "CryptUnprotectMemory",
                code: TEST_FAILURE_CODE,
            });
        }
        unprotect_raw(&key.page)?;
        let mut open = Self {
            key,
            state,
            raw_plain: true,
            built: false,
        };
        if !constant_time_eq(&key_check(open.raw()), &key.check) {
            open.raw_plain = false;
            lose(open.key, open.state, KeyLoss::Check);
            return Err(CryptError::KeyCheck);
        }
        Ok(open)
    }

    /// The key bytes in the clear.
    fn raw(&self) -> &[u8; KEY_LEN] {
        debug_assert!(self.raw_plain);
        // SAFETY: the key area is `KEY_LEN` initialized bytes; the mutex makes
        // this use exclusive, and nothing writes them while `self` is borrowed.
        unsafe { &*raw_ptr(&self.key.page) }
    }

    /// Builds the key schedule in the scratch area.
    fn build(&mut self) -> Result<(), CryptError> {
        // SAFETY: the scratch area lies in the page, is aligned for a
        // `LessSafeKey` (const assertions above), and does not overlap the key
        // area; the mutex makes this use exclusive.
        let scratch = unsafe { &mut *scratch_ptr(&self.key.page) };
        if !build_schedule(self.raw(), scratch) {
            return Err(CryptError::BadKey);
        }
        self.built = true;
        Ok(())
    }

    /// Encrypts the key bytes again, with one retry. If both calls fail the
    /// bytes are zeroed instead and the key is lost.
    fn protect(&mut self) -> Result<(), CryptError> {
        // Either encrypted or zeroed after this.
        self.raw_plain = false;
        let result = self.protect_once().or_else(|_| self.protect_once());
        if let Err(CryptError::MemoryProtection { code, .. }) = result {
            lose(self.key, self.state, KeyLoss::Protect { code });
        }
        result
    }

    /// One `CryptProtectMemory` call on the key bytes.
    fn protect_once(&mut self) -> Result<(), CryptError> {
        #[cfg(test)]
        if self.state.fail_protect > 0 {
            self.state.fail_protect -= 1;
            return Err(CryptError::MemoryProtection {
                call: "CryptProtectMemory",
                code: TEST_FAILURE_CODE,
            });
        }
        protect_raw(&self.key.page)
    }

    /// Encrypts the key bytes again and ends the use.
    fn finish(mut self) -> Result<(), CryptError> {
        self.protect()
    }

    /// The key schedule built by [`Self::build`].
    fn schedule(&self) -> &LessSafeKey {
        debug_assert!(self.built);
        // SAFETY: the scratch area lies in the page, aligned for a
        // `LessSafeKey`; the mutex makes this use exclusive.
        let slot = unsafe { &*scratch_ptr(&self.key.page) };
        // SAFETY: `build` wrote a valid `LessSafeKey` there, and only `drop`
        // removes it.
        unsafe { slot.assume_init_ref() }
    }
}

impl Drop for Unprotected<'_> {
    fn drop(&mut self) {
        if self.raw_plain {
            // A failure zeroes the key bytes; nothing else to do here.
            let _ = self.protect();
        }
        if self.built {
            self.built = false;
            // SAFETY: the scratch area lies in the page, aligned for a
            // `LessSafeKey`; the mutex makes this use exclusive.
            let slot = unsafe { &mut *scratch_ptr(&self.key.page) };
            // SAFETY: the scratch area holds the valid `LessSafeKey` that
            // `build` wrote; it is dropped once, here, and nothing refers to it.
            unsafe { slot.assume_init_drop() };
        }
        zero_scratch(&self.key.page);
    }
}

/// Builds the AES-128-GCM key from `raw` into `scratch`. `false` if `ring`
/// rejects the key.
#[inline(never)]
fn build_schedule(raw: &[u8; KEY_LEN], scratch: &mut MaybeUninit<LessSafeKey>) -> bool {
    let Ok(unbound) = UnboundKey::new(&AES_128_GCM, raw) else {
        return false;
    };
    scratch.write(LessSafeKey::new(unbound));
    true
}

/// The key check value: SHA-256 of [`CHECK_DOMAIN`] and the key bytes.
fn key_check(raw: &[u8; KEY_LEN]) -> [u8; SHA256_OUTPUT_LEN] {
    let mut context = digest::Context::new(&SHA256);
    context.update(CHECK_DOMAIN);
    context.update(raw);
    let mut check = [0; SHA256_OUTPUT_LEN];
    check.copy_from_slice(context.finish().as_ref());
    check
}

/// Runs `work` in a frame of its own and then overwrites [`SCRUB_BYTES`] of
/// the stack below the caller with zeros, also when `work` panics. The frames
/// of `work` and everything it calls lie in the scrubbed area.
fn scrubbed<R>(work: impl FnOnce() -> R) -> R {
    /// Scrubs the stack when it drops: after `work` returned or unwound.
    struct Scrub;
    impl Drop for Scrub {
        fn drop(&mut self) {
            scrub_stack();
        }
    }
    let _scrub = Scrub;
    out_of_line(work)
}

/// Calls `work` from a frame that is never inlined into the caller.
#[inline(never)]
fn out_of_line<R>(work: impl FnOnce() -> R) -> R {
    work()
}

/// The key area of `page`.
const fn raw_ptr(page: &LockedPage) -> *mut [u8; KEY_LEN] {
    // `RAW_AT` is 0: the key area starts the page.
    page.base().as_ptr().cast()
}

/// The scratch area of `page`.
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the page is page-aligned and SCRATCH_AT is a multiple of the alignment (const assertions above)"
)]
const fn scratch_ptr(page: &LockedPage) -> *mut MaybeUninit<LessSafeKey> {
    page.base()
        .as_ptr()
        .wrapping_add(SCRATCH_AT)
        .cast::<MaybeUninit<LessSafeKey>>()
}

/// Decrypts the key area of `page` in place. On failure nothing is changed.
fn unprotect_raw(page: &LockedPage) -> Result<(), CryptError> {
    // SAFETY: the key area is `KEY_LEN_U32` writable bytes, a whole number of
    // `CRYPTPROTECTMEMORY_BLOCK_SIZE` blocks, encrypted by `protect_raw` in
    // this process; the caller holds the mutex.
    let done = unsafe {
        CryptUnprotectMemory(
            raw_ptr(page).cast(),
            KEY_LEN_U32,
            CRYPTPROTECTMEMORY_SAME_PROCESS,
        )
    };
    if done == 0 {
        return Err(CryptError::MemoryProtection {
            call: "CryptUnprotectMemory",
            code: last_error(),
        });
    }
    Ok(())
}

/// Encrypts the key area of `page` in place.
fn protect_raw(page: &LockedPage) -> Result<(), CryptError> {
    // SAFETY: the key area is `KEY_LEN_U32` writable bytes, a whole number of
    // `CRYPTPROTECTMEMORY_BLOCK_SIZE` blocks; the caller has exclusive use.
    let done = unsafe {
        CryptProtectMemory(
            raw_ptr(page).cast(),
            KEY_LEN_U32,
            CRYPTPROTECTMEMORY_SAME_PROCESS,
        )
    };
    if done == 0 {
        return Err(CryptError::MemoryProtection {
            call: "CryptProtectMemory",
            code: last_error(),
        });
    }
    Ok(())
}

/// Overwrites the key area with zeros and records why the key is lost.
fn lose(key: &SegmentKey, state: &mut State, why: KeyLoss) {
    state.lost = Some(why);
    key.lost.store(true, Ordering::Release);
    let raw = raw_ptr(&key.page).cast::<u8>();
    for at in 0..KEY_LEN {
        // SAFETY: `raw + at` lies in the key area of the page.
        let byte = unsafe { raw.add(at) };
        // SAFETY: `byte` is valid and aligned; the caller has exclusive use.
        unsafe { ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// Overwrites the scratch area with zeros that the compiler cannot remove.
fn zero_scratch(page: &LockedPage) {
    let words = scratch_ptr(page).cast::<u64>();
    for at in 0..SCRATCH_LEN / mem::size_of::<u64>() {
        // SAFETY: `words + at` lies in the scratch area of the page.
        let word = unsafe { words.add(at) };
        // SAFETY: `word` is valid and 8-byte aligned (const assertions above);
        // no value lives in the scratch area any more.
        unsafe { ptr::write_volatile(word, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// Overwrites [`SCRUB_BYTES`] of stack below the caller with zeros, where the
/// frames of the key setup and of the seal or open were.
#[inline(never)]
fn scrub_stack() {
    let mut area = MaybeUninit::<[u64; SCRUB_BYTES / 8]>::uninit();
    let words = area.as_mut_ptr().cast::<u64>();
    for at in 0..SCRUB_BYTES / 8 {
        // SAFETY: `words + at` lies in `area`.
        let word = unsafe { words.add(at) };
        // SAFETY: `word` is valid and aligned for a `u64`.
        unsafe { ptr::write_volatile(word, 0) };
    }
    compiler_fence(Ordering::SeqCst);
    black_box(&area);
}

/// `a == b` in time that does not depend on where they differ.
fn constant_time_eq<const N: usize>(a: &[u8; N], b: &[u8; N]) -> bool {
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b) {
        diff |= black_box(x ^ y);
    }
    black_box(diff) == 0
}

/// Lowercase hex of a byte string, for the key id.
struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

#[cfg(test)]
#[path = "protected_tests.rs"]
mod protected_tests;
