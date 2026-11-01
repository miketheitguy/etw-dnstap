//! SCM-only host, with token validation before capture initialization.

use std::{
    io,
    mem::{offset_of, size_of},
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use anyhow::{Context, Result, anyhow, ensure};
use tracing::{Level, Metadata};
use tracing_subscriber::fmt::MakeWriter;
use windows_sys::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_CALL_NOT_IMPLEMENTED, ERROR_INSUFFICIENT_BUFFER,
            ERROR_SERVICE_SPECIFIC_ERROR, ERROR_SUCCESS, GetLastError, HANDLE, LUID,
        },
        Security::{
            GetTokenInformation, LUID_AND_ATTRIBUTES, LookupAccountNameW, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, SID_AND_ATTRIBUTES, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS,
            TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenGroups, TokenPrivileges, TokenUser,
        },
        System::{
            EventLog::{
                DeregisterEventSource, EVENTLOG_ERROR_TYPE, EVENTLOG_INFORMATION_TYPE,
                EVENTLOG_WARNING_TYPE, RegisterEventSourceW, ReportEventW,
            },
            Services::{
                RegisterServiceCtrlHandlerExW, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP,
                SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
                SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE,
                SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW,
                SERVICE_WIN32_OWN_PROCESS, SetServiceStatus, StartServiceCtrlDispatcherW,
            },
            SystemServices::{SE_GROUP_ENABLED, SE_GROUP_USE_FOR_DENY_ONLY},
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::w,
};

use crate::pipeline::{self, CaptureArgs};

static DISPATCHED: AtomicBool = AtomicBool::new(false);
// Argument errors are carried as text so the SCM path and console path report
// the same clap rendering without retaining parser state.
static ARGS: Mutex<Option<Result<CaptureArgs, String>>> = Mutex::new(None);
static RESULT: Mutex<Option<Result<()>>> = Mutex::new(None);
// Process-lifetime SCM checkpoint for pending states. Only ServiceMain reports
// status, so relaxed ordering keeps the counter monotonic per pending phase.
static CHECKPOINT: AtomicU32 = AtomicU32::new(0);
// A control callback may run until the dispatcher returns. Static storage avoids
// releasing callback state when ServiceMain reports Stopped.
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Connects to SCM; executing the binary from a console never starts capture.
pub(crate) fn run(args: Result<CaptureArgs, String>) -> Result<()> {
    ensure!(
        !DISPATCHED.swap(true, Ordering::AcqRel),
        "SCM dispatcher already invoked"
    );
    STOP.set(Arc::new(AtomicBool::new(false)))
        .map_err(|_| anyhow!("service stop state already initialized"))?;
    *ARGS
        .lock()
        .map_err(|_| anyhow!("service arguments lock poisoned"))? = Some(args);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: w!("EtwDnstap").cast_mut(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: The terminated table and UTF-16 service name remain valid for the
    // blocking dispatcher call. Callback state has process lifetime.
    let success = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if success == 0 {
        // A console launch fails here before ServiceMain runs. Surface invalid
        // arguments first so the console user sees the clap message.
        let invalid = ARGS
            .lock()
            .map_err(|_| anyhow!("service arguments lock poisoned"))?
            .as_ref()
            .and_then(|args| args.as_ref().err().cloned());
        if let Some(text) = invalid {
            return Err(anyhow!("{text}"));
        }
        return Err(last_error(
            "StartServiceCtrlDispatcherW (start EtwDnstap through SCM)",
        ));
    }
    RESULT
        .lock()
        .map_err(|_| anyhow!("service result lock poisoned"))?
        .take()
        .context("SCM dispatcher returned without a service result")?
}

unsafe extern "system" fn service_main(_count: u32, _arguments: *mut *mut u16) {
    // SAFETY: The service name has static UTF-16 storage. Handler uses only
    // process-lifetime STOP state, so no callback context pointer is borrowed.
    let handle =
        unsafe { RegisterServiceCtrlHandlerExW(w!("EtwDnstap"), Some(control_handler), null()) };
    if handle.is_null() {
        save_result(Err(last_error("RegisterServiceCtrlHandlerExW")));
        return;
    }
    let result = run_registered(handle);
    let final_guard = pipeline::ShutdownWatchdog::arm();
    if result.is_err() {
        let _status = report_status(handle, SERVICE_STOP_PENDING, false);
    }
    if let Err(error) = &result {
        report_failure(error);
    }
    // Running and pending notifications precede this final notification. Capture
    // has already drained and joined before successful run_registered returns.
    let failed = result.is_err();
    // SCM may terminate the process after Stopped, so publish the result first.
    save_result(result);
    if let Err(error) = report_status(handle, SERVICE_STOPPED, failed) {
        save_result(Err(error));
    }
    // This retires only the supervisor; all capture work and result publication
    // precede Stopped. No native callback storage is released here.
    if final_guard.disarm().is_err() {
        std::process::abort();
    }
}

fn run_registered(handle: SERVICE_STATUS_HANDLE) -> Result<()> {
    let mut startup_guard = Some(pipeline::ShutdownWatchdog::arm());
    let result = (|| {
        report_status(handle, SERVICE_START_PENDING, false)?;
        // Identity is verified before argument validation, so an unprivileged
        // or misconfigured host never reads its configuration.
        verify_identity()?;
        let args = ARGS
            .lock()
            .map_err(|_| anyhow!("service arguments lock poisoned"))?
            .take()
            .context("missing service arguments")?
            .map_err(|text| anyhow!("invalid service arguments: {text}"))?;
        let stop = STOP.get().context("missing service stop state")?;
        pipeline::run_capture(
            args,
            stop,
            || {
                report_status(handle, SERVICE_RUNNING, false)?;
                if let Some(guard) = startup_guard.take() {
                    guard.disarm()?;
                }
                Ok(())
            },
            || report_status(handle, SERVICE_STOP_PENDING, false),
        )
    })();
    if let Some(guard) = startup_guard.take() {
        guard.disarm()?;
    }
    result
}

fn save_result(result: Result<()>) {
    match RESULT.lock() {
        Ok(mut destination) => *destination = Some(result),
        // No unwind may cross an SCM ABI boundary, including a poisoned lock.
        Err(_) => std::process::abort(),
    }
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event: u32,
    _data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    STOP.get().map_or(ERROR_CALL_NOT_IMPLEMENTED, |stop| {
        handle_control(control, stop)
    })
}

fn handle_control(control: u32, stop: &AtomicBool) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            stop.store(true, Ordering::Release);
            ERROR_SUCCESS
        }
        SERVICE_CONTROL_INTERROGATE => ERROR_SUCCESS,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

