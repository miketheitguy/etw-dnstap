//! Reads arbitrary bytes as a Frame Streams control frame, as the handshake and
//! FINISH wait do. The reader must stay bounded and never panic.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((selector, frame)) = data.split_first() else {
        return;
    };
    // ACCEPT, READY, FINISH, START, STOP, and an invalid type.
    let expected = [1, 4, 5, 2, 3, u32::MAX][usize::from(selector >> 1) % 6];
    let _ = etw_dnstap::fuzzing::read_control(frame, expected, selector & 1 == 1);
});
