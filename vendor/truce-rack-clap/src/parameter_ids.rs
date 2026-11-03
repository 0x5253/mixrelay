//! Bounded main-thread parameter metadata and coalesced pending-value cache.

use truce_rack_core::error::{Error, Result};

/// Bound metadata, pending values, and the index list together to 16 MiB per plugin instance.
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const CACHE_ENTRY_BYTES: usize = std::mem::size_of::<CachedParameter>();
const ACTIVE_INDEX_BYTES: usize = std::mem::size_of::<usize>();
pub(crate) const MAX_PARAMETER_IDS: usize =
    MAX_CACHE_BYTES / (CACHE_ENTRY_BYTES + ACTIVE_INDEX_BYTES);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CachedParameter {
    pub(crate) id: u32,
    pub(crate) requires_process: bool,
    pending: bool,
    value: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParameterMetadata {
    pub(crate) id: u32,
    pub(crate) requires_process: bool,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct ParameterIds {
    entries: Vec<CachedParameter>,
    active_indices: Vec<usize>,
    flushable_pending_count: usize,
}

impl ParameterIds {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn id(&self, index: usize) -> Result<u32> {
        self.entries
            .get(index)
            .map(|entry| entry.id)
            .ok_or(Error::InvalidParameter(index))
    }

    pub(crate) fn queue(&mut self, index: usize, value: f64) -> Result<()> {
        let entry = self
            .entries
            .get_mut(index)
            .ok_or(Error::InvalidParameter(index))?;
        if !entry.pending {
            let requires_process = entry.requires_process;
            // The list was reserved for every cache entry, and each entry contributes at most
            // once. This guard makes the no-growth invariant explicit even if that changes.
            if self.active_indices.len() == self.active_indices.capacity() {
                return Err(Error::Other(
                    "CLAP pending parameter index capacity exhausted".into(),
                ));
            }
            self.active_indices.push(index);
            entry.pending = true;
            if !requires_process {
                self.flushable_pending_count += 1;
            }
        }
        entry.value = value;
        Ok(())
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.active_indices.is_empty()
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.active_indices.len()
    }

    pub(crate) fn has_flushable_pending(&self) -> bool {
        self.flushable_pending_count != 0
    }

    pub(crate) fn for_each_pending(&self, mut visit: impl FnMut(u32, f64)) {
        for &index in &self.active_indices {
            let entry = self.entries[index];
            visit(entry.id, entry.value);
        }
    }

    pub(crate) fn for_each_flushable(&self, mut visit: impl FnMut(u32, f64)) {
        for &index in &self.active_indices {
            let entry = self.entries[index];
            if !entry.requires_process {
                visit(entry.id, entry.value);
            }
        }
    }

    /// Drop only values delivered through a synchronous flush; process-required values remain.
    pub(crate) fn clear_flushed(&mut self) {
        let mut retained = 0;
        for position in 0..self.active_indices.len() {
            let index = self.active_indices[position];
            if self.entries[index].requires_process {
                self.active_indices[retained] = index;
                retained += 1;
            } else {
                self.entries[index].pending = false;
            }
        }
        self.active_indices.truncate(retained);
        self.flushable_pending_count = 0;
    }

    /// Clear pending state after a successful native process call or explicit invalidation.
    pub(crate) fn clear_all_pending(&mut self) {
        for &index in &self.active_indices {
            self.entries[index].pending = false;
        }
        self.active_indices.clear();
        self.flushable_pending_count = 0;
    }

    pub(crate) fn collect(
        count: u32,
        mut get_metadata: impl FnMut(u32) -> Result<ParameterMetadata>,
    ) -> Result<Self> {
        let count = usize::try_from(count)
            .map_err(|_| Error::Other("CLAP parameter count does not fit usize".into()))?;
        if count > MAX_PARAMETER_IDS {
            return Err(Error::Other(format!(
                "CLAP parameter cache exceeds 16 MiB: {count} parameters"
            )));
        }

        let mut entries = Vec::new();
        entries.try_reserve_exact(count).map_err(|error| {
            Error::Other(format!("cannot allocate CLAP parameter cache: {error}"))
        })?;
        let mut active_indices = Vec::new();
        active_indices.try_reserve_exact(count).map_err(|error| {
            Error::Other(format!("cannot allocate CLAP pending index cache: {error}"))
        })?;
        let allocated_bytes = entries
            .capacity()
            .checked_mul(CACHE_ENTRY_BYTES)
            .and_then(|bytes| {
                active_indices
                    .capacity()
                    .checked_mul(ACTIVE_INDEX_BYTES)
                    .and_then(|indices| bytes.checked_add(indices))
            })
            .ok_or_else(|| Error::Other("CLAP parameter cache size overflow".into()))?;
        if allocated_bytes > MAX_CACHE_BYTES {
            return Err(Error::Other(format!(
                "allocator capacity exceeds 16 MiB CLAP parameter cache limit: {allocated_bytes} bytes"
            )));
        }

        for index in 0..count {
            let native_index = u32::try_from(index)
                .map_err(|_| Error::Other("CLAP parameter index does not fit u32".into()))?;
            let metadata = get_metadata(native_index)?;
            entries.push(CachedParameter {
                id: metadata.id,
                requires_process: metadata.requires_process,
                pending: false,
                value: 0.0,
            });
        }
        Ok(Self {
            entries,
            active_indices,
            flushable_pending_count: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(index: u32, requires_process: bool) -> ParameterMetadata {
        ParameterMetadata {
            id: 11 + index,
            requires_process,
        }
    }

    #[test]
    fn cache_preserves_native_ids_and_rejects_invalid_indices() {
        let ids = ParameterIds::collect(3, |index| Ok(metadata(index, false))).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids.id(0).unwrap(), 11);
        assert_eq!(ids.id(2).unwrap(), 13);
        assert!(matches!(ids.id(3), Err(Error::InvalidParameter(3))));
    }

    #[test]
    fn cache_rejects_native_count_above_combined_sixteen_mib_budget_before_enumeration() {
        let mut called = false;
        let excessive_count = u32::try_from(MAX_PARAMETER_IDS + 1).unwrap();
        let result = ParameterIds::collect(excessive_count, |_| {
            called = true;
            Ok(metadata(0, false))
        });
        assert!(result.is_err());
        assert!(
            !called,
            "oversized native count must be rejected before enumeration"
        );
    }

    #[test]
    fn cache_surfaces_native_enumeration_failure_without_truncation() {
        let result = ParameterIds::collect(3, |index| {
            if index == 1 {
                Err(Error::Other("fake get_info failure".into()))
            } else {
                Ok(metadata(index, false))
            }
        });
        assert!(matches!(result, Err(Error::Other(message)) if message == "fake get_info failure"));
    }

    #[test]
    fn one_hundred_thousand_updates_coalesce_without_processing_or_capacity_growth() {
        let mut ids = ParameterIds::collect(2, |index| Ok(metadata(index, false))).unwrap();
        let entry_capacity = ids.entries.capacity();
        let active_index_capacity = ids.active_indices.capacity();
        let prepared_capacity_bytes = entry_capacity * std::mem::size_of::<CachedParameter>()
            + active_index_capacity * std::mem::size_of::<usize>();

        for update in 0..100_000_u32 {
            ids.queue(1, f64::from(update)).unwrap();
        }

        assert_eq!(ids.pending_count(), 1);
        let mut pending = Vec::new();
        ids.for_each_pending(|id, value| pending.push((id, value)));
        assert_eq!(pending, [(12, 99_999.0)]);
        assert_eq!(ids.entries.capacity(), entry_capacity);
        assert_eq!(ids.active_indices.capacity(), active_index_capacity);
        let combined_capacity_bytes = ids.entries.capacity()
            * std::mem::size_of::<CachedParameter>()
            + ids.active_indices.capacity() * std::mem::size_of::<usize>();
        assert_eq!(combined_capacity_bytes, prepared_capacity_bytes);
        assert!(combined_capacity_bytes <= MAX_CACHE_BYTES);
    }

    #[test]
    fn flush_clears_only_delivered_values_and_process_clear_clears_the_rest() {
        let mut ids = ParameterIds::collect(2, |index| Ok(metadata(index, index == 1))).unwrap();
        ids.queue(0, 0.4).unwrap();
        ids.queue(1, 0.7).unwrap();
        let mut flushable = Vec::new();
        ids.for_each_flushable(|id, value| flushable.push((id, value)));
        assert_eq!(flushable, [(11, 0.4)]);

        ids.clear_flushed();
        assert_eq!(ids.pending_count(), 1);
        let mut pending = Vec::new();
        ids.for_each_pending(|id, value| pending.push((id, value)));
        assert_eq!(pending, [(12, 0.7)]);

        ids.clear_all_pending();
        assert!(!ids.has_pending());
    }

    #[test]
    fn pending_clear_invalidates_restart_replacement_values() {
        let mut ids = ParameterIds::collect(1, |index| Ok(metadata(index, true))).unwrap();
        ids.queue(0, 0.9).unwrap();
        ids.clear_all_pending();
        assert!(!ids.has_pending());
        assert_eq!(ids.id(0).unwrap(), 11);
    }

    #[test]
    fn every_cache_capacity_stays_inside_combined_sixteen_mib_budget() {
        let ids = ParameterIds::collect(12, |index| Ok(metadata(index, false))).unwrap();
        let bytes = ids.entries.capacity() * CACHE_ENTRY_BYTES
            + ids.active_indices.capacity() * ACTIVE_INDEX_BYTES;
        assert!(bytes <= MAX_CACHE_BYTES);
    }
}