fn is_pending(state: u32) -> bool {
    matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING)
}

/// Advances the checkpoint for pending states and resets it for all others.
fn checkpoint_for(counter: &AtomicU32, state: u32) -> u32 {
    if is_pending(state) {
        counter.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    } else {
        counter.store(0, Ordering::Relaxed);
        0
    }
}

fn status_record(state: u32, failed: bool, checkpoint: u32) -> SERVICE_STATUS {
    let pending = is_pending(state);
    SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: if failed {
            ERROR_SERVICE_SPECIFIC_ERROR
        } else {
            ERROR_SUCCESS
        },
        dwServiceSpecificExitCode: u32::from(failed),
        dwCheckPoint: checkpoint,
        dwWaitHint: if pending { 15_000 } else { 0 },
    }
}

fn report_status(handle: SERVICE_STATUS_HANDLE, state: u32, failed: bool) -> Result<()> {
    let status = status_record(state, failed, checkpoint_for(&CHECKPOINT, state));
    // SAFETY: This handle was returned by RegisterServiceCtrlHandlerExW for this
    // ServiceMain; it is borrowed (never CloseHandle'd). status is initialized.
    if unsafe { SetServiceStatus(handle, &raw const status) } == 0 {
        return Err(last_error("SetServiceStatus"));
    }
    Ok(())
}

fn last_error(operation: &str) -> anyhow::Error {
    // SAFETY: Callers invoke this immediately after a failing Win32 API.
    let error = unsafe { GetLastError() };
    anyhow!(
        "{operation}: {} (code {error})",
        std::io::Error::from_raw_os_error(error.cast_signed())
    )
}

