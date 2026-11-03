//! The built executable carries one version resource that matches `Cargo.toml`.

#![allow(
    unsafe_code,
    reason = "reading a version resource needs the Win32 version API; each call is wrapped in its own SAFETY-commented block"
)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};

const EXE: &str = env!("CARGO_BIN_EXE_etw-dnstap");

fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn read_block() -> Vec<u8> {
    let path = wide(EXE);
    let mut handle = 0_u32;
    // SAFETY: `path` is NUL-terminated and `handle` is a valid out pointer.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &raw mut handle) };
    assert!(size > 0, "the executable has no version resource");
    let mut block = vec![0_u8; size as usize];
    // SAFETY: `block` holds `size` bytes and `path` is NUL-terminated.
    let ok = unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, block.as_mut_ptr().cast()) };
    assert!(ok != 0, "GetFileVersionInfoW failed");
    block
}

/// Returns the pointer and length (in units of the item) that `VerQueryValueW` reports.
fn query(block: &[u8], sub_block: &str) -> Option<(*const c_void, u32)> {
    let name = wide(sub_block);
    let mut data: *mut c_void = ptr::null_mut();
    let mut length = 0_u32;
    // SAFETY: `block` is a block that GetFileVersionInfoW filled, `name` is
    // NUL-terminated, and both out pointers are valid.
    let ok = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            name.as_ptr(),
            &raw mut data,
            &raw mut length,
        )
    };
    (ok != 0 && !data.is_null()).then_some((data.cast_const(), length))
}

fn string_value(block: &[u8], key: &str) -> String {
    let (data, length) = query(block, &format!("\\StringFileInfo\\040904B0\\{key}"))
        .unwrap_or_else(|| panic!("missing string value {key}"));
    // SAFETY: VerQueryValueW returned `length` UTF-16 units at `data`, inside `block`.
    let units = unsafe { std::slice::from_raw_parts(data.cast::<u16>(), length as usize) };
    String::from_utf16_lossy(units)
        .trim_end_matches('\0')
        .to_owned()
}

fn fixed_info(block: &[u8]) -> VS_FIXEDFILEINFO {
    let (data, _) = query(block, "\\").expect("missing fixed file info");
    // SAFETY: the root query returns a VS_FIXEDFILEINFO inside `block`; read it unaligned.
    unsafe { ptr::read_unaligned(data.cast::<VS_FIXEDFILEINFO>()) }
}

fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn numeric(major: &str, minor: &str, patch: &str) -> u64 {
    let part = |text: &str| text.parse::<u64>().expect("numeric version part");
    (part(major) << 48) | (part(minor) << 32) | (part(patch) << 16)
}

#[test]
fn version_resource_matches_cargo_metadata() {
    let block = read_block();
    let info = fixed_info(&block);
    let expected = numeric(
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
    );
    assert_eq!(
        pack(info.dwFileVersionMS, info.dwFileVersionLS),
        expected,
        "FILEVERSION"
    );
    assert_eq!(
        pack(info.dwProductVersionMS, info.dwProductVersionLS),
        expected,
        "PRODUCTVERSION"
    );

    let short = format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    );
    let strings = [
        ("CompanyName", "miketheitguy"),
        ("LegalCopyright", "Copyright (c) 2026 miketheitguy"),
        ("FileDescription", env!("CARGO_PKG_DESCRIPTION")),
        ("ProductName", "EtwDnstap"),
        ("OriginalFilename", "etw-dnstap.exe"),
        ("InternalName", "etw-dnstap.exe"),
        ("FileVersion", short.as_str()),
        ("ProductVersion", env!("CARGO_PKG_VERSION")),
    ];
    for (key, want) in strings {
        assert_eq!(string_value(&block, key), want, "{key}");
    }
}
