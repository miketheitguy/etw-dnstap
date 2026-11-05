//! The target-to-ID table.

use super::*;

#[test]
fn every_target_maps_to_its_own_id() {
    for &(target, id) in ALL {
        assert_eq!(warning_id(target), Some(id), "{target}");
        assert_eq!(target, format!("etw_dnstap::event::{id}"));
    }
}

#[test]
fn ids_are_unique_and_clear_of_the_level_ids() {
    let mut ids: Vec<u32> = ALL.iter().map(|&(_, id)| id).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "an ID is defined twice");
    // 1 to 6 are the level and failure IDs of `service`.
    assert!(ids.iter().all(|&id| id >= 10), "{ids:?}");
}

#[test]
fn loss_ids_are_10_to_16_and_risk_ids_are_20_to_28() {
    let expected = [
        (LOSS_ETW, 10),
        (LOSS_CHANNEL, 11),
        (LOSS_POOL, 12),
        (LOSS_NETWORK, 13),
        (LOSS_SPOOL, 14),
        (LOSS_DECODE, 15),
        (LOSS_INTERNAL, 16),
        (RISK_CHANNEL, 20),
        (RISK_ETW_BUFFERS, 21),
        (RISK_SEND_QUEUE, 22),
        (RISK_LINK_SATURATED, 23),
        (RISK_RECEIVER_DOWN, 24),
        (RISK_SPOOL_LIMIT, 25),
        (RISK_SPOOL_DISABLED, 26),
        (RISK_SPOOL_IO_ERROR, 27),
        (RISK_ETW_STATS, 28),
    ];
    assert_eq!(expected.len(), ALL.len(), "a new ID needs a row here");
    for (target, id) in expected {
        assert_eq!(warning_id(target), Some(id));
    }
}

#[test]
fn other_targets_have_no_id() {
    for target in [
        "",
        "etw_dnstap",
        "etw_dnstap::pipeline::supervisor",
        "etw_dnstap::event::",
        "etw_dnstap::event::3",
        "etw_dnstap::event::010",
        "etw_dnstap::event::10 ",
        "etw_dnstap::event::99",
        "other::event::10",
    ] {
        assert_eq!(warning_id(target), None, "{target:?}");
    }
}
