//! Command-line boundaries: receiver syntax, repeated flags, and cross-field limits.

use super::*;
use clap::error::ErrorKind;

fn parse(extra: &[&str]) -> Result<CaptureArgs, clap::Error> {
    let mut arguments = vec!["etw-dnstap", "--receiver", "127.0.0.1:6000"];
    arguments.extend_from_slice(extra);
    CaptureArgs::try_parse_from(arguments)
}

fn receiver(value: &str) -> Result<CaptureArgs, clap::Error> {
    CaptureArgs::try_parse_from(["etw-dnstap", "--receiver", value])
}

#[test]
fn ipv6_receivers_parse_with_brackets_and_every_port_edge() {
    for (text, expected) in [
        ("[::1]:6000", "[::1]:6000"),
        ("[2001:db8::1]:1", "[2001:db8::1]:1"),
        ("[2001:db8::1]:65535", "[2001:db8::1]:65535"),
        ("[::ffff:192.0.2.1]:53", "[::ffff:192.0.2.1]:53"),
        ("[fe80::1]:6000", "[fe80::1]:6000"),
        ("192.0.2.1:65535", "192.0.2.1:65535"),
        ("192.0.2.1:1", "192.0.2.1:1"),
    ] {
        let args = receiver(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        assert_eq!(args.receiver.to_string(), expected);
        assert!(PipelineConfig::try_from(args).is_ok(), "{text}");
    }
}

#[test]
fn malformed_receivers_are_rejected_without_panicking() {
    for text in [
        "",
        "::1",
        "::1:6000",
        "[::1]",
        "[::1]:",
        "[::1]:0",
        "[::1]:65536",
        "[::1]:-1",
        "[::1]:6000:1",
        "[::1:6000",
        "::1]:6000",
        "[::]:6000",
        "[ff02::1]:6000",
        "[ff00::]:1",
        "0.0.0.0:1",
        "224.0.0.1:1",
        "239.255.255.255:1",
        "127.0.0.1",
        "127.0.0.1:",
        "127.0.0.1:0",
        "127.0.0.1:65536",
        "127.0.0.1:99999999999999999999",
        "localhost:6000",
        "collector.example:6000",
        " 127.0.0.1:6000",
        "127.0.0.1:6000 ",
        "127.0.0.1:6000\n",
        "１２７.0.0.1:6000",
        "127.0.0.1:+6000",
    ] {
        assert!(receiver(text).is_err(), "{text:?} must be rejected");
    }
}

#[test]
fn port_zero_is_rejected_for_both_families_and_by_validate() {
    for text in ["127.0.0.1:0", "[::1]:0"] {
        let error = receiver(text).expect_err("port 0 is not a destination");
        assert_eq!(error.kind(), ErrorKind::ValueValidation, "{text}");
    }
    let mut config = PipelineConfig::try_from(parse(&[]).expect("valid")).expect("config");
    config.receiver = "[::1]:0".parse().expect("socket address");
    assert!(config.validate().is_err());
    config.receiver = "[::]:6000".parse().expect("socket address");
    assert!(config.validate().is_err());
    config.receiver = "[::1]:6000".parse().expect("socket address");
    assert!(config.validate().is_ok());
}

#[test]
fn repeated_and_conflicting_flags_are_errors_not_last_wins() {
    for pair in [
        ["--receiver", "127.0.0.1:6001"],
        ["--capacity", "2048"],
        ["--session-name", "Other"],
        ["--identity", "other"],
        ["--etw-flush-ms", "50"],
        ["--duration", "5"],
    ] {
        // Supply each flag twice (the base command line already has --receiver).
        let mut extra = pair.to_vec();
        if pair[0] != "--receiver" {
            extra.extend_from_slice(&pair);
        }
        let error = parse(&extra).expect_err("repeated flag");
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict, "{pair:?}");
    }
}

#[test]
fn flag_value_forms_and_missing_values() {
    assert_eq!(parse(&["--capacity=2048"]).expect("= form").capacity, 2048);
    assert_eq!(
        parse(&["--capacity", "2048"]).expect("space form").capacity,
        2048
    );
    for bad in [
        &["--capacity"][..],
        &["--capacity", ""],
        &["--capacity", "2048.5"],
        &["--capacity", "0x800"],
        &["--capacity", "2048 "],
        &["--duration", "604801"],
        &["--stats-interval-seconds", "86401"],
        &["--stats-interval-seconds", "-1"],
        &["--identity"],
        &["--receiver"],
        &["extra-positional"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        parse(&["--duration", "604800"]).expect("max").duration,
        Some(604_800)
    );
    assert_eq!(parse(&["--duration", "1"]).expect("min").duration, Some(1));
}

#[test]
fn etw_buffer_kib_must_be_a_multiple_of_four_at_validation() {
    // The flag range alone admits 5, so the multiple-of-four rule is enforced
    // when the session settings are built.
    for kib in [5_u32, 6, 7, 127] {
        let args = parse(&["--etw-buffer-kib", &kib.to_string()]).expect("within range");
        assert!(
            PipelineConfig::try_from(args).is_err(),
            "{kib} KiB is not a multiple of 4"
        );
    }
    for kib in [4_u32, 8, 128] {
        let args = parse(&["--etw-buffer-kib", &kib.to_string()]).expect("within range");
        assert!(PipelineConfig::try_from(args).is_ok(), "{kib} KiB");
    }
}

#[test]
fn identity_and_session_name_byte_and_unit_limits() {
    assert!(parse(&["--identity", &"a".repeat(1024)]).is_ok());
    assert!(parse(&["--identity", &"a".repeat(1025)]).is_err());
    // The limit counts UTF-8 bytes: 512 two-byte characters fit, 513 do not.
    assert!(parse(&["--identity", &"é".repeat(512)]).is_ok());
    assert!(parse(&["--identity", &"é".repeat(513)]).is_err());
    assert!(parse(&["--identity", ""]).is_ok());
    // Session names count UTF-16 units; a surrogate pair is two.
    assert!(parse(&["--session-name", &"a".repeat(200)]).is_ok());
    assert!(parse(&["--session-name", &"a".repeat(201)]).is_err());
    assert!(parse(&["--session-name", &"😀".repeat(100)]).is_ok());
    assert!(parse(&["--session-name", &"😀".repeat(101)]).is_err());
}

#[test]
fn help_and_version_win_when_they_come_first() {
    for flag in ["--help", "-h", "--version", "-V"] {
        let error = CaptureArgs::try_parse_from(["etw-dnstap", flag]).expect_err("informational");
        assert!(matches!(
            error.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        ));
        // Arguments are processed in order: an informational flag that comes
        // first is reported even when a later receiver is invalid.
        let error = CaptureArgs::try_parse_from(["etw-dnstap", flag, "--receiver", "bad"])
            .expect_err("informational");
        assert!(matches!(
            error.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        ));
    }
}

#[test]
fn jumbo_capacity_bound_comes_from_the_pool_and_validate_repeats_it() {
    let max = crate::etw::MAX_JUMBO_CAPACITY;
    let over = (u32::from(max) + 1).to_string();
    assert!(parse(&["--jumbo-capacity", &max.to_string()]).is_ok());
    assert!(parse(&["--jumbo-capacity", "0"]).is_ok());
    assert!(parse(&["--jumbo-capacity", &over]).is_err());
    let mut config = PipelineConfig::try_from(parse(&[]).expect("valid")).expect("config");
    config.jumbo_capacity = max;
    assert!(config.validate().is_ok());
    config.jumbo_capacity = max + 1;
    assert!(config.validate().is_err());
}
