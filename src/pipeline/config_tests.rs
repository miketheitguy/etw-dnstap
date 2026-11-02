//! Argument parsing, defaults, and validation.

use super::*;
use crate::pipeline::test_support::config_from;

#[test]
fn argument_parsing_returns_errors_instead_of_exiting() {
    let invalid = parse_args_from(["etw-dnstap", "--receiver", "not-an-address"]);
    assert!(invalid.is_err());
    let missing = parse_args_from(["etw-dnstap"]);
    assert_eq!(
        missing.err().map(|error| error.kind()),
        Some(clap::error::ErrorKind::MissingRequiredArgument)
    );
    let help = parse_args_from(["etw-dnstap", "--help"]);
    assert_eq!(
        help.err().map(|error| error.kind()),
        Some(clap::error::ErrorKind::DisplayHelp)
    );
    let valid = parse_args_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"]);
    assert!(valid.is_ok());
}

#[test]
fn pipeline_config_validation_matches_the_command_line() -> Result<()> {
    let valid = PipelineConfig::try_from(parse_args_from([
        "etw-dnstap",
        "--receiver",
        "127.0.0.1:6000",
    ])?)?;
    assert_eq!(valid.capacity, 24_576);
    let mut bad = valid.clone();
    bad.capacity = 1;
    assert!(bad.validate().is_err());
    let mut bad = valid;
    bad.receiver = "0.0.0.0:6000".parse()?;
    assert!(bad.validate().is_err());
    Ok(())
}

#[test]
fn validates_memory_and_cli_bounds() {
    for rejected in ["0", "-1", "1023", "262145"] {
        assert!(parse_capacity(rejected).is_err(), "{rejected}");
    }
    assert_eq!(parse_capacity("262144"), Ok(262_144));
    assert_eq!(parse_capacity("65536"), Ok(65_536));
    assert_eq!(parse_capacity("1024"), Ok(1024));
    assert_eq!(parse_capacity("8192"), Ok(8192));
    assert_eq!(
        CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"])
            .expect("receiver arguments are valid")
            .capacity,
        24_576
    );
    for capacity in ["0", "256"] {
        assert!(
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:6000",
                "--jumbo-capacity",
                capacity
            ])
            .is_ok()
        );
    }
    assert!(
        CaptureArgs::try_parse_from([
            "etw-dnstap",
            "--receiver",
            "127.0.0.1:6000",
            "--jumbo-capacity",
            "257"
        ])
        .is_err()
    );
    assert_eq!(
        CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", "127.0.0.1:6000"])
            .expect("default pool arguments")
            .jumbo_capacity,
        64
    );
    assert!(parse_session_name("").is_err());
    assert!(parse_session_name("bad\0name").is_err());
    assert!(parse_identity(&"a".repeat(1025)).is_err());
    assert!(
        CaptureArgs::try_parse_from([
            "etw-dnstap",
            "--receiver",
            "127.0.0.1:6000",
            "--duration",
            "0"
        ])
        .is_err()
    );
}

#[test]
fn etw_session_flags_default_and_enforce_ranges() {
    let base = ["etw-dnstap", "--receiver", "127.0.0.1:6000"];
    let parse = |extra: &[&str]| {
        let mut arguments = base.to_vec();
        arguments.extend_from_slice(extra);
        CaptureArgs::try_parse_from(arguments)
    };

    let defaults = parse(&[]).expect("default ETW arguments");
    assert_eq!(defaults.etw_buffer_kib, 16);
    assert_eq!(defaults.etw_min_buffers, 0);
    assert_eq!(defaults.etw_max_buffers, 64);
    assert_eq!(defaults.etw_flush_seconds, 1);

    for (flag, values) in [
        ("--etw-buffer-kib", &["0", "3", "132", "256", "1024"][..]),
        ("--etw-min-buffers", &["4097", "4294967296"][..]),
        ("--etw-max-buffers", &["0", "3", "4097"][..]),
        ("--etw-flush-seconds", &["0", "61", "4294967296"][..]),
    ] {
        for value in values {
            assert!(parse(&[flag, value]).is_err(), "{flag} {value}");
        }
    }
    for (flag, value) in [
        ("--etw-buffer-kib", "4"),
        ("--etw-buffer-kib", "128"),
        ("--etw-min-buffers", "0"),
        ("--etw-min-buffers", "4"),
        ("--etw-min-buffers", "4096"),
        ("--etw-max-buffers", "4"),
        ("--etw-max-buffers", "4096"),
        ("--etw-flush-seconds", "1"),
        ("--etw-flush-seconds", "60"),
    ] {
        assert!(parse(&[flag, value]).is_ok(), "{flag} {value}");
    }
}

