//! The filesystem durability contract (DC-76): what prikk-store requires of a filesystem to
//! mutate a repository safely, stated as guarantees rather than primitives.
//!
//! **Why a trait, not prose.** A markdown description of "what we need" can drift from what the
//! code actually does, silently, the moment either changes without the other. A trait is checked
//! by the compiler: every mutation path in `anchored.rs` calls through [`DurabilityContract`], so
//! there is exactly one place that can state — and one place a future platform's implementation
//! must satisfy — what durability prikk-store depends on. `Linux` and, as of DC-81, `Macos` are the
//! real implementors (`super::anchored::LinuxDurability`, `super::anchored::MacosDurability`);
//! DC-82 added `NoDurability` (`super::anchored::none::NoDurability`) as the implementor for every
//! platform with neither — "unsupported" is a third implementor, not a `target_os` arm at each of
//! `anchored.rs`'s call sites.
//!
//! **Guarantee, not syscall — the whole point.** [`atomic_replace`](DurabilityContract::atomic_replace)
//! says "replace this file's content atomically, durably" — never "write a temp file and call
//! `renameat`". A method named after a primitive would already be platform-specific before a
//! second platform exists.
//!
//! [`durable_directory_entry`](DurabilityContract::durable_directory_entry) was originally the
//! worked example this contract was built around, and it was itself the one method that missed the
//! bar: it read "once this returns, every mutation made under `relative` since the last durability
//! point survives a crash" — a directory-scoped **batching** guarantee, shaped after `fsync` on a
//! directory fd rather than after any real caller's need. **DC-88 traced every caller in the
//! codebase and found none that wanted batching** — every other method above already bundles its
//! own transition-scoped directory sync as an integral part of *its own* guarantee (see each
//! method's doc comment), and this method's two real callers (`worktree.rs`'s checkout
//! materialization, confirming one file's presence is durable even when a call wrote nothing) only
//! ever wanted that narrower, single-entry confirmation. The method is restated accordingly: it
//! still resolves to `fsync` on the containing directory's fd on Linux, and (per DC-76 addendum-1,
//! confirmed against `rustix` 1.1.4's own source) `fcntl(fd, F_FULLFSYNC)`
//! (`rustix::fs::fcntl_fullfsync`) on macOS instead — `fsync` alone does not give the same guarantee
//! there, and stating the method as "fsync" would already have been wrong, before macOS was ever
//! implemented — but what it *promises* is now the requirement its callers actually rely on, not the
//! primitive that happens to satisfy it on POSIX.
//!
//! ## Cross-cutting invariants — properties every method must hold, not separate methods
//!
//! These are not enumerated as trait methods because every operation below needs all of them
//! simultaneously; stating them once here, and testing them once per operation in the conformance
//! suite (`super::tests::conformance`), is more precise than restating "...and refuses symlinks"
//! on every doc comment.
//!
//! - **G1 — root-anchored resolution.** Every path this contract accepts is resolved relative to
//!   the [`MutationRoot`] that authorized it, one path component at a time, with no-follow on
//!   every component including the last. A symlink swapped in anywhere along the path — not only
//!   at the final component — must not let a mutation escape the root.
//! - **G6 — regular-file validation.** Any operation that opens an *existing* final entry
//!   confirms it is a regular file before mutating it. A device, FIFO, or symlink that raced into
//!   the resolved path is refused, not silently operated on.
//! - **G7 — non-blocking opens.** No operation may block indefinitely because a FIFO or device was
//!   substituted at the resolved path.
//! - **G8 — concurrent-process-safe directory creation.** Two processes racing to create the same
//!   directory component must both succeed; the loser observes and validates what the winner
//!   created rather than erroring.
//!
//! ## Guarantee-to-method map
//!
//! | Guarantee | Method |
//! |---|---|
//! | G2 (atomic content replacement) | [`atomic_replace`](DurabilityContract::atomic_replace) |
//! | G3 (durable-after-return) | every method below returns only once its effect is durable; [`durable_directory_entry`](DurabilityContract::durable_directory_entry) confirms one entry's presence directly, without relying on any other method's write having happened in the same call |
//! | G4 (exclusive creation) | [`create_exclusive`](DurabilityContract::create_exclusive) |
//! | G9 (mode-bit isolation) | [`set_permission_bits`](DurabilityContract::set_permission_bits) |
//!
//! G5 (race-safe no-clobber publication) retired in DC-98: its only method, `publish_immutable`,
//! had zero production callers left after RFC 102 Stage 3's container cutover, and the ruling that
//! kept it (design-v1.md §12.3) named its own discharge condition -- Stages 4-5 shipping and showing
//! no loose-file use remained -- which DC-98's RFC confirmed met.

