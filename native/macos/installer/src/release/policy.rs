//! Signature policies for owned installed code and code approved for launch.
//!
//! Both require Universal 2 slices, Hardened Runtime and secure timestamps after
//! signature integrity and publisher verification. Ownership ignores entitlements;
//! launch additionally requires the artifact role's exact entitlement allowlist.

#[cfg(any(target_os = "macos", test))]
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArtifactRole {
    Application,
    Engine,
    Hal,
    Driver,
    Installer,
    Uninstaller,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum VerificationPolicy {
    Ownership,
    Launch,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Architecture {
    Arm64,
    X86_64,
}

#[cfg(any(target_os = "macos", test))]
impl Architecture {
    pub(super) const ALL: [Self; 2] = [Self::Arm64, Self::X86_64];

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::X86_64 => "x86_64",
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SignatureMetadata {
    pub(super) flags: u32,
    pub(super) has_secure_timestamp: bool,
    pub(super) entitlement_count: usize,
    pub(super) entitlements: BTreeMap<String, Option<bool>>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PolicyViolation {
    ArchitectureSet,
    HardenedRuntime,
    SecureTimestamp,
    Entitlements,
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn verify_slices(
    role: ArtifactRole,
    policy: VerificationPolicy,
    slices: &[(Architecture, SignatureMetadata)],
) -> Result<(), (Architecture, PolicyViolation)> {
    if slices.len() != Architecture::ALL.len()
        || Architecture::ALL.iter().any(|expected| {
            slices
                .iter()
                .filter(|(actual, _)| actual == expected)
                .count()
                != 1
        })
    {
        return Err((Architecture::Arm64, PolicyViolation::ArchitectureSet));
    }

    for (architecture, metadata) in slices {
        verify_slice(role, policy, metadata).map_err(|violation| (*architecture, violation))?;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn verify_slice(
    role: ArtifactRole,
    policy: VerificationPolicy,
    metadata: &SignatureMetadata,
) -> Result<(), PolicyViolation> {
    if metadata.flags & CODE_SIGNATURE_RUNTIME == 0 {
        return Err(PolicyViolation::HardenedRuntime);
    }
    if !metadata.has_secure_timestamp {
        return Err(PolicyViolation::SecureTimestamp);
    }
    if policy == VerificationPolicy::Ownership {
        return Ok(());
    }
    let expected = expected_entitlements(role);
    if metadata.entitlement_count != expected.len()
        || metadata.entitlements.len() != expected.len()
        || expected
            .iter()
            .any(|(key, value)| metadata.entitlements.get(*key) != Some(&Some(*value)))
    {
        return Err(PolicyViolation::Entitlements);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
pub(super) const fn expected_entitlements(role: ArtifactRole) -> &'static [(&'static str, bool)] {
    match role {
        ArtifactRole::Application => &[("com.apple.security.device.audio-input", true)],
        ArtifactRole::Engine => &[
            ("com.apple.security.device.audio-input", true),
            ("com.apple.security.cs.disable-library-validation", true),
        ],
        _ => &[],
    }
}

#[cfg(any(target_os = "macos", test))]
const CODE_SIGNATURE_RUNTIME: u32 = 0x0001_0000;

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(role: ArtifactRole) -> SignatureMetadata {
        SignatureMetadata {
            flags: CODE_SIGNATURE_RUNTIME,
            has_secure_timestamp: true,
            entitlement_count: expected_entitlements(role).len(),
            entitlements: expected_entitlements(role)
                .iter()
                .map(|(key, value)| ((*key).to_owned(), Some(*value)))
                .collect(),
        }
    }

    fn universal(role: ArtifactRole) -> Vec<(Architecture, SignatureMetadata)> {
        Architecture::ALL
            .into_iter()
            .map(|architecture| (architecture, metadata(role)))
            .collect()
    }

    #[test]
    fn requires_hardened_runtime_on_both_architectures() {
        let mut slices = universal(ArtifactRole::Application);
        slices[1].1.flags = 0;
        assert_eq!(
            verify_slices(
                ArtifactRole::Application,
                VerificationPolicy::Launch,
                &slices
            ),
            Err((Architecture::X86_64, PolicyViolation::HardenedRuntime))
        );
    }

    #[test]
    fn requires_secure_timestamp_on_both_architectures() {
        let mut slices = universal(ArtifactRole::Application);
        slices[0].1.has_secure_timestamp = false;
        assert_eq!(
            verify_slices(
                ArtifactRole::Application,
                VerificationPolicy::Launch,
                &slices
            ),
            Err((Architecture::Arm64, PolicyViolation::SecureTimestamp))
        );
    }

    #[test]
    fn rejects_wrong_entitlement_values_and_extra_dangerous_keys() {
        let mut wrong_value = universal(ArtifactRole::Driver);
        wrong_value[0].1.entitlements.insert(
            "com.apple.security.device.audio-input".to_owned(),
            Some(false),
        );
        assert_eq!(
            verify_slices(
                ArtifactRole::Driver,
                VerificationPolicy::Launch,
                &wrong_value
            ),
            Err((Architecture::Arm64, PolicyViolation::Entitlements))
        );

        let mut extra = universal(ArtifactRole::Driver);
        extra[1]
            .1
            .entitlements
            .insert("com.apple.security.cs.allow-jit".to_owned(), Some(true));
        extra[1].1.entitlement_count += 1;
        assert_eq!(
            verify_slices(ArtifactRole::Driver, VerificationPolicy::Launch, &extra),
            Err((Architecture::X86_64, PolicyViolation::Entitlements))
        );
    }

    #[test]
    fn accepts_only_the_exact_role_entitlement_sets() {
        assert_eq!(
            verify_slices(
                ArtifactRole::Driver,
                VerificationPolicy::Launch,
                &universal(ArtifactRole::Driver)
            ),
            Ok(())
        );
        for role in [
            ArtifactRole::Application,
            ArtifactRole::Engine,
            ArtifactRole::Hal,
            ArtifactRole::Installer,
        ] {
            assert_eq!(
                verify_slices(role, VerificationPolicy::Launch, &universal(role)),
                Ok(())
            );
            let mut forbidden = universal(role);
            forbidden[0]
                .1
                .entitlements
                .insert("com.apple.security.cs.debugger".to_owned(), Some(true));
            assert_eq!(
                verify_slices(role, VerificationPolicy::Launch, &forbidden),
                Err((Architecture::Arm64, PolicyViolation::Entitlements))
            );
        }
    }

    #[test]
    fn shipped_app_audio_input_entitlement_is_accepted_without_engine_exemptions() {
        let configured = plist::Value::from_reader_xml(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../apps/desktop/src-tauri/Entitlements.plist"
            ))
            .as_slice(),
        )
        .unwrap()
        .into_dictionary()
        .unwrap();
        let actual = SignatureMetadata {
            flags: CODE_SIGNATURE_RUNTIME,
            has_secure_timestamp: true,
            entitlement_count: configured.len(),
            entitlements: configured
                .into_iter()
                .map(|(key, value)| (key, value.as_boolean()))
                .collect(),
        };
        let mut slices = Architecture::ALL
            .into_iter()
            .map(|architecture| (architecture, actual.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            verify_slices(
                ArtifactRole::Application,
                VerificationPolicy::Launch,
                &slices
            ),
            Ok(())
        );
        slices[0].1.entitlements.insert(
            "com.apple.security.cs.disable-library-validation".into(),
            Some(true),
        );
        slices[0].1.entitlement_count += 1;
        assert_eq!(
            verify_slices(
                ArtifactRole::Application,
                VerificationPolicy::Launch,
                &slices
            ),
            Err((Architecture::Arm64, PolicyViolation::Entitlements))
        );
    }

    #[test]
    fn rejects_missing_duplicate_or_unexpected_architecture_slices() {
        let only_one = vec![(Architecture::Arm64, metadata(ArtifactRole::Application))];
        assert_eq!(
            verify_slices(
                ArtifactRole::Application,
                VerificationPolicy::Launch,
                &only_one
            ),
            Err((Architecture::Arm64, PolicyViolation::ArchitectureSet))
        );

        let duplicate = vec![
            (Architecture::Arm64, metadata(ArtifactRole::Application)),
            (Architecture::Arm64, metadata(ArtifactRole::Application)),
        ];
        assert_eq!(
            verify_slices(
                ArtifactRole::Application,
                VerificationPolicy::Launch,
                &duplicate
            ),
            Err((Architecture::Arm64, PolicyViolation::ArchitectureSet))
        );
    }
}
