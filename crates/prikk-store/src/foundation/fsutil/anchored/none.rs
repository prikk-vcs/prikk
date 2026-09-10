//! DC-82: the implementor for every platform without a real `DurabilityContract` — every method
//! returns the same "unsupported" error the free-function dispatch in `anchored.rs` used to construct
//! directly, once per `#[cfg(not(any(target_os = "linux", target_os = "macos")))]` arm. Making
//! "unsupported" an implementor rather than a tenth arm at every call site is the whole point of this
//! increment: one gated constant (`anchored.rs`'s `ACTIVE_DURABILITY`) now picks the active
//! implementor, and every call site becomes unconditional.
//!
//! Gated `#[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]`, not just
//! `not(any(...))`: the `test` arm exists so DC-82 §3's requirement — "mutation fails at runtime, not
//! a compile error" — is a real, host-executed test, not an assertion resting on cross-target
//! `clippy`/`build` alone (`tests.rs`'s `no_durability_*` tests construct `NoDurability` directly and
//! observe its methods return `Err`, on whichever platform the gate set is run). The production
//! dispatch (`anchored.rs`'s `ACTIVE_DURABILITY`) is unaffected by this widening: it still only
//! selects `NoDurability` when genuinely compiled for neither Linux nor macOS.

use std::path::Path;

use prikk_error::Result;

use super::directory::MutationRoot;
use super::unsupported_mutation;
use crate::foundation::fsutil::contract::DurabilityContract;

/// Zero-sized, matching `LinuxDurability`/`MacosDurability`'s shape exactly.
pub(in crate::foundation::fsutil) struct NoDurability;

impl DurabilityContract for NoDurability {
    fn atomic_replace(&self, root: &MutationRoot, relative: &Path, bytes: &[u8]) -> Result<()> {
        let _ = (root, relative, bytes);
        unsupported_mutation()
    }

    fn durable_append(&self, root: &MutationRoot, relative: &Path, bytes: &[u8]) -> Result<()> {
        let _ = (root, relative, bytes);
        unsupported_mutation()
    }

    fn durable_truncate(&self, root: &MutationRoot, relative: &Path, len: u64) -> Result<()> {
        let _ = (root, relative, len);
        unsupported_mutation()
    }

    fn durable_truncate_to_empty(&self, root: &MutationRoot, relative: &Path) -> Result<()> {
        let _ = (root, relative);
        unsupported_mutation()
    }

    fn create_exclusive(
        &self,
        root: &MutationRoot,
        relative: &Path,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        let _ = (root, relative, bytes);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "repository mutation requires Linux, macOS, or Windows anchored filesystem primitives",
        ))
    }

    fn set_permission_bits(&self, root: &MutationRoot, relative: &Path, mode: u32) -> Result<()> {
        let _ = (root, relative, mode);
        unsupported_mutation()
    }

    fn remove_if_present(&self, root: &MutationRoot, relative: &Path) -> Result<bool> {
        let _ = (root, relative);
        unsupported_mutation()
    }

    fn ensure_directory(&self, root: &MutationRoot, relative: &Path) -> Result<()> {
        let _ = (root, relative);
        unsupported_mutation()
    }

    fn durable_directory_entry(&self, root: &MutationRoot, relative: &Path) -> Result<()> {
        let _ = (root, relative);
        unsupported_mutation()
    }
}
