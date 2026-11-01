use super::*;
use crate::{
    dnstap::Encoder,
    etw::{
        PROVIDER, PoolManager, decode,
        intake::copy_record,
        test_support::{
            AllocCounts, EXCLUDED_EVENT_IDS, assert_no_allocations, measured, utf16_nul_bytes,
        },
    },
};
use std::sync::Arc;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Diagnostics::Etw::EVENT_DESCRIPTOR;

/// Test-only adapter that decodes and encodes in one step, releasing the event
/// lease before returning bytes that borrow only the encoder.
trait ParseEncode {
    fn parse_encode<'e>(
        &mut self,
        event: RawEtwEvent,
        encoder: &'e mut Encoder,
    ) -> Result<Option<&'e [u8]>>;
}

impl ParseEncode for Parser {
    fn parse_encode<'e>(
        &mut self,
        event: RawEtwEvent,
        encoder: &'e mut Encoder,
    ) -> Result<Option<&'e [u8]>> {
        crate::pipeline::parse_and_encode(self, event, encoder)
    }
}

#[test]
fn fixed_property_descriptors_are_terminated_static_and_allocation_free() {
    let names = [
        PropertyName::Tcp,
        PropertyName::Port,
        PropertyName::InterfaceIp,
        PropertyName::Source,
        PropertyName::Destination,
        PropertyName::PacketData,
    ];
    let counts = measured(|| {
        for name in names {
            let (display, wide) = name.native();
            let descriptor = name.descriptor();
            assert_eq!(descriptor.ArrayIndex, u32::MAX);
            assert_eq!(descriptor.Reserved, 0);
            // Constant UTF-16 literals can be duplicated after inlining;
            // correctness depends on static lifetime and content, not identity.
            for pointer in [wide, descriptor.PropertyName as *const u16] {
                for (index, expected) in display.encode_utf16().chain(Some(0)).enumerate() {
                    // SAFETY: Both pointers come from terminated w! literals.
                    // Prior iterations matched nonzero units; a shorter native
                    // spelling fails at its terminator before advancing.
                    let actual = unsafe { pointer.add(index).read() };
                    assert_eq!(actual, expected, "static name spelling or terminator");
                }
            }
        }
    });
    assert_eq!(counts, AllocCounts::default());
}

#[test]
fn filetime_conversion_preserves_100ns_precision() -> Result<()> {
    assert_eq!(
        timestamp(116_444_736_010_000_001)?,
        UNIX_EPOCH + Duration::new(1, 100)
    );
    assert!(timestamp(116_444_735_999_999_999).is_err());
    Ok(())
}

#[test]
fn unknown_provider_endpoints_stay_absent_and_known_values_survive() -> Result<()> {
    assert_eq!(observed_address("0.0.0.0".parse()?), None);
    assert_eq!(observed_address("::".parse()?), None);
    assert_eq!(observed_port(0), None);
    let ipv4: IpAddr = "172.27.215.60".parse()?;
    let ipv6: IpAddr = "2001:db8::53".parse()?;
    assert_eq!(observed_address(ipv4), Some(ipv4));
    assert_eq!(observed_address(ipv6), Some(ipv6));
    assert_eq!(observed_port(53), Some(53));
    Ok(())
}

#[test]
fn rejects_malformed_addresses_and_boolean_encodings() {
    let unicode = PropertyType {
        input: TDH_INTYPE_UNICODESTRING,
        output: 0,
    };
    assert!(address(&[0x31, 0], unicode, &mut [0; 768]).is_err());
    assert!(address(&[0], unicode, &mut [0; 768]).is_err());
    assert!(
        integer(
            &[1],
            PropertyType {
                input: TDH_INTYPE_BOOLEAN,
                output: 0
            }
        )
        .is_err()
    );
}

fn address_kind(input: i32) -> PropertyType {
    PropertyType { input, output: 0 }
}

