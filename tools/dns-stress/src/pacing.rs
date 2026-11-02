//! Absolute per-worker send schedule for the `--qps` ceiling.
//!
//! Each worker owns `qps` send slots in every `threads` seconds. Slot `k` is due
//! `ceil(k * threads * 1e9 / qps)` nanoseconds after the shared start. The grid
//! does not depend on when a send occurs, so a late wake-up does not lower the
//! long-run rate: the worker sends every slot that is due when it wakes. The
//! exact rational grid also has no per-slot rounding drift.
//!
//! A backlog older than [`MAX_BACKLOG_NS`] is not sent as a burst. The worker
//! drops the old slots, counts them, and continues from the latest due slot.

/// Longest lateness of the oldest due slot that the worker sends as catch-up.
///
/// Sleep overshoot and scheduler delays are much shorter than this bound, so
/// the worker recovers them completely. A longer backlog comes from the
/// in-flight limit, the ID-rotation drain, or a stalled host; sending it would
/// be a burst that the configured ceiling did not request.
pub(crate) const MAX_BACKLOG_NS: u64 = 100_000_000;

/// The send grid of one worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Grid {
    /// Length of one grid window: `threads` seconds, in nanoseconds.
    window_ns: u64,
    /// Slots in one window: the global `--qps` value.
    slots_per_window: u64,
}

/// The sends that a worker can make now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Due {
    /// The first slot to send. It is later than the input slot after a backlog skip.
    pub(crate) slot: u64,
    /// Slots to send now, starting at `slot`. Never more than the limit.
    pub(crate) count: usize,
    /// Slots dropped because the backlog was longer than [`MAX_BACKLOG_NS`].
    pub(crate) skipped: u64,
}

impl Grid {
    /// Splits the global `qps` ceiling equally across `threads` workers.
    pub(crate) fn new(threads: u16, qps: u64) -> Self {
        Self {
            window_ns: u64::from(threads.max(1)) * 1_000_000_000,
            slots_per_window: qps.max(1),
        }
    }

    /// Nanoseconds from the shared start to `slot`. Rounded up, so the worker
    /// never sends ahead of its share of the ceiling.
    pub(crate) fn slot_offset_ns(self, slot: u64) -> u64 {
        let offset = (u128::from(slot) * u128::from(self.window_ns))
            .div_ceil(u128::from(self.slots_per_window));
        u64::try_from(offset).unwrap_or(u64::MAX)
    }

    /// The latest slot whose offset is not after `elapsed_ns`.
    fn last_due_slot(self, elapsed_ns: u64) -> u64 {
        let slot =
            u128::from(elapsed_ns) * u128::from(self.slots_per_window) / u128::from(self.window_ns);
        u64::try_from(slot).unwrap_or(u64::MAX)
    }
}

