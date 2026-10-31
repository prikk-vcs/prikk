//! The worktree's recorded baseline (0.56.0 step W2, R1/R2).
//!
//! **The gap this closes:** `worktree-status`'s "missing" check and `commit`'s own deletion-inference
//! loop both read "is this baseline file on disk" against a baseline derived *live*, every call, from
//! whatever the ref currently resolves to (`patch_replay::resolve_folded_worktree_baseline`). That
//! cannot tell a file the user **deliberately deleted** (commit must author `delete-file`) from a file
//! this worktree **never held**, because some other command moved the ref without touching the
//! worktree at all (`merge --into`, `archive import --adopt`, `sync seal --claim`, `branch create
//! --from` onto the unborn current branch -- W3's producers) or because `commit --ref <other>` authored
//! against a baseline the worktree was never written from. Both read as "missing" either way. So this
//! module records one new fact: **the state this worktree was last actually materialized to match.**
//!
//! **The identity (R1).** `{ref_name, baseline: Option<ObjectId>, wal_sequence: u64}` --
//! `baseline` is the sealed tip block this worktree's files were last written from (`None` for an
//! unpublished/genesis ref), and `wal_sequence` is the active WAL's sequence number up to which this
//! worktree's content already reflects the queued (unsealed) fold -- `0` when no queued patch is
//! folded in. This is exactly what `commit` already knows the instant it appends a patch (the sequence
//! `append_patch`/`append_patch_and_witness` just returned) and what `seal` already knows the instant
//! it publishes a new block and drains the WAL (the new block id, sequence back to `0`) -- recording it
//! costs no replay, no re-derivation, just writing down what the caller already just did.
//!
//! **Storage.** One record, replaced in place on every write -- never `atomic_replace`, for the exact
//! reason `worktree_marker.rs`'s own module doc gives: that primitive renames a temp file onto the
//! destination unconditionally, a new-name event whose Windows durability is still an open question.
//! `foundation::fsutil::rewrite_in_place_or_create_required` (RFC 168 §3.3, RFC 166 D1 -- already the
//! writer of the current-branch pointer and the active-ref-name metadata) truncates the existing file
//! and appends the new record's bytes, both durable, no rename at all; an absent file (a repository
//! from before this file existed, or before this round) is created exclusively, the one new-name case,
//! and only ever happens once. Unlike `worktree_marker.rs`'s two markers, this record must be
//! *replaced*, not appended to -- there is exactly one current baseline, never a history of them -- so
//! the append-only "latest well-formed record wins" idiom those two markers use does not fit here;
//! `rewrite_in_place_or_create_required` is the primitive this project already built for "replace the
//! one current record" (the current-branch pointer is exactly that shape), and reusing it here is more
//! directly on point than the append-only idiom would be.
//!
//! **Crash posture.** A crash mid-write of `rewrite_in_place_or_create_required` can only ever leave the
//! file durably empty (the truncate is itself fully durable before the append begins) or durably holding
//! the one complete new record -- never a torn one. An empty or absent file reads as [`None`] from
//! [`read_worktree_baseline`], which [`compare_worktree_baseline`] treats as [`WorktreeBaselineComparison::NoRecord`]
//! -- "treated as matching its current ref," today's assumption (R1), preserved for a repository that
//! predates this file or one an older binary wrote to without knowing about it. The record is written
//! **last**, after every worktree write of the call it brackets (and, where one exists, after the
//! worktree-dirty marker is cleared) -- so a crash between the worktree write and this record leaves the
//! record stale, which is the safe direction: the next `commit` sees a mismatch and refuses, never a
//! missed one, and `branch switch <the current branch>` (R2) clears it by re-running the same,
//! idempotent materialization and writing the record this time.
//!
//! **Not a format change.** This is worktree-local operational state, exactly like the unclean-shutdown
//! and provisional-worktree markers beside it -- never part of any versioned object schema, and never
//! read by anything that decides trust. `verify`/`doctor` do not scan `.prikk/` for unrecognized files
//! (confirmed by reading both: each reads specific known paths by name, never a directory listing of
//! `.prikk/` itself), so a repository from before this file existed is simply invisible to them, exactly
//! as it was before this round; this round adds no new `doctor`/`verify` check over it, matching the
//! ruling's own "probably ignore it" expectation -- a stale record is cleared by the ordinary resync, not
//! something `doctor` needs to repair.

