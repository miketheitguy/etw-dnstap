//! Bounded schema compilation and allocation-free decoding of ETW payloads.
//!
//! Only the prefix ending at the last required property is compiled. Metadata
//! after that prefix need not have a supported layout, but all property names
//! are checked for ambiguous required names. No observed payload offsets are
//! retained in a plan.

#![forbid(unsafe_code)]

use windows_sys::Win32::System::Diagnostics::Etw::{
    PropertyParamFixedLength, PropertyParamLength, TDH_INTYPE_ANSISTRING, TDH_INTYPE_BINARY,
    TDH_INTYPE_BOOLEAN, TDH_INTYPE_INT8, TDH_INTYPE_INT16, TDH_INTYPE_INT32, TDH_INTYPE_UINT8,
    TDH_INTYPE_UINT16, TDH_INTYPE_UINT32, TDH_INTYPE_UNICODESTRING, TDH_OUTTYPE_IPV4,
    TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT,
};

/// Bounds both cold-path plan storage and the stack-resident dependency table.
pub(super) const MAX_PLAN_FIELDS: usize = 256;
/// Plans up to this many steps use the small dependency table.
const SMALL_PLAN_STEPS: usize = 32;

#[derive(Debug)]
pub(super) struct FieldSpec {
    pub(super) name: String,
    pub(super) input: i32,
    pub(super) output: i32,
    pub(super) flags: u32,
    pub(super) count: u16,
    pub(super) length: u16,
}

#[derive(Debug)]
pub(super) struct Decoded<'a> {
    pub(super) tcp: &'a [u8],
    pub(super) port: &'a [u8],
    pub(super) local: &'a [u8],
    pub(super) peer: &'a [u8],
    pub(super) packet: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum DecodeError {
    #[error("truncated ETW payload")]
    Truncated,
    #[error("unterminated ETW string")]
    UnterminatedString,
    #[error("ETW property length overflow")]
    LengthOverflow,
    #[error("invalid compiled ETW length dependency")]
    InvalidDependency,
}

#[derive(Debug, Clone, Copy)]
enum Layout {
    Fixed(usize),
    Ansi,
    Unicode,
    BinaryFrom(usize),
}

#[derive(Debug)]
struct Step {
    layout: Layout,
    unsigned: bool,
    target: Option<usize>,
}

#[derive(Debug)]
pub(super) struct DecodePlan {
    steps: Vec<Step>,
}

impl DecodePlan {
    /// Returns `None` for layouts whose complete required prefix is unsupported.
    pub(super) fn compile(fields: &[FieldSpec], peer_name: &str) -> Option<Self> {
        if fields.len() > MAX_PLAN_FIELDS || !matches!(peer_name, "Source" | "Destination") {
            return None;
        }
        let names = ["TCP", "Port", "InterfaceIP", peer_name, "PacketData"];
        let mut targets = [None; 5];
        for (index, field) in fields.iter().enumerate() {
            if let Some(target) = names.iter().position(|name| *name == field.name) {
                let slot = targets.get_mut(target)?;
                if slot.replace(index).is_some() {
                    return None;
                }
            }
        }
        let mut last = 0;
        for target in targets {
            last = last.max(target?);
        }
        let prefix = fields.get(..=last)?;
        let mut steps = Vec::with_capacity(prefix.len());
        for (index, field) in prefix.iter().enumerate() {
            let layout = compile_layout(field, index, prefix)?;
            let target = targets
                .iter()
                .position(|candidate| *candidate == Some(index));
            if let Some(target) = target
                && !valid_target(target, field, layout)
            {
                return None;
            }
            steps.push(Step {
                layout,
                unsigned: is_unsigned(field.input),
                target,
            });
        }
        Some(Self { steps })
    }

