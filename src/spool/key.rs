//! Spool key: generation, wrapping with the installer's CNG machine key, and the
//! `WrappedKey` registry value (task K of `docs/history/spool-encryption-plan.md`).
//!
//! The spool encrypts its segments with a random AES-128 key. In memory it is
//! a [`SegmentKey`]: 16 bytes in a locked page, encrypted with
//! `CryptProtectMemory` between uses (see [`super::protected`]).
//! `NCryptDecrypt` and `BCryptGenRandom` write the key straight into that page;
//! no other buffer holds it. On disk the key is stored only in wrapped form: RSA-OAEP (SHA-256) with a persisted,
//! non-exportable RSA key in the Microsoft Software Key Storage Provider, named
//! [`WRAPPING_KEY_NAME`]. The private half of that key stays in the LSA key
//! isolation process, and the KSP checks the caller's token against the key's
//! DACL on every use.
//!
//! The installer creates the RSA key and its DACL, and the registry key
//! `HKLM\SOFTWARE\ETW-DNSTapService\Protect` with its own DACL. The registry
//! key lies outside the service key, so a major upgrade keeps it. This module
//! only opens the RSA key, encrypts, decrypts, and reads and writes one registry
//! value in that subkey, which it opens with `KEY_QUERY_VALUE | KEY_SET_VALUE` and no other right.
//! It never calls `NCryptCreatePersistedKey`, `NCryptSetProperty`, or
//! `NCryptFinalizeKey`, which the CNG documentation forbids in a service start
//! routine, and it never creates a registry key.
//!
//! The KSP reports a key the caller has no access to as `NTE_BAD_KEYSET`, the
//! same status as a missing key, so the warning for that status names both.
//!
//! [`load_or_create`] is the entry point. It never replaces an existing
//! `WrappedKey` value: an administrator deletes the value to force a new key.

use std::ptr::{self, NonNull};

use windows_sys::{
    Win32::{
        Foundation::{
            E_ACCESSDENIED, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA,
            ERROR_SUCCESS, NTE_BAD_KEYSET, NTE_PERM,
        },
        Security::Cryptography::{
            BCRYPT_OAEP_PADDING_INFO, BCRYPT_SHA256_ALGORITHM, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            BCryptGenRandom, MS_KEY_STORAGE_PROVIDER, NCRYPT_FLAGS, NCRYPT_KEY_HANDLE,
            NCRYPT_MACHINE_KEY_FLAG, NCRYPT_PAD_OAEP_FLAG, NCRYPT_PROV_HANDLE, NCRYPT_SILENT_FLAG,
            NCryptDecrypt, NCryptEncrypt, NCryptFreeObject, NCryptOpenKey,
            NCryptOpenStorageProvider,
        },
        System::Registry::{
            HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_SAM_FLAGS,
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        },
    },
    core::HRESULT,
};

use super::{
    crypt::{CryptError, SegmentKey},
    protected::KeyBuffer,
};

/// `ERROR_SUCCESS`, the only success `SECURITY_STATUS` of the `NCrypt` calls used
/// here. Any other value, positive or negative, is a failure.
const SEC_OK: HRESULT = 0;

/// Bytes in an AES-128 key.
pub(crate) const KEY_LEN: usize = KEY_LEN_U32 as usize;
/// Bytes in a key id. Each segment header names the key id it was written with.
pub(crate) const KEY_ID_LEN: usize = KEY_ID_LEN_U32 as usize;
/// [`KEY_LEN`] as the `u32` the CNG calls take.
const KEY_LEN_U32: u32 = 16;
/// [`KEY_ID_LEN`] as the `u32` the CNG calls take.
const KEY_ID_LEN_U32: u32 = 8;