use std::path::Path;

use prikk_error::Result;

use super::anchored::MutationRoot;

/// What prikk-store requires of a filesystem to mutate a repository durably and safely. See the
/// module documentation for the cross-cutting invariants (G1, G6, G7, G8) every method upholds and
/// the guarantee-to-method map. `root` names the authority every path is resolved against; `relative`
/// is always relative to it, never absolute.
///
/// Deliberately **not** gated to any specific `target_os`: the whole point of this contract is a
/// platform-neutral statement of what the store requires, and a trait that vanishes on the
/// platforms it exists to enable would defeat that (DC-76 addendum-2 B1). The B1 repair's
/// `#[allow(dead_code)]` — needed because off Linux and macOS nothing implemented this trait — no
/// longer applies as of DC-82: `NoDurability` (`super::anchored::none::NoDurability`) is the
/// implementor for every platform without a real one, so the trait is used unconditionally now,
/// exactly the outcome the B1 repair's own doc comment predicted ("expected to stop applying the
/// moment a third platform implements this trait").
pub(crate) trait DurabilityContract {
    /// Replace `relative`'s content atomically and durably: a reader never observes a partial
    /// write, and a crash mid-replace leaves either the complete previous content or the complete
    /// new content, never a mix.
    fn atomic_replace(&self, root: &MutationRoot, relative: &Path, bytes: &[u8]) -> Result<()>;

    /// Durably append `bytes` to an existing regular file. RFC 102 Stage 5, design-v1.md §14.3/§14.5:
    /// requires the file to already exist -- does not create it, unlike this trait's other two
    /// "existing" methods' shared name might suggest before this note. This doc was stale from
    /// `d8f5240` until Stage 5 round 3 caught it; see that commit for why creating on append is unsafe.
    fn durable_append(&self, root: &MutationRoot, relative: &Path, bytes: &[u8]) -> Result<()>;

    /// Durably truncate an existing regular file to `len`.
    fn durable_truncate(&self, root: &MutationRoot, relative: &Path, len: u64) -> Result<()>;

    /// Durably truncate an existing regular file to empty. RFC 102 Stage 5, design-v1.md §14.8:
    /// requires the file (and its parent directory) to already exist -- does not create either, the
    /// same discipline `durable_append` follows and `durable_truncate` already did.
    fn durable_truncate_to_empty(&self, root: &MutationRoot, relative: &Path) -> Result<()>;

    /// Create `relative` **exclusively** — refuses if any entry already exists there — write
    /// `bytes`, and durably publish it. No code path through this method can silently overwrite
    /// existing content.
    fn create_exclusive(
        &self,
        root: &MutationRoot,
        relative: &Path,
        bytes: &[u8],
    ) -> std::io::Result<()>;

    /// Set `relative`'s permission bits on an existing regular file, accepting a "recorded mode"
    /// that carries file-type bits (e.g. `0o100_755`, matching a sealed `CreateFile`/`ChangePerm`
    /// operation's own `mode` field) without letting them influence what gets applied. **Not
    /// independently testable on Linux**: `fchmod`'s mode argument already ignores non-permission
    /// bits at the kernel level (confirmed by a reverted negative control — masking `mode & 0o7777`
    /// out of `LinuxDurability::set_permission_bits` before applying it produces byte-identical
    /// results to leaving the file-type bits in), so this masking is deliberate, defensive input
    /// handling for a reader's clarity, not a guarantee whose omission is Linux-observable. Left in
    /// the contract because a future platform's `fchmod`-equivalent may not be as forgiving.
    fn set_permission_bits(&self, root: &MutationRoot, relative: &Path, mode: u32) -> Result<()>;

    /// Durably remove `relative` if present; returns whether an entry was actually removed. Absence
    /// is not an error — the durability guarantee (the parent directory entry's removal, or its
    /// confirmed prior absence, is synced) holds either way.
    fn remove_if_present(&self, root: &MutationRoot, relative: &Path) -> Result<bool>;

    /// Ensure a relative directory tree exists, durably, tolerating a concurrent creator (G8).
    fn ensure_directory(&self, root: &MutationRoot, relative: &Path) -> Result<()>;

    /// Durably confirm that `relative` — an existing regular file — is recorded in its containing
    /// directory: once this returns, `relative`'s presence survives a crash, even when this call
    /// itself wrote nothing (DC-88; see the module doc for why this is stated as a single-entry
    /// confirmation rather than the directory-scoped batching guarantee it originally read as).
    /// `relative` names the **entry to confirm**, not the directory to sync — an implementor
    /// resolves and syncs `relative`'s parent internally.
    fn durable_directory_entry(&self, root: &MutationRoot, relative: &Path) -> Result<()>;
}
