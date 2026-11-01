//! Provider keyword metadata and the subscription masks derived from it.

use std::{
    mem::{offset_of, size_of},
    ptr::null_mut,
};

use anyhow::{Context, Result, ensure};
use windows_sys::{
    Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        System::Diagnostics::Etw::{
            EventKeywordInformation, PROVIDER_FIELD_INFO, PROVIDER_FIELD_INFOARRAY,
            TdhEnumerateProviderFieldInformation,
        },
    },
    core::GUID,
};

use super::{MAX_METADATA, PROVIDER, check_status};

#[cfg(test)]
#[path = "keywords_tests.rs"]
mod tests;

/// Fallback `MatchAnyKeyword`: the Analytical channel bit, used when the
/// provider's keyword metadata cannot be resolved.
const SUBSCRIPTION_MATCH_ANY_KEYWORD: u64 = 0x8000_0000_0000_0000;
/// `MatchAllKeyword` in both the narrowed and fallback subscriptions. The
/// channel bit is not required, because only one build's channel bits are known.
const SUBSCRIPTION_MATCH_ALL_KEYWORD: u64 = 0;
/// Provider-defined keyword bits; bits 48-63 are reserved for channels and
/// Microsoft.
const PROVIDER_KEYWORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
/// Manifest keyword names whose bits select events 256 and 257.
const KEYWORD_QUERY_RECEIVED: &str = "QUERY_RECEIVED";
const KEYWORD_RESPONSE_SUCCESS: &str = "RESPONSE_SUCCESS";

/// The `MatchAnyKeyword` and `MatchAllKeyword` masks a session enables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeywordSelection {
    pub(crate) match_any: u64,
    pub(crate) match_all: u64,
    /// True when keyword metadata could not be resolved and the full channel
    /// mask is in use.
    pub(crate) fallback: bool,
}

/// The outcome of resolving the provider's keywords by name.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum KeywordChoice {
    Narrowed { match_any: u64 },
    Fallback { reason: String },
}

impl KeywordChoice {
    pub(super) fn selection(&self) -> KeywordSelection {
        match self {
            Self::Narrowed { match_any } => KeywordSelection {
                match_any: *match_any,
                match_all: SUBSCRIPTION_MATCH_ALL_KEYWORD,
                fallback: false,
            },
            Self::Fallback { .. } => KeywordSelection {
                match_any: SUBSCRIPTION_MATCH_ANY_KEYWORD,
                match_all: SUBSCRIPTION_MATCH_ALL_KEYWORD,
                fallback: true,
            },
        }
    }
}

/// Looks up one keyword by exact name and requires a single provider-defined bit.
fn keyword_bit(keywords: &[(String, u64)], name: &str) -> Result<u64, String> {
    let mut matches = keywords
        .iter()
        .filter(|(candidate, _)| candidate == name)
        .map(|&(_, value)| value);
    let value = matches
        .next()
        .ok_or_else(|| format!("keyword {name} is not defined"))?;
    if matches.any(|other| other != value) {
        return Err(format!("keyword {name} is defined with conflicting values"));
    }
    if value & !PROVIDER_KEYWORD_MASK != 0 {
        return Err(format!(
            "keyword {name} value {value:#x} is outside the provider-defined range"
        ));
    }
    if !value.is_power_of_two() {
        return Err(format!(
            "keyword {name} value {value:#x} is not a single bit"
        ));
    }
    Ok(value)
}

/// Chooses the narrowed mask from resolved keywords, or the fallback when the
/// lookup failed or the two keywords are missing, malformed, or share a bit.
fn select_keywords(lookup: Result<&[(String, u64)], &anyhow::Error>) -> KeywordChoice {
    let keywords = match lookup {
        Ok(keywords) => keywords,
        Err(error) => {
            return KeywordChoice::Fallback {
                reason: format!("keyword lookup failed: {error:#}"),
            };
        }
    };
    let bits = keyword_bit(keywords, KEYWORD_QUERY_RECEIVED)
        .and_then(|query| Ok((query, keyword_bit(keywords, KEYWORD_RESPONSE_SUCCESS)?)));
    match bits {
        Ok((query, response)) if query == response => KeywordChoice::Fallback {
            reason: format!(
                "{KEYWORD_QUERY_RECEIVED} and {KEYWORD_RESPONSE_SUCCESS} share bit {query:#x}"
            ),
        },
        Ok((query, response)) => KeywordChoice::Narrowed {
            match_any: query | response,
        },
        Err(reason) => KeywordChoice::Fallback { reason },
    }
}

