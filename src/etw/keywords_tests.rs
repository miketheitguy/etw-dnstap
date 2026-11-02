#![expect(
    clippy::shadow_unrelated,
    reason = "each case rebinds the same name for its own input; numbering the cases adds noise"
)]

use super::*;
use crate::etw::test_support::{offset_u32, utf16_nul_bytes};
use anyhow::anyhow;

fn keyword_list(entries: &[(&str, u64)]) -> Vec<(String, u64)> {
    entries
        .iter()
        .map(|&(name, value)| (name.to_owned(), value))
        .collect()
}

/// Asserts the partial narrowing: the mask omits `RESPONSE_FAILURE`.
fn assert_without_failure(choice: &KeywordChoice, expected_mask: u64, expected_reason: &str) {
    match choice {
        KeywordChoice::NarrowedWithoutFailure { match_any, reason } => {
            assert_eq!(*match_any, expected_mask);
            assert!(reason.contains(expected_reason), "reason: {reason}");
        }
        other => panic!("expected narrowing without failure, got {other:?}"),
    }
    assert_eq!(
        choice.selection(),
        KeywordSelection {
            match_any: expected_mask,
            match_all: 0,
            fallback: false,
            failed_responses: false,
        }
    );
}

fn assert_fallback(choice: &KeywordChoice, expected_reason: &str) {
    match choice {
        KeywordChoice::Fallback { reason } => {
            assert!(reason.contains(expected_reason), "reason: {reason}");
        }
        other => panic!("expected fallback, got {other:?}"),
    }
    assert_eq!(
        choice.selection(),
        KeywordSelection {
            match_any: 0x8000_0000_0000_0000,
            match_all: 0,
            fallback: true,
            failed_responses: true,
        }
    );
}

#[test]
fn select_keywords_narrows_three_single_bits() {
    let keywords = keyword_list(&[
        ("RECURSE_QUERY_OUT", 0x10),
        ("RESPONSE_FAILURE", 0x4),
        ("RESPONSE_SUCCESS", 0x2),
        ("QUERY_RECEIVED", 0x1),
        ("HIGH_PROVIDER_BIT", 0x0000_8000_0000_0000),
    ]);
    let choice = select_keywords(Ok(&keywords));
    assert_eq!(choice, KeywordChoice::Narrowed { match_any: 0x7 });
    assert_eq!(
        choice.selection(),
        KeywordSelection {
            match_any: 0x7,
            match_all: 0,
            fallback: false,
            failed_responses: true,
        }
    );

    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x0000_8000_0000_0000),
        ("RESPONSE_SUCCESS", 0x4),
        ("RESPONSE_FAILURE", 0x20),
    ]);
    assert_eq!(
        select_keywords(Ok(&keywords)),
        KeywordChoice::Narrowed {
            match_any: 0x0000_8000_0000_0024
        }
    );
}

#[test]
fn select_keywords_falls_back_on_missing_name() {
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("query_received", 0x2),
        ("RESPONSE_FAILURE", 0x4),
    ]);
    assert_fallback(
        &select_keywords(Ok(&keywords)),
        "RESPONSE_SUCCESS is not defined",
    );
    assert_fallback(&select_keywords(Ok(&[])), "QUERY_RECEIVED is not defined");
    // A missing query or success name falls back. A missing failure name only
    // drops failed responses from the mask.
    let all = [
        ("QUERY_RECEIVED", 0x1),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x4),
    ];
    for missing in 0..all.len() {
        let keywords: Vec<_> = all
            .iter()
            .enumerate()
            .filter(|&(index, _)| index != missing)
            .map(|(_, &entry)| entry)
            .collect();
        let expected = format!("{} is not defined", all[missing].0);
        let choice = select_keywords(Ok(&keyword_list(&keywords)));
        if missing == 2 {
            assert_without_failure(&choice, 0x3, &expected);
        } else {
            assert_fallback(&choice, &expected);
        }
    }
}

