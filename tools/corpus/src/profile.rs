//! The measurement-corpus profile format (RFC 139 §3, this increment's own handoff §3).
//!
//! A profile is a small, human-readable TOML document describing a real history's *shape* --
//! never its content, never its paths (RFC 139 §4's prohibition). Increment 2's builder reads a
//! profile and synthesizes a throwaway prikk repository from it, driving the same `prikk` CLI a
//! user would (RFC 139 §7) -- never `prikk-store` directly.
//!
//! `schema_version` is an integer field, following `release-signers.toml`'s own idiom
//! (`schema_version = 1`), not a string like this project's JSON reports -- a profile is a
//! human-editable *input*, the same category as `release-signers.toml`, not a machine-readable
//! *output* like `verify --format json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The only schema version this increment defines. Increment 2 will very likely be the first to
/// bump it (handoff §3).
pub const SCHEMA_VERSION: u64 = 1;

/// A measurement-corpus profile: provenance, shape, and the builder inputs a deterministic build
/// depends on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Must equal [`SCHEMA_VERSION`] for a profile this version of the crate can read.
    pub schema_version: u64,
    /// RFC 139 §4's provenance requirements, all four mandatory.
    pub provenance: Provenance,
    /// RFC 139 §4 / handoff §3's shape distributions.
    pub shape: Shape,
    /// Handoff §3's "builder inputs" -- fixed here so a corpus is reproducible (RFC 139 §5).
    pub builder_inputs: BuilderInputs,
}

/// RFC 139 §4: what history this profile was extracted from, and exactly how -- so a reader can
/// re-derive the numbers rather than trust them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Human-readable identification of the source repository (a path, a URL, or "prikk (self)").
    pub source_repository: String,
    /// The exact revision (or the end of the extracted range) the source repository was at.
    pub revision: String,
    /// Every extraction command, verbatim, as run. A reader must be able to copy-paste these and
    /// get this profile's own numbers back.
    pub extraction_commands: Vec<String>,
    /// The date extraction was performed, `YYYY-MM-DD`.
    pub extraction_date: String,
    /// Whether git's rename detection was in effect for the log extraction (handoff §4: left at
    /// git's default, `true`, because prikk has a first-class `RenamePath` operation that rename
    /// detection's output matches rather than distorts -- recorded because it changes the
    /// distinct-path and touches-per-path numbers, and a reader comparing two profiles needs to
    /// know both were extracted the same way).
    pub rename_detection: bool,
}

/// RFC 139 §4 / handoff §3: what the builder needs to synthesize a history of the same shape.
///
/// **Every histogram here is exact-value, not range-bucketed**: the map key is the literal
/// integer value (files changed, touch count, or byte size) rendered as a TOML string key (TOML
/// has no integer-keyed table), and the value is how many times that exact value was observed.
/// Chosen over range buckets for this increment because it is simpler to produce, simpler to
/// verify by hand against a small fixture (handoff §6 control 3), and loses no information a range
/// bucketing would have to invent boundaries to preserve. If increment 2's builder needs coarser
/// buckets, that is a `schema_version` bump, not a defect in this one -- `schema_version` exists
/// for exactly that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// Non-merge commits in the extracted range.
    pub commit_count: u64,
    /// Histogram: files changed in one commit -> number of commits with exactly that many.
    pub files_changed_per_commit: BTreeMap<String, u64>,
    /// The git `--name-status` letter mix across every changed path in the range (handoff §4):
    /// this records git's own vocabulary (added/modified/deleted/renamed/copied/type-changed), not
    /// prikk's `OperationKind` -- mapping git's coarser status letters onto prikk's own operation
    /// kinds (an `M` on a text file is `EditText`; on a binary file, `ReplaceBinary`; a permission-
    /// only change is `ChangePerm`; none of that is recoverable from `--name-status` alone) is the
    /// builder's own decision to make in increment 2, not something increment 1 can decide for it.
    pub operation_kind_mix: OperationKindMix,
    /// Count of distinct paths touched anywhere in the extracted range.
    pub distinct_paths: u64,
    /// Histogram: how many times one path was touched, over the whole range -> number of distinct
    /// paths touched exactly that many times. The concentration property RFC 139 §4's second-
    /// profile requirement turns on.
    pub path_touches: BTreeMap<String, u64>,
    /// Histogram: file byte size at the extracted revision (`git ls-tree -r -l`) -> number of
    /// files with exactly that size. RFC 133 §2 measured that commit cost follows bytes, not
    /// paths; a profile omitting this would let a builder invent the dominant cost driver.
    pub file_sizes: BTreeMap<String, u64>,
}