#[test]
fn address_reuse_validates_full_text_and_ignores_valid_zone_context() -> Result<()> {
    let mut scratch = [0; 768];
    let unicode = address_kind(TDH_INTYPE_UNICODESTRING);
    let ansi = address_kind(TDH_INTYPE_ANSISTRING);
    let long_zone = format!("fe80::1%{}", "界".repeat(247));
    let cases = [
        (utf16_nul_bytes(&long_zone), unicode, "fe80::1"),
        (b"192.0.2.53\0".to_vec(), ansi, "192.0.2.53"),
        (utf16_nul_bytes("::%接口"), unicode, "::"),
        (utf16_nul_bytes("::%\0ignored"), unicode, "::"),
        (b"::%bad\0suffix\0".to_vec(), ansi, "::"),
        (b"fe80::1%interface\0".to_vec(), ansi, "fe80::1"),
        (utf16_nul_bytes("2001:db8::53"), unicode, "2001:db8::53"),
    ];
    for (bytes, kind, expected) in &cases {
        assert_eq!(
            address(bytes, *kind, &mut scratch)?,
            expected.parse::<IpAddr>()?
        );
    }
    let malformed = [
        (vec![], unicode),
        (vec![0], unicode),
        (vec![b'1', 0], unicode),
        (vec![0, 0xd8, 0, 0], unicode),
        (utf16_nul_bytes("::\0%bad"), unicode),
        (vec![b':', 0, b':', 0, b'%', 0, 0, 0xd8, 0, 0], unicode),
        (utf16_nul_bytes(&format!("::%{}", "a".repeat(253))), unicode),
        (b"::%bad\xff\0".to_vec(), ansi),
        (b"::\0%bad\0".to_vec(), ansi),
        (b"192.0.2.1".to_vec(), ansi),
        (b"garbage\0".to_vec(), ansi),
    ];
    for (bytes, kind) in malformed {
        assert!(address(&bytes, kind, &mut scratch).is_err());
        assert_eq!(
            address(b"127.0.0.1\0", ansi, &mut scratch)?,
            IpAddr::from([127, 0, 0, 1])
        );
    }
    assert_no_allocations(|| {
        for _ in 0..32 {
            for (bytes, kind, _) in &cases {
                let result = address(bytes, *kind, &mut scratch);
                std::hint::black_box(result).expect("valid address");
            }
        }
    });
    Ok(())
}

#[test]
fn binary_addresses_require_the_native_length() -> Result<()> {
    let mut scratch = [0; 768];
    let kind = PropertyType {
        input: TDH_INTYPE_BINARY,
        output: TDH_OUTTYPE_IPV6,
    };
    assert_eq!(
        address(&[0; 16], kind, &mut scratch)?,
        IpAddr::from([0u8; 16])
    );
    for length in [0, 4, 15, 17] {
        assert!(address(&[0; 17][..length], kind, &mut scratch).is_err());
    }
    Ok(())
}

#[test]
fn bounded_property_seam_initializes_and_limits_every_native_view() -> Result<()> {
    // This seam covers buffer handling, not the DNS provider's native TDH implementation.
    let mut scratch = [0x55; 512];
    for size in [512, 1, 0, 4, 511] {
        let bytes = scratch_property(&mut scratch, size, |bytes| {
            assert_eq!(bytes.len(), size);
            assert!(bytes.iter().all(|byte| *byte == 0));
            bytes.fill(0xab);
            Ok(())
        })?;
        assert!(bytes.iter().all(|byte| *byte == 0xab));
    }
    assert!(scratch_property(&mut scratch, 513, |_| panic!("oversize extraction")).is_err());
    assert!(
        scratch_property(&mut scratch, 8, |bytes| {
            bytes.fill(0xff);
            bail!("injected native failure")
        })
        .is_err()
    );
    assert_no_allocations(|| {
        for size in [512, 1, 4, 0, 512] {
            let result = scratch_property(&mut scratch, size, |bytes| {
                bytes.fill(0x53);
                Ok(())
            });
            std::hint::black_box(result).expect("bounded extraction");
        }
    });
    Ok(())
}

