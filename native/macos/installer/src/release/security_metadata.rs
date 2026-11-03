//! Structured, slice-specific inspection of macOS code-signing metadata.

use super::{
    ReleaseError,
    policy::{
        Architecture, ArtifactRole, SignatureMetadata, VerificationPolicy, expected_entitlements,
    },
};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    data::CFData,
    date::CFDate,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::{CFString, CFStringRef},
    url::{CFURL, CFURLRef},
};
use std::{collections::BTreeMap, ffi::c_void, path::Path, ptr};

type SecStaticCodeRef = *const c_void;

const STRICT_VALIDATION: u32 = 1 << 4;
const NO_NETWORK_ACCESS: u32 = 1 << 29;
const SIGNING_INFORMATION: u32 = 1 << 1;
const MAX_ENTITLEMENTS: usize = 64;

struct EntitlementInspection {
    count: usize,
    values: BTreeMap<String, Option<bool>>,
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecCodeAttributeArchitecture: CFStringRef;
    static kSecCodeInfoFlags: CFStringRef;
    static kSecCodeInfoTimestamp: CFStringRef;
    static kSecCodeInfoEntitlements: CFStringRef;
    static kSecCodeInfoEntitlementsDict: CFStringRef;

    fn SecStaticCodeCreateWithPathAndAttributes(
        path: CFURLRef,
        flags: u32,
        attributes: CFDictionaryRef,
        static_code: *mut SecStaticCodeRef,
    ) -> i32;
    fn SecStaticCodeCheckValidity(
        static_code: SecStaticCodeRef,
        flags: u32,
        requirement: *const c_void,
    ) -> i32;
    fn SecCodeCopySigningInformation(
        static_code: SecStaticCodeRef,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
}

/// Reads signing flags, secure timestamp, and entitlements for each required Mach-O slice.
pub(super) fn inspect_slices(
    path: &Path,
    role: ArtifactRole,
    policy: VerificationPolicy,
) -> Result<Vec<(Architecture, SignatureMetadata)>, ReleaseError> {
    let mut slices = Vec::with_capacity(Architecture::ALL.len());
    for architecture in Architecture::ALL {
        slices.push((
            architecture,
            inspect_slice(path, role, policy, architecture)?,
        ));
    }
    Ok(slices)
}

fn inspect_slice(
    path: &Path,
    role: ArtifactRole,
    policy: VerificationPolicy,
    architecture: Architecture,
) -> Result<SignatureMetadata, ReleaseError> {
    let url = CFURL::from_path(path, false)
        .ok_or_else(|| metadata_error(path, architecture, "could not create file URL"))?;
    let architecture_key = security_key(
        // SAFETY: Security exports this process-lifetime CFString key.
        unsafe { kSecCodeAttributeArchitecture },
        path,
        architecture,
    )?;
    let architecture_value = CFString::new(architecture.name());
    let attributes = CFDictionary::from_CFType_pairs(&[(architecture_key, architecture_value)]);
    let code = create_static_code(&url, &attributes, path, architecture)?;
    check_static_code(&code, path, architecture)?;
    let information = copy_signing_information(&code, path, architecture)?;
    read_metadata(&information, role, policy, path, architecture)
}

fn create_static_code(
    url: &CFURL,
    attributes: &CFDictionary<CFString, CFString>,
    path: &Path,
    architecture: Architecture,
) -> Result<CFType, ReleaseError> {
    let mut code = ptr::null();
    // SAFETY: the URL and typed attribute dictionary are retained for the call, and the
    // dictionary selects the exact canonical architecture requested. Security returns +1.
    let status = unsafe {
        SecStaticCodeCreateWithPathAndAttributes(
            url.as_concrete_TypeRef(),
            0,
            attributes.as_concrete_TypeRef(),
            &raw mut code,
        )
    };
    if status != 0 {
        return Err(metadata_status(
            path,
            architecture,
            "selecting architecture",
            status,
        ));
    }
    if code.is_null() {
        return Err(metadata_error(
            path,
            architecture,
            "Security returned a null code object",
        ));
    }
    // SAFETY: Security documents the successful output as a retained SecStaticCodeRef.
    Ok(unsafe { CFType::wrap_under_create_rule(code) })
}

fn check_static_code(
    code: &CFType,
    path: &Path,
    architecture: Architecture,
) -> Result<(), ReleaseError> {
    // SAFETY: code owns a retained static code object. Network access is explicitly disabled;
    // the already-bounded codesign child verifies the publisher requirement and all slices.
    let status = unsafe {
        SecStaticCodeCheckValidity(
            code.as_CFTypeRef(),
            STRICT_VALIDATION | NO_NETWORK_ACCESS,
            ptr::null(),
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(metadata_status(
            path,
            architecture,
            "validating signature",
            status,
        ))
    }
}

fn copy_signing_information(
    code: &CFType,
    path: &Path,
    architecture: Architecture,
) -> Result<CFDictionary<CFString, CFType>, ReleaseError> {
    let mut information = ptr::null();
    // SAFETY: code is retained and information is a valid out-pointer. The signing flag
    // requests the certified timestamp along with generic signature metadata.
    let status = unsafe {
        SecCodeCopySigningInformation(
            code.as_CFTypeRef(),
            SIGNING_INFORMATION,
            &raw mut information,
        )
    };
    if status != 0 {
        return Err(metadata_status(
            path,
            architecture,
            "reading signature information",
            status,
        ));
    }
    if information.is_null() {
        return Err(metadata_error(
            path,
            architecture,
            "Security returned null signing information",
        ));
    }
    // SAFETY: Security returned an owned CFDictionaryRef on success; values are dynamically
    // typed CF objects, so the dictionary key/value generics reflect the checked CF boundary.
    Ok(unsafe { CFDictionary::wrap_under_create_rule(information) })
}

fn read_metadata(
    information: &CFDictionary<CFString, CFType>,
    role: ArtifactRole,
    policy: VerificationPolicy,
    path: &Path,
    architecture: Architecture,
) -> Result<SignatureMetadata, ReleaseError> {
    let flags = read_flags(information, path, architecture)?;
    let has_secure_timestamp = read_timestamp(information, path, architecture)?;
    let entitlements = match policy {
        VerificationPolicy::Ownership => EntitlementInspection {
            count: 0,
            values: BTreeMap::new(),
        },
        VerificationPolicy::Launch => read_entitlements(information, role, path, architecture)?,
    };
    Ok(SignatureMetadata {
        flags,
        has_secure_timestamp,
        entitlement_count: entitlements.count,
        entitlements: entitlements.values,
    })
}

fn read_flags(
    information: &CFDictionary<CFString, CFType>,
    path: &Path,
    architecture: Architecture,
) -> Result<u32, ReleaseError> {
    let key = security_key(
        // SAFETY: Security exports this process-lifetime CFString key.
        unsafe { kSecCodeInfoFlags },
        path,
        architecture,
    )?;
    let value = information
        .find(&key)
        .ok_or_else(|| metadata_error(path, architecture, "signature flags are missing"))?;
    let number = value
        .downcast::<CFNumber>()
        .ok_or_else(|| metadata_error(path, architecture, "signature flags are not numeric"))?;
    let flags = number
        .to_i32()
        .ok_or_else(|| metadata_error(path, architecture, "signature flags are unreadable"))?;
    Ok(u32::from_ne_bytes(flags.to_ne_bytes()))
}

fn read_timestamp(
    information: &CFDictionary<CFString, CFType>,
    path: &Path,
    architecture: Architecture,
) -> Result<bool, ReleaseError> {
    let key = security_key(
        // SAFETY: Security exports this process-lifetime CFString key.
        unsafe { kSecCodeInfoTimestamp },
        path,
        architecture,
    )?;
    match information.find(&key) {
        None => Ok(false),
        Some(value) if value.downcast::<CFDate>().is_some() => Ok(true),
        Some(_) => Err(metadata_error(
            path,
            architecture,
            "secure timestamp has an unexpected type",
        )),
    }
}

fn read_entitlements(
    information: &CFDictionary<CFString, CFType>,
    role: ArtifactRole,
    path: &Path,
    architecture: Architecture,
) -> Result<EntitlementInspection, ReleaseError> {
    let dictionary_key = security_key(
        // SAFETY: Security exports this process-lifetime CFString key.
        unsafe { kSecCodeInfoEntitlementsDict },
        path,
        architecture,
    )?;
    let blob_key = security_key(
        // SAFETY: Security exports this process-lifetime CFString key.
        unsafe { kSecCodeInfoEntitlements },
        path,
        architecture,
    )?;
    let blob = information.find(&blob_key);
    if blob
        .as_ref()
        .is_some_and(|value| value.downcast::<CFData>().is_none())
    {
        return Err(metadata_error(
            path,
            architecture,
            "entitlement blob has an unexpected type",
        ));
    }
    let Some(raw_dictionary) = information.find(&dictionary_key) else {
        if blob.is_some() {
            return Err(metadata_error(
                path,
                architecture,
                "entitlements are present but not a standard dictionary",
            ));
        }
        return Ok(EntitlementInspection {
            count: 0,
            values: BTreeMap::new(),
        });
    };
    let dictionary = wrap_entitlement_dictionary(&raw_dictionary, path, architecture)?;
    let count = dictionary.len();
    if count > MAX_ENTITLEMENTS {
        return Err(metadata_error(
            path,
            architecture,
            "too many embedded entitlements",
        ));
    }
    let mut entitlements = BTreeMap::new();
    for (name, _) in expected_entitlements(role) {
        let key = CFString::new(name);
        let value = dictionary
            .find(&key)
            .and_then(|item| item.downcast::<CFBoolean>())
            .map(bool::from);
        entitlements.insert((*name).to_owned(), value);
    }
    Ok(EntitlementInspection {
        count,
        values: entitlements,
    })
}

fn wrap_entitlement_dictionary(
    value: &core_foundation::base::ItemRef<'_, CFType>,
    path: &Path,
    architecture: Architecture,
) -> Result<CFDictionary<CFString, CFType>, ReleaseError> {
    if value.downcast::<CFDictionary>().is_none() {
        return Err(metadata_error(
            path,
            architecture,
            "entitlements are not a dictionary",
        ));
    }
    // SAFETY: `value` is borrowed from the retained signing-information dictionary, and the
    // checked Security key denotes a CFDictionary. wrap_under_get_rule retains the reference.
    Ok(unsafe { CFDictionary::wrap_under_get_rule(value.as_CFTypeRef().cast()) })
}

fn security_key(
    value: CFStringRef,
    path: &Path,
    architecture: Architecture,
) -> Result<CFString, ReleaseError> {
    if value.is_null() {
        return Err(metadata_error(
            path,
            architecture,
            "Security metadata key is null",
        ));
    }
    // SAFETY: Security's exported keys are process-lifetime CFStrings; get-rule wrapping
    // retains one reference and CFString owns/relinquishes that reference.
    Ok(unsafe { CFString::wrap_under_get_rule(value) })
}

fn metadata_error(path: &Path, architecture: Architecture, status: &str) -> ReleaseError {
    ReleaseError::SignatureMetadata {
        path: path.to_path_buf(),
        architecture: architecture.name(),
        status: status.to_owned(),
    }
}

fn metadata_status(
    path: &Path,
    architecture: Architecture,
    operation: &str,
    status: i32,
) -> ReleaseError {
    ReleaseError::SignatureMetadata {
        path: path.to_path_buf(),
        architecture: architecture.name(),
        status: format!("{operation} returned OSStatus {status}"),
    }
}

#[cfg(test)]
mod tests;
