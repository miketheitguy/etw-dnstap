#![expect(
    clippy::shadow_unrelated,
    reason = "sequential steps in one test rebind the same result name; numbering them adds noise"
)]

use std::{
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        Cryptography::{
            NCRYPT_LENGTH_PROPERTY, NCRYPT_RSA_ALGORITHM, NCRYPT_SECURITY_DESCR_PROPERTY,
            NCRYPT_UNIQUE_NAME_PROPERTY, NCryptCreatePersistedKey, NCryptDeleteKey,
            NCryptFinalizeKey, NCryptGetProperty, NCryptSetProperty,
        },
        DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, SetFileSecurityW,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::{
        Registry::{
            HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, RegCreateKeyExW,
            RegDeleteTreeW, RegSetKeySecurity,
        },
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

use super::*;

/// A name no other test or run uses.
fn unique(prefix: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{prefix} {}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A persisted per-user RSA-3072 key in the Microsoft Software KSP, deleted on
/// drop.
///
/// The KSP copies a key's DACL onto the NTFS DACL of the key's file, and checks
/// that file DACL again whenever the key is changed or deleted. Once a test
/// leaves the caller only read access, the KSP can no longer reset the DACL or
/// delete the key. The caller still owns the file, and an owner keeps
/// `WRITE_DAC`, so the guard restores the file DACL first and then deletes the
/// key through the KSP.
struct TestCngKey {
    name: String,
    /// The key file: the KSP's unique name in the user's key folder
    /// (`Microsoft\Crypto\Keys` under the roaming application data folder).
    file: PathBuf,
}

impl TestCngKey {
    fn create() -> Self {
        let name = unique("EtwDnstap Test Spool Key");
        let provider = open_provider().expect("the Software KSP opens");
        let wide = to_wide(&name, "test key name").expect("no NUL");
        let mut raw = 0;
        // SAFETY: `provider.0` is an open provider; `raw` is a live out
        // parameter; the algorithm and `wide` are NUL-terminated strings.
        let status = unsafe {
            NCryptCreatePersistedKey(
                provider.0,
                &raw mut raw,
                NCRYPT_RSA_ALGORITHM,
                wide.as_ptr(),
                0,
                0,
            )
        };
        assert!(status >= 0, "NCryptCreatePersistedKey {status:#010x}");
        let key = NcryptHandle(raw);
        let bits = 3072_u32.to_le_bytes();
        // SAFETY: `key.0` is a key not yet finalized; `bits` is 4 readable bytes.
        let status = unsafe {
            NCryptSetProperty(
                key.0,
                NCRYPT_LENGTH_PROPERTY,
                bits.as_ptr(),
                4,
                NCRYPT_SILENT_FLAG,
            )
        };
        assert!(status >= 0, "set key length {status:#010x}");
        // SAFETY: `key.0` is an open key that is not yet finalized.
        let status = unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) };
        assert!(status >= 0, "NCryptFinalizeKey {status:#010x}");
        let file = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .expect("APPDATA is set")
            .join(r"Microsoft\Crypto\Keys")
            .join(unique_name(key.0));
        assert!(file.is_file(), "the key file is where the KSP documents it");
        Self { name, file }
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Replaces the key's DACL through the KSP. Needs write access to the key.
    fn set_dacl(&self, sddl: &str) -> Result<(), HRESULT> {
        let key = WrappingKey::open(&self.name, KeyScope::User).map_err(|err| match err {
            KeyError::KeyOpen { status, .. } | KeyError::ProviderOpen { status } => status,
            _ => -1,
        })?;
        set_handle_dacl(key.key.0, sddl)
    }

    /// Gives the caller full access to the key file again, as its owner.
    fn restore_access(&self) -> bool {
        let descriptor = Descriptor::parse(&format!("D:P(A;;FA;;;{})", current_user_sid()));
        let path: Vec<u16> = self.file.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `path` is NUL-terminated; `descriptor.sd` is a valid
        // descriptor; both live across the call.
        unsafe { SetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, descriptor.sd) != 0 }
    }
}

/// The KSP's unique name (the key file name) of the open key `key`.
fn unique_name(key: NCRYPT_KEY_HANDLE) -> String {
    let mut buf = [0_u16; 260];
    let mut len = 0_u32;
    // SAFETY: `key` is open; `buf` is writable for the byte size passed; `len`
    // is a live out parameter.
    let status = unsafe {
        NCryptGetProperty(
            key,
            NCRYPT_UNIQUE_NAME_PROPERTY,
            buf.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&buf)).expect("small"),
            &raw mut len,
            0,
        )
    };
    assert!(status >= 0, "unique name {status:#010x}");
    let chars = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..chars])
}

