//! Event Log IDs for the loss and risk warnings.
//!
//! The Event Log writer in `service` knows only the tracing level. A warning
//! that needs its own ID names it through the tracing *target*, which the
//! writer sees in the event metadata: `tracing::warn!(target: LOSS_ETW, ...)`.
//! The writer reads the ID back with [`warning_id`]. Everything else keeps the
//! IDs of its level (2 information, 3 warning, 4 error). The target is not part
//! of the written text, because the subscriber is built without targets.
//!
//! The IDs are part of the public contract: operators filter on them. Keep
//! `docs/guide/monitoring.md` in step with this table.

/// Defines one target constant for each ID, and the table of all of them.
macro_rules! warning_ids {
    ($($(#[$doc:meta])* $name:ident = $id:literal;)+) => {
        $(
            $(#[$doc])*
            pub(crate) const $name: &str = concat!("etw_dnstap::event::", $id);
        )+

        /// Every target in this module with its ID, for lookup and tests.
        const ALL: &[(&str, u32)] = &[$(($name, $id)),+];
    };
}

warning_ids! {
    /// Events or buffers that ETW reports lost.
    LOSS_ETW = 10;
    /// Events dropped because the event channel was full (`--capacity`).
    LOSS_CHANNEL = 11;
    /// Events dropped because no pool slot was free (`--capacity`, `--jumbo-capacity`).
    LOSS_POOL = 12;
    /// Frames lost on the network path: a failed connection or a full send queue.
    LOSS_NETWORK = 13;
    /// Frames the spool removed, skipped, or refused unsent.
    LOSS_SPOOL = 14;
    /// Events received but not encoded: rejected or schema-rejected.
    LOSS_DECODE = 15;
    /// Events lost inside the service: a callback error or a closed internal channel.
    LOSS_INTERNAL = 16;
    /// The event channel peak is near `--capacity`.
    RISK_CHANNEL = 20;
    /// The allocated ETW buffers are near the applied maximum.
    RISK_ETW_BUFFERS = 21;
    /// The send queue between the parser and the router is near full.
    RISK_SEND_QUEUE = 22;
    /// The receiver or the network path takes fewer bytes than the service captures.
    RISK_LINK_SATURATED = 23;
    /// The receiver connection is lost or unavailable.
    RISK_RECEIVER_DOWN = 24;
    /// The spool backlog is near its size cap, or the volume is near the free-space floor.
    RISK_SPOOL_LIMIT = 25;
    /// No spool is in use, so a receiver outage loses frames.
    RISK_SPOOL_DISABLED = 26;
    /// A spool file operation failed and the spool pauses.
    RISK_SPOOL_IO_ERROR = 27;
    /// The ETW statistics query fails, so ETW loss is not visible.
    RISK_ETW_STATS = 28;
}

/// The Event Log ID that a warning with this tracing target carries, or `None`
/// for any other target.
pub(crate) fn warning_id(target: &str) -> Option<u32> {
    ALL.iter()
        .find_map(|&(known, id)| (known == target).then_some(id))
}

#[cfg(test)]
#[path = "event_ids_tests.rs"]
mod event_ids_tests;