#[test]
fn select_keywords_narrows_without_failure_when_only_failure_is_unusable() {
    let base = [("QUERY_RECEIVED", 0x1), ("RESPONSE_SUCCESS", 0x2)];
    let cases: [(&[(&str, u64)], &str); 5] = [
        (&[], "RESPONSE_FAILURE is not defined"),
        (&[("RESPONSE_FAILURE", 0)], "not a single bit"),
        (&[("RESPONSE_FAILURE", 0x6)], "not a single bit"),
        (
            &[("RESPONSE_FAILURE", 0x8000_0000_0000_0000)],
            "provider-defined range",
        ),
        (
            &[("RESPONSE_FAILURE", 0x4), ("RESPONSE_FAILURE", 0x8)],
            "conflicting values",
        ),
    ];
    for (extra, reason) in cases {
        let mut entries = base.to_vec();
        entries.extend_from_slice(extra);
        let choice = select_keywords(Ok(&keyword_list(&entries)));
        assert_without_failure(&choice, 0x3, reason);
    }
}

#[test]
fn select_keywords_falls_back_on_multi_bit_or_zero_value() {
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("RESPONSE_SUCCESS", 0x6),
        ("RESPONSE_FAILURE", 0x8),
    ]);
    assert_fallback(&select_keywords(Ok(&keywords)), "not a single bit");
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x4),
    ]);
    assert_fallback(&select_keywords(Ok(&keywords)), "not a single bit");
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x6),
    ]);
    assert_without_failure(
        &select_keywords(Ok(&keywords)),
        0x3,
        "RESPONSE_FAILURE value 0x6 is not a single bit",
    );
}

#[test]
fn select_keywords_falls_back_on_reserved_range_value() {
    for value in [
        0x8000_0000_0000_0001,
        0x8000_0000_0000_0000,
        0x0001_0000_0000_0000,
    ] {
        let keywords = keyword_list(&[
            ("QUERY_RECEIVED", value),
            ("RESPONSE_SUCCESS", 0x2),
            ("RESPONSE_FAILURE", 0x4),
        ]);
        assert_fallback(&select_keywords(Ok(&keywords)), "provider-defined range");
        let keywords = keyword_list(&[
            ("QUERY_RECEIVED", 0x1),
            ("RESPONSE_SUCCESS", 0x2),
            ("RESPONSE_FAILURE", value),
        ]);
        assert_without_failure(
            &select_keywords(Ok(&keywords)),
            0x3,
            "provider-defined range",
        );
    }
}

#[test]
fn select_keywords_falls_back_on_duplicate_bits_or_names() {
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x2),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x4),
    ]);
    assert_fallback(
        &select_keywords(Ok(&keywords)),
        "QUERY_RECEIVED and RESPONSE_SUCCESS share bit 0x2",
    );
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x1),
    ]);
    assert_fallback(
        &select_keywords(Ok(&keywords)),
        "QUERY_RECEIVED and RESPONSE_FAILURE share bit 0x1",
    );
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("RESPONSE_SUCCESS", 0x4),
        ("RESPONSE_FAILURE", 0x4),
    ]);
    assert_fallback(
        &select_keywords(Ok(&keywords)),
        "RESPONSE_SUCCESS and RESPONSE_FAILURE share bit 0x4",
    );
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("QUERY_RECEIVED", 0x4),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x8),
    ]);
    assert_fallback(&select_keywords(Ok(&keywords)), "conflicting values");
    // The same name repeated with the same value is not ambiguous.
    let keywords = keyword_list(&[
        ("QUERY_RECEIVED", 0x1),
        ("QUERY_RECEIVED", 0x1),
        ("RESPONSE_SUCCESS", 0x2),
        ("RESPONSE_FAILURE", 0x4),
    ]);
    assert_eq!(
        select_keywords(Ok(&keywords)),
        KeywordChoice::Narrowed { match_any: 0x7 }
    );
}

#[test]
fn select_keywords_falls_back_on_lookup_error() {
    let error = anyhow!("TDH keyword size query failed: 1168");
    assert_fallback(&select_keywords(Err(&error)), "size query failed: 1168");
}