/// Name of the persisted RSA key that the installer creates in the Microsoft
/// Software KSP with `NCRYPT_MACHINE_KEY_FLAG`.
pub(crate) const WRAPPING_KEY_NAME: &str = "EtwDnstap Spool Key";
/// The registry key under `HKEY_LOCAL_MACHINE` that holds [`VALUE_NAME`]. The
/// installer creates it with a DACL that gives the service SID query value, set
/// value, and read control; the service never creates it.
pub(crate) const PROTECT_SUBKEY: &str = r"SOFTWARE\ETW-DNSTapService\Protect";
/// Name of the `REG_BINARY` value that holds the wrapped spool key.
pub(crate) const VALUE_NAME: &str = "WrappedKey";
/// The only rights the service asks for on [`PROTECT_SUBKEY`].
const SUBKEY_ACCESS: REG_SAM_FLAGS = KEY_QUERY_VALUE | KEY_SET_VALUE;

/// The only `WrappedKey` layout version.
const VALUE_VERSION: u8 = 1;
/// Wrapping algorithm 1: RSA-OAEP with SHA-256, the key id as the OAEP label.
const ALG_RSA_OAEP_SHA256: u8 = 1;
/// Bytes before the wrapped key: version, key id, algorithm, wrapped length.
const VALUE_HEADER_LEN: usize = 1 + KEY_ID_LEN + 1 + 2;
/// The largest wrapped key accepted: one RSA-16384 block, the KSP's largest key.
/// RSA-3072 gives 384 bytes.
const MAX_WRAPPED_LEN: usize = MAX_WRAPPED_LEN_U32 as usize;
/// [`MAX_WRAPPED_LEN`] as a `u32`.
const MAX_WRAPPED_LEN_U32: u32 = 2048;
/// The largest `WrappedKey` value accepted.
const MAX_VALUE_LEN: usize = MAX_VALUE_LEN_U32 as usize;
/// [`MAX_VALUE_LEN`] as the `u32` the registry calls take.
const MAX_VALUE_LEN_U32: u32 = 12 + MAX_WRAPPED_LEN_U32;
const _: () = assert!(MAX_VALUE_LEN == VALUE_HEADER_LEN + MAX_WRAPPED_LEN);

/// Where the wrapping key and the registry value live. [`KeyLocation::PRODUCTION`]
/// is the service's; tests use a per-user key and a throwaway `HKCU` path.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KeyLocation<'a> {
    /// Name of the persisted RSA key in the Microsoft Software KSP.
    pub(crate) wrapping_key: &'a str,
    /// Whether the RSA key is a machine key or a key of the calling user.
    pub(crate) scope: KeyScope,
    /// Registry hive that holds `subkey`.
    pub(crate) hive: Hive,
    /// Registry key that holds the [`VALUE_NAME`] value. It must exist.
    pub(crate) subkey: &'a str,
}

impl KeyLocation<'static> {
    /// The installed service: the machine key [`WRAPPING_KEY_NAME`] and
    /// `HKLM\` [`PROTECT_SUBKEY`].
    pub(crate) const PRODUCTION: Self = Self {
        wrapping_key: WRAPPING_KEY_NAME,
        scope: KeyScope::Machine,
        hive: Hive::LocalMachine,
        subkey: PROTECT_SUBKEY,
    };
}

/// Scope of a persisted CNG key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyScope {
    /// A machine key (`NCRYPT_MACHINE_KEY_FLAG`), as the installer creates.
    Machine,
    /// A key of the calling user, for tests.
    #[cfg(test)]
    User,
}

impl KeyScope {
    const fn flags(self) -> NCRYPT_FLAGS {
        match self {
            Self::Machine => NCRYPT_MACHINE_KEY_FLAG,
            #[cfg(test)]
            Self::User => 0,
        }
    }
}

/// Registry hive for [`KeyLocation::subkey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hive {
    /// `HKEY_LOCAL_MACHINE`, for the service.
    LocalMachine,
    /// `HKEY_CURRENT_USER`, for tests.
    #[cfg(test)]
    CurrentUser,
}

impl Hive {
    const fn handle(self) -> HKEY {
        match self {
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
            #[cfg(test)]
            Self::CurrentUser => windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        }
    }
}

