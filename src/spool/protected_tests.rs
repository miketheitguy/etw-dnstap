//! The spool key between and during uses: what the locked page holds, the
//! guard after a panic, concurrent use, and the cost of each use.

use std::{
    panic::{self, AssertUnwindSafe},
    ptr,
    sync::Arc,
    thread,
    time::Instant,
};

use ring::aead::{AES_128_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};

use super::*;

const KEY: [u8; KEY_LEN] = [
    0x3c, 0x7a, 0x91, 0x0e, 0x55, 0xd2, 0x48, 0xb3, 0x16, 0xef, 0x60, 0x2d, 0x9b, 0x84, 0xc7, 0x0f,
];
const ID: [u8; KEY_ID_LEN] = [0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58];

fn new_key() -> SegmentKey {
    SegmentKey::from_bytes(&KEY, ID).expect("the key page locks")
}

/// The same key, built the ordinary way, to check results against.
fn reference() -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &KEY).expect("16-byte key"))
}

fn nonce(index: u32) -> Nonce {
    let mut bytes = [0_u8; NONCE_LEN];
    bytes[NONCE_LEN - 4..].copy_from_slice(&index.to_be_bytes());
    Nonce::assume_unique_for_key(bytes)
}

/// Asserts the page as it must be between uses: the key area is not the key
/// in the clear, and the scratch area is all zeros.
fn assert_at_rest(key: &SegmentKey) {
    let snapshot = key.snapshot();
    assert_ne!(
        snapshot.raw, KEY,
        "the key bytes are encrypted between uses"
    );
    assert_ne!(snapshot.raw, [0; KEY_LEN], "and not zeroed");
    assert!(
        snapshot.scratch.iter().all(|&byte| byte == 0),
        "the scratch area is zero between uses"
    );
    assert_eq!(snapshot.scratch.len(), SCRATCH_LEN);
}

/// Seals `data` with `key` through `with_aead`; returns ciphertext and tag.
fn seal_with(key: &SegmentKey, index: u32, data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut body = data.to_vec();
    let tag = key
        .with_aead(|aead| aead.seal_in_place_separate_tag(nonce(index), Aad::empty(), &mut body))
        .expect("the key is usable")
        .expect("the cipher seals");
    (body, tag.as_ref().to_vec())
}

#[test]
fn between_uses_the_page_holds_no_key_and_an_empty_scratch_area() {
    let key = new_key();
    assert_at_rest(&key);
    // The encrypted bytes are the key: they decrypt to it.
    assert_eq!(key.plaintext_for_test().expect("usable"), KEY);
    assert_at_rest(&key);
    let _ = seal_with(&key, 0, b"one record");
    assert_at_rest(&key);
}

#[test]
fn with_aead_gives_a_working_key_built_in_the_page_and_cleans_up() {
    let key = new_key();
    let data: Vec<u8> = (0..=255).cycle().take(4000).collect();

    // Seal through `with_aead`, open with the reference key.
    let (mut sealed, tag) = seal_with(&key, 7, &data);
    assert_ne!(sealed, data);
    assert_at_rest(&key);
    sealed.extend_from_slice(&tag);
    let opened = reference()
        .open_in_place(nonce(7), Aad::empty(), &mut sealed)
        .expect("the reference key opens it");
    assert_eq!(opened, &data[..]);

    // Seal with the reference key, open through `with_aead`.
    let mut body = data.clone();
    reference()
        .seal_in_place_append_tag(nonce(8), Aad::empty(), &mut body)
        .expect("seals");
    let opened_in_page = key
        .with_aead(|aead| {
            aead.open_in_place(nonce(8), Aad::empty(), &mut body)
                .map(|plain| plain.to_vec())
        })
        .expect("the key is usable")
        .expect("opens");
    assert_eq!(opened_in_page, data);
    assert_at_rest(&key);

    // The schedule the closure gets is the one in the scratch area of the page.
    let address = key
        .with_aead(|aead| ptr::from_ref::<LessSafeKey>(aead) as usize)
        .expect("usable");
    assert_eq!(address, key.scratch_address());
    assert!(address > key.page_address());
    assert!(address < key.page_address() + PAGE_BYTES);
    assert_at_rest(&key);
}

#[test]
fn a_panic_inside_with_aead_leaves_no_plaintext_and_the_key_works_after() {
    let key = new_key();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        key.with_aead(|aead| {
            // The key is built and usable at this point.
            let mut body = vec![1_u8; 64];
            let _ = aead.seal_in_place_separate_tag(nonce(1), Aad::empty(), &mut body);
            panic!("a closure that panics");
        })
    }));
    assert!(outcome.is_err(), "the panic reaches the caller");
    assert_at_rest(&key);
    assert!(key.state.is_poisoned(), "the mutex saw the panic");

    // The poisoned mutex is recovered and the key still seals correctly.
    let (mut sealed, tag) = seal_with(&key, 2, b"after the panic");
    sealed.extend_from_slice(&tag);
    let opened = reference()
        .open_in_place(nonce(2), Aad::empty(), &mut sealed)
        .expect("opens");
    assert_eq!(opened, b"after the panic");
    assert_at_rest(&key);
}

