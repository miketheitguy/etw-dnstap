use super::{DecodePlan, FieldSpec};
use std::ops::Range;
use windows_sys::Win32::System::Diagnostics::Etw::*;

// These fixtures independently encode the saved 256/257 v0 manifest field order.
// Their offsets are recorded while emitting bytes, never obtained from DecodePlan.
pub(in crate::etw) struct Fixture {
    pub(in crate::etw) fields: Vec<FieldSpec>,
    pub(in crate::etw) payload: Vec<u8>,
    pub(in crate::etw) id: u16,
    pub(in crate::etw) tcp: Range<usize>,
    pub(in crate::etw) port: Range<usize>,
    pub(in crate::etw) local: Range<usize>,
    pub(in crate::etw) peer: Range<usize>,
    pub(in crate::etw) packet: Range<usize>,
    pub(in crate::etw) length_offset: usize,
}

fn field(name: &str, input: i32) -> FieldSpec {
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

fn append(payload: &mut Vec<u8>, bytes: &[u8]) -> Range<usize> {
    let start = payload.len();
    payload.extend_from_slice(bytes);
    start..payload.len()
}

fn unicode(payload: &mut Vec<u8>, text: &str) {
    for unit in text.encode_utf16().chain([0]) {
        payload.extend_from_slice(&unit.to_le_bytes());
    }
}

fn append_trailing_metadata(fields: &mut Vec<FieldSpec>, payload: &mut Vec<u8>, id: u16) {
    fields.push(field("AdditionalInfo", TDH_INTYPE_UNICODESTRING));
    unicode(payload, "trailing data is not DNS");
    if id == 257 {
        fields.push(field("ElapsedTime", TDH_INTYPE_UINT32));
        payload.extend_from_slice(&37_u32.to_le_bytes());
    }
    fields.push(field("GUID", TDH_INTYPE_UNICODESTRING));
    unicode(payload, "fixture-guid");
    if id == 257 {
        fields.push(field("StaleRecordsPresent", TDH_INTYPE_UNICODESTRING));
        unicode(payload, "false");
    }
}

fn append_response_scope(fields: &mut Vec<FieldSpec>, payload: &mut Vec<u8>, variant: usize) {
    for name in ["Scope", "Zone", "PolicyName"] {
        fields.push(field(name, TDH_INTYPE_UNICODESTRING));
        unicode(payload, &"aĀ🦀".repeat(variant));
    }
}

pub(in crate::etw) fn fixture(id: u16, ipv6: bool, tcp: bool, variant: usize) -> Fixture {
    assert!(matches!(id, 256 | 257));
    let mut fields = vec![
        field("TCP", TDH_INTYPE_UINT8),
        field("InterfaceIP", TDH_INTYPE_ANSISTRING),
        field(
            if id == 256 { "Source" } else { "Destination" },
            TDH_INTYPE_ANSISTRING,
        ),
    ];
    let mut payload = Vec::new();
    let tcp = append(&mut payload, &[u8::from(tcp)]);
    let local = append(
        &mut payload,
        if ipv6 {
            b"2001:db8:1::53\0"
        } else {
            b"192.0.2.53\0"
        },
    );
    let peer = append(
        &mut payload,
        if ipv6 {
            b"2001:db8:2::42\0"
        } else {
            b"198.51.100.42\0"
        },
    );
    if id == 256 {
        fields.push(field("RD", TDH_INTYPE_UINT8));
        payload.push(1);
    } else {
        fields.extend([field("AA", TDH_INTYPE_UINT8), field("AD", TDH_INTYPE_UINT8)]);
        payload.extend_from_slice(&[1, 0]);
    }
    fields.push(field("QNAME", TDH_INTYPE_ANSISTRING));
    payload.extend(std::iter::repeat_n(b'q', variant));
    payload.push(0);
    fields.extend([
        field("QTYPE", TDH_INTYPE_UINT32),
        field("XID", TDH_INTYPE_UINT32),
    ]);
    payload.extend_from_slice(&28_u32.to_le_bytes());
    payload.extend_from_slice(&0x1234_u32.to_le_bytes());
    if id == 257 {
        fields.extend([
            field("DNSSEC", TDH_INTYPE_UINT8),
            field("RCODE", TDH_INTYPE_UINT32),
        ]);
        payload.push(1);
        payload.extend_from_slice(&0_u32.to_le_bytes());
    }
    fields.push(field("Port", TDH_INTYPE_UINT32));
    let port = append(&mut payload, &53000_u32.to_le_bytes());
    fields.push(field("Flags", TDH_INTYPE_UINT32));
    payload.extend_from_slice(&0x8100_u32.to_le_bytes());
    if id == 257 {
        append_response_scope(&mut fields, &mut payload, variant);
    }
    let dependency = fields.len();
    fields.push(field("BufferSize", TDH_INTYPE_UINT32));
    let length_offset = payload.len();
    payload.extend_from_slice(&17_u32.to_le_bytes());
    let mut packet_field = field("PacketData", TDH_INTYPE_BINARY);
    packet_field.flags = PropertyParamLength.cast_unsigned();
    packet_field.length = u16::try_from(dependency).expect("fixture field index fits u16");
    fields.push(packet_field);
    let packet = append(
        &mut payload,
        &[
            0x12,
            0x34,
            if id == 257 { 0x81 } else { 1 },
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            b'a',
            0,
            0,
            1,
        ],
    );
    append_trailing_metadata(&mut fields, &mut payload, id);
    Fixture {
        fields,
        payload,
        id,
        tcp,
        port,
        local,
        peer,
        packet,
        length_offset,
    }
}

fn compile(fixture: &Fixture) -> DecodePlan {
    DecodePlan::compile(
        &fixture.fields,
        if fixture.id == 256 {
            "Source"
        } else {
            "Destination"
        },
    )
    .expect("saved template must be supported")
}

#[test]
fn saved_templates_decode_borrowed_fields_across_variable_prefixes() {
    for id in [256, 257] {
        for ipv6 in [false, true] {
            for tcp in [false, true] {
                for variant in [0, 1, 2, 31, 257] {
                    let fixture = fixture(id, ipv6, tcp, variant);
                    let plan = compile(&fixture);
                    let decoded = plan
                        .decode(&fixture.payload)
                        .expect("valid saved-template fixture");
                    for (actual, range) in [
                        (decoded.tcp, fixture.tcp),
                        (decoded.port, fixture.port),
                        (decoded.local, fixture.local),
                        (decoded.peer, fixture.peer),
                        (decoded.packet, fixture.packet),
                    ] {
                        let expected = &fixture.payload[range];
                        assert_eq!(actual, expected);
                        assert_eq!(
                            actual.as_ptr(),
                            expected.as_ptr(),
                            "must borrow original payload"
                        );
                    }
                    assert_eq!(decoded.packet.len(), 17);
                }
            }
        }
    }
}

#[test]
fn every_truncation_before_packet_end_is_rejected() {
    for id in [256, 257] {
        let fixture = fixture(id, false, true, 2);
        let plan = compile(&fixture);
        for end in 0..fixture.packet.end {
            assert!(
                plan.decode(&fixture.payload[..end]).is_err(),
                "event {id}, end {end}"
            );
        }
        // No trailing metadata is required for extraction of the final target.
        assert!(plan.decode(&fixture.payload[..fixture.packet.end]).is_ok());
    }
}

#[test]
fn unterminated_and_odd_length_text_is_rejected() {
    let fixture = fixture(257, false, false, 0);
    let plan = compile(&fixture);
    let mut ansi = fixture.payload[..fixture.local.end - 1].to_vec();
    ansi.resize(ansi.len() + 64, b'x');
    assert!(plan.decode(&ansi).is_err());
    // The first response UTF-16 field follows Flags and precedes BufferSize.
    let scope_start = fixture.port.end + 4;
    for tail in [vec![], vec![1], vec![1, 0], vec![1, 0, 0]] {
        let mut payload = fixture.payload[..scope_start].to_vec();
        payload.extend_from_slice(&tail);
        assert!(plan.decode(&payload).is_err());
    }
}

#[test]
fn packet_length_is_exact_and_never_includes_unrelated_tail() {
    let mut fixture = fixture(257, true, true, 1);
    let plan = compile(&fixture);
    let payload_length = u32::try_from(fixture.payload.len()).expect("fixture length fits u32");
    for length in [u32::MAX, 65536, payload_length] {
        fixture.payload[fixture.length_offset..fixture.length_offset + 4]
            .copy_from_slice(&length.to_le_bytes());
        assert!(plan.decode(&fixture.payload).is_err());
    }
    for length in [0_u32, 1, 12, 17] {
        fixture.payload[fixture.length_offset..fixture.length_offset + 4]
            .copy_from_slice(&length.to_le_bytes());
        let decoded = plan
            .decode(&fixture.payload)
            .expect("packet length fits payload");
        assert_eq!(
            decoded.packet,
            &fixture.payload[fixture.packet.start..fixture.packet.start + length as usize]
        );
    }
}

#[test]
fn unsupported_schema_shapes_and_ambiguous_targets_reject_compilation() {
    for flags in [
        PropertyStruct,
        PropertyParamCount,
        PropertyParamFixedCount,
        PropertyHasCustomSchema,
        PropertyWBEMXmlFragment,
        0x100,
    ] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields[3].flags = flags.cast_unsigned();
        assert!(
            DecodePlan::compile(&fixture.fields, "Source").is_none(),
            "flags {flags}"
        );
    }
    for count in [0, 2, u16::MAX] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields[3].count = count;
        assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
    }
    for name in ["TCP", "Port", "InterfaceIP", "Source", "PacketData"] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields.push(field(name, TDH_INTYPE_UINT8));
        assert!(
            DecodePlan::compile(&fixture.fields, "Source").is_none(),
            "duplicate {name}"
        );
    }
    let mut fixture = fixture(257, false, false, 0);
    fixture.fields[12].flags = PropertyParamFixedLength.cast_unsigned();
    assert!(
        DecodePlan::compile(&fixture.fields, "Destination").is_none(),
        "fixed empty string is not terminated"
    );
}

