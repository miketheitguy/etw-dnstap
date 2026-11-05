//! Writes arbitrary bytes as a spool segment file (and, depending on the first
//! byte, a cursor file), then opens the spool and reads it to the end. The
//! reader must never panic, must stop, and must hand out only whole frames.
#![no_main]

mod sancov_bounds;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    etw_dnstap::fuzzing::spool_segment(data);
});
