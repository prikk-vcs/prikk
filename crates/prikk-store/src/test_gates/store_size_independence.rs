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
use crate::foundation::fsutil::read_tally;
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
    /// **An open finding**: the operation reads about as much more as the store holds more, and the report names it for a ruling. The
    /// row asserts the finding *still holds* (`large >= small + 8 MiB`), so that fixing it makes the row fail until it is promoted to
    /// `Flat` -- the debt cannot be paid without the table noticing.
    FollowsContent {
        finding: &'static str,
        /// **RFC 160 §9's ceiling on P2's open rows** (the external review's D11): the floor (`FollowsContent`'s own promotion rule)
        /// catches a fix; this catches a *regression* -- the finding growing worse without anyone noticing, since before this a row
        /// failed only when the finding disappeared. Set to 1.5x what the operation measured when this ceiling was written; the
        /// report gives the exact numbers.
        ceiling: u64,
    },
    /// **An open finding on the history axis**: the operation reads more as the history grows (`large >= small + 16 KiB`), the ref log
    /// and the object index being what grows. Same promotion rule as [`Expectation::FollowsContent`].
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
        expectation: Expectation::FollowsContent {
            finding: "F4 in the report: `replay_lineage` -> `blob_kind` reads every stored blob's frame to learn its kind, on every commit",
            ceiling: 25_178_865, // 1.5x the 16,785,910 bytes measured for RFC 160 R3's report
        },
        allowance: 4096,
        why: "authoring one patch: the baseline replay reads the block and patch records it needs, the same on both stores",
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
];

/// The small and the large store of the operation's own axis: `(small, large)` are the two sizes on that axis.
fn sizes(axis: Axis) -> (usize, usize) {
    match axis {
        Axis::Content => (SMALL_CONTENT, LARGE_CONTENT),
        Axis::Count => (SMALL_OBJECTS, LARGE_OBJECTS),
        Axis::History => (SMALL_HISTORY, LARGE_HISTORY),
        Axis::Refs => (SMALL_REFS, LARGE_REFS),
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
            Expectation::FollowsContent { finding, ceiling } if large < small + (8 << 20) => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes on the small / large store: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
                operation.name
            )),
            Expectation::FollowsContent { finding, ceiling } if large > *ceiling => failures.push(format!(
                "{}: an open finding ({finding}) reads {large} bytes on the large store, over its {ceiling}-byte ceiling (1.5x what it \
                 measured when the ceiling was set): the finding is getting worse, not just failing to improve",
                operation.name
            )),
            Expectation::FollowsHistory { finding, ceiling } if large < small + (16 << 10) => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes at the shallow / deep history: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
                operation.name
            )),
            Expectation::FollowsHistory { finding, ceiling } if large > *ceiling => failures.push(format!(
                "{}: an open finding ({finding}) reads {large} bytes at the deep history, over its {ceiling}-byte ceiling (1.5x what it \
                 measured when the ceiling was set): the finding is getting worse, not just failing to improve",
                operation.name
            )),
            Expectation::FollowsRefs { finding, ceiling } if large < small + (16 << 10) => failures.push(format!(
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