/// Why the spool key cannot be loaded or created. Each variant names the step
/// that failed and its status code, for the one spool-unavailable warning. No
/// variant holds key bytes.
#[derive(Debug, thiserror::Error)]
pub(crate) enum KeyError {
    #[error("cannot make random spool key bytes (BCryptGenRandom NTSTATUS {status:#010x})")]
    Random { status: i32 },
    #[error(
        "cannot open the Microsoft Software Key Storage Provider (SECURITY_STATUS {status:#010x})"
    )]
    ProviderOpen { status: HRESULT },
    #[error("cannot open the CNG key `{name}` (SECURITY_STATUS {status:#010x}{})", key_hint(*status))]
    KeyOpen { name: String, status: HRESULT },
    #[error("cannot wrap the spool key (NCryptEncrypt SECURITY_STATUS {status:#010x}{})", key_hint(*status))]
    Encrypt { status: HRESULT },
    #[error("cannot unwrap the spool key (NCryptDecrypt SECURITY_STATUS {status:#010x}{})", key_hint(*status))]
    Decrypt { status: HRESULT },
    #[error("cannot open the registry key `{subkey}` (Win32 error {code}{})", registry_hint(*code))]
    RegistryOpen { subkey: String, code: u32 },
    #[error("cannot read the `WrappedKey` registry value (Win32 error {code}{})", registry_hint(*code))]
    RegistryRead { code: u32 },
    #[error("cannot write the `WrappedKey` registry value (Win32 error {code}{})", registry_hint(*code))]
    RegistryWrite { code: u32 },
    #[error("the `WrappedKey` registry value is not valid: {0}")]
    BadValue(&'static str),
    #[error("a new spool key did not unwrap to the bytes that were wrapped")]
    Verify,
    /// The locked page for the key, or its memory protection, failed.
    #[error(transparent)]
    Memory(#[from] CryptError),
    #[error("the {0} has a NUL character")]
    Nul(&'static str),
}

/// A short reason for common CNG status codes, or nothing.
const fn key_hint(status: HRESULT) -> &'static str {
    match status {
        // The KSP also returns this when the caller has no access to the key.
        NTE_BAD_KEYSET => ": the key is missing or this account has no access to it",
        E_ACCESSDENIED | NTE_PERM => ": access denied by the key's permissions",
        _ => "",
    }
}

/// A short reason for common registry error codes, or nothing.
const fn registry_hint(code: u32) -> &'static str {
    match code {
        ERROR_FILE_NOT_FOUND => ": not found; the installer creates it",
        ERROR_ACCESS_DENIED => ": access denied",
        _ => "",
    }
}

/// A new random key, generated straight into a new locked page, and a new
/// random key id, both from the system-preferred RNG. The key bytes are in
/// the clear until the caller protects the buffer.
///
/// # Errors
/// [`KeyError::Memory`] if the page cannot be made, [`KeyError::Random`] if
/// `BCryptGenRandom` fails.
pub(crate) fn generate() -> Result<(KeyBuffer, [u8; KEY_ID_LEN]), KeyError> {
    let mut key = KeyBuffer::new()?;
    random(key.bytes_mut())?;
    let mut id = [0; KEY_ID_LEN];
    random(&mut id)?;
    Ok((key, id))
}

/// Fills `buf` from the system-preferred RNG.
fn random(buf: &mut [u8]) -> Result<(), KeyError> {
    // Both callers pass at most `KEY_LEN` bytes.
    let len = u32::try_from(buf.len()).map_err(|_| KeyError::Random { status: 0 })?;
    // SAFETY: `buf` is a live, writable buffer of `len` bytes. A null algorithm
    // handle is what `BCRYPT_USE_SYSTEM_PREFERRED_RNG` requires.
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            buf.as_mut_ptr(),
            len,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(KeyError::Random { status });
    }
    Ok(())
}

/// The fields of a `WrappedKey` registry value.
///
/// Layout (little-endian): version `u8` = 1, key id `[u8; 8]`, wrapping
/// algorithm `u8` = 1 (RSA-OAEP-SHA256), wrapped length `u16`, wrapped bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrappedKey {
    /// Id of the wrapped key, also the OAEP label of the wrapping.
    pub(crate) key_id: [u8; KEY_ID_LEN],
    /// The RSA-OAEP ciphertext of the 16 key bytes.
    pub(crate) wrapped: Vec<u8>,
}