#[test]
fn invalid_length_dependencies_reject_compilation() {
    for dependency in [10, 11, 12, u16::MAX] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields[10].length = dependency;
        assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
    }
    for dependency in [1, 2, 4, 10] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields[10].length = dependency;
        assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
    }
    let mut fixture = fixture(256, false, false, 0);
    fixture.fields[9].input = TDH_INTYPE_INT32;
    assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
}

#[test]
fn binary_prefix_requires_an_explicit_length_even_when_not_a_target() {
    for output in [TDH_OUTTYPE_HEXBINARY, TDH_OUTTYPE_IPV6] {
        let mut fixture = fixture(256, false, false, 0);
        // RD is an ignored field preceding QNAME and all later targets.
        // A zero metadata length can conceal TDH's legacy inferred width.
        fixture.fields[3].input = TDH_INTYPE_BINARY;
        fixture.fields[3].output = output;
        assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
    }

    let mut fixture = fixture(256, false, false, 0);
    fixture.fields[3].input = TDH_INTYPE_BINARY;
    fixture.fields[3].output = TDH_OUTTYPE_HEXBINARY;
    fixture.fields[3].flags = PropertyParamFixedLength.cast_unsigned();
    // Explicit fixed length zero consumes no bytes. Remove the emitted RD byte
    // independently so later fields must still land at their expected offsets.
    fixture.payload.remove(fixture.peer.end);
    let plan = compile(&fixture);
    let decoded = plan
        .decode(&fixture.payload)
        .expect("explicit empty ignored binary field");
    assert_eq!(decoded.port, 53000_u32.to_le_bytes());
    assert_eq!(decoded.packet.len(), 17);
    assert_eq!(decoded.packet[0..2], [0x12, 0x34]);
}