/// Best-effort failure detail; callers keep this synchronous API under a deadline.
fn report_failure(error: &anyhow::Error) {
    // Error chains contain operation/status metadata, never DNS message contents.
    let text = event_text(&format!("EtwDnstap failed: {error:#}"));
    if let Some(source) = EventSource::open() {
        // Event ID 1 is reserved for fatal service failures.
        source.report(EVENTLOG_ERROR_TYPE, 1, &text);
    }
}

/// Maximum insertion-string length in UTF-16 units, excluding the terminator.
const MAX_EVENT_UNITS: usize = 8192;
/// Bounds per-event buffering; the sanitizer caps what is finally reported.
const MAX_LINE_BYTES: usize = 16 * 1024;

/// Trims trailing CR/LF, replaces NUL, caps length, and appends the terminator.
fn event_text(message: &str) -> Vec<u16> {
    message
        .trim_end_matches(['\r', '\n'])
        .encode_utf16()
        .take(MAX_EVENT_UNITS)
        .map(|unit| if unit == 0 { 0xfffd } else { unit })
        .chain(Some(0))
        .collect()
}

/// Maps tracing severity to Event Log type and ID; ID 1 stays reserved.
fn event_kind(level: Level) -> (u16, u32) {
    if level == Level::ERROR {
        (EVENTLOG_ERROR_TYPE, 4)
    } else if level == Level::WARN {
        (EVENTLOG_WARNING_TYPE, 3)
    } else {
        (EVENTLOG_INFORMATION_TYPE, 2)
    }
}

struct EventSource(HANDLE);

// SAFETY: The source handle is process-lifetime state with no thread affinity.
// Only ReportEventW uses it, and the wrapper holds no Rust-side mutable state.
unsafe impl Send for EventSource {}
// SAFETY: Shared access only calls ReportEventW, which takes the handle by value
// and is safe to invoke concurrently from any thread on the same source.
unsafe impl Sync for EventSource {}

impl EventSource {
    /// Registers the `EtwDnstap` source on the local server, or returns `None`.
    fn open() -> Option<Self> {
        // SAFETY: null selects the local server and the source name is static UTF-16.
        let handle = unsafe { RegisterEventSourceW(null(), w!("EtwDnstap")) };
        (!handle.is_null()).then_some(Self(handle))
    }

    /// Reports one event; `text` must be terminated, as produced by `event_text`.
    fn report(&self, kind: u16, event_id: u32, text: &[u16]) {
        let strings = [text.as_ptr()];
        // SAFETY: self owns an open source; the one terminated UTF-16 insertion
        // string and pointer array remain alive throughout this synchronous call.
        // No SID, raw-data buffer, or remote server is supplied.
        unsafe {
            ReportEventW(
                self.0,
                kind,
                0,
                event_id,
                null_mut(),
                1,
                0,
                strings.as_ptr(),
                null(),
            );
        }
    }
}

impl Drop for EventSource {
    fn drop(&mut self) {
        // SAFETY: RegisterEventSourceW transferred this handle; this matching API
        // releases it exactly once after ReportEventW has finished using it.
        unsafe {
            DeregisterEventSource(self.0);
        }
    }
}

/// Registered lazily once per process; a failed registration is remembered and
/// later messages are dropped silently, so logging never stops capture.
static EVENT_SOURCE: OnceLock<Option<EventSource>> = OnceLock::new();

fn event_source() -> Option<&'static EventSource> {
    EVENT_SOURCE.get_or_init(EventSource::open).as_ref()
}

/// Buffers one formatted tracing event and reports it when dropped or flushed.
pub(crate) struct EventLogLine {
    kind: u16,
    event_id: u32,
    bytes: Vec<u8>,
}

impl EventLogLine {
    fn for_level(level: Level) -> Self {
        let (kind, event_id) = event_kind(level);
        Self {
            kind,
            event_id,
            bytes: Vec::new(),
        }
    }

    fn report(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let text = event_text(&String::from_utf8_lossy(&self.bytes));
        self.bytes.clear();
        if let Some(source) = event_source() {
            source.report(self.kind, self.event_id, &text);
        }
    }
}

impl io::Write for EventLogLine {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let room = MAX_LINE_BYTES.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&buffer[..buffer.len().min(room)]);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.report();
        Ok(())
    }
}

impl Drop for EventLogLine {
    fn drop(&mut self) {
        self.report();
    }
}

