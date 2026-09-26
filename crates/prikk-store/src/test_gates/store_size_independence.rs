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

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use prikk_error::Result;
use prikk_object::{ObjectEnvelope, ObjectType};

use crate::commit_boundary::worktree_patch::{
    WorktreePatchCommitOptions, commit_worktree_changes_signed,
};
use crate::foundation::fsutil::read_tally;
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::test_gates::test_support::{dummy_signature, unique_temp_dir};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, ObjectReader,
    ObjectWriteSession, ObjectWriter, RepositoryLayout, add_trusted_maintainer,
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
    FollowsContent { finding: &'static str },
    /// **An open finding on the history axis**: the operation reads more as the history grows (`large >= small + 16 KiB`), the ref log
    /// and the object index being what grows. Same promotion rule as [`Expectation::FollowsContent`].
    FollowsHistory { finding: &'static str },
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
        },
        allowance: 4096,
        why: "one seal after 4 or 64 sealed generations",
        run: one_block_seal,
    },
];

/// The small and the large store of the operation's own axis: `(small, large)` are the two sizes on that axis.
fn sizes(axis: Axis) -> (usize, usize) {
    match axis {
        Axis::Content => (SMALL_CONTENT, LARGE_CONTENT),
        Axis::Count => (SMALL_OBJECTS, LARGE_OBJECTS),
        Axis::History => (SMALL_HISTORY, LARGE_HISTORY),
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
            Expectation::FollowsContent { finding } if large < small + (8 << 20) => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes on the small / large store: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
                operation.name
            )),
            Expectation::FollowsHistory { finding } if large < small + (16 << 10) => failures.push(format!(
                "{}: an open finding ({finding}) now reads {small} / {large} bytes at the shallow / deep history: it is flat. Promote the \
                 row to `Expectation::Flat` and remove the finding",
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