/// The git `--name-status` letter mix (handoff §4). Field names match git's own status-letter
/// vocabulary, not prikk's `OperationKind` -- see [`Shape::operation_kind_mix`]'s own doc for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationKindMix {
    /// `A` -- added.
    pub added: u64,
    /// `M` -- modified.
    pub modified: u64,
    /// `D` -- deleted.
    pub deleted: u64,
    /// `R` (with any similarity suffix, e.g. `R100`) -- renamed.
    pub renamed: u64,
    /// `C` (with any similarity suffix) -- copied. Git only emits this with `--find-copies`,
    /// which this profile's own extraction command does not pass; kept at `0` rather than omitted,
    /// so a profile extracted with copy detection enabled later has somewhere to put a nonzero
    /// count without a schema change.
    pub copied: u64,
    /// `T` -- type changed (e.g. file to symlink). Prikk's own worktree authoring never produces
    /// this today, but a profile extracted from a different project's history might see it.
    pub type_changed: u64,
}

/// Handoff §3's "builder inputs": fixed here so a corpus is reproducible (RFC 139 §5). **This is
/// the field increment 1 found was not fully enumerable** -- see [`Self::known_nondeterminism_risks`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderInputs {
    /// Seed for the deterministic content/change generator increment 2 will build, matching the
    /// `SplitMix64`-from-a-fixed-seed precedent RFC 139 §5 names
    /// (`dc59_commit_benchmark.rs`'s own `CONTENT_SEED`) rather than inventing a third generator.
    pub generator_seed: u64,
    /// `PRIKK_AUTHOR_KEY_ID` the builder must use for every authored commit.
    pub author_key_id: String,
    /// The AUTHOR seed, 64 lowercase hex characters, the builder must use for every authored commit
    /// -- fixed here, not drawn from the OS CSPRNG, so two builds from this profile sign
    /// identically. Reaches prikk as a file through `PRIKK_AUTHOR_SEED_FILE` (RFC 148); the retired
    /// `PRIKK_AUTHOR_SEED` variable this field used to name is unread as of 0.41.
    pub author_seed_hex: String,
    /// `PRIKK_MAINTAINER_KEY_ID` the builder must use for every seal.
    pub maintainer_key_id: String,
    /// The MAINTAINER seed, 64 lowercase hex characters, the builder must use for every seal;
    /// reaches prikk as a file, as `author_seed_hex` does.
    pub maintainer_seed_hex: String,
    /// **Found while building this increment, not resolved by it** (out of scope: "any change to
    /// `crates/`"). Each entry names one thing that varies between two builds of the *same*
    /// profile through today's `prikk` CLI, which RFC 139 §5 requires not to happen. The one this
    /// increment found: `NodeId` is minted from the OS CSPRNG at authoring time
    /// (`crates/prikk-store/src/node_id_gen.rs`'s own module doc: "not derived from path, content,
    /// operation position, timestamp, or baseline state -- it is a uniform 256-bit draw from the
    /// OS CSPRNG"), with no CLI-level override -- so two corpora built from this exact profile,
    /// through the ordinary `prikk commit` surface RFC 139 §7 requires the builder to drive, will
    /// mint different node ids, and therefore different object ids, on every build. Increment 2's
    /// own determinism test (RFC 139 §5) is what will have to either close this gap (a `crates/`
    /// change, needing its own review) or prove it does not matter for what increment 2 actually
    /// measures.
    pub known_nondeterminism_risks: Vec<String>,
}

#[cfg(test)]
mod tests;
