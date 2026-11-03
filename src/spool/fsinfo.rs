//! Win32 calls the spool needs: free space on the spool volume and a trust check
//! on the spool folder. This is the spool's only `unsafe` code.

use std::{
    ffi::{OsStr, c_void},
    fs, io,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, Prefix},
    ptr, slice,
};

use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
        DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetLengthSid, IsValidSid,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    },
    Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, GetDiskFreeSpaceExW, GetDriveTypeW},
};

/// `S-1-5-18`, the local SYSTEM account, in binary form: revision 1, one
/// sub-authority, identifier authority 5 (NT, six big-endian bytes), then the
/// sub-authority 18 as a little-endian `u32`.
const SID_SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
/// `S-1-5-32-544`, the built-in Administrators group: two sub-authorities, 32 and 544.
const SID_ADMINISTRATORS: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 0x02, 0, 0];
/// Identifier authority 5 (NT) as it appears in a binary SID.
const NT_AUTHORITY: [u8; 6] = [0, 0, 0, 0, 0, 5];
/// First sub-authority of a service SID, `S-1-5-80-...`.
const SERVICE_SUB_AUTHORITY: u32 = 80;
/// `S-1-3-0`, CREATOR OWNER: on an inherit-only entry it names the account that
/// creates the file, which is the service.
const SID_CREATOR_OWNER: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];
/// The largest SID: an 8-byte header and 15 sub-authorities of 4 bytes.
const MAX_SID_BYTES: usize = 8 + 15 * 4;

/// `GetDriveTypeW` result for a local fixed disk, the only type the spool
/// accepts. Removable media can be pulled out and may carry no ACLs (FAT), and
/// a RAM disk loses the folder and its ACL at every restart, so the installer's
/// provisioning cannot hold on either. Remote, unknown, and missing-root drives
/// can block the startup path or fall outside the local ACL trust model.
const DRIVE_FIXED: u32 = 3;

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const INHERIT_ONLY_ACE: u8 = 0x08;
/// Rights that let an account change what is in the folder or who may: add a
/// file (`FILE_WRITE_DATA`), add a folder (`FILE_APPEND_DATA`), delete a child
/// (`FILE_DELETE_CHILD`), `DELETE`, `WRITE_DAC`, `WRITE_OWNER`, `GENERIC_WRITE`,
/// and `GENERIC_ALL`.
const WRITE_RIGHTS: u32 = 0x0000_0002
    | 0x0000_0004
    | 0x0000_0040
    | 0x0001_0000
    | 0x0004_0000
    | 0x0008_0000
    | 0x4000_0000
    | 0x1000_0000;
/// Rights that let an account read the folder or the files in it:
/// `FILE_READ_DATA` (`FILE_LIST_DIRECTORY`) and `GENERIC_READ`.
const READ_RIGHTS: u32 = 0x0000_0001 | 0x8000_0000;
/// Rights the spool keeps from every other account: the folder holds DNS data.
const PROTECTED_RIGHTS: u32 = WRITE_RIGHTS | READ_RIGHTS;

/// Why the spool folder is not trusted. Each variant names the reason for the
/// one startup warning.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SpoolDirError {
    #[error("the spool folder path is not absolute")]
    NotAbsolute,
    #[error("the spool folder path has a `.` or `..` component")]
    DotComponent,
    #[error(
        "the spool folder path is not on a local drive letter (network, device, and other path \
         forms are not accepted)"
    )]
    NotLocalDisk,
    #[error("the spool folder is not on a fixed local disk (drive type {code})")]
    DriveNotFixed { code: u32 },
    #[error("the spool folder has no parent folder")]
    NoParent,
    #[error("the spool folder does not exist")]
    Missing,
    #[error("the spool folder path is not a folder")]
    NotAFolder,
    #[error("the {which} is a reparse point")]
    ReparsePoint { which: &'static str },
    #[error("the spool folder owner is not SYSTEM or Administrators")]
    UntrustedOwner,
    #[error("the spool folder has no access control list, so every account can read and write it")]
    NullDacl,
    #[error(
        "the spool folder permissions let an account other than SYSTEM, Administrators, or a service read or write it"
    )]
    UnsafeDacl,
    #[error("cannot check the {which}: {source}")]
    Io {
        which: &'static str,
        source: io::Error,
    },
}