    /// Borrows every result from `payload`; this walk performs no allocations.
    /// Semantic endpoint and DNS packet validation remains with the caller.
    pub(super) fn decode<'a>(&self, payload: &'a [u8]) -> Result<Decoded<'a>, DecodeError> {
        // Real DNS plans have about ten steps. Zeroing a 128-byte table instead of
        // 1 KiB per event is measurable; longer plans still get the full table.
        if self.steps.len() <= SMALL_PLAN_STEPS {
            self.decode_with(payload, &mut [0u32; SMALL_PLAN_STEPS])
        } else {
            self.decode_with(payload, &mut [0u32; MAX_PLAN_FIELDS])
        }
    }

    /// `dependencies` must have at least one slot per step; a shorter slice
    /// surfaces as `InvalidDependency` through the checked accesses below.
    fn decode_with<'a>(
        &self,
        payload: &'a [u8],
        dependencies: &mut [u32],
    ) -> Result<Decoded<'a>, DecodeError> {
        let mut offset = 0usize;
        let mut values: [&[u8]; 5] = [&[]; 5];
        for (index, step) in self.steps.iter().enumerate() {
            let remaining = payload.get(offset..).ok_or(DecodeError::Truncated)?;
            let length = match step.layout {
                Layout::Fixed(length) => length,
                Layout::Ansi => remaining
                    .iter()
                    .position(|byte| *byte == 0)
                    .and_then(|position| position.checked_add(1))
                    .ok_or(DecodeError::UnterminatedString)?,
                // Chunks start at this property's offset, even if it is odd.
                Layout::Unicode => remaining
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .position(|unit| *unit == [0, 0])
                    .and_then(|position| position.checked_add(1))
                    .and_then(|units| units.checked_mul(2))
                    .ok_or(DecodeError::UnterminatedString)?,
                Layout::BinaryFrom(dependency) => usize::try_from(
                    *dependencies
                        .get(dependency)
                        .ok_or(DecodeError::InvalidDependency)?,
                )
                .map_err(|_| DecodeError::LengthOverflow)?,
            };
            let end = offset
                .checked_add(length)
                .ok_or(DecodeError::LengthOverflow)?;
            let bytes = payload.get(offset..end).ok_or(DecodeError::Truncated)?;
            if step.unsigned {
                let value = match bytes {
                    [a] => u32::from(*a),
                    [a, b] => u32::from(u16::from_le_bytes([*a, *b])),
                    [a, b, c, d] => u32::from_le_bytes([*a, *b, *c, *d]),
                    _ => return Err(DecodeError::InvalidDependency),
                };
                *dependencies
                    .get_mut(index)
                    .ok_or(DecodeError::InvalidDependency)? = value;
            }
            if let Some(target) = step.target {
                *values
                    .get_mut(target)
                    .ok_or(DecodeError::InvalidDependency)? = bytes;
            }
            offset = end;
        }
        let [tcp, port, local, peer, packet] = values;
        Ok(Decoded {
            tcp,
            port,
            local,
            peer,
            packet,
        })
    }
}

fn is_unsigned(input: i32) -> bool {
    matches!(
        input,
        TDH_INTYPE_UINT8 | TDH_INTYPE_UINT16 | TDH_INTYPE_UINT32
    )
}

fn compile_layout(field: &FieldSpec, index: usize, fields: &[FieldSpec]) -> Option<Layout> {
    // All arrays, structures, custom schemas and unrecognized flags fall back.
    let allowed = u32::try_from(PropertyParamLength | PropertyParamFixedLength).ok()?;
    if field.flags & !allowed != 0 || field.count != 1 {
        return None;
    }
    let parameter_length = field.flags & u32::try_from(PropertyParamLength).ok()? != 0;
    let fixed_length = field.flags & u32::try_from(PropertyParamFixedLength).ok()? != 0;
    if parameter_length && fixed_length {
        return None;
    }
    match field.input {
        TDH_INTYPE_BINARY if parameter_length => {
            let dependency = usize::from(field.length);
            let source = fields.get(dependency)?;
            if dependency >= index || !is_unsigned(source.input) {
                return None;
            }
            // Earlier descriptors have already passed scalar layout validation.
            Some(Layout::BinaryFrom(dependency))
        }
        // A missing binary length can carry provider-specific semantics (for
        // example native IPv6 output). Only proven fixed widths are walkable.
        TDH_INTYPE_BINARY if field.length != 0 || fixed_length => {
            Some(Layout::Fixed(usize::from(field.length)))
        }
        _ if parameter_length || fixed_length => None,
        TDH_INTYPE_ANSISTRING if field.length == 0 => Some(Layout::Ansi),
        TDH_INTYPE_UNICODESTRING if field.length == 0 => Some(Layout::Unicode),
        TDH_INTYPE_UINT8 | TDH_INTYPE_INT8 if matches!(field.length, 0 | 1) => {
            Some(Layout::Fixed(1))
        }
        TDH_INTYPE_UINT16 | TDH_INTYPE_INT16 if matches!(field.length, 0 | 2) => {
            Some(Layout::Fixed(2))
        }
        TDH_INTYPE_UINT32 | TDH_INTYPE_INT32 | TDH_INTYPE_BOOLEAN
            if matches!(field.length, 0 | 4) =>
        {
            Some(Layout::Fixed(4))
        }
        _ => None,
    }
}