/// Sends tracing output to the Application Event Log under source `EtwDnstap`.
pub(crate) struct EventLogWriter;

impl<'a> MakeWriter<'a> for EventLogWriter {
    type Writer = EventLogLine;

    fn make_writer(&'a self) -> Self::Writer {
        EventLogLine::for_level(Level::INFO)
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        EventLogLine::for_level(*meta.level())
    }
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: OpenProcessToken transferred this owned kernel handle; this
        // wrapper closes it exactly once after all native token reads complete.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct TokenBuffer {
    storage: Vec<u64>,
    used: usize,
}

impl TokenBuffer {
    fn read(token: &Token, class: TOKEN_INFORMATION_CLASS) -> Result<Self> {
        let mut size = 0u32;
        // SAFETY: token is open; this documented size query has no output buffer.
        let success = unsafe { GetTokenInformation(token.0, class, null_mut(), 0, &raw mut size) };
        // SAFETY: Read immediately following the expected size-query failure.
        let error = unsafe { GetLastError() };
        ensure!(
            success == 0 && error == ERROR_INSUFFICIENT_BUFFER,
            "GetTokenInformation size query failed: {error}"
        );
        ensure!(
            (1..=65_536).contains(&size),
            "invalid token information size"
        );
        let capacity = size;
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: The initialized u64 allocation supplies native token structure
        // alignment and at least capacity bytes; token remains open.
        if unsafe {
            GetTokenInformation(
                token.0,
                class,
                storage.as_mut_ptr().cast(),
                capacity,
                &raw mut size,
            )
        } == 0
        {
            return Err(last_error("GetTokenInformation"));
        }
        ensure!(size <= capacity, "token information exceeds allocation");
        Ok(Self {
            storage,
            used: size as usize,
        })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: GetTokenInformation initialized used bytes in this allocation,
        // and read verified used does not exceed its initialized capacity.
        unsafe { std::slice::from_raw_parts(self.storage.as_ptr().cast(), self.used) }
    }

    fn sid(&self, pointer: *mut core::ffi::c_void) -> Result<&[u8]> {
        bounded_sid(self.bytes(), pointer as usize)
    }
}

fn bounded_sid(buffer: &[u8], address: usize) -> Result<&[u8]> {
    let start = buffer.as_ptr() as usize;
    let offset = address
        .checked_sub(start)
        .context("SID precedes token buffer")?;
    ensure!(address.is_multiple_of(4), "misaligned token SID");
    let header = buffer
        .get(offset..)
        .and_then(|tail| tail.get(..8))
        .context("SID header exceeds token buffer")?;
    ensure!(
        header[0] == 1 && header[1] <= 15,
        "invalid token SID revision or length"
    );
    let length = 8 + usize::from(header[1]) * 4;
    buffer
        .get(offset..)
        .and_then(|tail| tail.get(..length))
        .context("SID exceeds token buffer")
}