/// Free bytes the calling account can use on the volume that holds `dir`
/// (`GetDiskFreeSpaceExW`, so disk quotas count).
pub(crate) fn available_bytes(dir: &Path) -> io::Result<u64> {
    let wide = to_wide(dir.as_os_str())?;
    let mut free = 0_u64;
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that lives across the
    // call. `free` is a live `u64` the call writes through; the two other out
    // parameters are optional and null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &raw mut free,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

/// The drive letter of a path that starts with `X:\` or `\\?\X:\`, or `None`
/// for every other form (UNC, verbatim UNC, device paths, no prefix).
fn disk_letter(dir: &Path) -> Option<char> {
    match dir.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(char::from(letter.to_ascii_uppercase()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Checks the shape of a spool folder path without touching the file system: it
/// is absolute, has no `.` or `..` component, and starts with a local drive
/// letter. Network (`\\host\share`, `\\?\UNC\...`), device (`\\.\...`), and
/// other verbatim forms are refused, because reaching them can block the
/// startup path and the ACL trust rule does not cover remote accounts.
///
/// # Errors
/// Returns the first rule the path breaks.
pub(crate) fn check_path_shape(dir: &Path) -> Result<(), SpoolDirError> {
    if !dir.is_absolute() {
        return Err(SpoolDirError::NotAbsolute);
    }
    // First: the device prefix `\\.\` holds a dot itself.
    if disk_letter(dir).is_none() {
        return Err(SpoolDirError::NotLocalDisk);
    }
    // `Path::components` drops a `.` inside a path, so look at the text.
    if dir
        .to_string_lossy()
        .split(['\\', '/'])
        .any(|part| part == "." || part == "..")
    {
        return Err(SpoolDirError::DotComponent);
    }
    Ok(())
}

/// True only for a fixed local disk. Takes the `GetDriveTypeW` result so the
/// rule is testable without a drive.
pub(crate) const fn drive_type_is_allowed(code: u32) -> bool {
    code == DRIVE_FIXED
}

/// The `GetDriveTypeW` code of the root of `letter`, for example `C:\`.
fn drive_type(letter: char) -> io::Result<u32> {
    let root = format!("{letter}:\\");
    let wide = to_wide(OsStr::new(&root))?;
    // SAFETY: `wide` is a NUL-terminated UTF-16 root path that lives across the
    // call, which only reads it.
    Ok(unsafe { GetDriveTypeW(wide.as_ptr()) })
}

/// Checks that the spool folder path is a plain local path (see
/// [`check_path_shape`]) on a fixed disk, that the folder exists, that it and its parent are not reparse points, that
/// SYSTEM or Administrators own it, and that its access control list gives no
/// other account (except a service) the right to write.
///
/// This is a check at one moment. The installer's ACL, which the service cannot
/// change, is what keeps other accounts from replacing the folder afterwards.
pub(crate) fn check_spool_dir(dir: &Path) -> Result<(), SpoolDirError> {
    check_path_shape(dir)?;
    // Before any other file system call: a remote or missing drive can block.
    let letter = disk_letter(dir).ok_or(SpoolDirError::NotLocalDisk)?;
    let code = drive_type(letter).map_err(|source| SpoolDirError::Io {
        which: "spool drive type",
        source,
    })?;
    if !drive_type_is_allowed(code) {
        return Err(SpoolDirError::DriveNotFixed { code });
    }
    let parent = dir.parent().ok_or(SpoolDirError::NoParent)?;
    // `symlink_metadata` does not follow a link or junction, so a reparse point
    // shows its own attributes.
    let metadata = fs::symlink_metadata(dir).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            SpoolDirError::Missing
        } else {
            SpoolDirError::Io {
                which: "spool folder",
                source,
            }
        }
    })?;
    if is_reparse_point(&metadata) {
        return Err(SpoolDirError::ReparsePoint {
            which: "spool folder",
        });
    }
    if !metadata.is_dir() {
        return Err(SpoolDirError::NotAFolder);
    }
    let parent_metadata = fs::symlink_metadata(parent).map_err(|source| SpoolDirError::Io {
        which: "parent of the spool folder",
        source,
    })?;
    if is_reparse_point(&parent_metadata) {
        return Err(SpoolDirError::ReparsePoint {
            which: "parent of the spool folder",
        });
    }
    let security = read_security(dir).map_err(|source| SpoolDirError::Io {
        which: "spool folder permissions",
        source,
    })?;
    if !owner_is_trusted(&security.owner) {
        return Err(SpoolDirError::UntrustedOwner);
    }
    match security.dacl {
        None => Err(SpoolDirError::NullDacl),
        Some(entries) if dacl_is_safe(&entries) => Ok(()),
        Some(_) => Err(SpoolDirError::UnsafeDacl),
    }
}

/// True when the binary SID is SYSTEM (`S-1-5-18`) or Administrators
/// (`S-1-5-32-544`). Any other SID, including a malformed one, is untrusted.
pub(crate) fn owner_is_trusted(sid: &[u8]) -> bool {
    sid == SID_SYSTEM || sid == SID_ADMINISTRATORS
}

/// True for a service SID, `S-1-5-80-...`, such as `NT SERVICE\EtwDnstap`.
fn is_service_sid(sid: &[u8]) -> bool {
    let well_formed = sid.len() >= 12
        && sid[0] == 1
        && usize::from(sid[1]) >= 2
        && sid.len() == 8 + 4 * usize::from(sid[1])
        && sid[2..8] == NT_AUTHORITY;
    well_formed && u32::from_le_bytes([sid[8], sid[9], sid[10], sid[11]]) == SERVICE_SUB_AUTHORITY
}

/// One entry of a discretionary access control list, reduced to what the trust
/// rule reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AceEntry {
    pub(crate) ace_type: u8,
    pub(crate) flags: u8,
    pub(crate) mask: u32,
    /// The trustee SID bytes; empty if the entry has no readable SID.
    pub(crate) sid: Vec<u8>,
}