impl Drop for TestCngKey {
    fn drop(&mut self) {
        if !self.restore_access() {
            eprintln!("cannot reset the DACL of test key {}", self.name);
        }
        let key = match WrappingKey::open(&self.name, KeyScope::User) {
            Ok(key) => key,
            Err(err) => {
                eprintln!("cannot open test key {} to delete it: {err}", self.name);
                return;
            }
        };
        let WrappingKey { key, _provider } = key;
        let raw = key.into_raw();
        // SAFETY: `raw` is an open key handle whose ownership was given up;
        // `NCryptDeleteKey` frees it on success.
        let status = unsafe { NCryptDeleteKey(raw, NCRYPT_SILENT_FLAG) };
        if status < 0 {
            // SAFETY: on failure the handle is still open and owned here.
            unsafe { NCryptFreeObject(raw) };
            eprintln!("cannot delete test key {}: {status:#010x}", self.name);
        }
    }
}

/// A security descriptor parsed from SDDL, freed on drop.
struct Descriptor {
    sd: PSECURITY_DESCRIPTOR,
    len: u32,
}

impl Descriptor {
    fn parse(sddl: &str) -> Self {
        let wide = to_wide(sddl, "SDDL").expect("no NUL");
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let mut len = 0_u32;
        // SAFETY: `wide` is a NUL-terminated SDDL string; `sd` and `len` are
        // live out parameters. `Drop` frees the descriptor with `LocalFree`.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &raw mut sd,
                &raw mut len,
            )
        };
        assert!(ok != 0, "SDDL {sddl} parses");
        Self { sd, len }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: `self.sd` was allocated by the SDDL conversion and is not
        // used after this.
        unsafe { LocalFree(self.sd) };
    }
}

/// Replaces the DACL of the open key `key` with the one in `sddl`.
fn set_handle_dacl(key: NCRYPT_KEY_HANDLE, sddl: &str) -> Result<(), HRESULT> {
    let descriptor = Descriptor::parse(sddl);
    // SAFETY: `key` is an open key; `descriptor.sd` is a self-relative
    // descriptor of `descriptor.len` bytes. `NCRYPT_SILENT_FLAG` is not
    // passed: its value is also `SCOPE_SECURITY_INFORMATION`.
    let status = unsafe {
        NCryptSetProperty(
            key,
            NCRYPT_SECURITY_DESCR_PROPERTY,
            descriptor.sd.cast(),
            descriptor.len,
            DACL_SECURITY_INFORMATION,
        )
    };
    if status < 0 {
        return Err(status);
    }
    Ok(())
}