#[test]
fn parser_excludes_other_ids_before_tdh_and_recycles_storage() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"test")?;
    for id in EXCLUDED_EVENT_IDS {
        let source = EVENT_RECORD {
            EventHeader: EVENT_HEADER {
                ProviderId: PROVIDER,
                EventDescriptor: EVENT_DESCRIPTOR {
                    Id: id,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: The empty event advertises no native input ranges.
        let event = unsafe { copy_record(&source, &pool) }.expect("empty excluded event");
        assert_no_allocations(|| {
            assert!(
                parser
                    .parse_encode(event, &mut encoder)
                    .expect("filtered before TDH")
                    .is_none()
            );
        });
    }
    let returned: Vec<_> = (0..3)
        .map(|_| pool.acquire(1).expect("parser returns excluded lease"))
        .collect();
    drop(returned);
    Ok(())
}

fn cached_schema(parser: &Parser, key: (u16, u8)) -> Option<&Schema> {
    parser
        .schemas
        .iter()
        .find(|(cached, _)| *cached == key)
        .map(|(_, schema)| schema)
}

fn cached_schema_mut(parser: &mut Parser, key: (u16, u8)) -> Option<&mut Schema> {
    parser
        .schemas
        .iter_mut()
        .find(|(cached, _)| *cached == key)
        .map(|(_, schema)| schema)
}

fn cached_fixture_schema(fields: &[FieldSpec], peer_name: &str) -> Schema {
    let kind = |name: &str| {
        let field = fields
            .iter()
            .find(|field| field.name == name)
            .expect("fixture field");
        PropertyType {
            input: field.input,
            output: field.output,
        }
    };
    Schema {
        plan: DecodePlan::compile(fields, peer_name),
        tcp: kind("TCP"),
        port: kind("Port"),
        local: kind("InterfaceIP"),
        peer: kind(peer_name),
        packet: kind("PacketData"),
    }
}

fn pooled_fixture(
    payload: &mut [u8],
    id: u16,
    version: u8,
    pool: &Arc<PoolManager>,
) -> RawEtwEvent {
    let source = EVENT_RECORD {
        EventHeader: EVENT_HEADER {
            ProviderId: PROVIDER,
            TimeStamp: 116_444_736_000_000_001,
            EventDescriptor: EVENT_DESCRIPTOR {
                Id: id,
                Version: version,
                ..Default::default()
            },
            ..Default::default()
        },
        UserDataLength: u16::try_from(payload.len()).expect("fixture size"),
        UserData: payload.as_mut_ptr().cast(),
        ..Default::default()
    };
    // SAFETY: The source owns the advertised payload until the copy completes.
    unsafe { copy_record(&source, pool) }.expect("fixture copied")
}

#[test]
fn cached_supported_parser_has_zero_tdh_calls_and_no_allocations() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"test")?;
    for id in [256, 257] {
        let mut fixture = decode::tests::fixture(id, true, true, 2);
        let schema = cached_fixture_schema(
            &fixture.fields,
            if id == 256 { "Source" } else { "Destination" },
        );
        assert!(schema.plan.is_some());
        parser.schemas.push(((id, 0), schema));
        // Warm final encoder capacity; subsequent pooled-buffer reuse is measured.
        let event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
        assert!(parser.parse_encode(event, &mut encoder)?.is_some());
        TDH_CALLS.with(|calls| calls.set([0; 3]));
        for _ in 0..4 {
            let event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
            assert_no_allocations(|| {
                assert!(
                    parser
                        .parse_encode(event, &mut encoder)
                        .expect("valid cached event")
                        .is_some()
                );
            });
        }
        assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
        // The consuming parser released every lease before returning encoder bytes.
        let leases: Vec<_> = (0..3)
            .map(|_| pool.acquire(1).expect("lease returned"))
            .collect();
        drop(leases);
    }
    Ok(())
}

