//! Metadata policy checks use synthetic CoreFoundation values without signed files.

use super::*;

#[test]
fn ownership_does_not_parse_entitlements_against_the_launch_schema() {
    // SAFETY: these exported Security.framework keys are process-lifetime CFStrings.
    let flags = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoFlags) };
    // SAFETY: same exported-key lifetime as above.
    let timestamp = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoTimestamp) };
    // SAFETY: same exported-key lifetime as above.
    let entitlements = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoEntitlementsDict) };
    let information = CFDictionary::from_CFType_pairs(&[
        (flags, CFNumber::from(0x0001_0000_i32).as_CFType()),
        (timestamp, CFDate::new(0.0).as_CFType()),
        (
            entitlements,
            CFString::new("different entitlement encoding").as_CFType(),
        ),
    ]);
    let path = Path::new("synthetic");
    let metadata = read_metadata(
        &information,
        ArtifactRole::Application,
        VerificationPolicy::Ownership,
        path,
        Architecture::Arm64,
    )
    .unwrap();
    assert_eq!(
        super::super::policy::verify_slices(
            ArtifactRole::Application,
            VerificationPolicy::Ownership,
            &Architecture::ALL.map(|architecture| (architecture, metadata.clone())),
        ),
        Ok(())
    );
    assert!(matches!(
        read_metadata(
            &information,
            ArtifactRole::Application,
            VerificationPolicy::Launch,
            path,
            Architecture::Arm64,
        ),
        Err(ReleaseError::SignatureMetadata { .. })
    ));
}