fn valid_target(target: usize, field: &FieldSpec, layout: Layout) -> bool {
    match target {
        0 | 1 => {
            (is_unsigned(field.input) || field.input == TDH_INTYPE_BOOLEAN)
                && (field.output != TDH_OUTTYPE_PORT || field.input == TDH_INTYPE_UINT16)
        }
        2 | 3 => match layout {
            Layout::Ansi | Layout::Unicode => {
                !matches!(field.output, TDH_OUTTYPE_IPV4 | TDH_OUTTYPE_IPV6)
            }
            Layout::Fixed(length) if field.input == TDH_INTYPE_BINARY => {
                matches!(
                    (field.output, length),
                    (TDH_OUTTYPE_IPV4, 4) | (TDH_OUTTYPE_IPV6, 16)
                )
            }
            _ => false,
        },
        4 => field.input == TDH_INTYPE_BINARY,
        _ => false,
    }
}

/// One property of the saved template, with the output type TDH reports for its
/// input type.
#[cfg(any(test, feature = "fuzzing"))]
pub(super) fn template_field(name: &str, input: i32) -> FieldSpec {
    use windows_sys::Win32::System::Diagnostics::Etw::{
        TDH_OUTTYPE_HEXBINARY, TDH_OUTTYPE_STRING, TDH_OUTTYPE_UNSIGNEDBYTE,
        TDH_OUTTYPE_UNSIGNEDINT,
    };
    let output = match input {
        TDH_INTYPE_UNICODESTRING | TDH_INTYPE_ANSISTRING => TDH_OUTTYPE_STRING,
        TDH_INTYPE_UINT8 => TDH_OUTTYPE_UNSIGNEDBYTE,
        TDH_INTYPE_UINT32 => TDH_OUTTYPE_UNSIGNEDINT,
        TDH_INTYPE_BINARY => TDH_OUTTYPE_HEXBINARY,
        _ => 0,
    };
    FieldSpec {
        name: name.into(),
        input,
        output,
        flags: 0,
        count: 1,
        length: 0,
    }
}

/// The saved v0 DNS Server manifest field order for event 256 (`response ==
/// false`) or 257, from `TCP` through `PacketData`. The packet length comes from
/// the preceding `BufferSize` property. Trailing metadata is not included.
#[cfg(any(test, feature = "fuzzing"))]
pub(super) fn saved_template(response: bool) -> Vec<FieldSpec> {
    let u8_field = |name| template_field(name, TDH_INTYPE_UINT8);
    let u32_field = |name| template_field(name, TDH_INTYPE_UINT32);
    let text = |name| template_field(name, TDH_INTYPE_ANSISTRING);
    let wide = |name| template_field(name, TDH_INTYPE_UNICODESTRING);
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
    let mut packet = template_field("PacketData", TDH_INTYPE_BINARY);
    packet.flags = PropertyParamLength.cast_unsigned();
    packet.length = u16::try_from(dependency).unwrap_or(0);
    fields.push(packet);
    fields
}

#[cfg(test)]
#[path = "decode_tests.rs"]
pub(crate) mod tests;
