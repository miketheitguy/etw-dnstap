use super::*;

use crate::etw::test_support::{offset_u32, utf16_nul};

const TEST_GUID: GUID = GUID::from_u128(0x43b7_b2fc_777a_5214_b6ed_665c_1ff9_e972);

/// A query result shaped like this program's own real-time session.
fn own_query(config: &SessionConfig) -> QueryProperties {
    let mut query = QueryProperties::new();
    query.properties.Wnode.Guid = TEST_GUID;
    query.properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    query.properties.BufferSize = config.buffer_kib;
    query.properties.LogFileNameOffset = offset_u32(offset_of!(QueryProperties, log_file_name));
    query
}

fn query_result(edit: impl FnOnce(&mut QueryProperties)) -> QueryProperties {
    let mut query = own_query(&SessionConfig::default());
    edit(&mut query);
    query
}

/// Fingerprint check against the default configuration.
fn reclaimable_by_default(query: &QueryProperties) -> bool {
    reclaimable(query, &TEST_GUID)
}

#[test]
fn query_properties_set_name_offsets_and_size() {
    let query = QueryProperties::new();
    assert_eq!(
        query.properties.Wnode.BufferSize,
        offset_u32(size_of::<QueryProperties>())
    );
    assert_eq!(query.properties.Wnode.Flags, WNODE_FLAG_TRACED_GUID);
    assert_eq!(
        query.properties.LoggerNameOffset,
        offset_u32(offset_of!(QueryProperties, logger_name))
    );
    assert_eq!(
        query.properties.LogFileNameOffset,
        offset_u32(offset_of!(QueryProperties, log_file_name))
    );
    assert!(query.logger_name.iter().all(|unit| *unit == 0));
    assert!(query.log_file_name.iter().all(|unit| *unit == 0));
}

#[test]
fn fingerprint_accepts_own_session_shape() {
    assert!(reclaimable_by_default(
        &own_query(&SessionConfig::default())
    ));
    // Offset 0 is the other shape ETW may report for a session with no log file.
    assert!(reclaimable_by_default(&query_result(|q| {
        q.properties.LogFileNameOffset = 0;
    })));
    // ETW may normalize counts and flush timer.
    assert!(reclaimable_by_default(&query_result(|q| {
        q.properties.MinimumBuffers = 8;
        q.properties.NumberOfBuffers = 8;
        q.properties.FlushTimer = 0;
    })));
}

#[test]
fn fingerprint_rejects_foreign_sessions() {
    // Nonempty log file name at the log-file field offset.
    assert!(!reclaimable_by_default(&query_result(|q| {
        q.log_file_name[0] = u16::from(b'C');
    })));
    // Offsets elsewhere: the logger-name field, the header, misaligned, or
    // beyond the buffer.
    for offset in [
        4,
        offset_of!(QueryProperties, logger_name),
        offset_of!(QueryProperties, log_file_name) + 1,
        size_of::<QueryProperties>(),
    ] {
        assert!(!reclaimable_by_default(&query_result(|q| {
            q.properties.LogFileNameOffset = offset_u32(offset);
        })));
    }
    // Real-time mode is required.
    assert!(!reclaimable_by_default(&query_result(|q| {
        q.properties.LogFileMode = 0;
    })));
    // Each file-mode bit, and each other logger mode, is forbidden.
    for mode in [
        EVENT_TRACE_FILE_MODE_SEQUENTIAL,
        EVENT_TRACE_FILE_MODE_CIRCULAR,
        EVENT_TRACE_FILE_MODE_APPEND,
        EVENT_TRACE_FILE_MODE_NEWFILE,
        EVENT_TRACE_SYSTEM_LOGGER_MODE,
        EVENT_TRACE_BUFFERING_MODE,
        EVENT_TRACE_PRIVATE_LOGGER_MODE,
    ] {
        assert!(!reclaimable_by_default(&query_result(|q| {
            q.properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | mode;
        })));
    }
}

