//! **P2 -- store-size independence, one table of operations** (RFC 160 §3.2).
//!
//! An operation on one object should cost the same whatever the store already holds. RFC 102's append-length round found three
//! per-object operations whose cost followed the whole store (an object append that read its container to learn its length, an
//! object read that read its container to decode one record, an index refresh that read the whole index after every own write) and
//! it took six weeks and a measurement to see them: nothing asserted the invariant. This file asserts it.
//!
//! **The table** ([`OPERATIONS`]) has one row per per-object operation: each row runs its operation on a **small** store and on a
//! **large** one and compares **the bytes the anchored reader was asked to read** (the read tally the fix round added), summed over
//! every path. The large store holds [`LARGE_CONTENT`] of content, the small one [`SMALL_CONTENT`] (1 MiB and 16 MiB, RFC 160's
//! suggestion); both hold the **same number of objects and the same history**, so the axis is the store's *size in bytes* -- the
//! axis RFC 133's node-count scaling confounded with breadth, and the one on which a whole-container read shows up as 15 MiB. The
//! row passes when `large <= small + allowance`, the allowance being what the operation's own records could differ by (stated per
//! row; each is a few hundred bytes -- three orders below the 15 MiB difference the store's content makes).
//!
//! **A new per-object operation joins this table in the round that adds it.** A row is the price of a new operation that touches the
//! store; an operation with no row is one nothing watches.
//!
//! **What this table does not measure**: cost that follows the *number of objects* (the index decode per operation is
//! ROADMAP AUD-01's) or the *length of history* (ref log reads, `ref-log-replay` in `whole_read_guard::SCOPES`). Those are the
//! declared scopes of the whole-read guard (P1); this table's axis is bytes.
//!
//! **0.49.0 step 5, D11/P2** (`014-review.md:290`): two more things joined the table.
//! - **A fourth axis, [`Axis::Refs`]**: the same one sealed commit, published under few and many branch names. Adding it found
//!   that "more refs" cannot be built without "more objects" in this format (every ref is its own RefState/RefUpdate), so its two
//!   rows are open findings (`Expectation::FollowsRefs`), not flat ones -- the axis could not be kept clean of the object-count
//!   axis it was meant to be independent from, and the table says so rather than hiding it behind a passing row.
//! - **A row for a command a consumer actually runs, not only a write-session primitive**: `status (ref count)`, through
//!   `worktree_status` directly (`prikk status`'s own store-level call).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use prikk_error::Result;
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectType, RefKind, RefStatePayload, RefUpdatePayload,
};

use crate::commit_boundary::worktree_patch::{
    WorktreePatchCommitOptions, commit_worktree_changes_signed,
};
use crate::extract::{ExtractPlanContent, prepare_extract_plan, read_extract_blob_content};
use crate::foundation::fsutil::read_tally;
use crate::object_store::ObjectReadSnapshot;
use crate::point::{Point, PointKind};
use crate::refs::read_current_ref_tip_block;
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::test_gates::test_support::{dummy_signature, unique_temp_dir};
use crate::worktree_status::worktree_status;
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, ObjectReader,
    ObjectWriteSession, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    add_trusted_maintainer, maintainer_signature,
};

const REF_NAME: &str = "heads/main";
const SMALL_CONTENT: usize = 1 << 20;
const LARGE_CONTENT: usize = 16 << 20;

fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("p2-maintainer", &[0x62; 32]).expect("signer")
}

fn author() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("p2-author", &[0x61; 32]).expect("signer")
}

/// Incompressible, deterministic bytes.
fn content(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_be_bytes()[0]
        })
        .collect()
}

/// A repository holding `content_bytes` of content across **four** blobs of that total, one committed and sealed generation: the same
/// objects and the same history whatever the size.
fn store_of(content_bytes: usize) -> Result<(RepositoryLayout, std::path::PathBuf)> {
    let root = unique_temp_dir(&format!("p2-store-{content_bytes}"));
    let layout = RepositoryLayout::init(root.clone())?;
    let maintainer = maintainer();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    let per_file = content_bytes / 4;
    for index in 0..4_u64 {
        std::fs::write(
            layout.root().join(format!("data{index}.bin")),
            content(per_file, index + 1),
        )?;
    }
    commit_worktree_changes_signed(
        &layout,
        REF_NAME,
        "p2 content",
        WorktreePatchCommitOptions::default(),
        &author(),
    )?;
    simulate_one_seal(&layout, REF_NAME, &maintainer)?;
    Ok((layout, root))
}

/// One row: a name, what the operation is, its allowance in bytes and why, and the operation itself (run against a store; the tally
/// is reset immediately before it and read immediately after).
struct Operation {
    name: &'static str,
    axis: Axis,
    expectation: Expectation,
    allowance: u64,
    why: &'static str,
    run: fn(&RepositoryLayout) -> Result<()>,
}

/// Which way the store grows between the small and the large one. **The two axes are separate on purpose**: a read of a container
/// follows the content's bytes; a read of the index follows the objects' count, and 16 MiB of content in a few blobs adds almost no
/// index (RFC 133's scaling grew both together and could tell neither apart).
#[derive(Clone, Copy)]
enum Axis {
    /// The same objects and history holding 1 MiB and 16 MiB of content.
    Content,
    /// The same bytes of content in [`SMALL_OBJECTS`] and [`LARGE_OBJECTS`] objects.
    Count,
    /// The same tiny files, committed and sealed [`SMALL_HISTORY`] and [`LARGE_HISTORY`] times: the ref log and the block chain grow.
    History,
    /// 0.49.0 step 5, D11/P2 (the external review's own `014-review.md:290`, "a refs axis"): the same one sealed commit, published
    /// under [`SMALL_REFS`] and [`LARGE_REFS`] branch names -- the axis RFC 165 R2 fixed a quadratic-shaped cost on
    /// (`current-state.md`'s own measured "a commit's cost follows the number of refs, roughly squared"). This table's job for this
    /// axis is to keep that fix fixed, not to re-discover it.
    Refs,
    /// 0.53.0 step 3B (M6): the same lineage, with [`SMALL_QUEUE`] and [`LARGE_QUEUE`] already-queued, unsealed prior commits ahead
    /// of the one this axis's own operation actually measures. A commit's own pre-append integrity check re-reads every
    /// already-queued record before stacking one more on it; that cost stays linear in the queue's own depth on purpose, so a
    /// damaged queued commit is always noticed before more work is stacked on top of it. Unlike [`Axis::Content`]'s F4 row, this
    /// one is not expected to go flat -- it is the standing regression guard that the check still costs three passes over the
    /// queue, not four (the fourth, removed in 0.53.0, ran no check of its own).
    Queue,
    /// 0.54.0 step 1 item 2 (external review 029, 5.1): a fixed, sealed lineage [`CHECKPOINT_GAP`] blocks past its one checkpoint
    /// (genesis, always one -- `checkpoint_due`, `block_state.rs`, returns `Ok(true)` unconditionally when a new block's parent is
    /// `None`), with [`SMALL_SINCE_CHECKPOINT`] and [`LARGE_SINCE_CHECKPOINT`] the *total* content split across those
    /// [`CHECKPOINT_GAP`] generations -- the same checkpoint distance in both, so the axis is the content added since the
    /// checkpoint alone, not the gap's own depth (that would be [`Axis::History`]'s job). Unlike [`Axis::Content`]'s `one-file
    /// commit` row (measured *at* a checkpoint, the flat best case), this fixture's own tip is cold and the nearest checkpoint is
    /// always [`CHECKPOINT_GAP`] blocks behind it.
    CheckpointGap,
}

