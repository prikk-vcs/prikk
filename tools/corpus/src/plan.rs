//! RFC 139 increment 2's planner (handoff §1, §2.2): profile -> **action manifest**, the ordered
//! sequence of filesystem operations a build would perform. Pure -- executes nothing, spawns
//! nothing, touches no repository. This is what makes the determinism test of RFC 139 §5a.2 possible
//! at "no binary, no repository" cost: two planner calls are compared directly, in-process or across
//! processes, with no `prikk` binary and no `.prikk` repository involved at all.
//!
//! ## Why the manifest has no binary identity (handoff §2.1)
//!
//! The manifest is deliberately *not* the artifact that records which `prikk` binary built a
//! repository. Provenance for that lives in [`crate::execute::BinaryIdentity`], recorded by the
//! **executor** at the moment it actually drives a real binary (`crate::execute::binary_identity`).
//! Splitting it this way is what lets the planner stay pure: a manifest is fully determined by a
//! profile and a target depth, with no dependency on which binary (if any) will ever execute it.
//!
//! ## What "same content" means here (RFC 139 §5a.2)
//!
//! No path's actual bytes are stored in the manifest -- at the depths this increment measures (up to
//! and beyond RFC 139 §6's 2,048-block floor) that would make the manifest itself the dominant cost.
//! Instead every content-bearing action carries a `content_seed` (drawn from the same seeded stream
//! that drove every other planning decision) and a `content_sha256` computed by feeding that seed
//! into [`crate::rng::generate_bytes`] -- the same pure function the executor calls to materialize
//! the real bytes. Two manifests being byte-identical therefore already proves "same content": the
//! hash is a cheap, independently-checkable witness of it, not the mechanism that produces it.
//!
//! ## Decision 1 (RFC 139 §9 item 2, handoff §3): the `--name-status` letter mapping
//!
//! Verified against `crates/prikk-store/src/worktree_patch/node_authoring.rs`'s own module doc
//! before deciding, not assumed:
//!
//! - `A` (added), `C` (copied) -> [`PlannedAction::CreateFile`] at a fresh path. A copy's content is
//!   freshly generated, not literally duplicated from a source path -- profiles never record source
//!   paths (RFC 139 §4's prohibition), so there is nothing to copy from.
//! - `M` (modified) -> [`PlannedAction::EditText`] against an existing path. No `--text-edits` flag
//!   is needed: `WorktreePatchCommitOptions::prefer_text_edits` is a documented no-op today --
//!   "existing-node `NodeKind` is now authoritative for the modified-file mapping" -- and every path
//!   this planner creates is valid UTF-8 (`crate::rng::generate_bytes`'s own doc), so its node kind is
//!   `Text` from creation and every later modification of it authors `EditText` automatically.
//! - `D` (deleted) -> [`PlannedAction::DeleteNode`].
//! - `R` (renamed) -> **not** a real rename. `node_authoring.rs`'s own module doc: *"rename inference
//!   (moves author as delete+create)"* is out of scope for prikk's worktree authoring, and no `prikk`
//!   subcommand authors a `RenamePath` operation at all. The planner emits exactly what driving the
//!   ordinary CLI through an actual `mv` would produce: a [`PlannedAction::DeleteNode`] of the old
//!   path immediately followed by a [`PlannedAction::CreateFile`] of a fresh path with freshly
//!   generated content (not the deleted path's own bytes -- carrying those forward would require the
//!   planner to retain full file contents for every live path for the whole build, which does not
//!   scale to RFC 139 §6's depths). One `renamed` draw therefore contributes **two** manifest actions
//!   for the one git status line it represents; see [`plan()`]'s own doc for what that does to a
//!   commit's action count.
//! - `T` (type_changed) -> modeled as an ordinary [`PlannedAction::EditText`], indistinguishable from
//!   `modified`. `node_authoring.rs`: *"existing-node kind is authoritative (no text<->binary
//!   transition)"* -- a real kind flip cannot be authored through the ordinary CLI at all, so there is
//!   no faithful way to realize it. Zero-weight in `profiles/prikk-self.toml`, so inert there; stated
//!   for when a future profile (increment 4) carries a nonzero count.
//!
//! **`ChangePerm` and `CreateSymlink` are never produced.** `--name-status` cannot distinguish a
//! mode-only change from a content change (both show as `M`), so there is no responsible way to
//! select `ChangePerm` from this letter vocabulary; and prikk's worktree authoring *fails closed* on
//! symlink authoring (`node_authoring.rs`: *"symlink authoring (fails closed until FDD-04 §5.4a)"*),
//! so ever emitting a `CreateSymlink`-shaped action would break the build outright, not merely
//! approximate one.
//!
//! ## Decision 2 (RFC 139 §9 item 3, handoff §3): application order and scaling
//!
//! A profile records **distributions**, not a sequence (RFC 139 §3). Turning them into an ordered
//! plan for an arbitrary target depth needs two decisions this format cannot make for itself:
//!
//! 1. **Sampling is weighted-with-replacement from one seeded stream**, drawn in a fixed traversal
//!    order (commit by commit, action by action within a commit) rather than an exact
//!    expand-and-shuffle of each histogram. This is what lets `target_commit_count` differ from the
//!    profile's own `shape.commit_count` at all -- a profile's `commit_count` is the *sample size the
//!    shape was estimated from*, not a cap on how deep a corpus built from it may go.
//! 2. **The distinct-path population scales with target depth.** `shape.distinct_paths` and
//!    `shape.path_touches` describe a `shape.commit_count`-commit history; building a longer history
//!    from the same shape without growing the path population would plateau tree size at the sample's
//!    own snapshot forever, which is not what "the same shape, deeper" should mean. The path
//!    population (and each touch-count bucket's occurrence count) is scaled by
//!    `target_commit_count / shape.commit_count`, keeping the *ratio* of tree growth to history depth
//!    invariant rather than the absolute count.
//!
//! **A path is touched at most once per planned commit.** This is not a builder preference -- git's
//! own `--name-status` never lists the same path twice for one commit (it is a single tree-vs-tree
//! diff), so a manifest that touched a path twice in one commit could not correspond to anything
//! `files_changed_per_commit` was measured from. Enforced by excluding already-touched slots from
//! every draw within a commit; if every feasible slot is already touched (or none exist) before a
//! commit's drawn file count is reached, that commit's action list simply ends short. This is expected
//! at very shallow depths (a 3-commit build has very few distinct paths yet to touch) and is not an
//! error.
//!
//! Path selection itself: a **create**-like action always takes the highest-weight remaining unborn
//! path (paths the source history touched most end up "core" and enter the tree earliest -- a
//! deterministic, hand-checkable rule, no draw needed); an **edit/delete/rename-source** action draws
//! a live path weighted by its `path_touches`-derived weight, so higher-touch-count paths are
//! selected proportionally more often across the whole build, in expectation reproducing the
//! concentration property `path_touches` records. This is a statistical match, not an exact per-path
//! reproduction -- exactness is not claimed and was never promised by a format that stores
//! distributions (RFC 139 §3).
//!
//! **`files_changed_per_commit` is the one distribution held to exact, not statistical, fidelity**
//! (handoff §6 control 3 asks for a hand-check, not a plausibility argument): its per-commit counts
//! are produced by scaling the histogram to `target_commit_count` with largest-remainder rounding
//! (exact, deterministic, computable from the histogram alone with no RNG involved) and then
//! shuffling that exact multiset into commit order with the seeded generator. The resulting multiset
//! of per-commit counts is therefore a fixed, arithmetic fact about the profile and the target depth
//! -- checkable by computing it directly -- and only the *order* the seed assigns them to commits is
//! left to the generator.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::profile::{OperationKindMix, Profile};
use crate::rng::{SplitMix64, generate_bytes};