#[test]
fn fingerprint_rejects_a_session_with_another_guid() {
    // Same shape, same name, but not started with this program's GUID.
    assert!(!reclaimable(
        &query_result(|q| q.properties.Wnode.Guid = GUID::from_u128(1)),
        &TEST_GUID
    ));
    assert!(!reclaimable(
        &query_result(|q| q.properties.Wnode.Guid = GUID::default()),
        &TEST_GUID
    ));
}

#[test]
fn reclaim_decision_refuses_foreign_guid_without_stopping() {
    let (result, stopped, count) = run_decision(
        ERROR_SUCCESS,
        |q| q.properties.Wnode.Guid = GUID::from_u128(7),
        ERROR_SUCCESS,
    );
    assert!(result.is_err());
    assert!(!stopped);
    assert_eq!(count, 0);
}

#[test]
fn query_outcome_classifies_statuses() {
    assert_eq!(query_outcome(ERROR_SUCCESS), QueryOutcome::Inspect);
    assert_eq!(
        query_outcome(ERROR_WMI_INSTANCE_NOT_FOUND),
        QueryOutcome::Vanished
    );
    for status in [ERROR_ALREADY_EXISTS, ERROR_CANCELLED, 5, u32::MAX] {
        assert_eq!(query_outcome(status), QueryOutcome::Failed);
    }
}

#[test]
fn session_properties_write_configured_values() {
    let config = SessionConfig::new(128, 512, 5).expect("valid configuration");
    let properties = SessionProperties::new(&config, GUID::default());
    assert_eq!(properties.properties.BufferSize, 128);
    assert_eq!(properties.properties.MaximumBuffers, 512);
    assert_eq!(properties.properties.FlushTimer, 5);
    assert_eq!(properties.properties.MinimumBuffers, 4);

    let defaults = SessionProperties::new(&SessionConfig::default(), GUID::default());
    assert_eq!(defaults.properties.BufferSize, 16);
    assert_eq!(defaults.properties.MaximumBuffers, 12);
    assert_eq!(defaults.properties.FlushTimer, 1);
}

#[test]
fn fingerprint_ignores_buffer_size_so_a_changed_flag_does_not_block_restart() {
    let config = SessionConfig::new(128, 512, 5).expect("valid configuration");
    assert!(reclaimable(&own_query(&config), &TEST_GUID));
    assert!(reclaimable(
        &own_query(&SessionConfig::default()),
        &TEST_GUID
    ));
    // The shape checks still apply regardless of size.
    assert!(!reclaimable(
        &query_result(|q| {
            q.properties.BufferSize = 128;
            q.properties.LogFileMode = 0;
        }),
        &TEST_GUID
    ));
    assert!(!reclaimable(
        &query_result(|q| {
            q.properties.BufferSize = 128;
            q.log_file_name[0] = u16::from(b'C');
        }),
        &TEST_GUID
    ));
}

/// Stops a session by name on drop, so ignored tests clean up even when an
/// assertion fails. Optionally deletes the log file the session wrote.
struct StopSessionOnDrop {
    name: Vec<u16>,
    log_file: Option<std::path::PathBuf>,
}

impl Drop for StopSessionOnDrop {
    fn drop(&mut self) {
        let mut properties = QueryProperties::new();
        // SAFETY: `name` is NUL-terminated and `properties` is a sized, aligned
        // buffer that outlives this synchronous call; a zero handle selects
        // lookup by name.
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE::default(),
                self.name.as_ptr(),
                &raw mut properties.properties,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
        if let Some(path) = &self.log_file {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Start properties with logger-name and log-file-name storage, for tests
/// that create a session with a log file.
#[repr(C)]
struct StartPropertiesWithLogFile {
    properties: EVENT_TRACE_PROPERTIES,
    logger_name: [u16; 1025],
    log_file_name: [u16; 1025],
}

/// Prints ETW's actual QUERY view of a session, so a live
/// `--ignored --nocapture` run records the behavior the fingerprint relies on.
fn print_query(label: &str, name: &[u16]) {
    let mut query = QueryProperties::new();
    // SAFETY: `name` is NUL-terminated and `query` is a sized, aligned buffer
    // that outlives this synchronous call; a zero handle selects lookup by name.
    let status = unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            name.as_ptr(),
            &raw mut query.properties,
            EVENT_TRACE_CONTROL_QUERY,
        )
    };
    eprintln!(
        "{label}: status={status} LogFileMode={:#x} BufferSize={} \
         LogFileNameOffset={} log_file_name[0]={}",
        query.properties.LogFileMode,
        query.properties.BufferSize,
        query.properties.LogFileNameOffset,
        query.log_file_name[0]
    );
}

/// The GUID ETW reports when querying the named session.
fn queried_guid(name: &[u16]) -> GUID {
    let mut query = QueryProperties::new();
    // SAFETY: `name` is NUL-terminated and `query` is a sized, aligned buffer
    // that outlives this synchronous call; a zero handle selects lookup by name.
    let status = unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            name.as_ptr(),
            &raw mut query.properties,
            EVENT_TRACE_CONTROL_QUERY,
        )
    };
    assert!(
        matches!(status, ERROR_SUCCESS | ERROR_MORE_DATA),
        "query status {status}"
    );
    query.properties.Wnode.Guid
}

