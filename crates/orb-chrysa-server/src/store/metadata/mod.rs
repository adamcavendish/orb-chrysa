//! Metadata store traits and types.
//!
//! This module is split into three sub-modules for maintainability:
//! - `types`: domain types (ManifestEntry, MirrorRule, SyncJob, etc.)
//! - `traits`: trait definitions (ManifestStore, MirrorConfigStore, etc.)
//! - `in_memory`: test-only InMemoryMetadataStore implementation

pub mod traits;
pub mod types;

#[cfg(test)]
pub(crate) mod in_memory;

// Re-export everything so existing `use crate::store::metadata::*` imports
// continue to work without changes.
pub use traits::*;
pub use types::*;

#[cfg(test)]
pub use in_memory::InMemoryMetadataStore;