/// This first deployment profile intentionally supports `LocalSystem` only.
fn verify_identity() -> Result<()> {
    const LOCAL_SYSTEM: &[u8] = &[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
    let mut raw = null_mut();
    // SAFETY: GetCurrentProcess returns a borrowed pseudo-handle; OpenProcessToken
    // creates the separate owned query handle written to raw.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut raw) } == 0 {
        return Err(last_error("OpenProcessToken"));
    }
    let token = Token(raw);
    let user = TokenBuffer::read(&token, TokenUser)?;
    ensure!(user.used >= size_of::<TOKEN_USER>(), "truncated token user");
    // SAFETY: The aligned buffer contains the complete TOKEN_USER header; the
    // embedded SID pointer is range-validated before inspecting SID bytes.
    let user_header = unsafe { &*user.storage.as_ptr().cast::<TOKEN_USER>() };
    ensure!(
        user.sid(user_header.User.Sid)? == LOCAL_SYSTEM,
        "EtwDnstap currently requires the LocalSystem account"
    );
    let expected_sid = service_sid()?;
    let groups = TokenBuffer::read(&token, TokenGroups)?;
    ensure!(
        groups.used >= size_of::<TOKEN_GROUPS>(),
        "truncated token groups"
    );
    // SAFETY: read supplies native alignment and the complete fixed header.
    let header = unsafe { &*groups.storage.as_ptr().cast::<TOKEN_GROUPS>() };
    let count = header.GroupCount as usize;
    let offset = offset_of!(TOKEN_GROUPS, Groups);
    let end = count
        .checked_mul(size_of::<SID_AND_ATTRIBUTES>())
        .and_then(|n| offset.checked_add(n))
        .context("token groups overflow")?;
    ensure!(
        count <= 1024 && end <= groups.used,
        "invalid token groups array"
    );
    // SAFETY: The native-aligned flexible array fits entirely in the initialized
    // token buffer as validated above; embedded SID pointers are checked below.
    let entries = unsafe {
        std::slice::from_raw_parts(
            groups
                .storage
                .as_ptr()
                .byte_add(offset)
                .cast::<SID_AND_ATTRIBUTES>(),
            count,
        )
    };
    // The SID must be enabled and must not be restricted to deny-only use.
    let restriction_mask = (SE_GROUP_ENABLED | SE_GROUP_USE_FOR_DENY_ONLY).cast_unsigned();
    let enabled_only = SE_GROUP_ENABLED.cast_unsigned();
    let mut service_enabled = false;
    for entry in entries {
        let sid = groups.sid(entry.Sid)?;
        if sid == expected_sid.as_slice() && entry.Attributes & restriction_mask == enabled_only {
            service_enabled = true;
        }
    }
    ensure!(
        service_enabled,
        "EtwDnstap service SID must be enabled and not deny-only (configure sc.exe sidtype EtwDnstap unrestricted)"
    );
    verify_privileges(&token)?;
    Ok(())
}

fn verify_privileges(token: &Token) -> Result<()> {
    let mut allowed = LUID::default();
    // SAFETY: The terminated privilege name is static and allowed receives the
    // identifier assigned to this privilege on the local machine.
    if unsafe { LookupPrivilegeValueW(null(), w!("SeChangeNotifyPrivilege"), &raw mut allowed) }
        == 0
    {
        return Err(last_error("LookupPrivilegeValueW"));
    }
    let buffer = TokenBuffer::read(token, TokenPrivileges)?;
    ensure!(
        buffer.used >= size_of::<TOKEN_PRIVILEGES>(),
        "truncated token privileges"
    );
    // SAFETY: TokenBuffer provides native alignment and a complete fixed header.
    let header = unsafe { &*buffer.storage.as_ptr().cast::<TOKEN_PRIVILEGES>() };
    let count = header.PrivilegeCount as usize;
    let offset = offset_of!(TOKEN_PRIVILEGES, Privileges);
    let end = count
        .checked_mul(size_of::<LUID_AND_ATTRIBUTES>())
        .and_then(|n| offset.checked_add(n))
        .context("token privilege array overflow")?;
    ensure!(
        count == 1 && end <= buffer.used,
        "configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege before starting EtwDnstap"
    );
    // SAFETY: Exactly one complete native-aligned entry fits in the initialized
    // token buffer, as checked above.
    let privilege = unsafe {
        &*buffer
            .storage
            .as_ptr()
            .byte_add(offset)
            .cast::<LUID_AND_ATTRIBUTES>()
    };
    ensure!(
        privilege.Luid.LowPart == allowed.LowPart
            && privilege.Luid.HighPart == allowed.HighPart
            && privilege.Attributes & SE_PRIVILEGE_ENABLED != 0,
        "EtwDnstap requires only an enabled SeChangeNotifyPrivilege in its process token"
    );
    Ok(())
}