impl WrappedKey {
    /// The registry value bytes.
    ///
    /// # Errors
    /// [`KeyError::BadValue`] if the wrapped bytes are empty or longer than
    /// the layout allows.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, KeyError> {
        let len = checked_wrapped_len(self.wrapped.len())?;
        let mut out = Vec::with_capacity(VALUE_HEADER_LEN + self.wrapped.len());
        out.push(VALUE_VERSION);
        out.extend_from_slice(&self.key_id);
        out.push(ALG_RSA_OAEP_SHA256);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&self.wrapped);
        Ok(out)
    }

    /// Parses registry value bytes. Every field is checked: the version, the
    /// algorithm, and the wrapped length, which must match the bytes that
    /// follow exactly.
    ///
    /// # Errors
    /// [`KeyError::BadValue`] naming the first field that is wrong.
    pub(crate) fn decode(value: &[u8]) -> Result<Self, KeyError> {
        let (header, wrapped) = value
            .split_first_chunk::<VALUE_HEADER_LEN>()
            .ok_or(KeyError::BadValue("shorter than its header"))?;
        let [version, id @ .., alg, len_lo, len_hi] = header;
        if *version != VALUE_VERSION {
            return Err(KeyError::BadValue("unknown version"));
        }
        if *alg != ALG_RSA_OAEP_SHA256 {
            return Err(KeyError::BadValue("unknown wrapping algorithm"));
        }
        if usize::from(u16::from_le_bytes([*len_lo, *len_hi])) != wrapped.len() {
            return Err(KeyError::BadValue("wrapped length does not match"));
        }
        checked_wrapped_len(wrapped.len())?;
        Ok(Self {
            key_id: *id,
            wrapped: wrapped.to_vec(),
        })
    }
}

/// `len` as the layout's `u16`, if it is a possible wrapped key length.
fn checked_wrapped_len(len: usize) -> Result<u16, KeyError> {
    if len == 0 {
        return Err(KeyError::BadValue("no wrapped key bytes"));
    }
    if len > MAX_WRAPPED_LEN {
        return Err(KeyError::BadValue("wrapped key is too long"));
    }
    u16::try_from(len).map_err(|_| KeyError::BadValue("wrapped key is too long"))
}

/// The result of [`load_or_create`].
#[derive(Debug)]
pub(crate) struct LoadedKey {
    /// The spool key, encrypted in its locked page.
    pub(crate) key: SegmentKey,
    /// True when no `WrappedKey` value existed and this call made and stored a
    /// new key. Segments already in the spool cannot be read with it, so the
    /// caller deletes them.
    pub(crate) created: bool,
}

/// Loads the spool key from the `WrappedKey` registry value, or makes and stores
/// a new one when the value does not exist.
///
/// A new key is generated into its locked page, wrapped from there, and
/// protected with `CryptProtectMemory`. It is then unwrapped again into a
/// second locked page, to prove the service can decrypt with the wrapping key,
/// compared in constant time, and only then written; the second page is zeroed
/// when it drops. An existing value is never replaced, also when it does not
/// unwrap.
///
/// # Errors
/// The [`KeyError`] of the step that failed: the registry key is missing or
/// cannot be opened, the value cannot be read or written, the value is
/// malformed, the KSP or the wrapping key cannot be opened, or encryption or
/// decryption fails, or the locked page or its memory protection fails.
pub(crate) fn load_or_create(location: &KeyLocation<'_>) -> Result<LoadedKey, KeyError> {
    let subkey = RegKey::open(location.hive, location.subkey)?;
    if let Some(value) = subkey.read()? {
        let wrapped = WrappedKey::decode(&value)?;
        let key = unwrap_key(&wrapped, location.wrapping_key, location.scope)?;
        return Ok(LoadedKey {
            key,
            created: false,
        });
    }
    // Opened first, so the new key is in the clear only across the wrap.
    let wrapping = WrappingKey::open(location.wrapping_key, location.scope)?;
    let (fresh, id) = generate()?;
    let wrapped = wrapping.wrap(fresh.bytes(), id)?;
    let key = fresh.protect(id)?;
    let mut check = KeyBuffer::new()?;
    // The key id is the OAEP label, so this unwrap also proves the stored id.
    wrapping.unwrap_into(&wrapped, check.bytes_mut())?;
    if !key.matches(check.bytes())? {
        return Err(KeyError::Verify);
    }
    drop(check);
    subkey.write(&wrapped.encode()?)?;
    Ok(LoadedKey { key, created: true })
}