/// Starts a session with a log file and the given mode under `guid`, returning
/// the status.
fn start_log_file_session(
    name: &[u16],
    guid: GUID,
    config: &SessionConfig,
    mode: u32,
    log_units: &[u16],
) -> u32 {
    let mut properties = StartPropertiesWithLogFile {
        properties: EVENT_TRACE_PROPERTIES::default(),
        logger_name: [0; 1025],
        log_file_name: [0; 1025],
    };
    properties.properties.Wnode.BufferSize = offset_u32(size_of::<StartPropertiesWithLogFile>());
    properties.properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    properties.properties.Wnode.Guid = guid;
    properties.properties.BufferSize = config.buffer_kib;
    properties.properties.LogFileMode = mode;
    properties.properties.LoggerNameOffset =
        offset_u32(offset_of!(StartPropertiesWithLogFile, logger_name));
    properties.properties.LogFileNameOffset =
        offset_u32(offset_of!(StartPropertiesWithLogFile, log_file_name));
    properties.log_file_name[..log_units.len()].copy_from_slice(log_units);
    let mut session = CONTROLTRACE_HANDLE::default();
    // SAFETY: `name` is NUL-terminated, and `properties` is a local, sized,
    // aligned buffer whose name storage holds the logger and log-file paths.
    // It outlives this synchronous call.
    unsafe {
        StartTraceW(
            &raw mut session,
            name.as_ptr(),
            &raw mut properties.properties,
        )
    }
}

/// Leaks a session that writes a log file and asserts reclaim refuses it.
fn assert_log_file_session_refused(name_text: &str, mode: u32) {
    let name = utf16_nul(name_text);
    let config = SessionConfig::default();
    let log_path = std::env::temp_dir().join(format!("{name_text}.etl"));
    let log_units = utf16_nul(&log_path.to_string_lossy());
    assert!(log_units.len() <= 1025, "temp log path fits the buffer");
    let _cleanup = StopSessionOnDrop {
        name: name.clone(),
        log_file: Some(log_path),
    };

    let guid = session_guid(name_text).expect("guid");
    let status = start_log_file_session(&name, guid, &config, mode, &log_units);
    assert_eq!(status, ERROR_SUCCESS, "log-file session should start");
    print_query("log-file session", &name);
    // reclaimable() also compares the GUID. Without this check a GUID mismatch
    // would make the refusal below pass without reaching the log-file checks.
    let reported = queried_guid(&name);
    assert!(
        same_guid(&reported, &guid),
        "ETW must report the GUID the session was started with"
    );

    let counters = CaptureCounters::default();
    let error = reclaim_stale_session(
        &name,
        name_text,
        &counters,
        &config,
        &session_guid(name_text).expect("guid"),
    )
    .expect_err("a session that writes a log file must not be reclaimed");
    assert!(
        error
            .to_string()
            .contains("does not match this program's session shape"),
        "unexpected error: {error}"
    );
    assert_eq!(counters.stale_sessions_reclaimed.load(Ordering::Relaxed), 0);
}

