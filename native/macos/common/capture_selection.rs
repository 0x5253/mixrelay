//! Stable identities shared by the capture producer and graph consumer.
use mixrelay_audio_io::ApplicationSelector;
use sha2::{Digest, Sha256};

/// Returns the native source identity for an exact app or a whole-system exclusion set.
#[must_use]
pub fn source_id(selector: &ApplicationSelector) -> Option<String> {
    match selector {
        ApplicationSelector::BundleId {
            bundle_id,
            include_descendants: false,
        } => Some(bundle_id.clone()),
        ApplicationSelector::AllExcept { bundle_ids } => {
            let mut ids = bundle_ids.iter().collect::<Vec<_>>();
            ids.sort_unstable();
            ids.dedup();
            let mut hash = Sha256::new();
            for id in ids {
                hash.update(id.len().to_le_bytes());
                hash.update(id.as_bytes());
            }
            Some(format!("coreaudio:system:{:x}", hash.finalize()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_scope_is_order_independent_and_distinct_from_app_capture() {
        let selector = |ids: &[&str]| ApplicationSelector::AllExcept {
            bundle_ids: ids.iter().map(|id| (*id).into()).collect(),
        };
        assert_eq!(
            source_id(&selector(&["a", "b", "a"])),
            source_id(&selector(&["b", "a"]))
        );
        assert_ne!(
            source_id(&selector(&["ab", "c"])),
            source_id(&selector(&["a", "bc"]))
        );
        assert_ne!(source_id(&selector(&[])), source_id(&selector(&["a"])));
        assert_ne!(
            source_id(&selector(&["a"])),
            source_id(&ApplicationSelector::BundleId {
                bundle_id: "a".into(),
                include_descendants: false,
            })
        );
    }
}