/// Returns the slots to send at `elapsed_ns` after the shared start.
///
/// `next_slot` is the first unsent slot. `limit` is the number of sends that
/// the batch size, the in-flight window, and the ID space permit now. With a
/// zero limit, nothing changes: the backlog check occurs when a send is
/// possible, so a blocked worker resumes with one send instead of a burst.
pub(crate) fn due(grid: Grid, elapsed_ns: u64, next_slot: u64, limit: usize) -> Due {
    let last = grid.last_due_slot(elapsed_ns);
    if limit == 0 || last < next_slot {
        return Due {
            slot: next_slot,
            count: 0,
            skipped: 0,
        };
    }
    let lateness = elapsed_ns.saturating_sub(grid.slot_offset_ns(next_slot));
    let (slot, skipped) = if lateness > MAX_BACKLOG_NS {
        (last, last - next_slot)
    } else {
        (next_slot, 0)
    };
    let count = usize::try_from(last - slot + 1).map_or(limit, |count| count.min(limit));
    Due {
        slot,
        count,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four workers sharing 50,000 QPS: one slot every 80 microseconds.
    const INTERVAL_NS: u64 = 80_000;

    fn grid() -> Grid {
        Grid::new(4, 50_000)
    }

    #[test]
    fn grid_offsets_follow_the_exact_interval() {
        let grid = grid();
        assert_eq!(grid.slot_offset_ns(0), 0);
        assert_eq!(grid.slot_offset_ns(1), INTERVAL_NS);
        assert_eq!(grid.slot_offset_ns(1_000), 1_000 * INTERVAL_NS);
        // A non-integral interval (3e9 / 7 ns) rounds each offset up, not each step.
        let uneven = Grid::new(3, 7);
        assert_eq!(uneven.slot_offset_ns(1), 428_571_429);
        assert_eq!(uneven.slot_offset_ns(7), 3_000_000_000);
    }

    #[test]
    fn on_time_wake_sends_one() {
        let grid = grid();
        let at = grid.slot_offset_ns(5);
        assert_eq!(
            due(grid, at - 1, 5, 64),
            Due {
                slot: 5,
                count: 0,
                skipped: 0
            }
        );
        assert_eq!(
            due(grid, at, 5, 64),
            Due {
                slot: 5,
                count: 1,
                skipped: 0
            }
        );
        assert_eq!(due(grid, at + INTERVAL_NS - 1, 5, 64).count, 1);
    }

    #[test]
    fn oversleep_of_k_intervals_sends_k_plus_one() {
        let grid = grid();
        let at = grid.slot_offset_ns(5);
        for k in 0..20 {
            let result = due(grid, at + k * INTERVAL_NS + INTERVAL_NS / 2, 5, 64);
            assert_eq!(result.count, usize::try_from(k + 1).unwrap_or(0));
            assert_eq!((result.slot, result.skipped), (5, 0));
        }
        // The lab overshoot: a 0.54 ms sleep spans 6.75 intervals of 80 us.
        assert_eq!(due(grid, 540_000, 0, 64).count, 7);
    }

    #[test]
    fn batch_cap_limits_catch_up_and_keeps_the_rest() {
        let grid = grid();
        // 50 ms late: 626 slots due, all inside the backlog bound.
        let elapsed = 50_000_000;
        let first = due(grid, elapsed, 0, 64);
        assert_eq!((first.slot, first.count, first.skipped), (0, 64, 0));
        let second = due(grid, elapsed, 64, 64);
        assert_eq!((second.slot, second.count, second.skipped), (64, 64, 0));
        let tail = due(grid, elapsed, 576, 64);
        assert_eq!((tail.slot, tail.count, tail.skipped), (576, 50, 0));
    }

    #[test]
    fn inflight_cap_limits_sends_and_defers_backlog_check() {
        let grid = grid();
        assert_eq!(due(grid, 5_000_000, 0, 3).count, 3);
        // A full window changes nothing, even past the backlog bound.
        let blocked = due(grid, 10 * MAX_BACKLOG_NS, 0, 0);
        assert_eq!(
            blocked,
            Due {
                slot: 0,
                count: 0,
                skipped: 0
            }
        );
    }

    #[test]
    fn backlog_beyond_bound_skips_to_latest_slot() {
        let grid = grid();
        let next = 10;
        let start = grid.slot_offset_ns(next);
        // At the bound: still a catch-up, bounded by the limit.
        let at_bound = due(grid, start + MAX_BACKLOG_NS, next, 64);
        assert_eq!(
            (at_bound.slot, at_bound.count, at_bound.skipped),
            (next, 64, 0)
        );
        // Past the bound: drop the old slots and send only the latest.
        let elapsed = start + MAX_BACKLOG_NS + 1;
        let last = grid.last_due_slot(elapsed);
        let skipped = due(grid, elapsed, next, 64);
        assert_eq!(
            skipped,
            Due {
                slot: last,
                count: 1,
                skipped: last - next
            }
        );
        assert_eq!(skipped.skipped, MAX_BACKLOG_NS / INTERVAL_NS);
        // After the skip, the grid continues at its original phase.
        assert_eq!(due(grid, elapsed, last + 1, 64).count, 0);
        assert_eq!(
            due(grid, grid.slot_offset_ns(last + 1), last + 1, 64).count,
            1
        );
    }

    /// Simulates a worker that wakes every `wake_ns` and sends all due slots.
    /// Returns the sent count, the skipped count, and the next unsent slot.
    fn simulate(grid: Grid, wake_ns: u64, until_ns: u64, limit: usize) -> (u64, u64, u64) {
        let (mut sent, mut skipped, mut next, mut elapsed) = (0, 0, 0, 0);
        while elapsed < until_ns {
            let result = due(grid, elapsed, next, limit);
            let count = u64::try_from(result.count).unwrap_or(0);
            skipped += result.skipped;
            sent += count;
            next = result.slot + count;
            elapsed += wake_ns;
        }
        (sent, skipped, next)
    }

    #[test]
    fn no_drift_over_many_iterations() {
        // Ten seconds of 540 us wakes at 12,500 QPS per worker. The last wake
        // is at 9.99972 s, so it sends every slot through 124,996.
        let (sent, skipped, next) = simulate(grid(), 540_000, 10_000_000_000, 64);
        assert_eq!((sent, skipped, next), (124_997, 0, 124_997));
        // An interval that is not a whole number of nanoseconds keeps the exact
        // long-run rate: 999,999,999 QPS on one worker over one millisecond.
        // A per-send interval rounded up to 2 ns would send only half of these.
        let fine = Grid::new(1, 999_999_999);
        let (fine_sent, fine_skipped, _) = simulate(fine, 1_000, 1_000_001, usize::MAX);
        assert_eq!((fine_sent, fine_skipped), (1_000_000, 0));
    }

    #[test]
    fn sustained_shortfall_counts_dropped_slots_and_bounds_backlog() {
        // Each 1 ms wake permits one send against 12.5 due slots, so the
        // worker falls behind and drops its backlog at the bound.
        let grid = grid();
        let (sent, skipped, next) = simulate(grid, 1_000_000, 2_000_000_000, 1);
        assert_eq!(sent, 2_000);
        assert!(skipped > 0);
        assert_eq!(sent + skipped, next);
        let due_slots = grid.last_due_slot(1_999_000_000) + 1;
        assert!(next <= due_slots);
        assert!(due_slots - next <= MAX_BACKLOG_NS / INTERVAL_NS + 1);
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        let grid = Grid::new(1, 1_000_000_000);
        let elapsed = 3_660 * 1_000_000_000;
        assert_eq!(grid.last_due_slot(elapsed), elapsed);
        assert_eq!(due(grid, elapsed, elapsed, usize::MAX).count, 1);
        let slow = Grid::new(64, 1);
        assert_eq!(slow.slot_offset_ns(1), 64_000_000_000);
        assert_eq!(
            due(slow, u64::MAX, 0, 64).skipped,
            slow.last_due_slot(u64::MAX)
        );
    }
}