#[test]
fn missing_targets_unsupported_types_and_plan_bound_reject_compilation() {
    for name in ["TCP", "Port", "InterfaceIP", "Source", "PacketData"] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields.retain(|field| field.name != name);
        assert!(
            DecodePlan::compile(&fixture.fields, "Source").is_none(),
            "missing {name}"
        );
    }
    for input in [TDH_INTYPE_UINT64, TDH_INTYPE_POINTER, TDH_INTYPE_SID] {
        let mut fixture = fixture(256, false, false, 0);
        fixture.fields[3].input = input;
        assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
    }
    let mut fixture = fixture(256, false, false, 0);
    while fixture.fields.len() <= super::MAX_PLAN_FIELDS {
        fixture.fields.push(field("trailing", TDH_INTYPE_UINT8));
    }
    assert!(DecodePlan::compile(&fixture.fields, "Source").is_none());
    assert!(DecodePlan::compile(&fixture.fields[..13], "OtherPeer").is_none());
}

#[test]
fn unsupported_metadata_after_last_required_field_needs_no_layout_guess() {
    let mut fixture = fixture(256, false, false, 1);
    fixture.fields[11].input = TDH_INTYPE_POINTER;
    fixture.fields[11].flags = PropertyHasCustomSchema.cast_unsigned();
    let plan = compile(&fixture);
    let decoded = plan
        .decode(&fixture.payload[..fixture.packet.end])
        .expect("complete required prefix");
    assert_eq!(decoded.packet, &fixture.payload[fixture.packet]);
}