const SMALL_HISTORY: usize = 4;
const LARGE_HISTORY: usize = 64;

/// A repository with `generations` committed-and-sealed generations of one tiny file each (RFC 111's cost gates build theirs the same
/// way).
fn store_of_history(generations: usize) -> Result<(RepositoryLayout, std::path::PathBuf)> {
    let root = unique_temp_dir(&format!("p2-history-{generations}"));
    let layout = RepositoryLayout::init(root.clone())?;
    let maintainer = maintainer();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    for index in 0..generations {
        std::fs::write(
            layout.root().join(format!("g{index}.txt")),
            format!("generation {index}\n"),
        )?;
        commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "p2 generation",
            WorktreePatchCommitOptions::default(),
            &author(),
        )?;
        simulate_one_seal(&layout, REF_NAME, &maintainer)?;
    }
    Ok((layout, root))
}

/// 1 and 400 (`dc59_commit_benchmark.rs`'s own own benchmark range, where the fix this axis guards measured 1.4 ms at 1 ref and
/// 72.1 ms at 400 -- the shape, not the exact count, is what matters here).
const SMALL_REFS: usize = 1;
const LARGE_REFS: usize = 400;

/// A repository with one sealed commit on `heads/main`, and `refs - 1` more branches (`heads/branch-0`, `heads/branch-1`, ...) each
/// published to point at that identical target block -- the same object content and the same one commit's worth of history
/// whatever `refs` is, so the axis is the ref count alone. Mirrors `prikk branch create --from heads/main`'s own store-level shape
/// (RefState/RefUpdate at `update_seq` 1, no previous state, same target) without the CLI's own key-file/signing plumbing, the same
/// reasoning `simulate_one_seal` already has for sealing.
fn store_of_refs(refs: usize) -> Result<(RepositoryLayout, std::path::PathBuf)> {
    let (layout, root) = store_of_history(1)?;
    let maintainer = maintainer();
    let ref_store = RefStore::new(layout.clone());
    let main_ref_state_id = ref_store
        .read_current_ref_state_id(REF_NAME)?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("heads/main has no state".to_string()))?;
    let object_store = ObjectWriteSession::open(&layout)?;
    let main_state = object_store
        .read_typed(main_ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("heads/main's RefState missing".to_string())
        })?;
    let main_payload = RefStatePayload::decode_canonical(
        &main_state.canonical_payload,
        main_state.schema_version,
    )?;
    for index in 0..refs.saturating_sub(1) {
        let branch_name = format!("heads/branch-{index}");
        let ref_state_payload = RefStatePayload {
            ref_name: branch_name.clone(),
            kind: RefKind::Branch,
            target_object_id: main_payload.target_object_id,
            update_seq: 1,
            previous_ref_state_id: None,
            required_attestation_ids: Vec::new(),
            closed: false,
        };
        let mut ref_state_envelope = ObjectEnvelope::unsigned(
            ObjectType::RefState,
            1,
            ref_state_payload.to_canonical_bytes()?,
        );
        let ref_state_id = ref_state_envelope.object_id();
        ref_state_envelope.add_signature(maintainer_signature(
            &maintainer,
            ObjectType::RefState,
            ref_state_id,
        )?)?;
        let ref_update_payload = RefUpdatePayload {
            ref_name: branch_name.clone(),
            old_ref_state_id: None,
            new_ref_state_id: ref_state_id,
            new_target_object_id: main_payload.target_object_id,
            update_seq: 1,
            created_at: 0,
            author_key_id: maintainer.key_id().to_string(),
        };
        let mut ref_update_envelope = ObjectEnvelope::unsigned(
            ObjectType::RefUpdate,
            1,
            ref_update_payload.to_canonical_bytes()?,
        );
        let ref_update_id = ref_update_envelope.object_id();
        ref_update_envelope.add_signature(maintainer_signature(
            &maintainer,
            ObjectType::RefUpdate,
            ref_update_id,
        )?)?;
        ref_store.publish(&RefPublication {
            ref_name: branch_name,
            expected_previous_ref_state_id: None,
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        })?;
    }
    Ok((layout, root))
}

/// 0.53.0 step 3B (M6), the design round's own two end points (100 and 3,200 queued commits,
/// `0.53.0-step-2-B-m6-design-report.md`). [`LARGE_QUEUE`] was 3,200 until release prep P1b: building that fixture is
/// quadratic in its own depth by construction (every queued commit re-reads the whole queue so far), which took the
/// release build to 17-18s but a CI debug build well past ten minutes, cancelling `msrv-1.85.0` at its own 20-minute
/// job limit (release-0.53.0-prep-P1-review-v1). 800, the design round's own middle point, keeps about 16x less
/// fixture-building work while still separating three full queue passes (today) from four (the regression control 2
/// below still catches) at the same order of magnitude the design round itself measured.
const SMALL_QUEUE: usize = 100;
const LARGE_QUEUE: usize = 800;

