//! Where bundle payloads come from and go to. The bundle format depends only on these
//! ports; [`StateStore`](crate::StateStore) implements both on the filesystem.
use std::io::{Read, Write};

use crate::{StateError, StateRef};

/// Verified payloads a bundle is written from.
pub trait StateBlobSource {
    /// Fails unless the payload `reference` names is present and intact.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::MissingBlob`], [`StateError::Integrity`] or any other
    /// [`StateError`] from the backing storage.
    fn verify(&self, reference: &StateRef) -> Result<(), StateError>;

    /// Streams the payload to `writer`, verifying it against `reference` as it goes.
    ///
    /// # Errors
    ///
    /// Returns the failures of [`verify`](Self::verify) or a write failure. On error
    /// `writer` may already hold part of the payload.
    fn copy_verified(&self, reference: &StateRef, writer: &mut dyn Write)
    -> Result<(), StateError>;
}

/// Keeps a bundle's payloads as they are read.
pub trait StateBlobSink {
    /// Reads exactly `reference.byte_length()` bytes from `reader` and keeps them if they
    /// hash to `reference`; nothing is kept otherwise. It must read the whole payload even
    /// when it already holds it, because the bundle continues after it.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Integrity`] for a short or mismatched payload, or any other
    /// [`StateError`] from the backing storage.
    fn store_verified(&self, reader: &mut dyn Read, reference: &StateRef)
    -> Result<(), StateError>;
}