#[test]
fn fixed_binary_addresses_borrow_exact_network_bytes() {
    for (output, local, peer) in [
        (
            TDH_OUTTYPE_IPV4,
            vec![192, 0, 2, 53],
            vec![198, 51, 100, 42],
        ),
        (
            TDH_OUTTYPE_IPV6,
            [0x20, 1, 0xd, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 53].to_vec(),
            [0x20, 1, 0xd, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 42].to_vec(),
        ),
    ] {
        let mut fields = vec![
            field("TCP", TDH_INTYPE_UINT8),
            field("Port", TDH_INTYPE_UINT32),
        ];
        for name in ["InterfaceIP", "Source"] {
            let mut address = field(name, TDH_INTYPE_BINARY);
            address.output = output;
            address.length = u16::try_from(local.len()).expect("native address length fits u16");
            fields.push(address);
        }
        let mut packet = field("PacketData", TDH_INTYPE_BINARY);
        packet.length = 12;
        fields.push(packet);
        let mut payload = vec![1];
        payload.extend_from_slice(&53_u32.to_le_bytes());
        let local_range = append(&mut payload, &local);
        let peer_range = append(&mut payload, &peer);
        payload.extend_from_slice(&[0; 12]);
        let plan = DecodePlan::compile(&fields, "Source").expect("supported fixed-address schema");
        let decoded = plan
            .decode(&payload)
            .expect("complete fixed-address payload");
        assert_eq!(decoded.local, local);
        assert_eq!(decoded.peer, peer);
        assert_eq!(decoded.local.as_ptr(), payload[local_range].as_ptr());
        assert_eq!(decoded.peer.as_ptr(), payload[peer_range].as_ptr());
        fields[2].length -= 1;
        assert!(DecodePlan::compile(&fields, "Source").is_none());
    }
}