/// DC-57's own default hard block (`DEFAULT_ACTIVE_PATCH_LIMIT`, 1,000) refuses the 1,001st queued commit under a repository's
/// default configuration -- a real, shipped limit (RFC 166 §2), not an artifact of this fixture. [`LARGE_QUEUE`] no longer
/// needs to pass it (800 is under 1,000), but this axis still raises the limit the same way the CLI does from
/// `PRIKK_ACTIVE_PATCH_LIMIT` (`WorktreePatchCommitOptions::with_active_patch_limit`) -- an administrative choice that already
/// exists, not a bypass invented for this table -- so this fixture stays independent of wherever that default is set.
fn queue_axis_commit_options() -> WorktreePatchCommitOptions {
    WorktreePatchCommitOptions::default().with_active_patch_limit(LARGE_QUEUE + 16)
}

/// 0.53.0 step 3B round 2 (Q-1): the one file every queued commit in [`store_of_queue`]/[`queued_commit`]
/// rewrites, so the tracked tree never grows with the queue's own depth. The architect's review found
/// the previous fixture (a new `q{index}.txt` per queued commit) made the "large" store also track 3,200
/// distinct files against 100 in the "small" store, so the object index, the blob container and the
/// commit-index cache all grew with the *tree*, not the queue -- conflating two different axes in one
/// measured number.
const QUEUE_AXIS_FILE: &str = "queued.txt";

/// A repository with one sealed commit on `heads/main`, then `queued` further commits authored and queued but never sealed --
/// the same lineage and the same one sealed generation whatever `queued` is, so the axis is the queue's own depth alone, not
/// history depth (`store_of_history`'s axis) or stored content (`store_of`'s). 0.53.0 step 3B round 2 (Q-1): every queued
/// commit rewrites the same [`QUEUE_AXIS_FILE`], never a new file, so the tracked tree stays exactly one file's worth of size
/// and count throughout -- only the queue's own depth (the count of queued, unsealed records in `queue.wal`) varies between
/// the small and large fixture.
fn store_of_queue(queued: usize) -> Result<(RepositoryLayout, std::path::PathBuf)> {
    let (layout, root) = store_of_history(1)?;
    for index in 0..queued {
        std::fs::write(
            layout.root().join(QUEUE_AXIS_FILE),
            format!("queued {index}\n"),
        )?;
        commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "p2 queued",
            queue_axis_commit_options(),
            &author(),
        )?;
    }
    Ok((layout, root))
}

/// Well under [`CHECKPOINT_CADENCE`] (64, `block_state.rs`'s own constant for the second checkpoint's own distance, confirmed by
/// hand-tracing `checkpoint_due` 0.53.0 P4: counting the Root as block 0, the second checkpoint is due at block 64, not 63) -- so
/// this fixture's own tip has exactly one checkpoint behind it, genesis, [`CHECKPOINT_GAP`] blocks back, never a second one.
const CHECKPOINT_GAP: usize = 32;

/// Genesis's own content (`store_of_checkpoint_gap`'s first commit), identical in both the small and large store -- large enough
/// that a full replay from genesis (the "perturb the anchor away" control) reads well past 1.5x either store's own anchored
/// figure, so losing the checkpoint is a measurable regression, not noise.
const CHECKPOINT_GAP_GENESIS_CONTENT: usize = 24 << 20;

/// The two stores' own total content added across [`CHECKPOINT_GAP`] further sealed generations past genesis -- 1 MiB and 16
/// MiB, external review 029's own "1 MiB against 16 MiB" suggestion (`0.54.0-step-1-small-items-handoff-v1.md` item 2).
const SMALL_SINCE_CHECKPOINT: usize = 1 << 20;
const LARGE_SINCE_CHECKPOINT: usize = 16 << 20;

/// A sealed lineage whose nearest checkpoint is genesis, exactly [`CHECKPOINT_GAP`] blocks behind the tip: genesis holds
/// [`CHECKPOINT_GAP_GENESIS_CONTENT`] (the same in both stores), and each of the [`CHECKPOINT_GAP`] generations since it rewrites
/// one ever-growing file by `since_checkpoint_bytes / CHECKPOINT_GAP` more bytes -- the *only* thing that varies between the
/// small and large store on this axis is `since_checkpoint_bytes`'s own total, never the checkpoint's own distance. The
/// incremental lifecycle-state cache (DC-64, `lifecycle_cache/incremental.rs::CACHE_FILE_NAME`) is removed at the end, so the
/// next commit this fixture feeds `one_file_commit` cannot reuse a resolved baseline and must derive cold, from the nearest
/// checkpoint forward -- the admitted-checkpoint cache (`.prikk/cache/replay-verified-blocks.v1`) stays, which is what lets that
/// cold derivation anchor at genesis instead of replaying the whole lineage (RFC 159 stack B, F4).
fn store_of_checkpoint_gap(
    since_checkpoint_bytes: usize,
) -> Result<(RepositoryLayout, std::path::PathBuf)> {
    let root = unique_temp_dir(&format!("p2-checkpoint-gap-{since_checkpoint_bytes}"));
    let layout = RepositoryLayout::init(root.clone())?;
    let maintainer = maintainer();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    std::fs::write(
        layout.root().join("genesis.bin"),
        content(CHECKPOINT_GAP_GENESIS_CONTENT, 1),
    )?;
    commit_worktree_changes_signed(
        &layout,
        REF_NAME,
        "p2 checkpoint genesis",
        WorktreePatchCommitOptions::default(),
        &author(),
    )?;
    simulate_one_seal(&layout, REF_NAME, &maintainer)?;
    let per_generation = since_checkpoint_bytes / CHECKPOINT_GAP;
    for index in 0..CHECKPOINT_GAP {
        std::fs::write(
            layout.root().join("growing.bin"),
            // Odd and strictly increasing: `content`'s own `seed | 1` collapses any two seeds that
            // differ only in their lowest bit (2 and 3 alike become 3) to the identical stream --
            // harmless for `store_of`'s own seeds (each writes a distinct file), fatal here, where
            // consecutive generations rewrite the *same* file and a collision reads as "no change".
            content(per_generation, (index as u64) * 2 + 101),
        )?;
        commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "p2 since checkpoint",
            WorktreePatchCommitOptions::default(),
            &author(),
        )?;
        simulate_one_seal(&layout, REF_NAME, &maintainer)?;
    }
    let _ = std::fs::remove_file(layout.cache_dir().join("lifecycle-state.v1"));
    Ok((layout, root))
}

const SMALL_OBJECTS: usize = 16;
const LARGE_OBJECTS: usize = 2048;