/// Wraps `key`, named `id`, with the persisted RSA key `name`. Production
/// code wraps only inside [`load_or_create`].
///
/// # Errors
/// [`KeyError::ProviderOpen`], [`KeyError::KeyOpen`], or [`KeyError::Encrypt`].
#[cfg(test)]
pub(crate) fn wrap_key(
    key: &[u8; KEY_LEN],
    id: [u8; KEY_ID_LEN],
    name: &str,
    scope: KeyScope,
) -> Result<WrappedKey, KeyError> {
    WrappingKey::open(name, scope)?.wrap(key, id)
}

/// Unwraps `wrapped` with the persisted RSA key `name`, straight into a new
/// locked page, and protects it there with `CryptProtectMemory`.
///
/// # Errors
/// [`KeyError::ProviderOpen`], [`KeyError::KeyOpen`], [`KeyError::Decrypt`]
/// (also for a wrong key, changed wrapped bytes, a changed key id, or a
/// plaintext longer than 16 bytes), [`KeyError::BadValue`] if the plaintext is
/// shorter than 16 bytes, or [`KeyError::Memory`].
pub(crate) fn unwrap_key(
    wrapped: &WrappedKey,
    name: &str,
    scope: KeyScope,
) -> Result<SegmentKey, KeyError> {
    let wrapping = WrappingKey::open(name, scope)?;
    let mut key = KeyBuffer::new()?;
    wrapping.unwrap_into(wrapped, key.bytes_mut())?;
    Ok(key.protect(wrapped.key_id)?)
}

/// An open CNG object handle, freed on drop.
struct NcryptHandle(usize);

impl NcryptHandle {
    /// Gives up ownership without freeing, for a call that frees the handle
    /// itself (`NCryptDeleteKey`).
    #[cfg(test)]
    const fn into_raw(self) -> usize {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}

impl Drop for NcryptHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a provider or key handle this wrapper owns and
        // has not freed. Nothing useful can be done if freeing fails.
        unsafe { NCryptFreeObject(self.0) };
    }
}

/// An open provider handle for the Microsoft Software KSP.
fn open_provider() -> Result<NcryptHandle, KeyError> {
    let mut provider: NCRYPT_PROV_HANDLE = 0;
    // SAFETY: `provider` is a live out parameter; the provider name is a static
    // NUL-terminated wide string.
    let status =
        unsafe { NCryptOpenStorageProvider(&raw mut provider, MS_KEY_STORAGE_PROVIDER, 0) };
    if status != SEC_OK {
        return Err(KeyError::ProviderOpen { status });
    }
    Ok(NcryptHandle(provider))
}

/// An open handle to the persisted RSA key used to wrap the spool key.
struct WrappingKey {
    // Declared before `_provider`, so it is freed first.
    key: NcryptHandle,
    _provider: NcryptHandle,
}

impl WrappingKey {
    /// Opens the persisted key `name` in the Microsoft Software KSP.
    fn open(name: &str, scope: KeyScope) -> Result<Self, KeyError> {
        let provider = open_provider()?;
        let wide = to_wide(name, "wrapping key name")?;
        let mut key: NCRYPT_KEY_HANDLE = 0;
        // SAFETY: `provider.0` is an open provider handle; `key` is a live out
        // parameter; `wide` is NUL-terminated and lives across the call.
        let status = unsafe {
            NCryptOpenKey(
                provider.0,
                &raw mut key,
                wide.as_ptr(),
                0,
                scope.flags() | NCRYPT_SILENT_FLAG,
            )
        };
        if status != SEC_OK {
            return Err(KeyError::KeyOpen {
                name: name.to_owned(),
                status,
            });
        }
        Ok(Self {
            key: NcryptHandle(key),
            _provider: provider,
        })
    }