#[test]
fn etw_min_buffers_must_not_exceed_etw_max_buffers() {
    let config = config_from(&["--etw-min-buffers", "32", "--etw-max-buffers", "64"]);
    assert_eq!(config.session.minimum_buffers, 32);
    assert_eq!(config.session.maximum_buffers, 64);
    config.validate().expect("minimum below maximum");

    let equal = config_from(&["--etw-min-buffers", "64", "--etw-max-buffers", "64"]);
    assert_eq!(equal.session.minimum_buffers, 64);

    // The range check passes in clap, so the failure comes from validation.
    let args = parse_args_from([
        "etw-dnstap",
        "--receiver",
        "127.0.0.1:6000",
        "--etw-min-buffers",
        "65",
        "--etw-max-buffers",
        "64",
    ])
    .expect("each value is in range");
    let error = PipelineConfig::try_from(args).expect_err("minimum above maximum");
    assert_eq!(
        error.to_string(),
        "ETW minimum buffers (65) must not exceed ETW maximum buffers (64)"
    );

    // The default maximum of 64 applies when only the minimum is given.
    let args = parse_args_from([
        "etw-dnstap",
        "--receiver",
        "127.0.0.1:6000",
        "--etw-min-buffers",
        "65",
    ])
    .expect("in range");
    assert!(PipelineConfig::try_from(args).is_err(), "65 > default 64");
}

#[test]
fn session_name_defaults_and_can_be_overridden() {
    let base = ["etw-dnstap", "--receiver", "127.0.0.1:6000"];
    let args = CaptureArgs::try_parse_from(base).expect("default arguments");
    assert_eq!(args.session_name, DEFAULT_SESSION_NAME);
    assert_eq!(args.session_name, "EtwDnstap");
    let args = CaptureArgs::try_parse_from([base[0], base[1], base[2], "--session-name", "Mine"])
        .expect("explicit session name");
    assert_eq!(args.session_name, "Mine");
}

#[test]
fn requires_a_numeric_receiver_and_rejects_removed_options() {
    assert_eq!(
        CaptureArgs::try_parse_from(["etw-dnstap"])
            .expect_err("receiver is required")
            .kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
    for option in [
        "--output",
        "-o",
        "--max-output-bytes",
        "--metrics",
        "--foreground",
    ] {
        assert_eq!(
            CaptureArgs::try_parse_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:6000",
                option,
                "anything"
            ])
            .expect_err("unsupported options must fail")
            .kind(),
            clap::error::ErrorKind::UnknownArgument
        );
    }
    for receiver in ["127.0.0.1:6000", "[::1]:6000"] {
        let args = CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", receiver])
            .expect("numeric unicast receiver is valid");
        assert_eq!(args.receiver.to_string(), receiver);
    }
    for invalid in [
        "collector:6000",
        "127.0.0.1:0",
        "0.0.0.0:6000",
        "224.0.0.1:6000",
        "[::]:6000",
        "[ff02::1]:6000",
    ] {
        assert!(CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", invalid]).is_err());
    }
}

#[cfg(feature = "diag-timing")]
#[test]
fn diag_flags_parse_and_default_to_unset() {
    let parse = |extra: &[&str]| {
        let mut arguments = vec!["etw-dnstap", "--receiver", "127.0.0.1:6000"];
        arguments.extend_from_slice(extra);
        CaptureArgs::try_parse_from(arguments)
    };
    let absent = parse(&[]).expect("defaults parse");
    assert_eq!(absent.diag_timer_resolution_ms, None);
    assert_eq!(absent.diag_thread_priority, None);
    for valid in ["1", "15"] {
        let args = parse(&["--diag-timer-resolution-ms", valid]).expect("in range");
        assert_eq!(args.diag_timer_resolution_ms, valid.parse().ok());
    }
    for invalid in ["0", "16", "-1", "x"] {
        assert!(parse(&["--diag-timer-resolution-ms", invalid]).is_err());
    }
    assert_eq!(
        parse(&["--diag-thread-priority", "above-normal"])
            .expect("above-normal")
            .diag_thread_priority,
        Some(crate::etw::ThreadPriority::AboveNormal)
    );
    assert_eq!(
        parse(&["--diag-thread-priority", "highest"])
            .expect("highest")
            .diag_thread_priority,
        Some(crate::etw::ThreadPriority::Highest)
    );
    assert!(parse(&["--diag-thread-priority", "realtime"]).is_err());
}

#[test]
fn stats_interval_flag_defaults_to_60_and_zero_disables() {
    assert_eq!(
        config_from(&[]).stats_interval(),
        Some(Duration::from_secs(60))
    );
    assert_eq!(
        config_from(&["--stats-interval-seconds", "5"]).stats_interval(),
        Some(Duration::from_secs(5))
    );
    assert_eq!(
        config_from(&["--stats-interval-seconds=0"]).stats_interval(),
        None
    );
    for bad in ["-1", "86401", "x", ""] {
        assert!(
            parse_args_from([
                "etw-dnstap",
                "--receiver",
                "127.0.0.1:6000",
                &format!("--stats-interval-seconds={bad}"),
            ])
            .is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn default_etw_settings_are_detected() {
    assert!(config_from(&[]).uses_default_etw_settings());
    assert!(!config_from(&["--etw-min-buffers", "8"]).uses_default_etw_settings());
    assert!(!config_from(&["--etw-max-buffers", "256"]).uses_default_etw_settings());
    assert!(!config_from(&["--etw-buffer-kib", "64"]).uses_default_etw_settings());
    assert!(!config_from(&["--etw-flush-seconds", "2"]).uses_default_etw_settings());
}

#[test]
fn default_capacity_matches_the_command_line_default() {
    assert_eq!(config_from(&[]).capacity, DEFAULT_CAPACITY);
}