/// The SID of the user the tests run as, for example `S-1-5-21-...`.
fn current_user_sid() -> String {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` has no preconditions; its pseudo handle
    // needs no closing.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: `process` is the current process; `token` is a live out
    // parameter.
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) };
    assert!(ok != 0, "the process token opens");
    // `u64` elements keep the buffer aligned for `TOKEN_USER`.
    let mut buf = [0_u64; 64];
    let mut len = 0_u32;
    // SAFETY: `token` is open with `TOKEN_QUERY`; `buf` is writable for the
    // size passed; `len` is a live out parameter.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&buf)).expect("small"),
            &raw mut len,
        )
    };
    // SAFETY: `token` was opened above and is not used again.
    unsafe { CloseHandle(token) };
    assert!(ok != 0, "TokenUser is read");
    // SAFETY: the call succeeded, so `buf` starts with an aligned `TOKEN_USER`
    // whose SID points into `buf`.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = ptr::null_mut();
    // SAFETY: `sid` is a valid SID inside `buf`; `text` is a live out parameter.
    let ok = unsafe { ConvertSidToStringSidW(sid, &raw mut text) };
    assert!(ok != 0, "the SID converts");
    let mut chars = 0;
    // SAFETY: `text` is a NUL-terminated wide string from the conversion, and
    // the loop stops at its NUL, so every unit read is inside it.
    while unsafe { *text.wrapping_add(chars) } != 0 {
        chars += 1;
    }
    // SAFETY: `text` holds `chars` initialized UTF-16 units before the NUL.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, chars) });
    // SAFETY: `text` was allocated by `ConvertSidToStringSidW` and is not used
    // again.
    unsafe { LocalFree(text.cast()) };
    sid
}

/// Creates the volatile key `HKCU\path`.
fn create_test_key(path: &str) {
    let wide = to_wide(path, "test key path").expect("no NUL");
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated; `key` is a live out parameter; the
    // class, security attributes, and disposition are optional and null.
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            ptr::null(),
            &raw mut key,
            ptr::null_mut(),
        )
    };
    assert_eq!(code, ERROR_SUCCESS, "test registry key {path} is created");
    // SAFETY: `key` was opened above and is not used again.
    unsafe { RegCloseKey(key) };
}

/// `WRITE_DAC`: the owner of a registry key has it without an ACE.
const WRITE_DAC: u32 = 0x0004_0000;

/// Volatile `HKCU\Software\<unique>\ETW-DNSTapService` and, if asked,
/// `...\ETW-DNSTapService\Protect`, the test copy of the installer's layout. All of
/// it is deleted on drop.
struct TestRegistry {
    root: String,
    subkey: String,
}

impl TestRegistry {
    fn create(with_subkey: bool) -> Self {
        let root = format!(r"Software\{}", unique("EtwDnstapKeyTest"));
        let product = format!(r"{root}\ETW-DNSTapService");
        create_test_key(&product);
        let subkey = format!(r"{product}\Protect");
        if with_subkey {
            create_test_key(&subkey);
        }
        Self { root, subkey }
    }

    fn location<'a>(&'a self, key: &'a TestCngKey) -> KeyLocation<'a> {
        KeyLocation {
            wrapping_key: key.name(),
            scope: KeyScope::User,
            hive: Hive::CurrentUser,
            subkey: &self.subkey,
        }
    }

    fn value(&self) -> Option<Vec<u8>> {
        read_value(Hive::CurrentUser, &self.subkey).expect("test value is readable")
    }

    /// Replaces the DACL of the `Protect` subkey, as the owner, who has
    /// `WRITE_DAC` whatever the DACL says.
    fn set_subkey_dacl(&self, sddl: &str) -> Result<(), u32> {
        let descriptor = Descriptor::parse(sddl);
        let wide = to_wide(&self.subkey, "test subkey").expect("no NUL");
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated; `key` is a live out parameter.
        let code =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, WRITE_DAC, &raw mut key) };
        if code != ERROR_SUCCESS {
            return Err(code);
        }
        // SAFETY: `key` is open with `WRITE_DAC`; `descriptor.sd` is a valid
        // descriptor that lives across the call.
        let code = unsafe { RegSetKeySecurity(key, DACL_SECURITY_INFORMATION, descriptor.sd) };
        // SAFETY: `key` was opened above and is not used again.
        unsafe { RegCloseKey(key) };
        if code != ERROR_SUCCESS {
            return Err(code);
        }
        Ok(())
    }
}

impl Drop for TestRegistry {
    fn drop(&mut self) {
        // A test may have left a DACL without delete rights on the subkey.
        let _ = self.set_subkey_dacl(&format!("D:P(A;;KA;;;{})", current_user_sid()));
        let Ok(wide) = to_wide(&self.root, "test root") else {
            return;
        };
        // SAFETY: `wide` is NUL-terminated and lives across the call. With a
        // subkey name, `RegDeleteTreeW` deletes that key as well.
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr()) };
        if code != ERROR_SUCCESS {
            eprintln!("cannot delete test registry key {}: {code}", self.root);
        }
    }
}

fn sample_value() -> WrappedKey {
    WrappedKey {
        key_id: [1, 2, 3, 4, 5, 6, 7, 8],
        wrapped: (0..=255).cycle().take(384).collect(),
    }
}

fn bad_value(value: &[u8]) -> &'static str {
    match WrappedKey::decode(value) {
        Err(KeyError::BadValue(why)) => why,
        other => panic!("expected BadValue, got {other:?}"),
    }
}

#[test]
fn generate_gives_distinct_keys_and_ids() {
    let a = SpoolKey::generate().expect("random key");
    let b = SpoolKey::generate().expect("random key");
    assert_ne!(a.key_bytes(), b.key_bytes());
    assert_ne!(a.key_id(), b.key_id());
    assert_ne!(a.key_bytes(), &[0; KEY_LEN], "key bytes were filled");
    assert_ne!(a.key_id(), [0; KEY_ID_LEN], "key id was filled");
}

#[test]
fn value_layout_and_round_trip() {
    let value = sample_value();
    let bytes = value.encode().expect("encodes");
    assert_eq!(bytes.len(), 12 + 384);
    assert_eq!(bytes[0], 1, "version");
    assert_eq!(&bytes[1..9], &[1, 2, 3, 4, 5, 6, 7, 8], "key id");
    assert_eq!(bytes[9], 1, "algorithm RSA-OAEP-SHA256");
    assert_eq!(&bytes[10..12], &384_u16.to_le_bytes(), "wrapped length");
    assert_eq!(&bytes[12..], &value.wrapped[..]);
    assert_eq!(WrappedKey::decode(&bytes).expect("decodes"), value);
}

#[test]
fn every_malformed_value_is_rejected() {
    let good = sample_value().encode().expect("encodes");
    for len in 0..VALUE_HEADER_LEN {
        assert_eq!(
            bad_value(&good[..len]),
            "shorter than its header",
            "{len} bytes"
        );
    }
    for version in [0, 2, 0xff] {
        let mut v = good.clone();
        v[0] = version;
        assert_eq!(bad_value(&v), "unknown version");
    }
    for alg in [0, 2, 0xff] {
        let mut v = good.clone();
        v[9] = alg;
        assert_eq!(bad_value(&v), "unknown wrapping algorithm");
    }
    // One byte more or less than the length field says.
    assert_eq!(
        bad_value(&good[..good.len() - 1]),
        "wrapped length does not match"
    );
    let mut longer = good.clone();
    longer.push(0);
    assert_eq!(bad_value(&longer), "wrapped length does not match");
    // A header alone, with a matching length field of 0.
    let mut empty = good[..VALUE_HEADER_LEN].to_vec();
    empty[10..12].copy_from_slice(&0_u16.to_le_bytes());
    assert_eq!(bad_value(&empty), "no wrapped key bytes");
    // A length field that matches but is above the limit.
    let mut huge = good[..VALUE_HEADER_LEN].to_vec();
    let len = u16::try_from(MAX_WRAPPED_LEN + 1).expect("fits");
    huge[10..12].copy_from_slice(&len.to_le_bytes());
    huge.resize(VALUE_HEADER_LEN + MAX_WRAPPED_LEN + 1, 0);
    assert_eq!(bad_value(&huge), "wrapped key is too long");
}

#[test]
fn encode_rejects_lengths_the_layout_cannot_hold() {
    let mut value = sample_value();
    value.wrapped.clear();
    assert!(matches!(value.encode(), Err(KeyError::BadValue(_))));
    value.wrapped = vec![0; MAX_WRAPPED_LEN + 1];
    assert!(matches!(value.encode(), Err(KeyError::BadValue(_))));
}

#[test]
fn debug_does_not_show_key_bytes() {
    let key_bytes = [
        0xde, 0xad, 0xbe, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x01, 0x23, 0x45,
        0x67,
    ];
    let key = SpoolKey::from_parts(key_bytes, [0xaa; KEY_ID_LEN]);
    let shown = format!("{key:?} {key:#?}");
    assert!(
        shown.contains("aaaaaaaaaaaaaaaa"),
        "the id is shown: {shown}"
    );
    assert!(shown.contains("<redacted>"), "{shown}");
    assert!(!shown.contains("deadbeef"), "{shown}");
    assert!(!shown.to_lowercase().contains("de, ad"), "{shown}");
    assert!(!shown.contains("222"), "decimal 0xde: {shown}");
    let loaded = format!("{:?}", LoadedKey { key, created: true });
    assert!(
        !loaded.contains("deadbeef") && !loaded.contains("222"),
        "{loaded}"
    );
}

#[test]
fn errors_name_the_step_and_the_status() {
    let missing = unique("EtwDnstap Missing Test Key");
    let key = SpoolKey::generate().expect("random key");
    let err = wrap_key(&key, &missing, KeyScope::User).expect_err("no such key");
    assert!(
        matches!(
            err,
            KeyError::KeyOpen {
                status: NTE_BAD_KEYSET,
                ..
            }
        ),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(text.contains("0x80090016"), "{text}");
    assert!(text.contains(&missing), "{text}");
}

#[test]
fn wrap_and_unwrap_round_trip_and_detect_changes() {
    let cng = TestCngKey::create();
    let key = SpoolKey::generate().expect("random key");
    let wrapped = wrap_key(&key, cng.name(), KeyScope::User).expect("wraps");
    assert_eq!(wrapped.key_id, key.key_id());
    assert_eq!(wrapped.wrapped.len(), 384, "one RSA-3072 block");
    assert_ne!(
        wrap_key(&key, cng.name(), KeyScope::User)
            .expect("wraps")
            .wrapped,
        wrapped.wrapped,
        "OAEP is randomized"
    );

    let back = unwrap_key(&wrapped, cng.name(), KeyScope::User).expect("unwraps");
    assert_eq!(back.key_bytes(), key.key_bytes());
    assert_eq!(back.key_id(), key.key_id());

    // Through the registry encoding too.
    let decoded = WrappedKey::decode(&wrapped.encode().expect("encodes")).expect("decodes");
    let back = unwrap_key(&decoded, cng.name(), KeyScope::User).expect("unwraps");
    assert_eq!(back.key_bytes(), key.key_bytes());

    for index in [0, 1, 191, 383] {
        let mut tampered = wrapped.clone();
        tampered.wrapped[index] ^= 0x01;
        let err = unwrap_key(&tampered, cng.name(), KeyScope::User).expect_err("tampered");
        assert!(
            matches!(err, KeyError::Decrypt { .. }),
            "byte {index}: {err:?}"
        );
    }
    let mut short = wrapped.clone();
    short.wrapped.pop();
    let err = unwrap_key(&short, cng.name(), KeyScope::User).expect_err("short");
    assert!(matches!(err, KeyError::Decrypt { .. }), "{err:?}");

    // The key id is the OAEP label, so changing it breaks the unwrap.
    let mut other_id = wrapped.clone();
    other_id.key_id[0] ^= 0x01;
    let err = unwrap_key(&other_id, cng.name(), KeyScope::User).expect_err("changed id");
    assert!(matches!(err, KeyError::Decrypt { .. }), "{err:?}");
}

#[test]
fn a_different_wrapping_key_cannot_unwrap() {
    let first = TestCngKey::create();
    let second = TestCngKey::create();
    let key = SpoolKey::generate().expect("random key");
    let wrapped = wrap_key(&key, first.name(), KeyScope::User).expect("wraps");
    let err = unwrap_key(&wrapped, second.name(), KeyScope::User).expect_err("other key");
    assert!(matches!(err, KeyError::Decrypt { .. }), "{err:?}");
}

/// The installer grants the service SID only `GENERIC_READ` on the key. This
/// proves that open, encrypt, and decrypt work with that right alone, and that
/// the KSP does enforce the DACL: with no entry for the caller, the key does
/// not open.
#[test]
fn generic_read_on_the_key_is_enough_to_unwrap() {
    let cng = TestCngKey::create();
    let sid = current_user_sid();
    let key = SpoolKey::generate().expect("random key");
    let wrapped = wrap_key(&key, cng.name(), KeyScope::User).expect("wraps");

    // The only entry is GENERIC_READ for the caller. As the key file's owner
    // the caller also keeps READ_CONTROL and WRITE_DAC, which grant no use of
    // the key; the service does not own the installer's key.
    cng.set_dacl(&format!("D:P(A;;GR;;;{sid})"))
        .expect("read-only DACL");
    let back = unwrap_key(&wrapped, cng.name(), KeyScope::User).expect("unwraps with GR");
    assert_eq!(back.key_bytes(), key.key_bytes());
    let again = wrap_key(&key, cng.name(), KeyScope::User).expect("wraps with GR");
    let back = unwrap_key(&again, cng.name(), KeyScope::User).expect("unwraps with GR");
    assert_eq!(back.key_bytes(), key.key_bytes());

    // With read only, the caller cannot change the key's DACL through the KSP.
    assert!(cng.set_dacl(&format!("D:P(A;;GA;;;{sid})")).is_err());

    // No entry for the caller: SYSTEM only. The KSP reports no access as
    // NTE_BAD_KEYSET, the same status as a missing key.
    assert!(cng.restore_access(), "owner resets the key file DACL");
    cng.set_dacl("D:P(A;;GA;;;SY)").expect("SYSTEM-only DACL");
    let err = unwrap_key(&wrapped, cng.name(), KeyScope::User).expect_err("no access");
    assert!(
        matches!(
            err,
            KeyError::KeyOpen {
                status: NTE_BAD_KEYSET,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("no access"), "{err}");
}

#[test]
fn load_or_create_creates_once_then_loads() {
    let cng = TestCngKey::create();
    let reg = TestRegistry::create(true);
    let location = reg.location(&cng);
    assert_eq!(reg.value(), None);

    let first = load_or_create(&location).expect("creates");
    assert!(first.created);
    let stored = reg.value().expect("value written");
    let decoded = WrappedKey::decode(&stored).expect("valid value");
    assert_eq!(decoded.key_id, first.key.key_id());

    let second = load_or_create(&location).expect("loads");
    assert!(!second.created);
    assert_eq!(second.key.key_bytes(), first.key.key_bytes());
    assert_eq!(second.key.key_id(), first.key.key_id());
    assert_eq!(reg.value().as_deref(), Some(&stored[..]), "value unchanged");
}

#[test]
fn load_or_create_never_overwrites_a_value_it_cannot_use() {
    let cng = TestCngKey::create();
    let reg = TestRegistry::create(true);
    let location = reg.location(&cng);
    let good = load_or_create(&location).expect("creates");
    let stored = reg.value().expect("value written");

    // Malformed.
    let garbage = b"not a spool key".to_vec();
    write_value(Hive::CurrentUser, &reg.subkey, &garbage).expect("writes");
    let err = load_or_create(&location).expect_err("malformed");
    assert!(matches!(err, KeyError::BadValue(_)), "{err:?}");
    assert_eq!(reg.value(), Some(garbage));

    // Well formed, but the wrapped bytes were changed.
    let mut tampered = stored.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x80;
    write_value(Hive::CurrentUser, &reg.subkey, &tampered).expect("writes");
    let err = load_or_create(&location).expect_err("tampered");
    assert!(matches!(err, KeyError::Decrypt { .. }), "{err:?}");
    assert_eq!(reg.value(), Some(tampered));

    // Restored, it loads the original key.
    write_value(Hive::CurrentUser, &reg.subkey, &stored).expect("writes");
    let back = load_or_create(&location).expect("loads");
    assert!(!back.created);
    assert_eq!(back.key.key_bytes(), good.key.key_bytes());
}

#[test]
fn load_or_create_needs_the_subkey_and_does_not_create_it() {
    // `ETW-DNSTapService` exists, `ETW-DNSTapService\Protect` does not.
    let reg = TestRegistry::create(false);
    let location = KeyLocation {
        wrapping_key: "unused",
        scope: KeyScope::User,
        hive: Hive::CurrentUser,
        subkey: &reg.subkey,
    };
    let err = load_or_create(&location).expect_err("no subkey");
    assert!(
        matches!(
            err,
            KeyError::RegistryOpen {
                code: ERROR_FILE_NOT_FOUND,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("installer"), "{err}");
    assert!(matches!(
        read_value(Hive::CurrentUser, &reg.subkey),
        Err(KeyError::RegistryOpen {
            code: ERROR_FILE_NOT_FOUND,
            ..
        })
    ));
}

/// The installer gives the service SID `0x20003` (query value, set value, read
/// control) on the subkey and nothing else. This proves create and load work
/// with exactly that.
#[test]
fn the_installer_subkey_rights_are_enough() {
    let cng = TestCngKey::create();
    let reg = TestRegistry::create(true);
    let sid = current_user_sid();
    reg.set_subkey_dacl(&format!("D:P(A;;0x20003;;;{sid})"))
        .expect("subkey DACL");
    let location = reg.location(&cng);
    let first = load_or_create(&location).expect("creates with 0x20003");
    assert!(first.created);
    let second = load_or_create(&location).expect("loads with 0x20003");
    assert!(!second.created);
    assert_eq!(second.key.key_bytes(), first.key.key_bytes());
}

/// The subkey is opened for query and set together, so a DACL without set
/// value fails at the open, before anything is generated.
#[test]
fn the_subkey_open_needs_set_value() {
    let reg = TestRegistry::create(true);
    let sid = current_user_sid();
    reg.set_subkey_dacl(&format!("D:P(A;;0x20001;;;{sid})"))
        .expect("subkey DACL");
    let location = KeyLocation {
        wrapping_key: "unused",
        scope: KeyScope::User,
        hive: Hive::CurrentUser,
        subkey: &reg.subkey,
    };
    let err = load_or_create(&location).expect_err("no set value right");
    assert!(
        matches!(
            err,
            KeyError::RegistryOpen {
                code: ERROR_ACCESS_DENIED,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn load_or_create_writes_nothing_without_the_wrapping_key() {
    let reg = TestRegistry::create(true);
    let missing = unique("EtwDnstap Missing Test Key");
    let location = KeyLocation {
        wrapping_key: &missing,
        scope: KeyScope::User,
        hive: Hive::CurrentUser,
        subkey: &reg.subkey,
    };
    let err = load_or_create(&location).expect_err("no wrapping key");
    assert!(
        matches!(
            err,
            KeyError::KeyOpen {
                status: NTE_BAD_KEYSET,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(reg.value(), None);
}

#[test]
fn a_value_of_another_type_is_rejected() {
    let reg = TestRegistry::create(true);
    let wide = to_wide(&reg.subkey, "test subkey").expect("no NUL");
    let name = to_wide(VALUE_NAME, "value").expect("no NUL");
    let data = 7_u32.to_le_bytes();
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated; `key` is a live out parameter.
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide.as_ptr(),
            0,
            KEY_SET_VALUE,
            &raw mut key,
        )
    };
    assert_eq!(code, ERROR_SUCCESS);
    // SAFETY: `key` is open; `name` is NUL-terminated; `data` is 4 bytes.
    let code = unsafe { RegSetValueExW(key, name.as_ptr(), 0, 4, data.as_ptr(), 4) };
    // SAFETY: `key` was opened above and is not used again.
    unsafe { RegCloseKey(key) };
    assert_eq!(code, ERROR_SUCCESS, "REG_DWORD is written");
    assert!(matches!(
        read_value(Hive::CurrentUser, &reg.subkey),
        Err(KeyError::BadValue("not REG_BINARY"))
    ));
}

/// The installer creates these names; they must not drift.
#[test]
fn production_location_matches_the_installer() {
    let location = KeyLocation::PRODUCTION;
    assert_eq!(location.wrapping_key, "EtwDnstap Spool Key");
    assert_eq!(location.wrapping_key, WRAPPING_KEY_NAME);
    assert_eq!(location.scope, KeyScope::Machine);
    assert_eq!(location.scope.flags(), NCRYPT_MACHINE_KEY_FLAG);
    assert_eq!(location.hive, Hive::LocalMachine);
    assert_eq!(location.hive.handle(), HKEY_LOCAL_MACHINE);
    assert_eq!(location.subkey, r"SOFTWARE\ETW-DNSTapService\Protect");
    assert_eq!(location.subkey, PROTECT_SUBKEY);
    assert_eq!(VALUE_NAME, "WrappedKey");
    assert_eq!(SUBKEY_ACCESS, 0x3, "KEY_QUERY_VALUE | KEY_SET_VALUE only");
}