/// The trust rule for one entry. A deny entry never grants access. An allow
/// entry is safe if it grants no right to read or change the folder, or if it
/// names SYSTEM, Administrators, or a service. This holds for inherit-only
/// entries too, because they decide who can read the segment files the service
/// creates; an inherit-only entry may also name CREATOR OWNER, which maps to the
/// service. Any other entry type is not safe.
pub(crate) fn ace_is_safe(ace_type: u8, flags: u8, mask: u32, sid: &[u8]) -> bool {
    match ace_type {
        ACCESS_DENIED_ACE_TYPE => true,
        ACCESS_ALLOWED_ACE_TYPE => {
            mask & PROTECTED_RIGHTS == 0
                || owner_is_trusted(sid)
                || is_service_sid(sid)
                || (flags & INHERIT_ONLY_ACE != 0 && sid == SID_CREATOR_OWNER)
        }
        _ => false,
    }
}

/// True when every entry passes [`ace_is_safe`].
pub(crate) fn dacl_is_safe(entries: &[AceEntry]) -> bool {
    entries
        .iter()
        .all(|entry| ace_is_safe(entry.ace_type, entry.flags, entry.mask, &entry.sid))
}

pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// True when `path` is a regular file and not a link or other reparse point.
/// The path itself is examined, not what it points to.
pub(super) fn is_plain_file(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_file() && !is_reparse_point(&metadata))
}

/// Frees a security descriptor that `GetNamedSecurityInfoW` allocated.
struct LocalAllocation(PSECURITY_DESCRIPTOR);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: The pointer came from `GetNamedSecurityInfoW`, which
            // allocates the descriptor with `LocalAlloc`, and this guard is its
            // only owner, so it is freed once.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// The owner and the discretionary ACL of a file or folder, copied out of the
/// system's security descriptor.
pub(crate) struct Security {
    pub(crate) owner: Vec<u8>,
    /// The ACL entries, or `None` for a null ACL (no protection).
    pub(crate) dacl: Option<Vec<AceEntry>>,
}

/// Copies the owner SID of the file or folder at `path` into an owned buffer.
#[cfg(test)]
pub(super) fn owner_sid(path: &Path) -> io::Result<Vec<u8>> {
    read_security(path).map(|security| security.owner)
}

/// Reads the owner and the DACL of `path` in one call.
pub(crate) fn read_security(path: &Path) -> io::Result<Security> {
    let wide = to_wide(path.as_os_str())?;
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that lives across the
    // call. `owner`, `dacl`, and `descriptor` are live out pointers; with only
    // OWNER and DACL information requested, the group and SACL out parameters
    // may be null. On success `owner` and `dacl` point into `descriptor`.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut dacl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    // Own the descriptor before any early return, so it is always freed. It
    // stays alive until the end of this function, after both copies are made.
    let _descriptor = LocalAllocation(descriptor);
    if status != 0 {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ));
    }
    let owner = copy_sid(owner)?;
    let dacl = if dacl.is_null() {
        None
    } else {
        Some(read_acl(dacl)?)
    };
    Ok(Security { owner, dacl })
}