use prikk_error::Result;
use prikk_object::ObjectId;

use crate::foundation::fsutil::{read_file_if_exists, rewrite_in_place_or_create_required};
use crate::foundation::layout::RepositoryLayout;
use crate::patch_replay::{WorktreeBaseline, resolve_worktree_baseline};
use crate::refs::validate_local_branch_ref;
use crate::wal::WalReplay;
use crate::{ActiveRefOwnership, active_ref_ownership};

/// The first (and only meaningful) field of the record: not parsed for content, just a recognizable
/// magic so the file is self-explanatory to inspection, on the same idiom as `worktree_marker.rs`'s
/// own sentinels.
const RECORD_MAGIC: &str = "PRIKK-WORKTREE-BASELINE-v1";

/// `baseline`'s on-disk spelling for a genesis (unpublished) ref -- never a valid [`ObjectId`] hex
/// string, so it cannot collide with a real block id.
const GENESIS_MARKER: &str = "-";

/// The one record's path, relative to `.prikk` -- flat, beside `worktree.marker`/`worktree.provisional`
/// (`layout.rs`'s own siblings), not a method on `RepositoryLayout`: this file is a plain, fixed name
/// under `.prikk`, needing no join against the repository root the way a worktree-content path would,
/// so a local constant here is exactly as clear and keeps `layout.rs` under this project's own
/// file-size gate.
fn baseline_record_path() -> &'static std::path::Path {
    std::path::Path::new("worktree.baseline")
}

/// What this worktree was last made to match, or what it currently, live, folds to -- the same shape
/// either way (R1): a ref name, the sealed baseline block (`None` for genesis), and the active WAL
/// sequence number folded on top (`0` for none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeBaselineIdentity {
    /// Canonical local branch ref name.
    pub(crate) ref_name: String,
    /// Sealed tip block this identity's content starts from; `None` for an unpublished ref.
    pub(crate) baseline: Option<ObjectId>,
    /// Active WAL sequence number folded on top of `baseline`; `0` when none is.
    pub(crate) wal_sequence: u64,
}

/// Replace the worktree's recorded baseline (R1). Called last by every command that makes the
/// worktree match a state, or moves a ref to a state the worktree already matches: `setup`/`init`,
/// `branch switch`, the `checkout` writes, `commit` (the queued fold just advanced), and `seal` (the
/// fold just sealed, WAL back to empty). `pub`, not `pub(crate)`: `seal` lives in `prikk-cli`
/// (`crates/prikk-cli/src/seal.rs`), not this crate, and is the one external caller.
pub fn record_worktree_baseline(
    layout: &RepositoryLayout,
    ref_name: &str,
    baseline: Option<ObjectId>,
    wal_sequence: u64,
) -> Result<()> {
    let canonical = validate_local_branch_ref(ref_name)?;
    let baseline_field = baseline.map_or_else(|| GENESIS_MARKER.to_string(), |id| id.to_string());
    let record = format!("{RECORD_MAGIC} {canonical} {baseline_field} {wal_sequence}\n");
    let relative = baseline_record_path();
    rewrite_in_place_or_create_required(
        layout.repository_mutation_root(),
        relative,
        record.as_bytes(),
    )
}