#[test]
fn full_schema_cache_hits_alternate_ids_and_versions_without_tdh() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"cache boundary")?;
    let mut fixtures = Vec::new();
    let mut expected = Vec::new();
    for id in [256, 257] {
        for version in 0..4 {
            let wide_tcp = version % 2 != 0;
            let mut fixture = decode::tests::fixture(id, !wide_tcp, wide_tcp, 0);
            if wide_tcp {
                fixture.fields[0].input = TDH_INTYPE_UINT32;
                fixture.fields[0].output = 0;
                fixture.payload.splice(0..1, 1u32.to_le_bytes());
            }
            let peer = if id == 256 { "Source" } else { "Destination" };
            parser
                .schemas
                .push(((id, version), cached_fixture_schema(&fixture.fields, peer)));
            let event = pooled_fixture(&mut fixture.payload, id, version, &pool);
            expected.push(
                parser
                    .parse_encode(event, &mut encoder)?
                    .expect("warm fixture")
                    .to_vec(),
            );
            fixtures.push((id, version, fixture));
        }
    }
    assert_eq!(parser.schemas.len(), MAX_CACHED_SCHEMAS);
    TDH_CALLS.with(|calls| calls.set([0; 3]));
    // Interleave IDs and versions so an accidentally incomplete key uses the
    // wrong TCP width or packet direction.
    for index in (0..4).flat_map(|version| [version, version + 4]) {
        let (id, version, fixture) = &mut fixtures[index];
        let event = pooled_fixture(&mut fixture.payload, *id, *version, &pool);
        assert_no_allocations(|| {
            assert_eq!(
                parser
                    .parse_encode(event, &mut encoder)
                    .expect("cached hit")
                    .expect("accepted"),
                expected[index]
            );
        });
        assert_eq!(parser.schemas.len(), MAX_CACHED_SCHEMAS);
    }
    assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
    let mut fixture = decode::tests::fixture(256, false, false, 0);
    let event = pooled_fixture(&mut fixture.payload, 256, 249, &pool);
    assert!(parser.parse_encode(event, &mut encoder).is_err());
    assert_eq!(TDH_CALLS.with(std::cell::Cell::get)[0], 1);
    assert_eq!(parser.schemas.len(), MAX_CACHED_SCHEMAS);
    assert!(cached_schema(&parser, (256, 249)).is_none());
    for (id, version, _) in &fixtures {
        assert!(cached_schema(&parser, (*id, *version)).is_some());
    }
    let (id, version, fixture) = &mut fixtures[0];
    let event = pooled_fixture(&mut fixture.payload, *id, *version, &pool);
    TDH_CALLS.with(|calls| calls.set([0; 3]));
    assert_eq!(
        parser.parse_encode(event, &mut encoder)?.expect("recovery"),
        expected[0]
    );
    assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
    Ok(())
}

#[test]
fn schema_cache_never_exceeds_bound_or_reallocates() {
    let fixture = decode::tests::fixture(256, false, false, 0);
    let mut parser = Parser::new();
    let capacity = parser.schemas.capacity();
    // MAX_CACHED_SCHEMAS + 1 distinct keys: the final insert clears a full cache.
    for version in 0..=u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache") {
        let schema = cached_fixture_schema(&fixture.fields, "Source");
        parser.insert_schema((256, version), schema);
        assert!(parser.schemas.len() <= MAX_CACHED_SCHEMAS);
        assert_eq!(parser.schemas.capacity(), capacity);
    }
    assert_eq!(parser.schemas.len(), 1);
    assert!(cached_schema(&parser, (256, 0)).is_none());
    let last = u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache");
    assert!(cached_schema(&parser, (256, last)).is_some());
}

