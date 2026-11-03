//! Immutable, content-addressed storage for processor state and portable bundles.

mod blobs;
mod bundle;
mod error;
mod payload;
mod reference;
mod store;

pub use blobs::{StateBlobSink, StateBlobSource};
pub use bundle::{
    MAX_BUNDLE_MANIFEST_BYTES, MAX_BUNDLE_PAYLOAD_BYTES, MAX_BUNDLE_REFERENCES, preview_bundle,
    read_bundle, write_bundle,
};
pub use error::StateError;
pub use reference::StateRef;
pub use store::StateStore;

/// Maximum size of one processor-state blob.
pub const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;