/// Read the worktree's recorded baseline. `None` when the file is absent or empty (no record has ever
/// been written -- a pre-0.56.0 repository, or one last written by a binary that did not know about
/// this file) **or** when its one record does not parse (treated the same, conservatively: migration
/// safety, not a new failure mode -- the write ordering in this module's own doc means a torn record
/// cannot occur in practice, so this tolerance exists for forward compatibility, not a known crash
/// window).
pub(crate) fn read_worktree_baseline(
    layout: &RepositoryLayout,
) -> Result<Option<WorktreeBaselineIdentity>> {
    let relative = baseline_record_path();
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), relative)? else {
        return Ok(None);
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&bytes);
    let Some(line) = text.lines().next() else {
        return Ok(None);
    };
    let fields: Vec<&str> = line.split(' ').collect();
    let [magic, ref_name, baseline_field, wal_sequence_field] = fields.as_slice() else {
        return Ok(None);
    };
    if *magic != RECORD_MAGIC {
        return Ok(None);
    }
    let Ok(canonical) = validate_local_branch_ref(ref_name) else {
        return Ok(None);
    };
    let baseline = if *baseline_field == GENESIS_MARKER {
        Some(None)
    } else {
        baseline_field.parse::<ObjectId>().ok().map(Some)
    };
    let wal_sequence = wal_sequence_field.parse::<u64>().ok();
    match (baseline, wal_sequence) {
        (Some(baseline), Some(wal_sequence)) => Ok(Some(WorktreeBaselineIdentity {
            ref_name: canonical,
            baseline,
            wal_sequence,
        })),
        _ => Ok(None),
    }
}

/// Establish the worktree's baseline record for a freshly initialized repository (`init`/`setup`,
/// R1): a genesis record for `heads/main`, the current branch `RepositoryLayout::init` itself
/// establishes (RFC 151 §2.1). A no-op when a record already exists -- `init`/`setup` on an
/// already-initialized repository must not clobber a record a later `commit`/`seal`/`branch
/// switch`/checkout has since advanced, the same "only when absent" rule `init`'s own current-branch
/// pointer write follows. Called from the CLI (`run_init`, `run_setup`), not from
/// `foundation::layout::RepositoryLayout::init` itself: `foundation` is this crate's base layer, and
/// this module already depends on it (`fsutil`, `layout`) -- calling back into it from inside
/// `foundation` would be the cycle the boundary-check gate exists to catch.
pub fn record_initial_worktree_baseline(layout: &RepositoryLayout) -> Result<()> {
    if read_worktree_baseline(layout)?.is_some() {
        return Ok(());
    }
    record_worktree_baseline(layout, "heads/main", None, 0)
}

/// `ref_name`'s live identity, right now (R2): the sealed baseline it currently resolves to, plus the
/// active WAL's sequence number if it owns `ref_name` and is non-empty, else `0`. `active_replay` is a
/// parameter, not read here, so a caller that already decoded the active WAL this call (every one of
/// this function's three callers does) is not made to decode it a second time -- the same reasoning
/// `resolve_folded_worktree_baseline`'s own doc comment gives for taking it as a parameter.
pub(crate) fn live_baseline_identity(
    layout: &RepositoryLayout,
    ref_name: &str,
    active_replay: &WalReplay,
) -> Result<WorktreeBaselineIdentity> {
    let canonical = validate_local_branch_ref(ref_name)?;
    let baseline = match resolve_worktree_baseline(layout, &canonical)? {
        WorktreeBaseline::Published { baseline_block, .. } => Some(baseline_block),
        WorktreeBaseline::Genesis => None,
    };
    let wal_sequence = if active_replay.records.is_empty() {
        0
    } else {
        match active_ref_ownership(layout, &canonical)? {
            ActiveRefOwnership::Owned => {
                active_replay.records.last().map_or(0, |record| record.seq)
            }
            ActiveRefOwnership::OwnedByOther(_) => 0,
        }
    };
    Ok(WorktreeBaselineIdentity {
        ref_name: canonical,
        baseline,
        wal_sequence,
    })
}