#[test]
fn supported_malformed_rejects_without_tdh_but_unsupported_schema_uses_fallback() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"test")?;
    let mut fixture = decode::tests::fixture(256, false, false, 0);
    parser
        .schemas
        .push(((256, 0), cached_fixture_schema(&fixture.fields, "Source")));
    let event = pooled_fixture(&mut fixture.payload, 256, 0, &pool);
    assert!(parser.parse_encode(event, &mut encoder)?.is_some());
    fixture.payload.truncate(fixture.packet.end - 1);
    TDH_CALLS.with(|calls| calls.set([0; 3]));
    let event = pooled_fixture(&mut fixture.payload, 256, 0, &pool);
    assert!(parser.parse_encode(event, &mut encoder).is_err());
    assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
    let mut unsupported = cached_fixture_schema(&fixture.fields, "Source");
    unsupported.plan = None;
    parser.schemas.push(((256, 249), unsupported));
    let event = pooled_fixture(&mut fixture.payload, 256, 249, &pool);
    assert!(parser.parse_encode(event, &mut encoder).is_err());
    let calls = TDH_CALLS.with(std::cell::Cell::get);
    assert_eq!(calls[0], 0);
    assert_eq!(calls[1], 1);
    Ok(())
}

#[test]
fn cached_parser_rejects_semantic_mutations_and_recovers_without_tdh() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"mutation recovery")?;
    for id in [256, 257] {
        let mut valid = decode::tests::fixture(id, false, false, 0);
        let peer = if id == 256 { "Source" } else { "Destination" };
        parser
            .schemas
            .push(((id, 0), cached_fixture_schema(&valid.fields, peer)));
        let event = pooled_fixture(&mut valid.payload, id, 0, &pool);
        let expected = parser
            .parse_encode(event, &mut encoder)?
            .expect("valid fixture")
            .to_vec();
        for mutation in 0..7 {
            let mut fixture = decode::tests::fixture(id, false, false, 0);
            match mutation {
                0 => fixture.payload[fixture.tcp.start] = 2,
                1 => fixture.payload[fixture.port.clone()].copy_from_slice(&u32::MAX.to_le_bytes()),
                2 => {
                    // Keep the layout internally consistent while exceeding the
                    // parser's shared TDH-compatible address extraction bound.
                    let mut oversized = vec![b'a'; 513];
                    oversized.push(0);
                    fixture.payload.splice(fixture.local.clone(), oversized);
                }
                3 => fixture.payload[fixture.local.start] = b'!',
                4 => fixture.payload[fixture.length_offset..fixture.length_offset + 4]
                    .copy_from_slice(&11u32.to_le_bytes()),
                5 => fixture.payload[fixture.packet.start + 2] ^= 0x80,
                _ => {}
            }
            let mut event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
            if mutation == 6 {
                event.header_mut().TimeStamp = 0;
            }
            TDH_CALLS.with(|calls| calls.set([0; 3]));
            assert!(
                parser.parse_encode(event, &mut encoder).is_err(),
                "mutation {mutation}, event {id}"
            );
            assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
            let leases: Vec<_> = (0..3)
                .map(|_| pool.acquire(1).expect("rejected lease recycled"))
                .collect();
            drop(leases);
            let event = pooled_fixture(&mut valid.payload, id, 0, &pool);
            assert_eq!(
                parser
                    .parse_encode(event, &mut encoder)?
                    .expect("recovery accepted"),
                expected
            );
            assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
        }
    }
    Ok(())
}

