use super::*;
use std::sync::Arc;

impl InstanceIds {
    /// Identifiers with only `u64::MAX` left to issue.
    pub(crate) fn final_available() -> Self {
        Self {
            next: AtomicU64::new(u64::MAX),
        }
    }
}

#[test]
fn final_identifier_is_issued_once_and_exhaustion_never_wraps_to_zero() {
    let ids = InstanceIds {
        next: AtomicU64::new(u64::MAX - 1),
    };
    assert_eq!(ids.issue().unwrap().get(), u64::MAX - 1);
    assert_eq!(ids.issue().unwrap().get(), u64::MAX);
    assert!(ids.issue().is_err());
    assert!(ids.issue().is_err());
    assert!(InstanceId::new(0).is_none());
}

#[test]
fn concurrent_issuers_share_one_order_without_duplicate_or_zero_identities() {
    let ids = Arc::new(InstanceIds::new());
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let ids = Arc::clone(&ids);
            std::thread::spawn(move || {
                (0..1024)
                    .map(|_| ids.issue().unwrap().get())
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut issued: Vec<_> = handles
        .into_iter()
        .flat_map(|handle| handle.join().unwrap())
        .collect();
    issued.sort_unstable();
    assert_eq!(issued, (1..=8192).collect::<Vec<_>>());
}
