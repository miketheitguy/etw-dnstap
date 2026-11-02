//! Decodes arbitrary payload bytes with the saved DNS Server event templates
//! for events 256, 257, and 258. The entry point itself checks that no decoded
//! field leaves the payload.
#![no_main]

use libfuzzer_sys::fuzz_target;

const EVENT_IDS: [u16; 3] = [256, 257, 258];

fuzz_target!(|data: &[u8]| {
    let Some((selector, payload)) = data.split_first() else {
        return;
    };
    let event_id = EVENT_IDS[usize::from(*selector) % EVENT_IDS.len()];
    let _ = etw_dnstap::fuzzing::decode_reference(event_id, payload);
});
