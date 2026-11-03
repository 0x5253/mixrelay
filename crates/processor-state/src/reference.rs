use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use crate::{MAX_STATE_BYTES, StateError};

/// A validated SHA-256 content reference to an immutable state blob.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateRef {
    sha256: String,
    byte_length: u64,
}

impl StateRef {
    /// Creates a state reference after validating its digest and declared size.
    ///
    /// # Arguments
    /// * `sha256` - The lowercase hexadecimal SHA-256 digest of the payload.
    /// * `byte_length` - The exact payload length in bytes.
    ///
    /// # Returns
    /// A structurally valid content reference.
    ///
    /// # Errors
    /// Returns [`StateError::InvalidReference`] for a non-lowercase SHA-256 digest or a
    /// byte length larger than [`MAX_STATE_BYTES`].
    pub fn new(sha256: impl Into<String>, byte_length: u64) -> Result<Self, StateError> {
        let reference = Self {
            sha256: sha256.into(),
            byte_length,
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Returns the lowercase hexadecimal SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Returns the exact payload length in bytes.
    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }

    /// Rechecks this value before it crosses a storage or archive boundary.
    ///
    /// # Returns
    /// Returns `Ok(())` when the fields satisfy the reference contract.
    ///
    /// # Errors
    /// Returns [`StateError::InvalidReference`] if either field is invalid.
    pub fn validate(&self) -> Result<(), StateError> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(StateError::InvalidReference {
                reason: "digest must be 64 lowercase hexadecimal characters",
            });
        }
        if self.byte_length > MAX_STATE_BYTES as u64 {
            return Err(StateError::InvalidReference {
                reason: "state exceeds the maximum byte length",
            });
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for StateRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct WireReference {
            sha256: String,
            byte_length: u64,
        }

        let wire = WireReference::deserialize(deserializer)?;
        Self::new(wire.sha256, wire.byte_length).map_err(D::Error::custom)
    }
}