/// A repository holding `count` small blobs written through one session (no history: the axis is the index's length).
fn store_of_count(count: usize) -> Result<(RepositoryLayout, std::path::PathBuf)> {
    let root = unique_temp_dir(&format!("p2-count-{count}"));
    let layout = RepositoryLayout::init(root.clone())?;
    let mut session = ObjectWriteSession::open(&layout)?;
    for index in 0..count {
        session.write_object(&small_blob(&format!("filler {index}")))?;
    }
    Ok((layout, root))
}

/// What the table expects of a row today.
enum Expectation {
    /// The operation's cost does not follow the store's size (`large <= small + allowance`).
    Flat,
    /// **An open finding on the content axis** (0.53.0 step 3A round 2, R-5: restored from
    /// `06ea56a4^`, where it was removed when F4 closed its one user -- 3B's own queue axis needs
    /// the identical floor/ceiling mechanism, so it is re-added here rather than reinvented):
    /// the operation reads more as the store's stored content grows (`large >= small + 8 MiB`).
    /// **The floor** (this promotion check) catches a fix: once a finding's own row reads flat, the
    /// row is promoted to `Flat` and the finding removed, never left as an open row with nothing
    /// to catch. **The ceiling** (the ceiling field below) catches a regression instead: the
    /// finding getting worse, not just failing to improve, which the floor alone would miss. Both
    /// checks are the same promotion rule [`Expectation::FollowsHistory`]/[`Expectation::FollowsRefs`]/
    /// the future queue axis apply to their own axis.
    ///
    /// **0.53.0 step 3B (M6): generalized to carry its own floor** (the 3A round 2 review's own
    /// carried-over item), rather than the hard-coded `8 << 20` the content axis alone needed --
    /// the queue axis's own row (below) reads hundreds of KB to a few MB at 100 to 3,200 queued
    /// commits, three orders below the content axis's 1-to-16-MiB range, so one hard-coded floor
    /// cannot serve both. **The content axis's own behaviour is unchanged by this**: a future
    /// content-axis row passing `floor: 8 << 20` reproduces exactly what the old hard-coded
    /// constant checked, byte for byte -- confirmed by running this file's own full test before and
    /// after this change and diffing its output (identical, since no content-axis row exists yet to
    /// exercise the difference either way).
    FollowsContent {
        finding: &'static str,
        /// The floor (this promotion check): below `small + floor`, the finding is closed and the
        /// row is promoted to [`Expectation::Flat`]. Sized per axis from what a genuinely flat
        /// version of the operation would read, so the floor catches a real fix without false-passing
        /// one that only *looks* flat within the far-larger range a shared constant would allow.
        floor: u64,
        /// **RFC 160 §9's ceiling on P2's open rows** (the external review's D11): set to 1.5x
        /// what the operation measured when this ceiling was written; the report gives the exact
        /// numbers.
        ceiling: u64,
    },
    /// **An open finding on the history axis**: the operation reads more as the history grows (`large >= small + 16 KiB`), the ref log
    /// and the object index being what grows. **The floor** (this promotion check) catches a fix, promoting the row to `Flat` and
    /// removing the finding; **the ceiling** (below) catches a regression instead -- the finding getting worse, not just failing to
    /// improve, which the floor alone would miss. Same promotion rule as [`Expectation::FollowsContent`].
    FollowsHistory {
        finding: &'static str,
        /// Same ceiling, same reason, for the history axis.
        ceiling: u64,
    },
    /// **An open finding on the refs axis** (0.49.0 step 5, D11/P2: found by adding the axis, not assumed): the operation reads
    /// more as the ref count grows (`large >= small + (16 << 10)`), because every additional ref is at least one more RefState and
    /// RefUpdate object -- "more refs" cannot be constructed without "more objects" in this format (`RefStatePayload`/
    /// `RefUpdatePayload` each encode their own `ref_name`, so a ref cannot share another ref's object). The object index
    /// (ROADMAP AUD-01's own linear-scan cost) and the ref-pointer index both grow with it; RFC 165 R2's fix (no longer reading
    /// every ref's whole *history*) is a different cost and stays fixed, which is why this is `FollowsRefs`, not a report that R2
    /// regressed. Same promotion rule as [`Expectation::FollowsHistory`].
    FollowsRefs {
        finding: &'static str,
        /// Same ceiling, same reason, for the refs axis.
        ceiling: u64,
    },
}

fn small_blob(label: &str) -> ObjectEnvelope {
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, format!("p2 blob {label}").into_bytes());
    envelope
        .add_signature(dummy_signature())
        .expect("signature");
    envelope
}

/// **Object append** (through a write session, as a commit does it): the container's length comes from the descriptor, the index
/// entry is written and read back as its own tail. Nothing of the store's size is read.
fn object_append(layout: &RepositoryLayout) -> Result<()> {
    let mut session = ObjectWriteSession::open(layout)?;
    // The session's own open decodes the index once; the measured region is the writes after it.
    read_tally::reset();
    session.write_object(&small_blob("append one"))?;
    session.write_object(&small_blob("append two"))?;
    Ok(())
}

/// **Object read** through an open snapshot: one record, from its own frame.
fn object_read(layout: &RepositoryLayout) -> Result<()> {
    let mut session = ObjectWriteSession::open(layout)?;
    let id = session.write_object(&small_blob("read me"))?;
    let snapshot = ObjectWriteSession::open(layout)?;
    read_tally::reset();
    let read = snapshot.read_object(id)?;
    assert!(read.is_some(), "the object was read back");
    Ok(())
}

/// **The index refresh after a session's own write**: a write while a stale snapshot is open makes the session refresh from the tail;
/// what is read is the bytes appended since, not the index.
fn index_refresh_after_an_own_write(layout: &RepositoryLayout) -> Result<()> {
    let mut first = ObjectWriteSession::open(layout)?;
    let mut second = ObjectWriteSession::open(layout)?;
    first.write_object(&small_blob("first writer"))?;
    // `second` is now stale: its next write refreshes from the tail.
    read_tally::reset();
    second.write_object(&small_blob("second writer"))?;
    Ok(())
}

/// **A one-file commit through the store API.**
fn one_file_commit(layout: &RepositoryLayout) -> Result<()> {
    std::fs::write(layout.root().join("one.txt"), b"one small file\n")?;
    read_tally::reset();
    commit_worktree_changes_signed(
        layout,
        REF_NAME,
        "p2 one file",
        WorktreePatchCommitOptions::default(),
        &author(),
    )?;
    Ok(())
}