/// The only schema version this increment defines for [`ActionManifest`].
pub const PLAN_SCHEMA_VERSION: u64 = 1;

/// Every `EditText` action appends this many freshly generated bytes to the target path's existing
/// content. A profile carries no bytes-changed-per-edit distribution (increment 1's own "what the
/// format cannot express" candidate list); a small fixed append, matching
/// `dc59_commit_benchmark.rs`'s own `mutate_files` precedent, is simpler to produce and to verify by
/// hand than inventing a second size distribution for this increment. A future increment adding one
/// is a `schema_version` bump, not a defect in this one.
pub const EDIT_APPEND_BYTES: u64 = 16;

/// Files created in the same synthetic directory before starting the next one -- purely cosmetic
/// (avoids one flat directory of thousands of files), matching `dc59_commit_benchmark.rs`'s own
/// tree-shape precedent of bucketing generated files rather than piling them in one directory.
const PATH_BUCKET_SIZE: usize = 200;

/// The planner's pure output: the ordered sequence of filesystem operations a build performs.
/// Carries no binary identity and no repository state -- see this module's own doc for why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionManifest {
    /// Must equal [`PLAN_SCHEMA_VERSION`].
    pub schema_version: u64,
    /// The depth this manifest was planned for. Recorded so a reader of a serialized manifest does
    /// not have to count `commits` to know what was asked for versus what the plan could achieve.
    pub target_commit_count: u64,
    /// One entry per planned commit, in build order.
    pub commits: Vec<PlannedCommit>,
}