    /// RSA-OAEP-SHA256 encryption of the key bytes, with the key id as the
    /// OAEP label so the id cannot be changed without decryption failing.
    fn wrap(&self, input: &[u8; KEY_LEN], id: [u8; KEY_ID_LEN]) -> Result<WrappedKey, KeyError> {
        let padding = oaep_padding(&id);
        let flags = NCRYPT_PAD_OAEP_FLAG | NCRYPT_SILENT_FLAG;
        let mut needed = 0_u32;
        // SAFETY: `self.key.0` is an open key handle. `input` is 16 readable
        // bytes. `padding` and the label it points to (`id`) live across the
        // call. A null output buffer asks for the size, written to `needed`.
        let sized = unsafe {
            NCryptEncrypt(
                self.key.0,
                input.as_ptr(),
                KEY_LEN_U32,
                (&raw const padding).cast(),
                ptr::null_mut(),
                0,
                &raw mut needed,
                flags,
            )
        };
        if sized != SEC_OK {
            return Err(KeyError::Encrypt { status: sized });
        }
        let mut wrapped = vec![0_u8; needed as usize];
        let mut written = 0_u32;
        // SAFETY: as above; `wrapped` is a writable buffer of `needed` bytes.
        let status = unsafe {
            NCryptEncrypt(
                self.key.0,
                input.as_ptr(),
                KEY_LEN_U32,
                (&raw const padding).cast(),
                wrapped.as_mut_ptr(),
                needed,
                &raw mut written,
                flags,
            )
        };
        if status != SEC_OK {
            return Err(KeyError::Encrypt { status });
        }
        wrapped.truncate(written as usize);
        Ok(WrappedKey {
            key_id: id,
            wrapped,
        })
    }

    /// RSA-OAEP-SHA256 decryption straight into `out`, the key area of a
    /// locked page. No other buffer of this process receives the plaintext.
    /// On failure `out` may hold part of it; the caller's page is zeroed when
    /// it drops.
    fn unwrap_into(&self, wrapped: &WrappedKey, out: &mut [u8; KEY_LEN]) -> Result<(), KeyError> {
        let input_len = u32::from(checked_wrapped_len(wrapped.wrapped.len())?);
        let padding = oaep_padding(&wrapped.key_id);
        let mut written = 0_u32;
        // SAFETY: `self.key.0` is an open key handle. The input is
        // `input_len` readable bytes. `padding` and its label live across the
        // call. `out` is a writable buffer of `KEY_LEN_U32` bytes; the KSP
        // writes at most that many. `written` is a live out parameter.
        let status = unsafe {
            NCryptDecrypt(
                self.key.0,
                wrapped.wrapped.as_ptr(),
                input_len,
                (&raw const padding).cast(),
                out.as_mut_ptr(),
                KEY_LEN_U32,
                &raw mut written,
                NCRYPT_PAD_OAEP_FLAG | NCRYPT_SILENT_FLAG,
            )
        };
        if status != SEC_OK {
            return Err(KeyError::Decrypt { status });
        }
        if written as usize != KEY_LEN {
            return Err(KeyError::BadValue("the unwrapped key is not 16 bytes"));
        }
        Ok(())
    }
}

/// OAEP padding with SHA-256 and `label` as the label. The result points into
/// `label`, which must outlive every call that uses it.
const fn oaep_padding(label: &[u8; KEY_ID_LEN]) -> BCRYPT_OAEP_PADDING_INFO {
    BCRYPT_OAEP_PADDING_INFO {
        pszAlgId: BCRYPT_SHA256_ALGORITHM,
        // The API takes `*mut` but only reads the label.
        pbLabel: label.as_ptr().cast_mut(),
        cbLabel: KEY_ID_LEN_U32,
    }
}

/// An open registry key, closed on drop.
struct RegKey(NonNull<core::ffi::c_void>);