#[test]
fn warmed_decode_does_not_allocate_and_reuses_changed_payload() {
    let mut fixture = fixture(257, true, true, 31);
    let plan = compile(&fixture);
    assert!(plan.decode(&fixture.payload).is_ok());
    for byte in [0x12, 0xab, 0xff] {
        fixture.payload[fixture.packet.start] = byte;
        super::super::assert_no_allocations(|| {
            let decoded = plan.decode(&fixture.payload).expect("valid reused fixture");
            assert_eq!(decoded.packet[0], byte);
            assert_eq!(decoded.packet.len(), 17);
        });
    }
}

/// Every saved template shape: event ID, address family, transport, and string
/// length variants that move all later offsets.
fn all_fixtures() -> impl Iterator<Item = Fixture> {
    [256_u16, 257].into_iter().flat_map(|id| {
        [false, true].into_iter().flat_map(move |ipv6| {
            [false, true].into_iter().flat_map(move |tcp| {
                [0_usize, 1, 2, 31, 257]
                    .into_iter()
                    .map(move |variant| fixture(id, ipv6, tcp, variant))
            })
        })
    })
}

#[test]
fn truncation_sweep_covers_every_template_and_prefix() {
    let mut checked = 0_usize;
    for fixture in all_fixtures() {
        let plan = compile(&fixture);
        // Every strict prefix that ends before the last required byte fails and
        // never panics; the first prefix containing the packet succeeds.
        for end in 0..fixture.packet.end {
            assert!(
                plan.decode(&fixture.payload[..end]).is_err(),
                "event {} prefix {end}/{}",
                fixture.id,
                fixture.payload.len()
            );
            checked += 1;
        }
        for end in fixture.packet.end..=fixture.payload.len() {
            let decoded = plan
                .decode(&fixture.payload[..end])
                .expect("trailing metadata is never required");
            assert_eq!(decoded.packet, &fixture.payload[fixture.packet.clone()]);
        }
    }
    assert!(checked > 2_000, "sweep must be exhaustive, ran {checked}");
}

#[test]
fn length_field_overflow_is_rejected_for_every_template() {
    for mut fixture in all_fixtures() {
        let plan = compile(&fixture);
        for length in [
            u32::MAX,
            u32::MAX - 1,
            u32::MAX / 2 + 1,
            0x8000_0000,
            0x7fff_ffff,
            0x0001_0000,
        ] {
            fixture.payload[fixture.length_offset..fixture.length_offset + 4]
                .copy_from_slice(&length.to_le_bytes());
            let available = fixture.payload.len() - fixture.packet.start;
            if length as usize > available {
                assert!(plan.decode(&fixture.payload).is_err(), "length {length:#x}");
            }
        }
    }
}

#[test]
fn zero_length_packet_decodes_as_empty_without_consuming_the_tail() {
    for mut fixture in all_fixtures() {
        let plan = compile(&fixture);
        fixture.payload[fixture.length_offset..fixture.length_offset + 4]
            .copy_from_slice(&0_u32.to_le_bytes());
        let decoded = plan
            .decode(&fixture.payload)
            .expect("empty packet is framed");
        assert_eq!(decoded.packet, &[] as &[u8]);
        // The empty packet still starts at the same offset: later bytes are not
        // reinterpreted as part of an earlier property.
        assert_eq!(decoded.port, &fixture.payload[fixture.port.clone()]);
    }
}

#[test]
fn maximum_dns_packet_is_framed_exactly_and_one_more_byte_is_not_invented() {
    for id in [256_u16, 257] {
        for size in [65_535_usize, 65_536] {
            let mut fixture = fixture(id, true, false, 3);
            let plan = compile(&fixture);
            let packet = fixture.packet.clone();
            let body: Vec<u8> = (0..size)
                .map(|index| u8::try_from(index % 251).expect("below 251"))
                .collect();
            fixture.payload.splice(packet.clone(), body.iter().copied());
            let length = u32::try_from(size).expect("size fits u32");
            fixture.payload[fixture.length_offset..fixture.length_offset + 4]
                .copy_from_slice(&length.to_le_bytes());
            let decoded = plan.decode(&fixture.payload).expect("large packet fits");
            assert_eq!(decoded.packet.len(), size);
            assert_eq!(decoded.packet, body.as_slice());
            // One byte short of the declared size is a truncation, not a shorter packet.
            let short = &fixture.payload[..packet.start + size - 1];
            assert!(plan.decode(short).is_err());
        }
    }
}