#[test]
fn process_local_manifest_tdh_matches_borrowed_fields_and_parser_output() -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Diagnostics::Etw::{TdhLoadManifest, TdhUnloadManifest};
    struct Manifest(Vec<u16>);
    impl Drop for Manifest {
        fn drop(&mut self) {
            // SAFETY: This exact terminated path remains owned through unload.
            let status = unsafe { TdhUnloadManifest(self.0.as_ptr()) };
            assert_eq!(status, ERROR_SUCCESS, "unload process-local test manifest");
        }
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/etw/testdata/decoder-fixture.man");
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: The absolute terminated manifest path is live for the synchronous
    // process-local decoder registration; this does not register an ETW publisher.
    let status = unsafe { TdhLoadManifest(path.as_ptr()) };
    check_status(status, "load fixture manifest")?;
    let _manifest = Manifest(path);
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"native oracle")?;
    let fixture = decode::tests::fixture(256, false, false, 0);
    for version in 0..u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache") {
        parser.schemas.push((
            (256, version),
            cached_fixture_schema(&fixture.fields, "Source"),
        ));
    }
    for id in [256, 257] {
        for (ipv6, tcp, variant) in [(false, false, 0), (true, true, 2), (false, true, 1)] {
            let mut fixture = decode::tests::fixture(id, ipv6, tcp, variant);
            let event = pooled_fixture(&mut fixture.payload, id, 250, &pool);
            event.with_record(|record| -> Result<()> {
                let peer = if id == 256 {
                    PropertyName::Source
                } else {
                    PropertyName::Destination
                };
                let mut scratch = [0; 512];
                for (name, range) in [
                    (PropertyName::Tcp, fixture.tcp.clone()),
                    (PropertyName::Port, fixture.port.clone()),
                    (PropertyName::InterfaceIp, fixture.local.clone()),
                    (peer, fixture.peer.clone()),
                ] {
                    assert_eq!(
                        property(record, name, 512, &mut scratch)?,
                        &fixture.payload[range]
                    );
                }
                let size = property_size(record, PropertyName::PacketData, 65535)?;
                let mut packet = vec![0; size];
                read_property(record, PropertyName::PacketData, &mut packet)?;
                assert_eq!(packet, fixture.payload[fixture.packet.clone()]);
                Ok(())
            })?;
            let was_missing = cached_schema(&parser, (id, 250)).is_none();
            let was_full = parser.schemas.len() == MAX_CACHED_SCHEMAS;
            TDH_CALLS.with(|calls| calls.set([0; 3]));
            let fast = parser
                .parse_encode(event, &mut encoder)?
                .expect("fixture supported")
                .to_vec();
            assert_eq!(
                TDH_CALLS.with(std::cell::Cell::get)[0],
                2 * usize::from(was_missing)
            );
            if was_full {
                assert_eq!(parser.schemas.len(), 1);
                assert!(cached_schema(&parser, (id, 250)).is_some());
            }
            let schema = cached_schema_mut(&mut parser, (id, 250)).expect("native schema cached");
            let plan = schema.plan.take().expect("native template compiles");
            let event = pooled_fixture(&mut fixture.payload, id, 250, &pool);
            assert_eq!(
                parser
                    .parse_encode(event, &mut encoder)?
                    .expect("TDH fallback"),
                fast
            );
            cached_schema_mut(&mut parser, (id, 250))
                .expect("schema retained")
                .plan = Some(plan);
            TDH_CALLS.with(|calls| calls.set([0; 3]));
            let event = pooled_fixture(&mut fixture.payload, id, 250, &pool);
            assert_no_allocations(|| {
                assert_eq!(
                    parser
                        .parse_encode(event, &mut encoder)
                        .expect("warm native plan")
                        .expect("accepted"),
                    fast
                );
            });
            assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
        }
    }
    Ok(())
}

