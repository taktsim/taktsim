//! How many replications run at once (ADR-0025 §8, ADR-0010). The first replication
//! that actually runs goes alone; its fold gives an estimate per replication, and the
//! count is the smallest of the cores, `--threads` and `--max-memory-mib` over that
//! estimate, and at least one. A wrong estimate costs speed or memory, never a byte of
//! the result: no thread count reaches a document.

use std::num::{NonZeroU64, NonZeroUsize};

/// What a replication takes beyond what it holds per item, in bytes.
const BASE: u64 = 16 * MIB;
/// What the fold keeps per delivered unit: its lead time, in bytes.
const PER_DELIVERED: u64 = 8;
/// What a run holds per item alive at once, in bytes.
const PER_ALIVE: u64 = 128;
/// Bytes in a MiB.
const MIB: u64 = 1024 * 1024;
/// The memory replications at once may take without `--max-memory-mib`: the spec's
/// target (ADR-0010).
pub(crate) const DEFAULT_MAX_MEMORY_MIB: u64 = 2048;

/// The estimated memory of one replication that delivered `delivered` units with at
/// most `peak_alive` items alive at once, in bytes.
pub(crate) const fn estimate(delivered: u64, peak_alive: u64) -> u64 {
    BASE.saturating_add(PER_DELIVERED.saturating_mul(delivered))
        .saturating_add(PER_ALIVE.saturating_mul(peak_alive))
}

/// How many replications run at once: the smallest of `cores`, `threads` and the
/// memory budget over `estimate` bytes, and at least one.
pub(crate) fn parallel(
    cores: NonZeroUsize,
    threads: Option<NonZeroUsize>,
    max_memory_mib: Option<NonZeroU64>,
    estimate: u64,
) -> NonZeroUsize {
    let budget = max_memory_mib
        .map_or(DEFAULT_MAX_MEMORY_MIB, NonZeroU64::get)
        .saturating_mul(MIB);
    let by_memory = usize::try_from(budget / estimate.max(1)).unwrap_or(usize::MAX);
    let wanted = threads.map_or(cores, |threads| threads.min(cores));
    NonZeroUsize::new(by_memory.min(wanted.get())).unwrap_or(NonZeroUsize::MIN)
}

/// The most threads a pool needs before the estimate is known.
pub(crate) fn pool_size(cores: NonZeroUsize, threads: Option<NonZeroUsize>) -> NonZeroUsize {
    threads.map_or(cores, |threads| threads.min(cores))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn n(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    #[test]
    fn the_estimate_of_adr_0025() {
        assert_eq!(estimate(0, 0), 16 * MIB);
        // 1,000 delivered and 50 alive at once: 16 MiB + 8,000 B + 6,400 B.
        assert_eq!(estimate(1_000, 50), 16 * MIB + 8_000 + 6_400);
        assert_eq!(estimate(u64::MAX, u64::MAX), u64::MAX, "saturates");
    }

    #[test]
    fn the_smallest_of_three_and_at_least_one() {
        let small = estimate(0, 0);
        // 2048 MiB over 16 MiB is 128: the cores decide.
        assert_eq!(parallel(n(8), None, None, small), n(8));
        assert_eq!(parallel(n(8), Some(n(3)), None, small), n(3));
        assert_eq!(parallel(n(8), Some(n(64)), None, small), n(8));
        // 40 MiB over 16 MiB is 2.
        assert_eq!(parallel(n(8), None, NonZeroU64::new(40), small), n(2));
        // Less than one replication's estimate: still one.
        assert_eq!(parallel(n(8), None, NonZeroU64::new(1), small), n(1));
        assert_eq!(parallel(n(8), None, None, u64::MAX), n(1));
        assert_eq!(pool_size(n(4), None), n(4));
        assert_eq!(pool_size(n(4), Some(n(2))), n(2));
        assert_eq!(pool_size(n(4), Some(n(9))), n(4));
    }

    proptest! {
        #[test]
        fn never_above_the_cores_or_threads(
            cores in 1_usize..64,
            threads in proptest::option::of(1_usize..64),
            mib in proptest::option::of(1_u64..1_000_000),
            delivered in any::<u64>(),
            alive in any::<u64>(),
        ) {
            let count = parallel(
                n(cores),
                threads.map(n),
                mib.and_then(NonZeroU64::new),
                estimate(delivered, alive),
            );
            prop_assert!(count.get() <= cores);
            prop_assert!(threads.is_none_or(|t| count.get() <= t));
            prop_assert!(count <= pool_size(n(cores), threads.map(n)));
        }
    }
}
