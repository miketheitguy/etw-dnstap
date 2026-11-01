//! Compiles an arbitrary event schema and, if it compiles, decodes an arbitrary
//! payload with it. Neither step may panic or read outside the payload.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct Field {
    name: u8,
    input: u8,
    output: u8,
    flags: u8,
    count: u16,
    length: u16,
}

#[derive(Debug, Arbitrary)]
struct Input {
    fields: Vec<Field>,
    response: bool,
    payload: Vec<u8>,
}

fuzz_target!(|input: Input| {
    let fields: Vec<_> = input
        .fields
        .iter()
        .take(300)
        .map(|field| etw_dnstap::fuzzing::FuzzField {
            name: field.name,
            input: field.input,
            output: field.output,
            flags: field.flags,
            count: field.count,
            length: field.length,
        })
        .collect();
    let _ = etw_dnstap::fuzzing::compile_and_decode(&fields, input.response, &input.payload);
});
