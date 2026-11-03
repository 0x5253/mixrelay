//! Content-addressed processor state references embedded in portable workspaces.

use std::collections::{BTreeMap, BTreeSet};

use mixrelay_processor_state::StateRef;

use super::{Catalog, NodeConfig, ProtocolError, invalid};

/// Collects the unique state blobs authored by provider nodes in digest order.
///
/// Repeated references are allowed when providers share an identical immutable blob.
/// Inline state is rejected because it cannot participate in a portable bundle.
pub(super) fn referenced(catalog: &Catalog) -> Result<Vec<StateRef>, ProtocolError> {
    let mut references = BTreeMap::<String, u64>::new();
    for (_, document) in catalog.documents() {
        for node in &document.nodes {
            let NodeConfig::Provider(provider) = &node.config else {
                continue;
            };
            if provider.parameters.get("stateBase64").is_some() {
                return Err(invalid(
                    "inline provider stateBase64 is unsupported; use a portable stateRef",
                ));
            }
            let Some(value) = provider.parameters.get("stateRef") else {
                continue;
            };
            let reference: StateRef = serde_json::from_value(value.clone())
                .map_err(|_| invalid("provider stateRef is malformed"))?;
            let digest = reference.sha256().to_owned();
            let byte_length = reference.byte_length();
            if references
                .insert(digest, byte_length)
                .is_some_and(|previous| previous != byte_length)
            {
                return Err(invalid("provider stateRefs disagree on blob length"));
            }
        }
    }
    references
        .into_iter()
        .map(|(sha256, byte_length)| {
            StateRef::new(sha256, byte_length)
                .map_err(|_| invalid("provider stateRef is malformed"))
        })
        .collect()
}

/// Fully verifies references that are not unchanged and already trusted in this process.
///
/// A verified reference is reusable only while it remains in the currently accepted catalog.
/// The returned set contains exactly the candidate references and is produced only after every
/// required verification succeeds, so callers can replace their bounded cache transactionally.
/// Store writes never overwrite immutable blobs. External in-place edits are not covered by
/// this metadata-write cache; every worker restore and portable export still hashes all bytes.
pub(super) fn verify_catalog_delta<E>(
    candidate: &[StateRef],
    accepted: &[StateRef],
    verified: &BTreeSet<StateRef>,
    mut verify: impl FnMut(&StateRef) -> Result<(), E>,
) -> Result<BTreeSet<StateRef>, E> {
    let accepted = accepted.iter().collect::<BTreeSet<_>>();
    for reference in candidate {
        if accepted.contains(&reference) && verified.contains(reference) {
            continue;
        }
        verify(reference)?;
    }
    Ok(candidate.iter().cloned().collect())
}

/// Requires bundle assets to be the exact unique set referenced by the catalog.
pub(super) fn require_exact(
    references: &[StateRef],
    bundled: &[StateRef],
) -> Result<(), ProtocolError> {
    let expected = reference_map(references, "catalog")?;
    let actual = reference_map(bundled, "bundle")?;
    if expected != actual {
        return Err(invalid(
            "workspace bundle assets do not exactly match provider stateRefs",
        ));
    }
    Ok(())
}

fn reference_map<'a>(
    references: &'a [StateRef],
    source: &str,
) -> Result<BTreeMap<&'a str, u64>, ProtocolError> {
    let mut result = BTreeMap::new();
    for reference in references {
        let digest = reference.sha256();
        let byte_length = reference.byte_length();
        if result.insert(digest, byte_length).is_some() {
            return Err(invalid(&format!(
                "workspace {source} contains a duplicate state asset"
            )));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::{GraphDocument, NodeConfig, NodeId, NodeSpec, ProviderNodeConfig};

    #[test]
    fn provider_without_native_state_contributes_no_asset() {
        let catalog = Catalog::imported(GraphDocument::new(
            1,
            vec![NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: NodeId::from("provider"),
                label: None,
                config: NodeConfig::Provider(ProviderNodeConfig {
                    type_id: "vst3:missing".into(),
                    ports: None,
                    parameters: serde_json::json!({"parameterValues":{"7":0.5}}),
                }),
            }],
            vec![],
        ));

        assert!(referenced(&catalog).unwrap().is_empty());
    }

    #[test]
    fn bundle_asset_set_rejects_duplicate_digests() {
        let reference = StateRef::new("0".repeat(64), 0).unwrap();

        let duplicated = [reference.clone(), reference.clone()];
        assert!(require_exact(std::slice::from_ref(&reference), &duplicated).is_err());
    }

    #[test]
    fn loaded_state_refs_are_verified_once_then_skipped_for_metadata_writes() {
        const BLOB_BYTES: u64 = 9 * 1024 * 1024;

        let accepted = (1..=3)
            .map(|digest| StateRef::new(format!("{digest:064x}"), BLOB_BYTES).unwrap())
            .collect::<Vec<_>>();
        let candidate = accepted.clone();
        let mut verified = BTreeSet::new();
        let mut hash_calls = 0;
        let mut bytes_read = 0;

        verified = verify_catalog_delta(&candidate, &accepted, &verified, |reference| {
            hash_calls += 1;
            bytes_read += reference.byte_length();
            Ok::<(), ()>(())
        })
        .expect("initial catalog refs are verified");

        assert_eq!(hash_calls, 3);
        assert_eq!(bytes_read, 3 * BLOB_BYTES);

        for _ in 0..2 {
            verified = verify_catalog_delta(&candidate, &accepted, &verified, |reference| {
                hash_calls += 1;
                bytes_read += reference.byte_length();
                Ok::<(), ()>(())
            })
            .expect("unchanged catalog refs remain verified");
        }

        assert_eq!(hash_calls, 3);
        assert_eq!(bytes_read, 3 * BLOB_BYTES);
        assert_eq!(verified, accepted.into_iter().collect());
    }

    #[test]
    fn changed_digest_or_length_requires_full_verification() {
        let original = StateRef::new("1".repeat(64), 128).unwrap();
        let changed_digest = StateRef::new("2".repeat(64), 128).unwrap();
        let changed_length = StateRef::new("1".repeat(64), 129).unwrap();
        let accepted = vec![original.clone()];
        let verified = BTreeSet::from([original]);
        let candidate = vec![changed_digest, changed_length];
        let mut verified_refs = Vec::new();

        let next = verify_catalog_delta(&candidate, &accepted, &verified, |reference| {
            verified_refs.push(reference.clone());
            Ok::<(), ()>(())
        })
        .expect("new refs are verified");

        assert_eq!(verified_refs, candidate);
        assert_eq!(next, candidate.into_iter().collect());
    }

    #[test]
    fn failed_verification_returns_no_replacement_trust_set() {
        let original = StateRef::new("1".repeat(64), 128).unwrap();
        let replacement = StateRef::new("2".repeat(64), 256).unwrap();
        let accepted = vec![original.clone()];
        let verified = BTreeSet::from([original.clone()]);
        let candidate = vec![replacement.clone()];
        let original_verified = verified.clone();

        let result = verify_catalog_delta(&candidate, &accepted, &verified, |_| {
            Err("corrupt or missing blob")
        });

        assert_eq!(result, Err("corrupt or missing blob"));
        assert_eq!(verified, original_verified);
        assert!(!verified.contains(&replacement));
    }
}