/// **A one-block seal** (the store-level replica of `prikk seal`'s own sequence, RFC 111's `simulate_one_seal`), of the commit the
/// row above's setup made.
fn one_block_seal(layout: &RepositoryLayout) -> Result<()> {
    std::fs::write(layout.root().join("seal-me.txt"), b"seal me\n")?;
    commit_worktree_changes_signed(
        layout,
        REF_NAME,
        "p2 seal me",
        WorktreePatchCommitOptions::default(),
        &author(),
    )?;
    read_tally::reset();
    simulate_one_seal(layout, REF_NAME, &maintainer())?;
    Ok(())
}

/// **`prikk status`'s own store-level call** (0.49.0 step 5, D11/P2, "the commands consumers call" --
/// `014-review.md:290` -- every row above measures an internal write-session primitive; nothing before this one
/// measured a command a consumer runs directly, and this is the one run far more often than any write).
fn status_check(layout: &RepositoryLayout) -> Result<()> {
    read_tally::reset();
    let _report = worktree_status(layout, REF_NAME)?;
    Ok(())
}

/// **0.53.0 step 3B (M6): one more commit against an already-deep queue.** The queue itself
/// (`store_of_queue`'s own setup) is already in place before the tally resets; this measures only
/// the one additional commit's own pre-append integrity check over whatever was already queued.
/// 0.53.0 step 3B round 2 (Q-1): rewrites the same [`QUEUE_AXIS_FILE`] `store_of_queue` used, not a
/// new file, so this measured commit adds nothing to the tracked tree either.
fn queued_commit(layout: &RepositoryLayout) -> Result<()> {
    std::fs::write(
        layout.root().join(QUEUE_AXIS_FILE),
        b"one more queued commit\n",
    )?;
    read_tally::reset();
    commit_worktree_changes_signed(
        layout,
        REF_NAME,
        "p2 one more queued",
        queue_axis_commit_options(),
        &author(),
    )?;
    Ok(())
}

/// **0.54.0 step 3 Part 4: `prikk extract`'s own whole, real cost** -- not planning alone (Part
/// 3c's own correction: a planning-only figure, presented without saying so, reads as the whole
/// command's own cost, which it is not). This mirrors `crates/prikk-cli/src/extract.rs`'s own
/// `write_plan` exactly: `prepare_extract_plan` builds the plan (never reading a `Blob` entry's own
/// content, by the streaming design `patch_replay::extract` keeps), then every `Blob` entry is read
/// once via `read_extract_blob_content`, the identical read `write_plan` makes before writing each
/// file -- a `Bytes` entry (a text file edited since the admitted anchor) is already resident and
/// needs no further read, so it is skipped here exactly as `write_plan` skips it.
fn extract_check(layout: &RepositoryLayout) -> Result<()> {
    let reader = ObjectReadSnapshot::open(layout)?;
    let tip = read_current_ref_tip_block(layout, &reader, REF_NAME)?;
    let point = Point {
        name: REF_NAME.to_string(),
        kind: PointKind::LocalBranch,
        block_id: tip,
    };
    read_tally::reset();
    let plan = prepare_extract_plan(layout, &reader, &point, false)?;
    for entry in &plan.entries {
        if let ExtractPlanContent::Blob(blob_id) = &entry.content {
            let _ = read_extract_blob_content(&reader, *blob_id)?;
        }
    }
    Ok(())
}