/// One planned commit: the ordered list of filesystem actions to apply before invoking `prikk
/// commit`, followed (by the executor) with a seal -- RFC 139 depth is sealed-block count, and a
/// fixed one-seal-per-commit cadence is the simplest cadence that makes "N commits planned" and "N
/// sealed blocks" the same number, which is how this RFC and its increments talk about depth
/// throughout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedCommit {
    /// Actions to apply, in order, before this commit. May be shorter than the file count drawn for
    /// this commit if the distinct-path population was exhausted first (see module doc).
    pub actions: Vec<PlannedAction>,
}

/// One filesystem-level operation. Deliberately only three shapes -- see this module's own doc for
/// why `RenamePath`, `ReplaceBinary`, `ChangePerm` and `CreateSymlink` are not represented.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind")]
pub enum PlannedAction {
    /// Write a brand-new file at `path` with `size_bytes` of content generated from `content_seed`.
    CreateFile {
        /// Repository-relative path.
        path: String,
        /// Content length in bytes.
        size_bytes: u64,
        /// Seed for [`crate::rng::generate_bytes`]; reproduces the same content independently at
        /// execution time.
        content_seed: u64,
        /// Hex SHA-256 of the content this action's `content_seed`/`size_bytes` produce -- an
        /// independently checkable witness that two identical manifests imply identical bytes.
        content_sha256: String,
    },
    /// Append `append_bytes` of content generated from `content_seed` to the existing file at `path`.
    EditText {
        /// Repository-relative path; must already exist (created earlier in the plan).
        path: String,
        /// Bytes appended -- always [`EDIT_APPEND_BYTES`] this increment.
        append_bytes: u64,
        /// Seed for the appended bytes.
        content_seed: u64,
        /// Hex SHA-256 of the appended bytes (not the whole file).
        content_sha256: String,
    },
    /// Delete the file at `path`.
    DeleteNode {
        /// Repository-relative path; must already exist.
        path: String,
    },
}