#[test]
fn unregistered_provider_keyword_query_falls_back() {
    let unregistered = GUID::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
    let error = query_provider_keywords(&unregistered)
        .expect_err("an unregistered provider has no keyword metadata");
    let choice = select_keywords(Err(&error));
    assert_fallback(&choice, "");
    assert_eq!(choice.selection().match_any, 0x8000_0000_0000_0000);
}

/// Builds a `PROVIDER_FIELD_INFOARRAY` byte image: header, entries, then
/// NUL-terminated UTF-16 names. `name_offset` may override the real offset.
fn keyword_image(entries: &[(&str, u64)], name_offset: Option<u32>) -> Vec<u8> {
    let first = offset_of!(PROVIDER_FIELD_INFOARRAY, FieldInfoArray);
    let entry_size = size_of::<PROVIDER_FIELD_INFO>();
    let mut names = Vec::new();
    let mut offsets = Vec::new();
    for (name, _) in entries {
        offsets.push(offset_u32(first + entries.len() * entry_size + names.len()));
        names.extend_from_slice(&utf16_nul_bytes(name));
    }
    let mut image = vec![0u8; first];
    image[..4].copy_from_slice(&offset_u32(entries.len()).to_le_bytes());
    for ((_, value), offset) in entries.iter().zip(offsets) {
        let mut entry = vec![0u8; entry_size];
        let name_at = offset_of!(PROVIDER_FIELD_INFO, NameOffset);
        entry[name_at..name_at + 4].copy_from_slice(&name_offset.unwrap_or(offset).to_le_bytes());
        let value_at = offset_of!(PROVIDER_FIELD_INFO, Value);
        entry[value_at..value_at + 8].copy_from_slice(&value.to_le_bytes());
        image.extend_from_slice(&entry);
    }
    image.extend_from_slice(&names);
    image
}

#[test]
fn parse_keyword_array_reads_names_and_values() {
    let image = keyword_image(
        &[
            ("QUERY_RECEIVED", 0x1),
            ("RESPONSE_SUCCESS", 0x2),
            ("CHANNEL", 0x8000_0000_0000_0000),
        ],
        None,
    );
    assert_eq!(
        parse_keyword_array(&image).expect("well-formed image"),
        keyword_list(&[
            ("QUERY_RECEIVED", 0x1),
            ("RESPONSE_SUCCESS", 0x2),
            ("CHANNEL", 0x8000_0000_0000_0000),
        ])
    );
    let empty = keyword_image(&[], None);
    assert_eq!(
        parse_keyword_array(&empty).expect("empty image"),
        keyword_list(&[])
    );
}

#[test]
fn parse_keyword_array_rejects_malformed_images() {
    // Name offset past the end of the buffer.
    let image = keyword_image(&[("QUERY_RECEIVED", 0x1)], Some(10_000));
    assert!(parse_keyword_array(&image).is_err());
    // Name offset whose terminator lies past the end.
    let image = keyword_image(&[("QUERY_RECEIVED", 0x1)], None);
    assert!(parse_keyword_array(&image[..image.len() - 2]).is_err());
    // Name offset one unit before the end, so no NUL follows its last unit.
    let last_unit = offset_u32(image.len() - 2);
    let mut unterminated = keyword_image(&[("QUERY_RECEIVED", 0x1)], Some(last_unit));
    let end = unterminated.len();
    unterminated[end - 2] = b'A';
    assert!(parse_keyword_array(&unterminated).is_err());
    // Element count larger than the buffer can hold.
    let mut image = keyword_image(&[("QUERY_RECEIVED", 0x1)], None);
    image[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_keyword_array(&image).is_err());
    // Truncated header.
    assert!(parse_keyword_array(&[1, 0]).is_err());
    assert!(parse_keyword_array(&[]).is_err());
}

#[test]
#[ignore = "requires the DNS Server role"]
fn resolves_real_dns_server_keywords() {
    let keywords = query_provider_keywords(&PROVIDER).expect("resolve DNS Server keywords");
    for (name, value) in &keywords {
        println!("{name} = {value:#x}");
    }
    let choice = select_keywords(Ok(&keywords));
    println!("{choice:?} -> {:?}", choice.selection());
    assert!(matches!(choice, KeywordChoice::Narrowed { .. }));
}