impl RegKey {
    /// Opens the existing key `hive\subkey` with [`SUBKEY_ACCESS`]; never
    /// creates it.
    fn open(hive: Hive, subkey: &str) -> Result<Self, KeyError> {
        let wide = to_wide(subkey, "registry key path")?;
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: the hive is a predefined key; `wide` is NUL-terminated and
        // lives across the call; `key` is a live out parameter.
        let code =
            unsafe { RegOpenKeyExW(hive.handle(), wide.as_ptr(), 0, SUBKEY_ACCESS, &raw mut key) };
        if code != ERROR_SUCCESS {
            return Err(KeyError::RegistryOpen {
                subkey: subkey.to_owned(),
                code,
            });
        }
        NonNull::new(key).map(Self).ok_or(KeyError::RegistryOpen {
            subkey: subkey.to_owned(),
            code: ERROR_FILE_NOT_FOUND,
        })
    }

    /// The [`VALUE_NAME`] value, or `None` if it does not exist.
    fn read(&self) -> Result<Option<Vec<u8>>, KeyError> {
        let name = to_wide(VALUE_NAME, "registry value name")?;
        let mut buf = vec![0_u8; MAX_VALUE_LEN];
        let mut len = MAX_VALUE_LEN_U32;
        let mut kind = 0_u32;
        // SAFETY: `self.0` is an open key; `name` is NUL-terminated; `kind`
        // and `len` are live out parameters; `buf` is writable for `len` bytes.
        let code = unsafe {
            RegQueryValueExW(
                self.0.as_ptr(),
                name.as_ptr(),
                ptr::null(),
                &raw mut kind,
                buf.as_mut_ptr(),
                &raw mut len,
            )
        };
        match code {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            ERROR_MORE_DATA => return Err(KeyError::BadValue("too long")),
            code => return Err(KeyError::RegistryRead { code }),
        }
        if kind != REG_BINARY {
            return Err(KeyError::BadValue("not REG_BINARY"));
        }
        buf.truncate(len as usize);
        Ok(Some(buf))
    }

    /// Sets the [`VALUE_NAME`] value to `value` as `REG_BINARY`.
    fn write(&self, value: &[u8]) -> Result<(), KeyError> {
        let len = u32::try_from(value.len()).map_err(|_| KeyError::BadValue("too long"))?;
        let name = to_wide(VALUE_NAME, "registry value name")?;
        // SAFETY: `self.0` is an open key; `name` is NUL-terminated; `value`
        // is `len` readable bytes.
        let code = unsafe {
            RegSetValueExW(
                self.0.as_ptr(),
                name.as_ptr(),
                0,
                REG_BINARY,
                value.as_ptr(),
                len,
            )
        };
        if code != ERROR_SUCCESS {
            return Err(KeyError::RegistryWrite { code });
        }
        Ok(())
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a key this wrapper opened and has not closed.
        unsafe { RegCloseKey(self.0.as_ptr()) };
    }
}

/// The [`VALUE_NAME`] value under `hive\subkey`, or `None` if the value does
/// not exist. A missing `subkey` is an error: the installer creates it.
///
/// # Errors
/// [`KeyError::RegistryOpen`], [`KeyError::RegistryRead`], or
/// [`KeyError::BadValue`] if the value is not `REG_BINARY` or is too long.
#[cfg(test)]
pub(crate) fn read_value(hive: Hive, subkey: &str) -> Result<Option<Vec<u8>>, KeyError> {
    RegKey::open(hive, subkey)?.read()
}

/// Sets the [`VALUE_NAME`] value under `hive\subkey` to `value` as
/// `REG_BINARY`. A missing `subkey` is an error: it is not created.
///
/// # Errors
/// [`KeyError::RegistryOpen`] or [`KeyError::RegistryWrite`].
#[cfg(test)]
pub(crate) fn write_value(hive: Hive, subkey: &str, value: &[u8]) -> Result<(), KeyError> {
    RegKey::open(hive, subkey)?.write(value)
}

/// `text` as NUL-terminated UTF-16.
fn to_wide(text: &str, what: &'static str) -> Result<Vec<u16>, KeyError> {
    if text.contains('\0') {
        return Err(KeyError::Nul(what));
    }
    Ok(text.encode_utf16().chain(Some(0)).collect())
}

#[cfg(test)]
#[path = "key_tests.rs"]
mod key_tests;