const OPERATIONS: &[Operation] = &[
    Operation {
        name: "object append",
        axis: Axis::Content,
        expectation: Expectation::Flat,
        allowance: 512,
        why: "two appends, each reading back its own index entry (about 133 bytes)",
        run: object_append,
    },
    Operation {
        name: "object read",
        axis: Axis::Content,
        expectation: Expectation::Flat,
        allowance: 0,
        why: "one record read from its own frame; identical bytes on any store",
        run: object_read,
    },
    Operation {
        name: "index refresh after an own write",
        axis: Axis::Content,
        expectation: Expectation::Flat,
        allowance: 512,
        why: "the tail since the stale snapshot: one or two index entries",
        run: index_refresh_after_an_own_write,
    },
    Operation {
        name: "one-file commit",
        axis: Axis::Content,
        // 0.53.0 step 3 (F4): the cold baseline now derives from the nearest trusted checkpoint
        // (RFC 159 stack B) instead of a full lineage replay, so it no longer reads every stored
        // blob's frame to learn its kind. Promoted from `FollowsContent` -- its F4 finding is closed.
        expectation: Expectation::Flat,
        allowance: 4096,
        why: "authoring one patch against an anchored baseline: the block and patch records it needs, the same on both stores",
        run: one_file_commit,
    },
    Operation {
        name: "one-block seal",
        axis: Axis::Content,
        expectation: Expectation::Flat,
        allowance: 4096,
        why: "one seal: the state derivation from the tip's checkpoint, the same on both stores",
        run: one_block_seal,
    },
    Operation {
        name: "object append (object count)",
        axis: Axis::Count,
        expectation: Expectation::Flat,
        allowance: 512,
        why: "two appends, each reading back its own index entry: the index's length is not in it",
        run: object_append,
    },
    Operation {
        name: "object read (object count)",
        axis: Axis::Count,
        expectation: Expectation::Flat,
        allowance: 0,
        why: "one record from its own frame",
        run: object_read,
    },
    Operation {
        name: "index refresh after an own write (object count)",
        axis: Axis::Count,
        expectation: Expectation::Flat,
        allowance: 512,
        why: "the tail since the stale snapshot: one or two index entries, not the index",
        run: index_refresh_after_an_own_write,
    },
    Operation {
        name: "one-file commit (history depth)",
        axis: Axis::History,
        expectation: Expectation::FollowsHistory {
            finding: "F1 in the report: reads that follow the history -- the ref log whole up to three times per publication, the pointer index and the object index whole (AUD-01), earlier blocks and ref states by frame",
            ceiling: 402_989, // 1.5x the 268,659 bytes measured for RFC 160 R3's report
        },
        allowance: 4096,
        why: "authoring one patch after 4 or 64 sealed generations",
        run: one_file_commit,
    },
    Operation {
        name: "one-block seal (history depth)",
        axis: Axis::History,
        expectation: Expectation::FollowsHistory {
            finding: "F1 in the report: reads that follow the history -- the ref log whole up to three times per publication, the pointer index and the object index whole (AUD-01), earlier blocks, patches and ref states by frame",
            ceiling: 411_386, // 1.5x the 274,257 bytes measured for RFC 160 R3's report
        },
        allowance: 4096,
        why: "one seal after 4 or 64 sealed generations",
        run: one_block_seal,
    },
    // 0.49.0 step 5, D11/P2 (`014-review.md:290`, "a refs axis"): RFC 165 R2 already fixed a commit's own write-path precondition
    // (`ensure_no_incomplete_publication`) from reading every ref's whole history to one pass over the pointer index and the ref
    // log, measured in `current-state.md` at 6.8 s -> 4.4 ms at 4,000 refs. This row is the standing regression guard for that fix,
    // not a re-discovery of the finding it fixed.
    Operation {
        name: "one-file commit (ref count)",
        axis: Axis::Refs,
        expectation: Expectation::FollowsRefs {
            finding: "every new branch in `store_of_refs` is one more RefState and RefUpdate object (the format gives each ref its \
                       own, by `ref_name`); the object index (AUD-01) and the ref-pointer index containers both read whole and both \
                       grow with the object count this creates, measured at 5,335 -> 535,057 bytes from 1 to 400 refs (0.49.0 step 5, \
                       D11/P2)",
            ceiling: 802_586, // 1.5x the 535,057 bytes measured when this ceiling was written
        },
        allowance: 4096,
        why: "authoring one patch: the baseline replay reads the block and patch records it needs, independent of how many other \
              refs exist -- RFC 165 R2's own fix (no longer reading every ref's whole history) stays fixed; the index/pointer-index \
              growth above is a different, open cost",
        run: one_file_commit,
    },
    Operation {
        name: "status (ref count)",
        axis: Axis::Refs,
        expectation: Expectation::FollowsRefs {
            finding: "the same object-index/pointer-index growth as the row above, reached through `worktree_status` instead of a \
                       commit: measured at 3,215 -> 219,253 bytes from 1 to 400 refs (0.49.0 step 5, D11/P2)",
            ceiling: 328_880, // 1.5x the 219,253 bytes measured when this ceiling was written
        },
        allowance: 4096,
        why: "`worktree_status` compares the worktree against the replay baseline for one ref; the other refs are not on its \
              commit/seal path, but the object index and pointer index it reads are shared",
        run: status_check,
    },
    Operation {
        name: "one-file commit (queue depth)",
        axis: Axis::Queue,
        expectation: Expectation::FollowsContent {
            finding: "a commit against an already-queued fixture makes two full passes over `queue.wal` under the same \
                       held active-session lock -- `refs::ensure_publication_precondition_with_replay`'s own \
                       `has_incomplete_active_cleanup_with_replay` (`refs/evidence.rs`) and `author_inner`'s own \
                       `active_replay` (`node_authoring.rs`, feeding the tail/damage/active-patch-limit checks, the \
                       RFC 166 witness classification and the worktree baseline fold over queued patches) now share one \
                       decode, and `append_patch_reporting`'s own replay at the durable append (`wal.rs`, via \
                       `append_patch_and_witness`) stays separate, by the owner's own kept decision -- so this row is \
                       linear in the queue's own depth on purpose, so that a damaged queued commit is always noticed \
                       before more work is stacked on top of it. Before 0.54.0 the publication-cleanup check paid for \
                       its own, third, separate decode; that pass is gone. Every queued commit in this fixture rewrites \
                       one file, never a new one, so only the queue's own depth varies, not the tracked tree: measured \
                       at 107,074 -> 808,781 bytes from 100 to 800 queued commits (0.54.0-step-1-report.md). This row's \
                       own ceiling is what still catches a regression back to the third pass",
            floor: 16 << 10,
            // Not 1.5x (1,213,172): restoring the publication-cleanup check's own separate decode (0.54.0 step 1,
            // measured directly on this build by reintroducing it) reproduces the pre-0.54.0 three-pass figure almost
            // exactly, 1,209,604 bytes on the large store -- about 1.50x, but a literal 1.5x ceiling (1,213,172) sits
            // just *above* that regression and would not catch it. 1,000,000 sits strictly between today's figure
            // (808,781) and the measured regression (1,209,604).
            ceiling: 1_000_000,
        },
        allowance: 4096,
        why: "one more commit against an already-queued fixture: the pre-append check's own cost follows the queue it re-checks, \
              which is the design this row stands guard over, not a defect -- a return to three full reads (the pre-0.54.0 \
              shape) is what the ceiling exists to catch",
        run: queued_commit,
    },
    Operation {
        name: "one-file commit (since checkpoint)",
        axis: Axis::CheckpointGap,
        expectation: Expectation::FollowsContent {
            finding: "external review 029, 5.1: a cold commit between checkpoints costs as much as the content added since the \
                       nearest one, up to 63 blocks -- the `one-file commit` row above is measured right at a checkpoint, the \
                       best case this one is not. Fixed checkpoint distance (genesis, 32 blocks behind the tip, both stores), \
                       `lifecycle-state.v1` removed so the commit is cold: measured at 2,199,724 -> 33,165,486 bytes from 1 to \
                       16 MiB added since the checkpoint (0.54.0-step-1-report.md)",
            floor: 8 << 20,
            ceiling: 49_748_229, // 1.5x the 33,165,486 bytes measured when this ceiling was written
        },
        allowance: 4096,
        why: "a cold commit's baseline derivation walks forward from the nearest checkpoint through every generation since -- \
              reading each one's own patch and blob to re-derive the files it touched -- so its cost follows the content added \
              since the checkpoint on purpose, not the store's own total content (the row above, measured at a checkpoint, \
              stays flat); a cost that instead followed the *whole* lineage (the anchor lost) is what the ceiling exists to \
              catch",
        run: one_file_commit,
    },
    Operation {
        name: "extract (since checkpoint)",
        axis: Axis::CheckpointGap,
        expectation: Expectation::FollowsContent {
            finding: "0.54.0 step 3 Part 4: extract's own whole command (the plan plus every Blob \
                       entry's one write-time read, mirroring `write_plan`) streams every file \
                       whose content has not changed since the admitted anchor straight from its \
                       stored blob id -- so its cost follows the content added since the checkpoint \
                       (the one file this fixture rewrites each generation) on purpose, not the \
                       store's own total content: measured at 26,303,721 -> 42,523,881 bytes from 1 \
                       to 16 MiB added since the checkpoint (this report)",
            floor: 8 << 20,
            ceiling: 63_785_822, // 1.5x the 42,523,881 bytes measured when this ceiling was written
        },
        allowance: 4096,
        why: "0.54.0 step 3 Part 4: extract's own whole command (plan plus every Blob entry's one \
              read, mirroring `write_plan`) streams every file whose content has not changed since \
              the admitted anchor straight from its stored blob id, one at a time, by design -- so \
              its cost follows the content added since the checkpoint (the single growing file this \
              fixture rewrites) on purpose, not the store's own total content; a cost that instead \
              followed the *whole* lineage (the admitted anchor lost, every binary held resident \
              again) is what the ceiling exists to catch",
        run: extract_check,
    },
];

