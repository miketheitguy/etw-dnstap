//! Decodes arbitrary payload bytes with the saved DNS Server event templates.
//! The entry point itself checks that no decoded field leaves the payload.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((selector, payload)) = data.split_first() else {
        return;
    };
    let _ = etw_dnstap::fuzzing::decode_reference(selector & 1 == 1, payload);
});