/// Resolves the installed provider's keywords and applies [`select_keywords`].
pub(super) fn resolve_keywords() -> KeywordChoice {
    let lookup = query_provider_keywords(&PROVIDER);
    select_keywords(lookup.as_deref())
}

/// Reads the provider's keyword names and values from TDH.
fn query_provider_keywords(provider: &GUID) -> Result<Vec<(String, u64)>> {
    let header = offset_of!(PROVIDER_FIELD_INFOARRAY, FieldInfoArray);
    let mut size = 0;
    // SAFETY: PROVIDER is a constant GUID. A null buffer with a zero size is the
    // documented size query, so TDH writes only `size`.
    let status = unsafe {
        TdhEnumerateProviderFieldInformation(
            provider,
            EventKeywordInformation,
            null_mut(),
            &raw mut size,
        )
    };
    ensure!(
        status == ERROR_INSUFFICIENT_BUFFER,
        "TDH keyword size query failed: {}",
        std::io::Error::from_raw_os_error(status.cast_signed())
    );
    ensure!(
        (header..=MAX_METADATA).contains(&(size as usize)),
        "invalid TDH keyword metadata size"
    );
    // u64 storage supplies the alignment of PROVIDER_FIELD_INFOARRAY, whose
    // entries hold u64 values.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let capacity = size;
    // SAFETY: storage is aligned, initialized, and at least size bytes long.
    let status = unsafe {
        TdhEnumerateProviderFieldInformation(
            provider,
            EventKeywordInformation,
            storage.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    check_status(status, "TdhEnumerateProviderFieldInformation")?;
    ensure!(
        size <= capacity && size as usize >= header,
        "invalid TDH keyword result size"
    );
    // SAFETY: `storage` is zero-initialized, at least `capacity >= size` bytes
    // long, and outlives `bytes` without being mutated.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize) };
    parse_keyword_array(bytes)
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(size_of::<u32>())?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(size_of::<u64>())?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

/// Reads a NUL-terminated UTF-16 string that must end inside `bytes`.
fn read_utf16_name(bytes: &[u8], offset: usize) -> Option<String> {
    let mut units = Vec::new();
    let mut position = offset;
    loop {
        let end = position.checked_add(size_of::<u16>())?;
        let unit = u16::from_le_bytes(bytes.get(position..end)?.try_into().ok()?);
        if unit == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(unit);
        position = end;
    }
}

/// Parses a `PROVIDER_FIELD_INFOARRAY` into `(name, value)` pairs, checking the
/// element count and every name against the buffer length. Offsets are relative
/// to the start of the buffer.
fn parse_keyword_array(bytes: &[u8]) -> Result<Vec<(String, u64)>> {
    let count = read_u32(
        bytes,
        offset_of!(PROVIDER_FIELD_INFOARRAY, NumberOfElements),
    )
    .context("truncated TDH keyword header")? as usize;
    let first = offset_of!(PROVIDER_FIELD_INFOARRAY, FieldInfoArray);
    let entries_end = count
        .checked_mul(size_of::<PROVIDER_FIELD_INFO>())
        .and_then(|length| length.checked_add(first))
        .context("TDH keyword count overflows")?;
    ensure!(
        entries_end <= bytes.len(),
        "TDH keyword count {count} exceeds the {}-byte buffer",
        bytes.len()
    );
    (0..count)
        .map(|index| {
            let entry = first + index * size_of::<PROVIDER_FIELD_INFO>();
            let name_offset = read_u32(bytes, entry + offset_of!(PROVIDER_FIELD_INFO, NameOffset))
                .context("truncated TDH keyword entry")? as usize;
            let value = read_u64(bytes, entry + offset_of!(PROVIDER_FIELD_INFO, Value))
                .context("truncated TDH keyword entry")?;
            let name = read_utf16_name(bytes, name_offset)
                .with_context(|| format!("TDH keyword {index} name is outside the buffer"))?;
            Ok((name, value))
        })
        .collect()
}
