//! The name-based GUID of the service's own ETW session.
//!
//! ETW checks `TRACELOG_CREATE_REALTIME` against the security descriptor of
//! the session's GUID, and falls back to the shared default descriptor
//! `{0811c1af-7a07-4a06-82ed-869455cdf713}` when that GUID has none of its own.
//! Giving the session a stable GUID lets provisioning grant the restricted
//! service SID that one right on this GUID alone, the way Windows scopes its
//! own services' ETW grants, instead of editing the shared default.
//!
//! The GUID is SHA-1 over the UTF-16LE text `EtwDnstap.Session:<name>`, with
//! the RFC 9562 version-5 and variant bits set, laid out as .NET
//! `new Guid(byte[])` reads the first 16 hash bytes. The session name is
//! hashed exactly as given, with no case folding.
//! `scripts/provision/Grant-EtwDnstapEtwAccess.ps1` computes the same value.

use anyhow::{Result, ensure};
use windows_sys::{
    Win32::Security::Cryptography::{BCRYPT_SHA1_ALG_HANDLE, BCryptHash},
    core::GUID,
};

/// Prefix hashed in front of the session name.
const PREFIX: &str = "EtwDnstap.Session:";

/// Returns the session GUID for `name`.
pub(crate) fn session_guid(name: &str) -> Result<GUID> {
    let input: Vec<u8> = PREFIX
        .encode_utf16()
        .chain(name.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let length = u32::try_from(input.len())?;
    let mut digest = [0u8; 20];
    // SAFETY: BCRYPT_SHA1_ALG_HANDLE is a documented pseudo-handle that needs no
    // open or close. `input` and `digest` are live locals whose lengths are
    // passed exactly, and the synchronous call writes only `digest`.
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA1_ALG_HANDLE,
            core::ptr::null(),
            0,
            input.as_ptr(),
            length,
            digest.as_mut_ptr(),
            20,
        )
    };
    ensure!(status == 0, "BCryptHash(SHA1) failed: NTSTATUS {status:#x}");
    Ok(guid_from_digest(&digest))
}

fn guid_from_digest(digest: &[u8; 20]) -> GUID {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 5 lives in the high nibble of data3 (byte 7, little-endian);
    // the RFC variant in the top two bits of data4[0] (byte 8).
    bytes[7] = (bytes[7] & 0x0F) | 0x50;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    GUID {
        data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        data2: u16::from_le_bytes([bytes[4], bytes[5]]),
        data3: u16::from_le_bytes([bytes[6], bytes[7]]),
        data4: [
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ],
    }
}

/// Formats a GUID as lowercase `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
pub(crate) fn format_guid(guid: &GUID) -> String {
    let d = guid.data4;
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1, guid.data2, guid.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_values_the_provisioning_script_computes() {
        // Reference values from .NET: SHA1 over UTF-16LE, version/variant bits,
        // `new Guid(byte[16])`, as in Grant-EtwDnstapEtwAccess.ps1.
        let default = session_guid("EtwDnstap").expect("hash");
        assert_eq!(
            format_guid(&default),
            "05e1db3c-1322-58c7-8235-0463fccba32b"
        );
        let lab = session_guid("EtwDnstap-Lab").expect("hash");
        assert_eq!(format_guid(&lab), "43b7b2fc-777a-5214-b6ed-665c1ff9e972");
    }

    #[test]
    fn names_are_hashed_exactly_as_given() {
        let upper = session_guid("ETWDNSTAP").expect("hash");
        let default = session_guid("EtwDnstap").expect("hash");
        assert_ne!(format_guid(&upper), format_guid(&default));
    }

    #[test]
    fn version_and_variant_bits_are_set() {
        let guid = guid_from_digest(&[0xFF; 20]);
        assert_eq!(guid.data3 >> 12, 5);
        assert_eq!(guid.data4[0] & 0xC0, 0x80);
        let guid = guid_from_digest(&[0; 20]);
        assert_eq!(guid.data3 >> 12, 5);
        assert_eq!(guid.data4[0] & 0xC0, 0x80);
    }

    #[test]
    fn format_is_lowercase_hyphenated() {
        let guid = GUID::from_u128(0x0811_C1AF_7A07_4A06_82ED_8694_55CD_F713);
        assert_eq!(format_guid(&guid), "0811c1af-7a07-4a06-82ed-869455cdf713");
    }
}