#[test]
#[ignore = "requires an elevated session"]
fn reclaim_replaces_leaked_real_time_session() {
    const NAME: &str = "EtwDnstap-ReclaimTest-Replace";
    let name = utf16_nul(NAME);
    // A non-default size exercises exact BufferSize matching beyond 64 KiB.
    let config = SessionConfig::new(128, 256, 1).expect("valid configuration");
    let _cleanup = StopSessionOnDrop {
        name: name.clone(),
        log_file: None,
    };
    // Leak a real-time-only session with this program's shape.
    let (status, _leaked) = start_session(&name, session_guid(NAME).expect("guid"), &config);
    assert_eq!(status, ERROR_SUCCESS, "leaking a session should start");
    print_query("leaked session", &name);

    let (status, _duplicate) = start_session(&name, session_guid(NAME).expect("guid"), &config);
    assert_eq!(
        status, ERROR_ALREADY_EXISTS,
        "the leaked session must block a second start"
    );

    let counters = CaptureCounters::default();
    reclaim_stale_session(
        &name,
        NAME,
        &counters,
        &config,
        &session_guid(NAME).expect("guid"),
    )
    .expect("reclaim succeeds");
    let (status, _session) = start_session(&name, session_guid(NAME).expect("guid"), &config);
    assert_eq!(status, ERROR_SUCCESS, "retry after reclaim should start");
    print_query("restarted session", &name);
    assert_eq!(counters.stale_sessions_reclaimed.load(Ordering::Relaxed), 1);
}

/// Real-time mode only plus a log-file name: the case where ETW writes a
/// sequential file without any file-mode bit.
#[test]
#[ignore = "requires an elevated session"]
fn reclaim_refuses_session_that_writes_log_file() {
    assert_log_file_session_refused("EtwDnstap-ReclaimTest-LogFile", EVENT_TRACE_REAL_TIME_MODE);
}

#[test]
#[ignore = "requires an elevated session"]
fn reclaim_refuses_sequential_log_file_session() {
    assert_log_file_session_refused(
        "EtwDnstap-ReclaimTest-Sequential",
        EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_FILE_MODE_SEQUENTIAL,
    );
}

/// Runs the decision with canned statuses, recording whether stop ran.
fn run_decision(
    query_status: u32,
    query_edit: impl FnOnce(&mut QueryProperties),
    stop_status: u32,
) -> (Result<()>, bool, u64) {
    let counters = CaptureCounters::default();
    let config = SessionConfig::default();
    let mut stopped = false;
    let result = reclaim_decision(
        "test",
        &counters,
        &config,
        &TEST_GUID,
        |properties| {
            *properties = own_query(&config);
            query_edit(properties);
            query_status
        },
        |_| {
            stopped = true;
            stop_status
        },
    );
    let count = counters.stale_sessions_reclaimed.load(Ordering::Relaxed);
    (result, stopped, count)
}

#[test]
fn reclaim_decision_vanished_session_neither_stops_nor_counts() {
    let (result, stopped, count) =
        run_decision(ERROR_WMI_INSTANCE_NOT_FOUND, |_| {}, ERROR_SUCCESS);
    assert!(result.is_ok());
    assert!(!stopped);
    assert_eq!(count, 0);
}

#[test]
fn reclaim_decision_failed_query_errors_without_stop() {
    let (result, stopped, count) = run_decision(5, |_| {}, ERROR_SUCCESS);
    let error = result.expect_err("query failure is an error");
    assert!(error.to_string().contains("query existing ETW session"));
    assert!(!stopped);
    assert_eq!(count, 0);
}

#[test]
fn reclaim_decision_shape_mismatch_errors_without_stop() {
    let (result, stopped, count) = run_decision(
        ERROR_SUCCESS,
        |q| q.log_file_name[0] = u16::from(b'C'),
        ERROR_SUCCESS,
    );
    let error = result.expect_err("mismatch is an error");
    assert!(
        error
            .to_string()
            .contains("does not match this program's session shape")
    );
    assert!(!stopped);
    assert_eq!(count, 0);
}