/// The small and the large store of the operation's own axis: `(small, large)` are the two sizes on that axis.
fn sizes(axis: Axis) -> (usize, usize) {
    match axis {
        Axis::Content => (SMALL_CONTENT, LARGE_CONTENT),
        Axis::Count => (SMALL_OBJECTS, LARGE_OBJECTS),
        Axis::History => (SMALL_HISTORY, LARGE_HISTORY),
        Axis::Refs => (SMALL_REFS, LARGE_REFS),
        Axis::Queue => (SMALL_QUEUE, LARGE_QUEUE),
        Axis::CheckpointGap => (SMALL_SINCE_CHECKPOINT, LARGE_SINCE_CHECKPOINT),
    }
}

type Tally = std::collections::BTreeMap<std::path::PathBuf, u64>;

/// Build a store of `size` on the operation's axis, run the operation with the tally reset just before it (the operation may reset
/// it again after its own set-up), and return the bytes read in total and by path.
fn measure(operation: &Operation, size: usize) -> Result<(u64, Tally)> {
    let (layout, root) = match operation.axis {
        Axis::Content => store_of(size)?,
        Axis::Count => store_of_count(size)?,
        Axis::History => store_of_history(size)?,
        Axis::Refs => store_of_refs(size)?,
        Axis::Queue => store_of_queue(size)?,
        Axis::CheckpointGap => store_of_checkpoint_gap(size)?,
    };
    read_tally::reset();
    (operation.run)(&layout)?;
    let by_path = read_tally::snapshot();
    let total = by_path.values().sum();
    let _ = std::fs::remove_dir_all(root);
    Ok((total, by_path))
}

/// **The table's assertion.** Each operation, on the small store and on the large one **of its own axis** (content bytes: 1 MiB and
/// 16 MiB; object count: 16 and 2,048 objects), reads the same bytes (within its allowance): the store's size is not in its cost.
/// **Perturb (RFC 160 §5):** put `index.rs`'s whole-container length read back in the append -- `object append` goes red (and the
/// whole-read guard, P1, fails first; this row is what stays red where a scope hides the read); put S-2's whole container read back
/// in the object read -- `object read` goes red; put site C's whole index read back -- `index refresh after an own write` goes red.
#[test]
fn no_per_object_operation_reads_more_on_a_larger_store() -> Result<()> {
    let mut report = String::new();
    let mut failures = Vec::new();
    for operation in OPERATIONS {
        let (small_size, large_size) = sizes(operation.axis);
        let (small, small_paths) = measure(operation, small_size)?;
        let (large, large_paths) = measure(operation, large_size)?;
        report.push_str(&format!(
            "{}\t{small}\t{large}\t{}\t{}\n",
            operation.name,
            i128::from(large) - i128::from(small),
            operation.allowance
        ));
        if let Ok(path) = std::env::var("PRIKK_STORE_SIZE_REPORT") {
            let mut detail = String::new();
            for (label, paths) in [("small", &small_paths), ("large", &large_paths)] {
                for (path, bytes) in paths {
                    detail.push_str(&format!(
                        "{}\t{label}\t{}\t{bytes}\n",
                        operation.name,
                        path.display()
                    ));
                }
            }
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| std::io::Write::write_all(&mut file, detail.as_bytes()));
        }
        match &operation.expectation {
            Expectation::Flat if large > small + operation.allowance => failures.push(format!(
                "{}: {small} bytes read on the small store ({small_size}), {large} on the large ({large_size}) (allowance {}: {}). \
                 By path (small / large): {small_paths:?} / {large_paths:?}",
                operation.name,
                operation.allowance,
                operation.why
            )),
            Expectation::FollowsContent { finding, floor, ceiling: _ } if large < small + floor => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes on the small / large store: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
                operation.name
            )),
            Expectation::FollowsContent { finding, ceiling, .. } if large > *ceiling => failures.push(format!(
                "{}: an open finding ({finding}) reads {large} bytes on the large store, over its {ceiling}-byte ceiling (1.5x what it \
                 measured when the ceiling was set): the finding is getting worse, not just failing to improve",
                operation.name
            )),
            Expectation::FollowsHistory { finding, ceiling: _ } if large < small + (16 << 10) => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes at the shallow / deep history: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
                operation.name
            )),
            Expectation::FollowsHistory { finding, ceiling } if large > *ceiling => failures.push(format!(
                "{}: an open finding ({finding}) reads {large} bytes at the deep history, over its {ceiling}-byte ceiling (1.5x what it \
                 measured when the ceiling was set): the finding is getting worse, not just failing to improve",
                operation.name
            )),
            Expectation::FollowsRefs { finding, ceiling: _ } if large < small + (16 << 10) => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes at the few / many refs: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
                operation.name
            )),
            Expectation::FollowsRefs { finding, ceiling } if large > *ceiling => failures.push(format!(
                "{}: an open finding ({finding}) reads {large} bytes at the many refs, over its {ceiling}-byte ceiling (1.5x what it \
                 measured when the ceiling was set): the finding is getting worse, not just failing to improve",
                operation.name
            )),
            _ => {}
        }
    }
    assert!(
        failures.is_empty(),
        "operations whose cost follows the store's size:\n{}\n(all rows: name, small, large, difference, allowance)\n{report}",
        failures.join("\n")
    );
    Ok(())
}