#[test]
fn decoder_does_not_validate_address_families_or_directions() {
    // Semantic checks belong to the caller: an IPv4 interface with an IPv6 peer
    // borrows both texts unchanged, and the encoder (tested separately) rejects
    // the mixed pair.
    let mut fixture = fixture(256, false, false, 0);
    let plan = compile(&fixture);
    let mut peer = b"2001:db8:2::42\0".to_vec();
    fixture.payload.splice(fixture.peer.clone(), peer.drain(..));
    let decoded = plan
        .decode(&fixture.payload)
        .expect("layout is still valid");
    assert_eq!(decoded.local, b"192.0.2.53\0");
    assert_eq!(decoded.peer, b"2001:db8:2::42\0");
}

mod properties {
    use super::{Fixture, compile, fixture};
    use crate::etw::decode::{DecodePlan, Decoded, FieldSpec};
    use proptest::prelude::*;

    fn assert_inside(decoded: &Decoded<'_>, payload: &[u8]) {
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
                "field escapes the payload"
            );
        }
    }

    fn params() -> impl Strategy<Value = (u16, bool, bool, usize)> {
        (
            prop::sample::select(vec![256_u16, 257]),
            any::<bool>(),
            any::<bool>(),
            0_usize..300,
        )
    }

    fn build((id, ipv6, tcp, variant): (u16, bool, bool, usize)) -> Fixture {
        fixture(id, ipv6, tcp, variant)
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic_and_never_escape_the_payload(
            params in params(),
            payload in prop::collection::vec(any::<u8>(), 0..2048),
        ) {
            let template = build(params);
            let plan = compile(&template);
            if let Ok(decoded) = plan.decode(&payload) {
                assert_inside(&decoded, &payload);
            }
        }

        #[test]
        fn mutated_valid_payloads_stay_inside_the_payload(
            params in params(),
            edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let template = build(params);
            let plan = compile(&template);
            let mut payload = template.payload.clone();
            for (index, value) in edits {
                let position = index.index(payload.len());
                payload[position] = value;
            }
            payload.truncate(cut.index(payload.len() + 1));
            if let Ok(decoded) = plan.decode(&payload) {
                assert_inside(&decoded, &payload);
            }
        }

        #[test]
        fn every_strict_prefix_of_the_required_region_fails(
            params in params(),
            cut in any::<prop::sample::Index>(),
        ) {
            let template = build(params);
            let plan = compile(&template);
            let end = cut.index(template.packet.end);
            prop_assert!(plan.decode(&template.payload[..end]).is_err());
        }

        #[test]
        fn arbitrary_schemas_never_panic_the_compiler_or_decoder(
            fields in prop::collection::vec(
                (0_usize..8, -4_i32..40, -2_i32..40, any::<u32>(), 0_u16..4, 0_u16..20),
                0..40,
            ),
            payload in prop::collection::vec(any::<u8>(), 0..512),
            response in any::<bool>(),
        ) {
            const NAMES: [&str; 8] = [
                "TCP", "Port", "InterfaceIP", "Source", "Destination", "PacketData", "X", "Y",
            ];
            let specs: Vec<FieldSpec> = fields
                .into_iter()
                .map(|(name, input, output, flags, count, length)| FieldSpec {
                    name: NAMES[name].to_owned(),
                    input,
                    output,
                    flags: flags & 0b111,
                    count,
                    length,
                })
                .collect();
            let peer = if response { "Destination" } else { "Source" };
            if let Some(plan) = DecodePlan::compile(&specs, peer)
                && let Ok(decoded) = plan.decode(&payload)
            {
                assert_inside(&decoded, &payload);
            }
        }
    }
}