#[test]
fn concurrent_uses_from_many_threads_are_correct() {
    let key = Arc::new(new_key());
    let threads: Vec<_> = (0..8_u32)
        .map(|worker| {
            let key = Arc::clone(&key);
            thread::spawn(move || {
                let check = reference();
                for round in 0..200_u32 {
                    let index = worker * 1000 + round;
                    let data: Vec<u8> = (0..300_u32)
                        .map(|at| at.to_le_bytes()[0] ^ index.to_le_bytes()[0])
                        .collect();
                    let (mut sealed, tag) = seal_with(&key, index, &data);
                    sealed.extend_from_slice(&tag);
                    let mut copy = sealed.clone();
                    let opened = check
                        .open_in_place(nonce(index), Aad::empty(), &mut copy)
                        .expect("the reference key opens it");
                    assert_eq!(opened, &data[..]);
                    let opened_in_page = key
                        .with_aead(|aead| {
                            aead.open_in_place(nonce(index), Aad::empty(), &mut sealed)
                                .map(|plain| plain.to_vec())
                        })
                        .expect("usable")
                        .expect("opens");
                    assert_eq!(opened_in_page, data);
                }
            })
        })
        .collect();
    for handle in threads {
        handle.join().expect("no thread panics");
    }
    assert_at_rest(&key);
}

#[test]
fn matches_compares_the_key_in_the_page() {
    let key = new_key();
    assert!(key.matches(&KEY).expect("usable"));
    let mut other = KEY;
    other[15] ^= 1;
    assert!(!key.matches(&other).expect("usable"));
    assert!(!key.matches(&[0; KEY_LEN]).expect("usable"));
    assert_at_rest(&key);
}

#[test]
fn a_zeroed_key_fails_closed() {
    let key = new_key();
    key.zero_for_test();
    let snapshot = key.snapshot();
    assert_eq!(snapshot.raw, [0; KEY_LEN], "the key bytes are zero");
    assert!(snapshot.scratch.iter().all(|&byte| byte == 0));
    let mut ran = false;
    assert_eq!(
        key.with_aead(|_| ran = true),
        Err(CryptError::KeyZeroed),
        "no key is built from zeroed bytes"
    );
    assert!(!ran, "the closure does not run");
    assert_eq!(key.matches(&KEY), Err(CryptError::KeyZeroed));
    assert_eq!(key.plaintext_for_test(), Err(CryptError::KeyZeroed));
    assert_eq!(
        key.loss(),
        Some(KeyLoss::Protect {
            code: TEST_FAILURE_CODE
        })
    );
}

/// A failed `CryptUnprotectMemory` leaves the bytes encrypted: nothing is
/// zeroed, and a later use works.
#[test]
fn a_failed_unprotect_changes_nothing_and_a_later_use_works() {
    let key = new_key();
    let before = key.snapshot();
    key.fail_unprotect_for_test(2);
    for _ in 0..2 {
        let mut ran = false;
        assert_eq!(
            key.with_aead(|_| ran = true),
            Err(CryptError::MemoryProtection {
                call: "CryptUnprotectMemory",
                code: TEST_FAILURE_CODE,
            })
        );
        assert!(!ran, "the closure does not run");
    }
    assert_eq!(key.loss(), None, "the key is not lost");
    assert_eq!(key.snapshot().raw, before.raw, "the bytes are unchanged");
    assert_at_rest(&key);
    let (mut sealed, tag) = seal_with(&key, 3, b"after a failed unprotect");
    sealed.extend_from_slice(&tag);
    let opened = reference()
        .open_in_place(nonce(3), Aad::empty(), &mut sealed)
        .expect("opens");
    assert_eq!(opened, b"after a failed unprotect");
}

/// One failed `CryptProtectMemory` is retried; the use succeeds.
#[test]
fn a_failed_protect_is_retried_once() {
    let key = new_key();
    key.fail_protect_for_test(1);
    let (mut sealed, tag) = seal_with(&key, 4, b"retried");
    assert_eq!(key.loss(), None);
    assert_at_rest(&key);
    sealed.extend_from_slice(&tag);
    let opened = reference()
        .open_in_place(nonce(4), Aad::empty(), &mut sealed)
        .expect("opens");
    assert_eq!(opened, b"retried");
}

/// Two failed `CryptProtectMemory` calls in a row zero the key for good; the
/// closure never runs with a key whose bytes could not be encrypted again.
#[test]
fn a_protect_that_fails_twice_zeroes_the_key() {
    let key = new_key();
    key.fail_protect_for_test(2);
    let mut ran = false;
    assert_eq!(
        key.with_aead(|_| ran = true),
        Err(CryptError::MemoryProtection {
            call: "CryptProtectMemory",
            code: TEST_FAILURE_CODE,
        })
    );
    assert!(!ran);
    assert_eq!(
        key.loss(),
        Some(KeyLoss::Protect {
            code: TEST_FAILURE_CODE
        })
    );
    let snapshot = key.snapshot();
    assert_eq!(snapshot.raw, [0; KEY_LEN]);
    assert!(snapshot.scratch.iter().all(|&byte| byte == 0));
    assert_eq!(key.with_aead(|_| ()), Err(CryptError::KeyZeroed));
}