/// [`compare_worktree_baseline`]'s answer -- **the one shared predicate** (R2): "the worktree's
/// recorded state, compared with the ref's state," consumed by `commit`'s guard, both directions of
/// `branch switch`, and `worktree-status` -- not four independently-written comparisons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeBaselineComparison {
    /// No record exists. Treated exactly like `Current` by every caller (R1's migration-safety rule) --
    /// kept as its own variant only so a caller that wants to say so (`worktree-status`) can.
    NoRecord,
    /// The recorded and live baseline blocks are the same content, whatever ref either names.
    Current,
    /// The record is stale: its baseline block is not the one `ref_name` resolves to now. Carries
    /// both for the caller's own message -- including each one's `ref_name`, which a caller may use
    /// for its own secondary purposes (which sentence to print; whether the record even claims to
    /// describe the ref being asked about) even though ref identity plays no part in `Current` vs
    /// `Behind` itself (see [`compare_worktree_baseline_with_live`]'s own doc for why).
    Behind {
        /// What this worktree was last recorded as matching.
        recorded: WorktreeBaselineIdentity,
        /// What `ref_name` resolves to right now.
        live: WorktreeBaselineIdentity,
    },
}

/// Compare the worktree's recorded baseline against `ref_name`'s live state (R2).
pub(crate) fn compare_worktree_baseline(
    layout: &RepositoryLayout,
    ref_name: &str,
    active_replay: &WalReplay,
) -> Result<WorktreeBaselineComparison> {
    let live = live_baseline_identity(layout, ref_name, active_replay)?;
    compare_worktree_baseline_with_live(layout, live)
}

/// [`compare_worktree_baseline`], for a caller that already has `ref_name`'s live identity in hand
/// from work it did anyway (0.56.0 step W2, measured need: `worktree_status` already calls
/// `resolve_folded_worktree_baseline`, which itself calls `resolve_worktree_baseline` internally --
/// re-deriving the live baseline a second, independent time through [`live_baseline_identity`] read
/// the object/pointer index an avoidable extra time, enough to trip `store_size_independence`'s own
/// ref-count ceiling. The comparison logic itself -- the one predicate R2 asks for -- lives here,
/// exactly once; [`compare_worktree_baseline`] above is the convenience wrapper for every caller that
/// has not already resolved the live side.
pub(crate) fn compare_worktree_baseline_with_live(
    layout: &RepositoryLayout,
    live: WorktreeBaselineIdentity,
) -> Result<WorktreeBaselineComparison> {
    let Some(recorded) = read_worktree_baseline(layout)? else {
        return Ok(WorktreeBaselineComparison::NoRecord);
    };
    // Deliberately `baseline` **alone** -- not `ref_name`, and not `wal_sequence` (both found by
    // testing against the real suite, not assumed up front).
    //
    // **Not `ref_name`:** the ruling's own words for the commit guard are explicit that state, not
    // name, is what governs -- "even if it named the same ref, that ref's current state has to match
    // what was recorded," which only reads one way round if a *different* ref whose current state
    // happens to match is equally fine. Measured directly: `branch create heads/topic --from
    // heads/main` (an ordinary new sibling branch, not the current one -- ouside this round's five
    // hazard cases entirely) leaves `heads/topic` at exactly `heads/main`'s own block, byte for byte,
    // until something actually changes it; a worktree recorded against `heads/main` committing onto
    // `heads/topic` immediately after is not a hazard -- nothing the new state added is missing from
    // this worktree, because there is nothing new yet. Requiring `ref_name` equality too refused that
    // real, pre-existing CLI test (`dc61_branch_closure::corruption_detection_for_pointer_loss_is_
    // unchanged_by_dc61`) outright. Every one of this round's five required hazard cases is still
    // caught by `baseline` alone: each one changes the *sealed content* a ref resolves to (a merge, an
    // adopt, a sealed claim, a freshly-created block with real content, or a genuinely different
    // sealed branch) -- `ref_name` was never load-bearing for detecting any of them, only for phrasing
    // the refusal afterward (kept for that, in `Behind`'s own fields).
    //
    // **Not `wal_sequence`:** every one of the five hazard cases moves `baseline` itself, never only
    // `wal_sequence` alone, so comparing it too catches nothing `baseline` does not already catch --
    // while actively breaking two legitimate shapes it cannot tell apart from a hazard: (1) a crash
    // between `commit`'s own WAL append and its own record-write (not bracketed by any lock-spanning
    // marker the way `branch switch`'s writes are) leaves the record naming *fewer* queued patches
    // than the WAL durably holds; retrying the identical commit (the WAL layer's own append-dedup
    // already makes that safe) must still succeed, not be refused by this guard over a record that
    // merely has not caught up to its own writer's last attempt. (2) a doctor repair verb that
    // discards or truncates damaged WAL records (`discard_damaged_commits`,
    // `Wal::truncate_trailing_partial`) legitimately *reduces* the live queue below what the record
    // last named, with the ref and sealed baseline both untouched -- not a hazard, and not one of
    // this round's required write sites either (repair is doctor's own territory, RFC 166 D5).
    //
    // Both fields stay in the stored/live identity (R1's own recommendation, and genuinely
    // informative in every message this module renders) -- they are just not part of what makes two
    // identities "the same" for the refusal/resync decision itself.
    if recorded.baseline == live.baseline {
        Ok(WorktreeBaselineComparison::Current)
    } else {
        Ok(WorktreeBaselineComparison::Behind { recorded, live })
    }
}