#[test]
fn reclaim_decision_counts_stop_success_and_more_data() {
    for stop_status in [ERROR_SUCCESS, ERROR_MORE_DATA] {
        let (result, stopped, count) = run_decision(ERROR_SUCCESS, |_| {}, stop_status);
        assert!(result.is_ok());
        assert!(stopped);
        assert_eq!(count, 1, "stop status {stop_status}");
    }
}

#[test]
fn reclaim_decision_does_not_count_session_that_vanished_before_stop() {
    let (result, stopped, count) =
        run_decision(ERROR_SUCCESS, |_| {}, ERROR_WMI_INSTANCE_NOT_FOUND);
    assert!(result.is_ok());
    assert!(stopped);
    assert_eq!(count, 0);
}

#[test]
fn reclaim_decision_stop_failure_errors_without_count() {
    let (result, stopped, count) = run_decision(ERROR_SUCCESS, |_| {}, 5);
    let error = result.expect_err("stop failure is an error");
    assert!(error.to_string().contains("stopping stale ETW session"));
    assert!(stopped);
    assert_eq!(count, 0);
}

#[test]
fn session_config_enforces_ranges_and_accepts_max_buffer_product() {
    assert!(SessionConfig::new(4, 4, 1).is_ok());
    assert!(SessionConfig::new(128, 4096, 1).is_ok(), "512 MiB peak");
    assert!(SessionConfig::new(128, 256, 1).is_ok());
    for rejected in [3, 132, 256, 1024] {
        assert!(SessionConfig::new(rejected, 256, 1).is_err(), "{rejected}");
    }
    assert!(SessionConfig::new(64, 3, 1).is_err());
    assert!(SessionConfig::new(64, 4097, 1).is_err());
    assert!(SessionConfig::new(64, 256, 0).is_err());
    assert!(SessionConfig::new(64, 256, 61).is_err());
    assert!(
        SessionConfig::new(65, 256, 1).is_err(),
        "not a multiple of 4"
    );
    assert!(
        SessionConfig::new(66, 256, 1).is_err(),
        "not a multiple of 4"
    );
    assert!(SessionConfig::new(68, 256, 1).is_ok());
}

#[test]
fn stop_outcome_classifies_statuses() {
    for status in [ERROR_SUCCESS, ERROR_MORE_DATA, ERROR_WMI_INSTANCE_NOT_FOUND] {
        assert_eq!(stop_outcome(status), StopOutcome::Stopped);
    }
    for status in [ERROR_ALREADY_EXISTS, ERROR_CANCELLED, 5, u32::MAX] {
        assert_eq!(stop_outcome(status), StopOutcome::Failed);
    }
}

#[test]
fn event_id_filter_payload_bytes_are_exact() {
    let payload = EventIdFilterPayload::new();
    // SAFETY: The payload is an 8-byte repr(C) struct without padding, so
    // every byte is initialized.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(&payload).cast::<u8>(),
            size_of::<EventIdFilterPayload>(),
        )
    };
    assert_eq!(bytes, [1, 0, 2, 0, 0x00, 0x01, 0x01, 0x01]);
}

#[test]
fn event_id_descriptor_and_parameters_describe_payload() {
    let payload = EventIdFilterPayload::new();
    let mut descriptor = event_id_descriptor(&payload);
    assert_eq!(descriptor.Type, EVENT_FILTER_TYPE_EVENT_ID);
    assert_eq!(descriptor.Size, 8);
    assert_eq!(descriptor.Ptr, std::ptr::from_ref(&payload) as u64);

    let descriptor_ptr = &raw mut descriptor;
    let parameters = enable_parameters(descriptor_ptr, 1);
    assert_eq!(parameters.Version, ENABLE_TRACE_PARAMETERS_VERSION_2);
    assert_eq!(parameters.EnableProperty, 0x10);
    assert_eq!(
        parameters.EnableProperty,
        EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0
    );
    assert_eq!(parameters.FilterDescCount, 1);
    assert_eq!(parameters.EnableFilterDesc, descriptor_ptr);
}