#[test]
fn native_normalization_keeps_unsupported_trailing_names_without_reading_unions() -> Result<()> {
    use windows_sys::Win32::System::Diagnostics::Etw::EVENT_PROPERTY_INFO_0_0;
    let fixture = decode::tests::fixture(256, false, false, 0);
    let mut bytes = Vec::new();
    let mut properties = Vec::new();
    for field in &fixture.fields {
        let mut property = EVENT_PROPERTY_INFO {
            NameOffset: u32::try_from(bytes.len())?,
            Flags: field.flags.cast_signed(),
            ..Default::default()
        };
        bytes.extend(utf16_nul_bytes(&field.name));
        property.Anonymous1.nonStructType = EVENT_PROPERTY_INFO_0_0 {
            InType: u16::try_from(field.input)?,
            OutType: u16::try_from(field.output)?,
            MapNameOffset: 0,
        };
        property.Anonymous2.count = field.count;
        property.Anonymous3.length = field.length;
        properties.push(property);
    }
    let trailing = properties.len() - 1;
    for flags in [PropertyStruct, PropertyHasCustomSchema, i32::MIN] {
        properties[trailing].Flags = flags;
        let fields = normalize_fields(&properties, &bytes)?.expect("names retained");
        assert_eq!(fields[trailing].input, -1);
        assert!(DecodePlan::compile(&fields, "Source").is_some());
        properties[3].Flags = flags;
        let fields = normalize_fields(&properties, &bytes)?.expect("names retained");
        assert!(DecodePlan::compile(&fields, "Source").is_none());
        properties[3].Flags = fixture.fields[3].flags.cast_signed();
    }
    // An unsupported trailing target name must still disqualify ambiguity.
    properties[trailing].NameOffset = properties[2].NameOffset;
    let fields = normalize_fields(&properties, &bytes)?.expect("names retained");
    assert!(DecodePlan::compile(&fields, "Source").is_none());
    Ok(())
}

#[test]
fn native_normalization_validates_names_and_union_selection() -> Result<()> {
    let array_offset = offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
    assert!(validate_property_array(0, 1, array_offset).is_err());
    assert!(
        validate_property_array(2, 1, array_offset + size_of::<EVENT_PROPERTY_INFO>()).is_err()
    );
    assert!(validate_property_array(usize::MAX, 0, usize::MAX).is_err());
    validate_property_array(2, 1, array_offset + 2 * size_of::<EVENT_PROPERTY_INFO>())?;
    let bytes: Vec<u8> = "PacketData\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut property = EVENT_PROPERTY_INFO::default();
    property.Anonymous1.nonStructType =
        windows_sys::Win32::System::Diagnostics::Etw::EVENT_PROPERTY_INFO_0_0 {
            InType: u16::try_from(TDH_INTYPE_BINARY)?,
            ..Default::default()
        };
    property.Flags = PropertyParamLength;
    property.Anonymous2.count = 1;
    property.Anonymous3.lengthPropertyIndex = 7;
    let fields = normalize_fields(&[property], &bytes)?.expect("known scalar metadata");
    assert_eq!(fields[0].name, "PacketData");
    assert_eq!(fields[0].length, 7);
    assert_eq!(fields[0].count, 1);
    for flags in [PropertyStruct, PropertyHasCustomSchema, i32::MIN] {
        property.Flags = flags;
        let fields = normalize_fields(&[property], &bytes)?.expect("names-only metadata");
        assert_eq!(fields[0].input, -1);
        assert_eq!(fields[0].flags, flags.cast_unsigned());
    }
    property.Flags = 0;
    for offset in [1, u32::MAX] {
        property.NameOffset = offset;
        assert!(normalize_fields(&[property], &bytes).is_err());
    }
    assert!(property_name(&[b'A', 0, b'B'], 0).is_err());
    assert!(property_name(&[0, 0xd8, 0, 0], 0).is_err());
    assert!(normalize_fields(&vec![EVENT_PROPERTY_INFO::default(); 257], &bytes)?.is_none());
    Ok(())
}