/// Copies a SID out of a security descriptor that the caller keeps alive.
fn copy_sid(sid: PSID) -> io::Result<Vec<u8>> {
    if sid.is_null() {
        return Err(io::Error::other("the folder has no owner"));
    }
    // SAFETY: `sid` is non-null and points into the live descriptor.
    let valid = unsafe { IsValidSid(sid) };
    if valid == 0 {
        return Err(io::Error::other("the folder owner is not a valid SID"));
    }
    // SAFETY: `sid` is a valid SID in the live descriptor.
    let length = unsafe { GetLengthSid(sid) };
    let length = usize::try_from(length).unwrap_or(usize::MAX);
    if length > MAX_SID_BYTES {
        return Err(io::Error::other("the folder owner SID is too long"));
    }
    // SAFETY: A valid SID occupies `GetLengthSid` bytes at `sid`, the
    // descriptor outlives this borrow, and the bytes are copied before it ends.
    let bytes = unsafe { slice::from_raw_parts(sid.cast::<u8>(), length) };
    Ok(bytes.to_vec())
}

/// Copies every entry of an ACL that lives in a descriptor the caller keeps
/// alive. Each entry is checked against the ACL's bytes in use before it is read.
fn read_acl(acl: *const ACL) -> io::Result<Vec<AceEntry>> {
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    let info_length = u32::try_from(size_of::<ACL_SIZE_INFORMATION>())
        .map_err(|_| io::Error::other("ACL information size exceeds u32"))?;
    // SAFETY: `acl` is a non-null ACL in the live descriptor. `info` is a live
    // `ACL_SIZE_INFORMATION` and `info_length` is its size, as the
    // `AclSizeInformation` class requires.
    let sized = unsafe {
        GetAclInformation(
            acl,
            (&raw mut info).cast::<c_void>(),
            info_length,
            AclSizeInformation,
        )
    };
    if sized == 0 {
        return Err(io::Error::last_os_error());
    }
    let start = acl.addr();
    let end = start
        .checked_add(usize::try_from(info.AclBytesInUse).unwrap_or(usize::MAX))
        .ok_or_else(|| io::Error::other("ACL size overflows"))?;
    let mut entries = Vec::new();
    for index in 0..info.AceCount {
        let mut ace: *mut c_void = ptr::null_mut();
        // SAFETY: `acl` is a valid ACL in the live descriptor, `index` is below
        // its entry count, and `ace` is a live out pointer.
        let fetched = unsafe { GetAce(acl, index, &raw mut ace) };
        if fetched == 0 || ace.is_null() {
            return Err(io::Error::other("cannot read an ACL entry"));
        }
        let address = ace.addr();
        // The entry header (type, flags, size) must lie inside the ACL.
        if address < start
            || address
                .checked_add(4)
                .is_none_or(|header_end| header_end > end)
        {
            return Err(io::Error::other("an ACL entry lies outside the ACL"));
        }
        // SAFETY: The four header bytes at `ace` are inside the ACL's bytes in
        // use, checked above, and the descriptor is live.
        let header = unsafe { slice::from_raw_parts(ace.cast::<u8>(), 4) };
        let size = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if size < 8
            || address
                .checked_add(size)
                .is_none_or(|ace_end| ace_end > end)
        {
            return Err(io::Error::other("an ACL entry has a bad size"));
        }
        // SAFETY: `size` bytes at `ace` are inside the ACL's bytes in use,
        // checked above; they are copied by `parse_ace` before the descriptor
        // is freed.
        let bytes = unsafe { slice::from_raw_parts(ace.cast::<u8>(), size) };
        entries.push(parse_ace(bytes));
    }
    Ok(entries)
}

/// Reads the fields the trust rule needs from the bytes of one entry (at least
/// 8 bytes). Only allow and deny entries have a mask and a SID in this layout;
/// any other type keeps its type and gets no SID.
fn parse_ace(bytes: &[u8]) -> AceEntry {
    let ace_type = bytes[0];
    let mut entry = AceEntry {
        ace_type,
        flags: bytes[1],
        mask: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        sid: Vec::new(),
    };
    if matches!(ace_type, ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE) {
        let sid = &bytes[8..];
        if sid.len() >= 8 && sid[1] <= 15 {
            let length = 8 + 4 * usize::from(sid[1]);
            if length <= sid.len() {
                entry.sid = sid[..length].to_vec();
            }
        }
    }
    entry
}

/// UTF-16 with one terminating NUL. An embedded NUL would end the string early
/// in the Win32 call, so it is rejected.
fn to_wide(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = text.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains a NUL character",
        ));
    }
    wide.push(0);
    Ok(wide)
}