#[test]
fn pid_descriptor_describes_a_single_pid() {
    let payload = PidFilterPayload::new(0x1234);
    let descriptor = pid_descriptor(&payload);
    assert_eq!(descriptor.Type, EVENT_FILTER_TYPE_PID);
    assert_eq!(descriptor.Type, 0x8000_0004);
    assert_eq!(descriptor.Size, 4);
    assert_eq!(descriptor.Ptr, std::ptr::from_ref(&payload) as u64);
    assert_eq!(payload.pids, [0x1234]);
}

#[test]
fn both_filters_are_passed_to_enable_parameters() {
    let ids = EventIdFilterPayload::new();
    let pid = PidFilterPayload::new(7);
    let mut descriptors = [event_id_descriptor(&ids), pid_descriptor(&pid)];
    let parameters = enable_parameters(descriptors.as_mut_ptr(), 2);
    assert_eq!(parameters.FilterDescCount, 2);
    assert_eq!(parameters.EnableFilterDesc, descriptors.as_mut_ptr());
}

#[test]
fn target_change_classification() {
    assert_eq!(classify_target(10, Some(10)), None);
    assert_eq!(classify_target(10, None), None);
    assert!(
        classify_target(10, Some(0))
            .expect("stopped")
            .contains("stopped")
    );
    assert!(
        classify_target(10, Some(11))
            .expect("restarted")
            .contains("10 -> 11")
    );
}

#[test]
fn invalid_handle_is_benign_only_after_a_successful_stop() {
    assert!(close_trace_benign(ERROR_INVALID_HANDLE, true));
    assert!(!close_trace_benign(ERROR_INVALID_HANDLE, false));
    assert!(close_trace_benign(ERROR_CTX_CLOSE_PENDING, false));
    assert!(!close_trace_benign(5, true));
    assert!(process_trace_benign(ERROR_INVALID_HANDLE, true));
    assert!(!process_trace_benign(ERROR_INVALID_HANDLE, false));
    assert!(process_trace_benign(ERROR_CANCELLED, false));
    assert!(!process_trace_benign(5, true));
}

#[test]
fn wait_until_is_bounded_and_returns_early() {
    let ms = Duration::from_millis;
    let started = Instant::now();
    assert!(wait_until(|| true, Duration::from_secs(5), ms(1)));
    assert!(started.elapsed() < Duration::from_secs(1));
    let mut polls = 0;
    assert!(wait_until(
        || {
            polls += 1;
            polls >= 3
        },
        Duration::from_secs(5),
        ms(1)
    ));
    let started = Instant::now();
    assert!(!wait_until(|| false, ms(30), ms(5)));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn access_denied_enable_names_the_missing_right() {
    let message = check_enable_status(ERROR_ACCESS_DENIED)
        .expect_err("denied")
        .to_string();
    assert!(message.contains("TRACELOG_GUID_ENABLE"));
    assert!(message.contains("eb79061a-a566-4698-9119-3ed2807060e7"));
    // The grant script removes service-SID ACEs from the provider descriptor,
    // so the message must not tell the operator to add one there.
    assert!(message.contains("Do not grant on that GUID"));
    assert!(check_enable_status(ERROR_SUCCESS).is_ok());
}

#[test]
fn access_denied_start_names_create_realtime_on_the_session_guid() {
    let guid = format_guid(&session_guid("EtwDnstap").expect("guid"));
    let message = check_start_status(ERROR_ACCESS_DENIED, "EtwDnstap", &guid)
        .expect_err("denied")
        .to_string();
    assert!(message.contains("TRACELOG_CREATE_REALTIME (0x20)"));
    assert!(
        message.contains("{05e1db3c-1322-58c7-8235-0463fccba32b}"),
        "{message}"
    );
    assert!(message.contains("-SessionName 'EtwDnstap'"));
    assert!(message.contains("restart the computer"));
    assert!(!message.contains("0811c1af"));
    assert!(check_start_status(ERROR_SUCCESS, "EtwDnstap", &guid).is_ok());
}

#[test]
fn session_properties_carry_the_session_guid() {
    let guid = session_guid("EtwDnstap").expect("guid");
    let properties = SessionProperties::new(&SessionConfig::default(), guid);
    assert_eq!(
        format_guid(&properties.properties.Wnode.Guid),
        "05e1db3c-1322-58c7-8235-0463fccba32b"
    );
}
