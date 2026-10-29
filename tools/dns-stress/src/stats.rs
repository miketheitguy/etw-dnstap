//! Fixed-size latency histogram and serializable cumulative counters.

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Counts {
    pub(crate) sent: u64,
    pub(crate) completed: u64,
    pub(crate) successful: u64,
    pub(crate) timeouts: u64,
    pub(crate) send_errors: u64,
    pub(crate) receive_errors: u64,
    pub(crate) malformed: u64,
    pub(crate) mismatched: u64,
    pub(crate) unsolicited: u64,
    pub(crate) late: u64,
    pub(crate) duplicate: u64,
    pub(crate) bytes_sent: u64,
    pub(crate) bytes_received: u64,
    pub(crate) aa_missing: u64,
    pub(crate) ra_missing: u64,
    pub(crate) rd_mismatch: u64,
    pub(crate) truncated: u64,
    pub(crate) noerror_without_answers: u64,
    pub(crate) rcode: [u64; 16],
    pub(crate) peak_outstanding: usize,
    pub(crate) sockets_used: usize,
    pub(crate) rotation_count: u64,
    pub(crate) rotation_wait_us: u64,
}

impl Counts {
    pub(crate) fn merge(&mut self, other: &Self) {
        macro_rules! add {
            ($($field:ident),*) => { $(self.$field += other.$field;)* };
        }
        add!(
            sent,
            completed,
            successful,
            timeouts,
            send_errors,
            receive_errors,
            malformed,
            mismatched,
            unsolicited,
            late,
            duplicate,
            bytes_sent,
            bytes_received,
            aa_missing,
            ra_missing,
            rd_mismatch,
            truncated,
            noerror_without_answers,
            sockets_used,
            rotation_count,
            rotation_wait_us
        );
        for (left, right) in self.rcode.iter_mut().zip(other.rcode) {
            *left += right;
        }
        // Sum of worker peaks is an upper bound, not a simultaneous peak.
        self.peak_outstanding += other.peak_outstanding;
    }
}

#[derive(Debug)]
pub(crate) struct Histogram {
    pub(crate) bins: Vec<u64>,
    count: u64,
    max: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Latency {
    samples: u64,
    p50_ns_upper: Option<u64>,
    p95_ns_upper: Option<u64>,
    p99_ns_upper: Option<u64>,
    max_ns: Option<u64>,
}

impl Histogram {
    pub(crate) fn new() -> Self {
        Self {
            bins: vec![0; 2048],
            count: 0,
            max: 0,
        }
    }

    pub(crate) fn record(&mut self, nanos: u64) {
        let value = nanos.max(1);
        let power = value.ilog2();
        let base = 1_u64 << power;
        // The fraction is always in 0..32.
        let fraction =
            usize::try_from((u128::from(value - base) * 32) / u128::from(base)).unwrap_or(31);
        self.bins[power as usize * 32 + fraction] += 1;
        self.count += 1;
        self.max = self.max.max(nanos);
    }

    fn quantile(&self, percent: u64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let rank = self.count.saturating_mul(percent).div_ceil(100);
        let mut seen = 0;
        for (index, count) in self.bins.iter().enumerate() {
            seen += count;
            if seen >= rank {
                let base = 1_u128 << (index / 32);
                let upper = base + (base * (index % 32 + 1) as u128).div_ceil(32) - 1;
                return Some(u64::try_from(upper).unwrap_or(u64::MAX).min(self.max));
            }
        }
        None
    }

    pub(crate) fn snapshot(&self) -> Latency {
        Latency {
            samples: self.count,
            p50_ns_upper: self.quantile(50),
            p95_ns_upper: self.quantile(95),
            p99_ns_upper: self.quantile(99),
            max_ns: (self.count != 0).then_some(self.max),
        }
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        for (left, right) in self.bins.iter_mut().zip(&other.bins) {
            *left += right;
        }
        self.count += other.count;
        self.max = self.max.max(other.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn histogram_quantile_contains_true_rank() {
        let mut histogram = Histogram::new();
        assert!(histogram.quantile(50).is_none());
        for value in 1..=100_000 {
            histogram.record(value);
        }
        for percentile in [50, 95, 99] {
            let bound = histogram.quantile(percentile).unwrap_or(0);
            let exact = percentile * 1000;
            assert!(bound >= exact && bound <= exact + exact / 32 + 1);
        }
    }
}
