//! Fuzzing entry points, compiled only with the `fuzzing` feature.
//!
//! They expose the bounded-decoder surface to the `fuzz/` crate without making
//! any internal type public. Every function checks the decoder's safety
//! contract itself and panics when it is violated, so a fuzzer reports it.

use windows_sys::Win32::System::Diagnostics::Etw::{
    PropertyParamFixedLength, PropertyParamLength, TDH_INTYPE_ANSISTRING, TDH_INTYPE_BINARY,
    TDH_INTYPE_BOOLEAN, TDH_INTYPE_INT8, TDH_INTYPE_INT16, TDH_INTYPE_INT32, TDH_INTYPE_UINT8,
    TDH_INTYPE_UINT16, TDH_INTYPE_UINT32, TDH_INTYPE_UNICODESTRING, TDH_OUTTYPE_IPV4,
    TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT,
};

use super::decode::{DecodePlan, Decoded, FieldSpec};

/// One schema property as a fuzzer describes it, using small selectors instead
/// of raw ETW constants.
#[derive(Clone, Debug)]
pub struct FuzzField {
    /// Selects a required name (0..=5) or an ignored one (anything else).
    pub name: u8,
    /// Selects a TDH input type; values past the table map to raw numbers.
    pub input: u8,
    /// Selects a TDH output type.
    pub output: u8,
    /// Selects property flags (bit 0 length-by-parameter, bit 1 fixed length,
    /// higher bits are passed through shifted to exercise rejection).
    pub flags: u8,
    pub count: u16,
    pub length: u16,
}

const NAMES: [&str; 6] = [
    "TCP",
    "Port",
    "InterfaceIP",
    "Source",
    "Destination",
    "PacketData",
];
const INPUTS: [i32; 11] = [
    TDH_INTYPE_UINT8,
    TDH_INTYPE_INT8,
    TDH_INTYPE_UINT16,
    TDH_INTYPE_INT16,
    TDH_INTYPE_UINT32,
    TDH_INTYPE_INT32,
    TDH_INTYPE_BOOLEAN,
    TDH_INTYPE_ANSISTRING,
    TDH_INTYPE_UNICODESTRING,
    TDH_INTYPE_BINARY,
    TDH_INTYPE_BINARY,
];
const OUTPUTS: [i32; 4] = [0, TDH_OUTTYPE_IPV4, TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT];

fn spec(field: &FuzzField) -> FieldSpec {
    let flags = match field.flags & 3 {
        0 => 0,
        1 => PropertyParamLength.cast_unsigned(),
        2 => PropertyParamFixedLength.cast_unsigned(),
        _ => u32::from(field.flags) << 6,
    };
    FieldSpec {
        name: NAMES
            .get(usize::from(field.name))
            .map_or("Other", |name| *name)
            .to_owned(),
        input: INPUTS
            .get(usize::from(field.input))
            .copied()
            .unwrap_or_else(|| i32::from(field.input)),
        output: OUTPUTS
            .get(usize::from(field.output))
            .copied()
            .unwrap_or_else(|| i32::from(field.output)),
        flags,
        count: field.count,
        length: field.length,
    }
}

/// Panics unless every borrowed field lies wholly inside `payload`.
fn check_borrows(decoded: &Decoded<'_>, payload: &[u8]) {
    let start = payload.as_ptr() as usize;
    let end = start + payload.len();
    for part in [
        decoded.tcp,
        decoded.port,
        decoded.local,
        decoded.peer,
        decoded.packet,
    ] {
        let from = part.as_ptr() as usize;
        assert!(
            part.is_empty() || (from >= start && from + part.len() <= end),
            "decoded field escapes the payload"
        );
    }
}

/// Compiles an arbitrary schema; if it compiles, decodes `payload` with it.
/// Returns whether the schema compiled.
#[must_use]
pub fn compile_and_decode(fields: &[FuzzField], response: bool, payload: &[u8]) -> bool {
    let specs: Vec<FieldSpec> = fields.iter().map(spec).collect();
    let peer = if response { "Destination" } else { "Source" };
    let Some(plan) = DecodePlan::compile(&specs, peer) else {
        return false;
    };
    if let Ok(decoded) = plan.decode(payload) {
        check_borrows(&decoded, payload);
    }
    true
}

fn property(name: &str, input: i32, output: i32) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        input,
        output,
        flags: 0,
        count: 1,
        length: 0,
    }
}

/// Decodes `payload` with the saved v0 DNS Server template for event 256
/// (`response == false`) or 257.
///
/// # Panics
/// Panics if the built-in template stops compiling or a decoded field escapes
/// `payload`.
#[must_use]
pub fn decode_reference(response: bool, payload: &[u8]) -> bool {
    let u8_field = |name| property(name, TDH_INTYPE_UINT8, 0);
    let u32_field = |name| property(name, TDH_INTYPE_UINT32, 0);
    let text = |name| property(name, TDH_INTYPE_ANSISTRING, 0);
    let wide = |name| property(name, TDH_INTYPE_UNICODESTRING, 0);
    let mut fields = vec![
        u8_field("TCP"),
        text("InterfaceIP"),
        text(if response { "Destination" } else { "Source" }),
    ];
    if response {
        fields.extend([u8_field("AA"), u8_field("AD")]);
    } else {
        fields.push(u8_field("RD"));
    }
    fields.extend([text("QNAME"), u32_field("QTYPE"), u32_field("XID")]);
    if response {
        fields.extend([u8_field("DNSSEC"), u32_field("RCODE")]);
    }
    fields.extend([u32_field("Port"), u32_field("Flags")]);
    if response {
        fields.extend([wide("Scope"), wide("Zone"), wide("PolicyName")]);
    }
    let dependency = fields.len();
    fields.push(u32_field("BufferSize"));
    let mut packet = property("PacketData", TDH_INTYPE_BINARY, 0);
    packet.flags = PropertyParamLength.cast_unsigned();
    packet.length = u16::try_from(dependency).unwrap_or(0);
    fields.push(packet);
    let plan = DecodePlan::compile(&fields, if response { "Destination" } else { "Source" })
        .expect("the saved template must compile");
    match plan.decode(payload) {
        Ok(decoded) => {
            check_borrows(&decoded, payload);
            true
        }
        Err(_) => false,
    }
}