/// Bytes that decrypt to something other than the key are never used: the key
/// is zeroed instead of sealing records no start could read.
#[test]
fn bytes_that_fail_the_key_check_zero_the_key() {
    let key = new_key();
    key.tamper_for_test();
    let mut ran = false;
    assert_eq!(key.with_aead(|_| ran = true), Err(CryptError::KeyCheck));
    assert!(!ran);
    assert_eq!(key.loss(), Some(KeyLoss::Check));
    assert_eq!(key.snapshot().raw, [0; KEY_LEN]);
    assert_eq!(key.with_aead(|_| ()), Err(CryptError::KeyZeroed));
}

#[test]
fn key_losses_name_the_call_and_the_code() {
    assert_eq!(
        KeyLoss::Protect { code: 1784 }.to_string(),
        "CryptProtectMemory, Win32 error 1784"
    );
    assert!(KeyLoss::Check.to_string().contains("key check"));
    assert!(CryptError::KeyCheck.is_key_error());
    assert!(CryptError::KeyZeroed.is_key_error());
    assert!(!CryptError::Seal.is_key_error());
    assert!(!CryptError::BadPlaintextLength.is_key_error());
}

#[test]
fn a_key_buffer_becomes_the_key_without_a_copy() {
    let mut buffer = KeyBuffer::new().expect("the page locks");
    assert_eq!(buffer.bytes(), &[0; KEY_LEN], "a new page is zero");
    buffer.bytes_mut().copy_from_slice(&KEY);
    let address = buffer.address();
    assert_eq!(ptr::from_ref(buffer.bytes()) as usize, address);
    let key = buffer.protect(ID).expect("protects");
    assert_eq!(key.page_address(), address, "the same page, not a copy");
    assert_eq!(key.id(), ID);
    assert_at_rest(&key);
    assert_eq!(key.plaintext_for_test().expect("usable"), KEY);
}

#[test]
fn debug_shows_the_id_only() {
    let key = new_key();
    let shown = format!("{key:?} {key:#?}");
    assert!(shown.contains("5152535455565758"), "{shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
    assert!(!shown.contains("3c7a"), "{shown}");
    assert!(!shown.contains("60"), "decimal 0x3c: {shown}");
    assert_eq!(
        format!("{:?}", KeyBuffer::new().expect("locks")),
        "KeyBuffer { .. }"
    );
}

#[test]
fn memory_protection_errors_name_the_call_and_the_code() {
    let error = CryptError::MemoryProtection {
        call: "CryptUnprotectMemory",
        code: 87,
    };
    let text = error.to_string();
    assert!(text.contains("CryptUnprotectMemory"), "{text}");
    assert!(text.contains("Win32 error 87"), "{text}");
    // A failed unprotect zeroes nothing, so the text must not say so.
    assert!(!text.contains("zeroed"), "{text}");
    assert!(!text.contains("  "), "one space between words: {text}");
    let zeroed = CryptError::KeyZeroed.to_string();
    assert!(zeroed.contains("zeroed"), "{zeroed}");
    assert!(!zeroed.contains("  "), "one space between words: {zeroed}");
}

/// Manual: ns per seal of a 64 KiB record through `with_aead`, against the
/// same seal with a key built once, and the cost of an empty `with_aead`. Run
/// with optimization:
///
/// ```text
/// CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test with_aead_cost -- --ignored --nocapture
/// ```
#[test]
#[ignore = "manual benchmark; run with optimization and --ignored --nocapture"]
fn with_aead_cost_benchmark() {
    const ROUNDS: u32 = 20_000;
    let key = new_key();
    let plain = reference();
    let mut record = vec![0x5a_u8; 64 * 1024];

    let time = |run: &mut dyn FnMut(u32)| {
        for index in 0..ROUNDS / 10 {
            run(index);
        }
        let start = Instant::now();
        for index in 0..ROUNDS {
            run(index);
        }
        start.elapsed().as_nanos() / u128::from(ROUNDS)
    };
    let raw = time(&mut |index| {
        let tag = plain
            .seal_in_place_separate_tag(nonce(index), Aad::empty(), &mut record)
            .expect("seals");
        let _ = std::hint::black_box(tag);
    });
    let protected = time(&mut |index| {
        let tag = key
            .with_aead(|aead| {
                aead.seal_in_place_separate_tag(nonce(index), Aad::empty(), &mut record)
            })
            .expect("usable")
            .expect("seals");
        let _ = std::hint::black_box(tag);
    });
    let empty = time(&mut |_| {
        key.with_aead(|aead| {
            std::hint::black_box(aead);
        })
        .expect("usable");
    });
    let setup = time(&mut |_| {
        std::hint::black_box(reference());
    });
    eprintln!(
        "64 KiB seal: raw {raw} ns, with_aead {protected} ns, overhead {} ns; \
         empty with_aead {empty} ns; ring key setup alone {setup} ns",
        protected.saturating_sub(raw)
    );
}