/// Render an identity's state in words, for a refusal or status message: "Block <id>", "Block <id>
/// with N queued patch(es) folded on top", or "an unpublished (unborn) state" (with the same queued
/// suffix when `wal_sequence` is non-zero).
pub(crate) fn describe_identity(baseline: Option<ObjectId>, wal_sequence: u64) -> String {
    let base = match baseline {
        Some(block) => format!("Block {block}"),
        None => "an unpublished (unborn) state".to_string(),
    };
    if wal_sequence == 0 {
        base
    } else {
        format!("{base} with {wal_sequence} queued patch(es) folded on top")
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::RepositoryLayout;
    use crate::test_gates::test_support::unique_temp_dir;

    fn block(byte: u8) -> ObjectId {
        ObjectId::from_bytes([byte; 32])
    }

    #[test]
    fn absent_record_reads_as_none() {
        let root = unique_temp_dir("worktree-baseline-absent");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        assert_eq!(read_worktree_baseline(&layout).expect("read"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn genesis_record_round_trips() {
        let root = unique_temp_dir("worktree-baseline-genesis");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        record_worktree_baseline(&layout, "heads/main", None, 0).expect("record");
        let recorded = read_worktree_baseline(&layout)
            .expect("read")
            .expect("some");
        assert_eq!(recorded.ref_name, "heads/main");
        assert_eq!(recorded.baseline, None);
        assert_eq!(recorded.wal_sequence, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn published_record_round_trips_and_replaces_in_place() {
        let root = unique_temp_dir("worktree-baseline-published");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        record_worktree_baseline(&layout, "heads/main", Some(block(1)), 0).expect("record");
        record_worktree_baseline(&layout, "heads/main", Some(block(2)), 3).expect("record again");
        let recorded = read_worktree_baseline(&layout)
            .expect("read")
            .expect("some");
        assert_eq!(recorded.baseline, Some(block(2)));
        assert_eq!(recorded.wal_sequence, 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn describe_identity_renders_each_shape() {
        assert_eq!(describe_identity(None, 0), "an unpublished (unborn) state");
        assert_eq!(
            describe_identity(None, 2),
            "an unpublished (unborn) state with 2 queued patch(es) folded on top"
        );
        let id = block(3);
        assert_eq!(describe_identity(Some(id), 0), format!("Block {id}"));
        assert_eq!(
            describe_identity(Some(id), 1),
            format!("Block {id} with 1 queued patch(es) folded on top")
        );
    }
}