fn service_sid() -> Result<Vec<u8>> {
    let mut sid = [0u64; 9];
    let mut size = 68u32;
    let mut domain = [0u16; 256];
    let mut domain_size = 256u32;
    let mut sid_use = 0;
    // SAFETY: All output buffers are aligned, initialized, and match the supplied
    // sizes. The service account name has process-lifetime UTF-16 storage.
    if unsafe {
        LookupAccountNameW(
            null(),
            w!("NT SERVICE\\EtwDnstap"),
            sid.as_mut_ptr().cast(),
            &raw mut size,
            domain.as_mut_ptr(),
            &raw mut domain_size,
            &raw mut sid_use,
        )
    } == 0
    {
        return Err(last_error("LookupAccountNameW(NT SERVICE\\EtwDnstap)"));
    }
    ensure!(
        size as usize <= size_of_val(&sid),
        "service SID exceeds allocation"
    );
    // SAFETY: The successful API initialized size bytes within the SID allocation.
    let bytes = unsafe { std::slice::from_raw_parts(sid.as_ptr().cast::<u8>(), size as usize) };
    Ok(bounded_sid(bytes, bytes.as_ptr() as usize)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_only_signal_stop_and_reject_unknown_codes() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            handle_control(SERVICE_CONTROL_INTERROGATE, &stop),
            ERROR_SUCCESS
        );
        assert!(!stop.load(Ordering::Acquire));
        assert_eq!(handle_control(999, &stop), ERROR_CALL_NOT_IMPLEMENTED);
        assert_eq!(handle_control(SERVICE_CONTROL_STOP, &stop), ERROR_SUCCESS);
        assert!(stop.load(Ordering::Acquire));
    }

    #[test]
    fn pending_status_accepts_no_controls_and_failure_is_reported() {
        let pending = status_record(SERVICE_START_PENDING, false, 1);
        assert_eq!(pending.dwControlsAccepted, 0);
        assert!(pending.dwWaitHint > 0);
        assert_eq!(pending.dwCheckPoint, 1);
        let running = status_record(SERVICE_RUNNING, false, 0);
        assert_eq!(
            running.dwControlsAccepted,
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        );
        let stopped = status_record(SERVICE_STOPPED, true, 0);
        assert_eq!(stopped.dwWin32ExitCode, ERROR_SERVICE_SPECIFIC_ERROR);
        assert_eq!(stopped.dwServiceSpecificExitCode, 1);
        assert_eq!(stopped.dwCheckPoint, 0);
    }

    #[test]
    fn checkpoint_advances_across_pending_reports_and_resets_otherwise() {
        let counter = AtomicU32::new(0);
        assert_eq!(checkpoint_for(&counter, SERVICE_START_PENDING), 1);
        assert_eq!(checkpoint_for(&counter, SERVICE_START_PENDING), 2);
        assert_eq!(checkpoint_for(&counter, SERVICE_STOP_PENDING), 3);
        assert_eq!(checkpoint_for(&counter, SERVICE_RUNNING), 0);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert_eq!(checkpoint_for(&counter, SERVICE_STOPPED), 0);
        assert_eq!(checkpoint_for(&counter, SERVICE_START_PENDING), 1);
    }

    #[test]
    fn sid_validation_rejects_truncated_and_out_of_range_pointers() {
        let sid = [0x0500_0000_0000_0101u64, 18];
        // SAFETY: sid is a live initialized 16-byte array, viewed as native bytes.
        let bytes = unsafe { std::slice::from_raw_parts(sid.as_ptr().cast::<u8>(), 16) };
        let address = bytes.as_ptr() as usize;
        assert!(bounded_sid(bytes, address).is_ok());
        assert!(bounded_sid(&bytes[..8], address).is_err());
        assert!(bounded_sid(bytes, address + 1).is_err());
        assert!(bounded_sid(bytes, address + bytes.len()).is_err());
        assert!(bounded_sid(bytes, address - 1).is_err());
    }

    #[test]
    fn event_text_trims_terminators_replaces_nul_and_caps_units() {
        assert_eq!(
            event_text("a\0b\r\n"),
            [u16::from(b'a'), 0xfffd, u16::from(b'b'), 0]
        );
        let units = event_text(&"x".repeat(MAX_EVENT_UNITS + 10));
        assert_eq!(units.len(), MAX_EVENT_UNITS + 1);
        assert_eq!(units.last(), Some(&0));
        assert!(
            units[..MAX_EVENT_UNITS]
                .iter()
                .all(|&unit| unit == u16::from(b'x'))
        );
    }

    #[test]
    fn levels_map_to_event_types_and_ids() {
        assert_eq!(event_kind(Level::ERROR), (EVENTLOG_ERROR_TYPE, 4));
        assert_eq!(event_kind(Level::WARN), (EVENTLOG_WARNING_TYPE, 3));
        assert_eq!(event_kind(Level::INFO), (EVENTLOG_INFORMATION_TYPE, 2));
        assert_eq!(event_kind(Level::DEBUG), (EVENTLOG_INFORMATION_TYPE, 2));
    }
}