#[test]
fn decodes_host_and_network_order_ports_and_binary_addresses() -> Result<()> {
    let native_port = PropertyType {
        input: TDH_INTYPE_UINT16,
        output: 0,
    };
    let network_port = PropertyType {
        output: TDH_OUTTYPE_PORT,
        ..native_port
    };
    assert_eq!(integer(&53u16.to_le_bytes(), native_port)?, 53);
    assert_eq!(integer(&53u16.to_be_bytes(), network_port)?, 53);
    assert_eq!(
        address(
            &[192, 0, 2, 1],
            PropertyType {
                input: TDH_INTYPE_UINT32,
                output: TDH_OUTTYPE_IPV4,
            },
            &mut [0; 768],
        )?,
        "192.0.2.1".parse::<IpAddr>()?
    );
    Ok(())
}

#[test]
fn failed_schema_load_is_cached_and_not_retried() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"negative cache")?;
    let mut fixture = decode::tests::fixture(256, false, false, 0);
    TDH_CALLS.with(|calls| calls.set([0; 3]));
    // No manifest exists for this provider version in the test process, so the
    // first load fails.
    let event = pooled_fixture(&mut fixture.payload, 256, 77, &pool);
    assert!(parser.parse_encode(event, &mut encoder).is_err());
    let after_first = TDH_CALLS.with(std::cell::Cell::get);
    assert!(
        after_first.iter().sum::<usize>() > 0,
        "first load calls TDH"
    );
    assert_eq!(parser.schema_rejected_events(), 1);
    assert!(parser.schemas.is_empty());
    for _ in 0..3 {
        let event = pooled_fixture(&mut fixture.payload, 256, 77, &pool);
        let error = parser
            .parse_encode(event, &mut encoder)
            .expect_err("rejected key stays rejected");
        assert!(error.to_string().contains("rejected earlier"));
    }
    assert_eq!(
        TDH_CALLS.with(std::cell::Cell::get),
        after_first,
        "cached rejections make no TDH calls"
    );
    assert_eq!(parser.schema_rejected_events(), 4);
    // Another version is a different key and still reaches TDH once.
    let event = pooled_fixture(&mut fixture.payload, 256, 78, &pool);
    assert!(parser.parse_encode(event, &mut encoder).is_err());
    assert_ne!(TDH_CALLS.with(std::cell::Cell::get), after_first);
    assert_eq!(parser.rejected_keys.len(), 2);
    Ok(())
}

#[test]
fn rejected_key_list_is_bounded_without_reallocating() {
    let mut parser = Parser::new();
    let capacity = parser.rejected_keys.capacity();
    for version in 0..=u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache") {
        parser.remember_rejected((256, version));
        assert!(parser.rejected_keys.len() <= MAX_CACHED_SCHEMAS);
    }
    assert_eq!(parser.rejected_keys.len(), 1);
    assert_eq!(parser.rejected_keys.capacity(), capacity);
}

#[test]
fn mixed_ipv4_ipv6_endpoints_are_rejected_and_recovered_from() -> Result<()> {
    let pool = PoolManager::new(1, 0)?;
    let mut parser = Parser::new();
    let mut encoder = Encoder::new(b"mixed families")?;
    for id in [256, 257] {
        let peer = if id == 256 { "Source" } else { "Destination" };
        let mut valid = decode::tests::fixture(id, false, false, 0);
        parser
            .schemas
            .push(((id, 0), cached_fixture_schema(&valid.fields, peer)));
        for (local_ipv6, peer_text) in [
            (false, &b"2001:db8:2::42\0"[..]),
            (true, &b"198.51.100.42\0"[..]),
        ] {
            let mut fixture = decode::tests::fixture(id, local_ipv6, false, 0);
            fixture
                .payload
                .splice(fixture.peer.clone(), peer_text.iter().copied());
            let event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
            let error = parser
                .parse_encode(event, &mut encoder)
                .expect_err("mixed address families are not a DNS exchange");
            assert!(
                format!("{error:#}").contains("address families"),
                "event {id}: {error:#}"
            );
        }
        let event = pooled_fixture(&mut valid.payload, id, 0, &pool);
        assert!(parser.parse_encode(event, &mut encoder)?.is_some());
    }
    Ok(())
}