/// 0.54.0 step 1 item 2's own control: losing the checkpoint anchor (`replay-verified-blocks.v1`
/// removed, alongside `lifecycle-state.v1`) forces the cold commit to full-replay from genesis
/// instead of anchoring [`CHECKPOINT_GAP`] blocks back -- genesis's own
/// [`CHECKPOINT_GAP_GENESIS_CONTENT`] (24 MiB, identical in both stores) now joins whatever the
/// content added since the checkpoint; the "since checkpoint" row's own ceiling must fail here, or
/// it is not sensitive to the thing it is meant to catch.
#[test]
fn losing_the_checkpoint_anchor_fails_the_since_checkpoint_ceiling() -> Result<()> {
    fn measure_without_anchor(size: usize) -> Result<u64> {
        let (layout, root) = store_of_checkpoint_gap(size)?;
        std::fs::remove_file(layout.cache_dir().join("replay-verified-blocks.v1"))?;
        read_tally::reset();
        one_file_commit(&layout)?;
        let total = read_tally::snapshot().values().sum();
        let _ = std::fs::remove_dir_all(root);
        Ok(total)
    }
    let large = measure_without_anchor(LARGE_SINCE_CHECKPOINT)?;
    let row = OPERATIONS
        .iter()
        .find(|op| op.name == "one-file commit (since checkpoint)")
        .expect("the row exists");
    let Expectation::FollowsContent { ceiling, .. } = &row.expectation else {
        panic!("the row's own expectation must be FollowsContent");
    };
    let ceiling = *ceiling;
    assert!(
        large > ceiling,
        "losing the checkpoint anchor must read past the row's own {ceiling}-byte ceiling (full \
         replay from genesis, {CHECKPOINT_GAP_GENESIS_CONTENT} bytes of its own plus the content \
         since it), got {large} -- or the ceiling is not sensitive to losing the anchor"
    );
    Ok(())
}

/// 0.54.0 step 3 Part 4's own control, the identical shape as the row above, for `extract`
/// instead of a commit: losing the checkpoint anchor forces a full replay from genesis, reading
/// every one of `CHECKPOINT_GAP`'s own `ReplaceBinary` operations' new blob in full (to validate its
/// kind, 0.54.0 step 3 Part 2's own `953ae32c` fix) rather than streaming only the final content
/// once from an admitted anchor -- the "since checkpoint" row's own ceiling must fail here, or it is
/// not sensitive to the thing it is meant to catch.
#[test]
fn losing_the_checkpoint_anchor_fails_the_extract_since_checkpoint_ceiling() -> Result<()> {
    fn measure_without_anchor(size: usize) -> Result<u64> {
        let (layout, root) = store_of_checkpoint_gap(size)?;
        std::fs::remove_file(layout.cache_dir().join("replay-verified-blocks.v1"))?;
        let total = {
            read_tally::reset();
            extract_check(&layout)?;
            read_tally::snapshot().values().sum()
        };
        let _ = std::fs::remove_dir_all(root);
        Ok(total)
    }
    let large = measure_without_anchor(LARGE_SINCE_CHECKPOINT)?;
    let row = OPERATIONS
        .iter()
        .find(|op| op.name == "extract (since checkpoint)")
        .expect("the row exists");
    let Expectation::FollowsContent { ceiling, .. } = &row.expectation else {
        panic!("the row's own expectation must be FollowsContent");
    };
    let ceiling = *ceiling;
    assert!(
        large > ceiling,
        "losing the checkpoint anchor must read past the row's own {ceiling}-byte ceiling (every \
         ReplaceBinary's own new blob read in full to validate its kind, {CHECKPOINT_GAP} times, \
         plus {CHECKPOINT_GAP_GENESIS_CONTENT} bytes for genesis's own CreateFile), got {large} -- \
         or the ceiling is not sensitive to losing the anchor"
    );
    Ok(())
}

/// 0.54.0 step 1 item 2's own other control: the same, small amount of content added since the
/// checkpoint on both sides (so there is nothing for a cost that follows it to differ by) reads
/// within the row's own floor of itself -- demonstrating the floor would promote the real row once
/// a future fix stops following the since-checkpoint content, the same way
/// [`the_table_reports_a_read_that_follows_the_stores_size`] demonstrates the table can see a read
/// that does not stay flat.
#[test]
fn a_flat_since_checkpoint_reading_would_promote_the_row() -> Result<()> {
    let op = Operation {
        name: "one-file commit (since checkpoint), held flat for this control",
        axis: Axis::CheckpointGap,
        expectation: Expectation::Flat,
        allowance: 0,
        why: "",
        run: one_file_commit,
    };
    let (first, _) = measure(&op, 1024)?;
    let (second, _) = measure(&op, 1024)?;
    let row = OPERATIONS
        .iter()
        .find(|candidate| candidate.name == "one-file commit (since checkpoint)")
        .expect("the row exists");
    let Expectation::FollowsContent { floor, .. } = &row.expectation else {
        panic!("the row's own expectation must be FollowsContent");
    };
    let floor = *floor;
    assert!(
        second < first + floor,
        "the same since-checkpoint content on both sides ({first} / {second}) must read within \
         the row's own {floor}-byte floor of each other, or the floor would not promote a \
         genuinely flat future reading"
    );
    Ok(())
}

/// The table can see a whole read: a control that reads the large store's blob container **whole** (through a ranged read of every
/// byte -- the P1 guard forbids the plain whole read) makes `large` exceed `small` by the content difference, and the comparison the
/// table makes reports it. Without this, "every row is flat" could mean "the tally sees nothing".
#[test]
fn the_table_reports_a_read_that_follows_the_stores_size() -> Result<()> {
    fn read_the_container_whole(layout: &RepositoryLayout) -> Result<()> {
        let container = layout.container_slot_path(
            ObjectType::Blob,
            crate::foundation::layout::ContainerSlot::A,
        );
        let relative = layout.repository_relative(&container)?;
        crate::foundation::fsutil::read_file_range_if_exists(
            layout.repository_mutation_root(),
            &relative,
            0,
            usize::MAX,
        )?;
        Ok(())
    }
    let cheating = Operation {
        name: "control: a whole read of the blob container",
        axis: Axis::Content,
        expectation: Expectation::Flat,
        allowance: 4096,
        why: "none: this is the defect",
        run: read_the_container_whole,
    };
    let (small, _) = measure(&cheating, SMALL_CONTENT)?;
    let (large, _) = measure(&cheating, LARGE_CONTENT)?;
    assert!(
        large > small + cheating.allowance + (8 << 20),
        "the control's difference ({small} -> {large}) is the content's difference, and far above the allowance"
    );
    Ok(())
}