/// Why planning failed. A malformed or unrealizable profile is refused rather than silently
/// producing a plan that does not match what it was asked to model (`extract.rs`'s own
/// "malformed input refused" precedent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// `profile.shape.commit_count` is zero; scaling to any target depth would divide by zero.
    ZeroProfileCommitCount,
    /// A histogram key could not be parsed as an integer count.
    MalformedHistogramKey {
        /// Which field.
        field: &'static str,
        /// The unparseable key.
        key: String,
    },
    /// A histogram this planner needs to draw from has no entries.
    EmptyHistogram {
        /// Which field.
        field: &'static str,
    },
    /// No action of any kind was realizable at this point -- every population this profile's
    /// `operation_kind_mix` needs to draw from is exhausted. See module doc: a *short* commit is
    /// expected and not an error; this is only raised when a plan cannot even start (e.g. a profile
    /// whose `operation_kind_mix` has zero weight on every create-like kind, so no path could ever be
    /// created for anything else to act on).
    Unrealizable {
        /// 0-based commit index where planning stalled.
        commit_index: u64,
    },
    /// A slot index this planner itself just selected (from `slots.iter().enumerate()`) turned out
    /// not to exist. Never expected to occur; exists so an out-of-bounds index is a typed error
    /// rather than a panic (the workspace's own `indexing_slicing` lint requires this shape here).
    Internal(&'static str),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroProfileCommitCount => {
                write!(
                    f,
                    "profile shape.commit_count is zero, cannot scale to a target depth"
                )
            }
            Self::MalformedHistogramKey { field, key } => {
                write!(f, "{field}: key {key:?} is not a non-negative integer")
            }
            Self::EmptyHistogram { field } => write!(f, "{field}: histogram has no entries"),
            Self::Unrealizable { commit_index } => write!(
                f,
                "commit {commit_index}: no feasible action -- every population this profile's \
                 operation_kind_mix needs is exhausted before this commit could plan anything"
            ),
            Self::Internal(message) => write!(f, "internal planner error: {message}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Plan a corpus build: profile shape + seed (`profile.builder_inputs.generator_seed`) -> an
/// [`ActionManifest`] of exactly `target_commit_count` planned commits (each possibly short -- see
/// module doc). Pure: no I/O, no binary, no repository.
///
/// **A `renamed` draw contributes two actions to its commit** (`DeleteNode` then `CreateFile` --
/// decision 1), so a commit's `actions.len()` can exceed the file count drawn for it by up to the
/// number of renames drawn within it. This is a faithful, disclosed consequence of prikk's own
/// worktree authoring being unable to author a real rename, not an approximation error.
pub fn plan(profile: &Profile, target_commit_count: u64) -> Result<ActionManifest, PlanError> {
    let shape = &profile.shape;
    if shape.commit_count == 0 {
        return Err(PlanError::ZeroProfileCommitCount);
    }
    let files_changed_histogram =
        numeric_sorted(&shape.files_changed_per_commit, "files_changed_per_commit")?;
    if files_changed_histogram.is_empty() {
        return Err(PlanError::EmptyHistogram {
            field: "files_changed_per_commit",
        });
    }
    let file_sizes_histogram = numeric_sorted(&shape.file_sizes, "file_sizes")?;
    if file_sizes_histogram.is_empty() {
        return Err(PlanError::EmptyHistogram {
            field: "file_sizes",
        });
    }

    let scale = target_commit_count as f64 / shape.commit_count as f64;
    let mut slots = build_path_slots(&shape.path_touches, scale)?;

    let mut rng = SplitMix64::new(profile.builder_inputs.generator_seed);
    let mut files_changed_sequence =
        expand_and_scale(&files_changed_histogram, target_commit_count);
    shuffle(&mut rng, &mut files_changed_sequence);

    let mut commits =
        Vec::with_capacity(usize::try_from(target_commit_count).unwrap_or(usize::MAX));

    for files_changed in files_changed_sequence {
        let commit_index = commits.len() as u64;
        let mut touched: BTreeSet<usize> = BTreeSet::new();
        let mut actions = Vec::new();

        for _ in 0..files_changed {
            let has_unborn = slots.iter().any(|slot| slot.state == SlotState::Unborn);
            let live_available = slots
                .iter()
                .enumerate()
                .any(|(index, slot)| slot.state == SlotState::Live && !touched.contains(&index));

            let Some(kind) = pick_feasible_kind(
                &mut rng,
                &shape.operation_kind_mix,
                has_unborn,
                live_available,
            ) else {
                if actions.is_empty() && commit_index == 0 {
                    return Err(PlanError::Unrealizable { commit_index });
                }
                break;
            };

            match kind {
                Kind::Add | Kind::Copy => {
                    let Some(index) = pick_highest_weight_unborn(&slots) else {
                        break;
                    };
                    let action = create_action(&mut rng, &file_sizes_histogram, &mut slots, index)?;
                    touched.insert(index);
                    actions.push(action);
                }
                Kind::Modify | Kind::TypeChanged => {
                    let Some(index) = pick_live_weighted(&mut rng, &slots, &touched) else {
                        break;
                    };
                    touched.insert(index);
                    let path = slot_ref(&slots, index)?.name.clone();
                    actions.push(edit_action(&mut rng, &path));
                }
                Kind::Delete => {
                    let Some(index) = pick_live_weighted(&mut rng, &slots, &touched) else {
                        break;
                    };
                    let slot = slot_mut(&mut slots, index)?;
                    slot.state = SlotState::Dead;
                    let path = slot.name.clone();
                    touched.insert(index);
                    actions.push(PlannedAction::DeleteNode { path });
                }
                Kind::Rename => {
                    let Some(old_index) = pick_live_weighted(&mut rng, &slots, &touched) else {
                        break;
                    };
                    let old_slot = slot_mut(&mut slots, old_index)?;
                    old_slot.state = SlotState::Dead;
                    let old_path = old_slot.name.clone();
                    touched.insert(old_index);
                    actions.push(PlannedAction::DeleteNode { path: old_path });
                    let Some(new_index) = pick_highest_weight_unborn(&slots) else {
                        break;
                    };
                    let action =
                        create_action(&mut rng, &file_sizes_histogram, &mut slots, new_index)?;
                    touched.insert(new_index);
                    actions.push(action);
                }
            }
        }

        commits.push(PlannedCommit { actions });
    }

    Ok(ActionManifest {
        schema_version: PLAN_SCHEMA_VERSION,
        target_commit_count,
        commits,
    })
}

/// `slots.get(index)`, turning an out-of-bounds index (never expected -- see [`PlanError::Internal`])
/// into a typed error instead of the workspace's `indexing_slicing` lint refusing a bare `slots[index]`.
fn slot_ref(slots: &[PathSlot], index: usize) -> Result<&PathSlot, PlanError> {
    slots
        .get(index)
        .ok_or(PlanError::Internal("slot index out of bounds"))
}

/// `slots.get_mut(index)`, the mutable counterpart to [`slot_ref`].
fn slot_mut(slots: &mut [PathSlot], index: usize) -> Result<&mut PathSlot, PlanError> {
    slots
        .get_mut(index)
        .ok_or(PlanError::Internal("slot index out of bounds"))
}

fn create_action(
    rng: &mut SplitMix64,
    file_sizes_histogram: &[(u64, u64)],
    slots: &mut [PathSlot],
    index: usize,
) -> Result<PlannedAction, PlanError> {
    let Some(size_bytes) = weighted_pick_value(rng, file_sizes_histogram) else {
        return Err(PlanError::EmptyHistogram {
            field: "file_sizes",
        });
    };
    let content_seed = rng.next_u64();
    let content_sha256 = hex_sha256(&generate_bytes(content_seed, size_bytes));
    let slot = slot_mut(slots, index)?;
    slot.state = SlotState::Live;
    let path = slot.name.clone();
    Ok(PlannedAction::CreateFile {
        path,
        size_bytes,
        content_seed,
        content_sha256,
    })
}

fn edit_action(rng: &mut SplitMix64, path: &str) -> PlannedAction {
    let content_seed = rng.next_u64();
    let content_sha256 = hex_sha256(&generate_bytes(content_seed, EDIT_APPEND_BYTES));
    PlannedAction::EditText {
        path: path.to_owned(),
        append_bytes: EDIT_APPEND_BYTES,
        content_seed,
        content_sha256,
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    prikk_hash::to_hex(&prikk_hash::sha256(bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Unborn,
    Live,
    Dead,
}

#[derive(Debug, Clone)]
struct PathSlot {
    weight: u64,
    name: String,
    state: SlotState,
}

fn path_name(index: usize) -> String {
    let bucket = index / PATH_BUCKET_SIZE;
    format!("corpus/d{bucket:04}/f{index:07}.txt")
}

/// Build the distinct-path population, scaled per this module's own decision-2 doc. Slots are
/// created in ascending-touch-weight order (an artifact of `numeric_sorted`'s ordering, not itself
/// meaningful) -- selection order is governed entirely by `pick_highest_weight_unborn` and
/// `pick_live_weighted`, not by insertion order.
fn build_path_slots(
    path_touches: &BTreeMap<String, u64>,
    scale: f64,
) -> Result<Vec<PathSlot>, PlanError> {
    let histogram = numeric_sorted(path_touches, "path_touches")?;
    let mut slots = Vec::new();
    for (touches, count) in histogram {
        if touches == 0 {
            return Err(PlanError::MalformedHistogramKey {
                field: "path_touches",
                key: "0".to_owned(),
            });
        }
        let scaled_count = ((count as f64) * scale).round().max(0.0) as u64;
        for _ in 0..scaled_count {
            let index = slots.len();
            slots.push(PathSlot {
                weight: touches,
                name: path_name(index),
                state: SlotState::Unborn,
            });
        }
    }
    if slots.is_empty() {
        return Err(PlanError::EmptyHistogram {
            field: "path_touches",
        });
    }
    Ok(slots)
}

fn numeric_sorted(
    histogram: &BTreeMap<String, u64>,
    field: &'static str,
) -> Result<Vec<(u64, u64)>, PlanError> {
    let mut pairs = Vec::with_capacity(histogram.len());
    for (key, value) in histogram {
        let parsed: u64 = key.parse().map_err(|_| PlanError::MalformedHistogramKey {
            field,
            key: key.clone(),
        })?;
        pairs.push((parsed, *value));
    }
    pairs.sort_unstable_by_key(|(value, _)| *value);
    Ok(pairs)
}

/// Scale a `(value, occurrences)` histogram to exactly `target_total` items via largest-remainder
/// rounding, then flatten it into an unshuffled multiset (ascending by value, since `histogram` is
/// numeric-sorted -- callers shuffle separately). Deterministic and RNG-free: the *count* of each
/// value in the result is a fixed arithmetic function of the histogram and `target_total` alone,
/// which is what makes handoff §6 control 3 hand-checkable rather than a plausibility argument over a
/// random draw.
fn expand_and_scale(histogram: &[(u64, u64)], target_total: u64) -> Vec<u64> {
    let source_total: u64 = histogram.iter().map(|(_, count)| *count).sum();
    if source_total == 0 || target_total == 0 {
        return Vec::new();
    }
    let scale = target_total as f64 / source_total as f64;
    let mut allotted: Vec<(u64, u64, f64)> = histogram
        .iter()
        .map(|&(value, count)| {
            let exact = (count as f64) * scale;
            (value, exact.floor().max(0.0) as u64, exact.fract())
        })
        .collect();
    let assigned: u64 = allotted.iter().map(|(_, count, _)| *count).sum();
    let mut remainder = target_total.saturating_sub(assigned);
    let mut order: Vec<usize> = (0..allotted.len()).collect();
    order.sort_by(|&left, &right| {
        let left_fraction = allotted.get(left).map_or(0.0, |entry| entry.2);
        let right_fraction = allotted.get(right).map_or(0.0, |entry| entry.2);
        right_fraction
            .partial_cmp(&left_fraction)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for index in order {
        if remainder == 0 {
            break;
        }
        if let Some(entry) = allotted.get_mut(index) {
            entry.1 += 1;
        }
        remainder -= 1;
    }
    let mut expanded = Vec::with_capacity(target_total as usize);
    for (value, count, _) in allotted {
        for _ in 0..count {
            expanded.push(value);
        }
    }
    expanded
}

/// Fisher-Yates shuffle, driven by the same seeded stream as everything else this planner draws.
fn shuffle(rng: &mut SplitMix64, items: &mut [u64]) {
    for index in (1..items.len()).rev() {
        let swap_with = rng.below((index as u64) + 1) as usize;
        items.swap(index, swap_with);
    }
}

/// Weighted draw with replacement over `(value, occurrences)` pairs, returning the drawn `value`.
fn weighted_pick_value(rng: &mut SplitMix64, histogram: &[(u64, u64)]) -> Option<u64> {
    let total: u64 = histogram.iter().map(|(_, count)| *count).sum();
    if total == 0 {
        return None;
    }
    let draw = rng.below(total);
    let mut accumulated = 0_u64;
    for &(value, count) in histogram {
        accumulated += count;
        if draw < accumulated {
            return Some(value);
        }
    }
    histogram.last().map(|&(value, _)| value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Add,
    Modify,
    Delete,
    Rename,
    Copy,
    TypeChanged,
}

impl Kind {
    const ALL: [Kind; 6] = [
        Kind::Add,
        Kind::Modify,
        Kind::Delete,
        Kind::Rename,
        Kind::Copy,
        Kind::TypeChanged,
    ];

    fn base_weight(self, mix: &OperationKindMix) -> u64 {
        match self {
            Kind::Add => mix.added,
            Kind::Modify => mix.modified,
            Kind::Delete => mix.deleted,
            Kind::Rename => mix.renamed,
            Kind::Copy => mix.copied,
            Kind::TypeChanged => mix.type_changed,
        }
    }

    /// Whether this kind can be realized at all given the current population state -- decision 1's
    /// mapping to what each kind needs: create-like kinds need an unborn path, everything else needs
    /// a live path not already touched this commit.
    fn feasible(self, has_unborn: bool, live_available: bool) -> bool {
        match self {
            Kind::Add | Kind::Copy => has_unborn,
            Kind::Modify | Kind::Delete | Kind::TypeChanged => live_available,
            Kind::Rename => has_unborn && live_available,
        }
    }
}

fn pick_feasible_kind(
    rng: &mut SplitMix64,
    mix: &OperationKindMix,
    has_unborn: bool,
    live_available: bool,
) -> Option<Kind> {
    let weights: Vec<(Kind, u64)> = Kind::ALL
        .into_iter()
        .map(|kind| {
            let weight = if kind.feasible(has_unborn, live_available) {
                kind.base_weight(mix)
            } else {
                0
            };
            (kind, weight)
        })
        .collect();
    let total: u64 = weights.iter().map(|(_, weight)| *weight).sum();
    if total == 0 {
        return None;
    }
    let draw = rng.below(total);
    let mut accumulated = 0_u64;
    for (kind, weight) in weights {
        accumulated += weight;
        if draw < accumulated {
            return Some(kind);
        }
    }
    None
}

fn pick_highest_weight_unborn(slots: &[PathSlot]) -> Option<usize> {
    slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.state == SlotState::Unborn)
        .max_by_key(|(_, slot)| slot.weight)
        .map(|(index, _)| index)
}

/// Weighted draw over live slots not already touched this commit, weighted by each slot's
/// `path_touches`-derived weight -- higher-touch-count paths are selected proportionally more often
/// across the whole build (module doc's decision-2: a statistical match to the concentration
/// property, not an exact per-path reproduction).
fn pick_live_weighted(
    rng: &mut SplitMix64,
    slots: &[PathSlot],
    touched: &BTreeSet<usize>,
) -> Option<usize> {
    let candidates: Vec<usize> = slots
        .iter()
        .enumerate()
        .filter(|(index, slot)| slot.state == SlotState::Live && !touched.contains(index))
        .map(|(index, _)| index)
        .collect();
    let total: u64 = candidates
        .iter()
        .map(|&index| slots.get(index).map_or(0, |slot| slot.weight))
        .sum();
    if candidates.is_empty() || total == 0 {
        return None;
    }
    let draw = rng.below(total);
    let mut accumulated = 0_u64;
    for &index in &candidates {
        accumulated += slots.get(index).map_or(0, |slot| slot.weight);
        if draw < accumulated {
            return Some(index);
        }
    }
    candidates.last().copied()
}

#[cfg(test)]
mod tests;
