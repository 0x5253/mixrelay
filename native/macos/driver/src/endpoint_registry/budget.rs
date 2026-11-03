//! Admission counts retained and provisional owners before allocating new storage.
//! This is a product storage bound, not the kernel's process `HighWaterMark`.
use std::io;

pub(super) const MAX_MAPPINGS: usize = 256;
pub(super) const MAX_REGION_BYTES: usize = 1 << 30;

pub(super) struct Budget {
    regions: [usize; MAX_MAPPINGS],
    region_count: usize,
    mappings: usize,
    bytes: usize,
    mapping_limit: usize,
    byte_limit: usize,
}

impl Budget {
    pub(super) fn new(mapping_limit: usize, byte_limit: usize) -> Self {
        Self {
            regions: [0; MAX_MAPPINGS],
            region_count: 0,
            mappings: 0,
            bytes: 0,
            mapping_limit: mapping_limit.min(MAX_MAPPINGS),
            byte_limit,
        }
    }

    // Existing/metadata-only owners share a region key. New allocations have no key.
    pub(super) fn reserve(&mut self, region: Option<usize>, bytes: usize) -> io::Result<()> {
        let mappings = self.mappings.checked_add(1).ok_or_else(full)?;
        let shared = region.is_some_and(|key| self.regions[..self.region_count].contains(&key));
        let total = self
            .bytes
            .checked_add(if shared { 0 } else { bytes })
            .ok_or_else(full)?;
        if mappings > self.mapping_limit || total > self.byte_limit {
            return Err(full());
        }
        if let Some(key) = region.filter(|_| !shared) {
            self.regions[self.region_count] = key;
            self.region_count += 1;
        }
        self.mappings = mappings;
        self.bytes = total;
        Ok(())
    }
}

fn full() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "driver storage budget exhausted; release retired owners before retrying",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_retired_and_provisional_storage_share_one_bound() {
        let mut budget = Budget::new(4, 100);
        budget.reserve(Some(1), 40).unwrap(); // active
        budget.reserve(Some(2), 30).unwrap(); // pinned retired
        budget.reserve(None, 30).unwrap(); // provisional: commit or cancel
        assert_eq!(
            budget.reserve(None, 1).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!((budget.mappings, budget.bytes), (3, 100));
        // Reclaiming a retired owner restores admission; no OS allocation is used.
        let mut reclaimed = Budget::new(4, 100);
        reclaimed.reserve(Some(1), 40).unwrap();
        reclaimed.reserve(None, 60).unwrap();
    }

    #[test]
    fn metadata_owners_reuse_region_bytes_but_still_have_a_count_limit() {
        let mut budget = Budget::new(3, 40);
        for _ in 0..3 {
            budget.reserve(Some(1), 40).unwrap();
        }
        assert_eq!(budget.bytes, 40);
        assert!(budget.reserve(Some(1), 40).is_err());
        assert_eq!(budget.mappings, 3);
    }

    #[test]
    fn rejected_reservations_do_not_consume_capacity_and_overflow_is_rejected() {
        let mut budget = Budget::new(2, usize::MAX);
        budget.reserve(Some(1), usize::MAX).unwrap();
        assert!(budget.reserve(None, 1).is_err());
        budget.reserve(Some(1), usize::MAX).unwrap();
        assert_eq!(budget.bytes, usize::MAX);
        assert_eq!(budget.mappings, 2);
    }

    #[test]
    fn fixed_registry_capacity_also_bounds_unique_region_keys() {
        let mut budget = Budget::new(usize::MAX, usize::MAX);
        for key in 0..MAX_MAPPINGS {
            budget.reserve(Some(key), 1).unwrap();
        }
        assert!(budget.reserve(Some(MAX_MAPPINGS), 1).is_err());
        assert_eq!(budget.region_count, MAX_MAPPINGS);
        assert_eq!(budget.bytes, MAX_MAPPINGS);
    }
}
