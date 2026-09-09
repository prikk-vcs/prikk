//! Worktree status tests (RFC 122, `replay-baseline-handoff-v1.md`).
//!
//! Every fixture here is a real, sealed `commit`/`seal` repository -- the same shape the CLI
//! actually produces -- never the old snapshot-only-block fixture this file used before RFC 122.
//! That fixture was not what any production path ever wrote (only a test helper here did), which
//! is exactly why the pre-fix `worktree_status` implementation could pass every test in this file
//! while failing on every repository the CLI can actually create (the bug RFC 122 fixes).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_object::{
    CanonicalEncode, ChangePerm, NodeId, ObjectEnvelope, ObjectType, Operation, OperationKind,
    PatchPayload, PatchPurpose, RenamePath,
};

use super::{QueuedPathResolution, enumerate_queued_patches};
use crate::author::author_signing::author_signature;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::worktree_patch::commit_worktree_changes_signed;
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout,
    WorktreeChangeKind, WorktreePatchCommitOptions, worktree_status,
};

fn author_signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("worktree-status-author", &[0x11; 32]).unwrap()
}

fn maintainer_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("worktree-status-maintainer", &[0x22; 32]).unwrap()
}

fn trust_maintainer(layout: &RepositoryLayout, maintainer: &Ed25519MaintainerSigner) {
    crate::trust::add_trusted_maintainer(
        layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .unwrap();
}

/// One real generation: write `path`, commit, seal -- the same commit/seal cycle
/// `prikk commit`/`prikk seal` perform, via `commit_worktree_changes_signed` (production, real
/// `NodeIdGenerator`) and `simulate_one_seal` (RFC 111 Stage 2's drift-guarded seal replica,
/// checked against the real `prikk seal` binary by its own gate).
fn generation(layout: &RepositoryLayout, path: &str, bytes: &[u8], message: &str) {
    std::fs::write(layout.root().join(path), bytes).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        message,
        WorktreePatchCommitOptions::default(),
        &author_signer(),
    )
    .unwrap();
    crate::rfc111_seal_simulation::simulate_one_seal(layout, "heads/main", &maintainer_signer())
        .unwrap();
}

#[test]
fn worktree_status_is_clean_after_commit_and_seal() {
    let root = unique_temp_dir("worktree-status-clean");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "README.md", b"hello\n", "genesis");

    let report = worktree_status(&layout, "heads/main").unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.tracked_files, 1);
    assert_eq!(report.unchanged_files, 1);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn worktree_status_reports_modified_file() {
    let root = unique_temp_dir("worktree-status-modified");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "README.md", b"hello\n", "genesis");

    std::fs::write(root.join("README.md"), b"changed\n").unwrap();
    let report = worktree_status(&layout, "heads/main").unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.count_kind(WorktreeChangeKind::Modified), 1);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn worktree_status_reports_missing_file() {
    let root = unique_temp_dir("worktree-status-missing");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "README.md", b"hello\n", "genesis");

    std::fs::remove_file(root.join("README.md")).unwrap();
    let report = worktree_status(&layout, "heads/main").unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.count_kind(WorktreeChangeKind::Missing), 1);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn worktree_status_reports_untracked_file() {
    let root = unique_temp_dir("worktree-status-untracked");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "README.md", b"hello\n", "genesis");

    std::fs::write(root.join("extra.txt"), b"extra\n").unwrap();
    let report = worktree_status(&layout, "heads/main").unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.count_kind(WorktreeChangeKind::Untracked), 1);

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 122 §3's design choice, demonstrated: `worktree-status` folds an already-queued (unsealed)
/// patch onto the sealed baseline, the same way `commit` would author against it. Sealed
/// generation 1 (`a.txt`), then a second commit on `b.txt` left deliberately unsealed -- if folding
/// were not happening, `b.txt` would read as untracked (it is not in any sealed block yet) rather
/// than as part of the tracked, unchanged baseline.
#[test]
fn worktree_status_folds_an_already_queued_unsealed_patch() {
    let root = unique_temp_dir("worktree-status-queue");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"first\n", "first");

    // Second commit, deliberately not sealed -- stays queued in the active WAL.
    std::fs::write(root.join("b.txt"), b"second\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "second, unsealed",
        WorktreePatchCommitOptions::default(),
        &author_signer(),
    )
    .unwrap();

    let report = worktree_status(&layout, "heads/main").unwrap();
    assert!(
        report.is_clean(),
        "the queued file's own worktree bytes still match what was just committed: {report:?}"
    );
    assert_eq!(
        report.tracked_files, 2,
        "the queued commit's own file must be folded into the baseline, not left untracked: \
         {report:?}"
    );
    assert_eq!(report.unchanged_files, 2);
    assert_eq!(
        report.queued_elsewhere, None,
        "the queue belongs to this same ref, so there is nothing 'elsewhere' to report: {report:?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 122 amendment §4: a non-empty active WAL owned by a *different* ref is correctly not
/// folded into this ref's baseline (it is not part of it), but `worktree-status` must still say
/// so — the queued file is real, committed work, not a stray file, even though it plays no part
/// in the ref actually being checked here. `heads/other` has never been published (a Genesis
/// baseline), so this needs no second branch: an unpublished ref's own baseline is legitimately
/// empty, independent of what any other ref's active queue holds.
#[test]
fn worktree_status_reports_a_queue_owned_by_a_different_ref() {
    let root = unique_temp_dir("worktree-status-queue-elsewhere");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"first\n", "first");

    // Second commit on heads/main, deliberately not sealed -- the active WAL now belongs to
    // heads/main, not heads/other.
    std::fs::write(root.join("b.txt"), b"second\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "second, unsealed",
        WorktreePatchCommitOptions::default(),
        &author_signer(),
    )
    .unwrap();

    let report = worktree_status(&layout, "heads/other").unwrap();
    assert_eq!(
        report.queued_elsewhere,
        Some("heads/main".to_string()),
        "the queue belongs to heads/main, not the ref being checked: {report:?}"
    );
    // `b.txt` is real, committed-but-unsealed work on heads/main -- correctly still reported as
    // untracked *relative to heads/other's own (empty) baseline*, which has never heard of it.
    // `queued_elsewhere` adds context; it must not reclassify this.
    assert_eq!(report.tracked_files, 0);
    assert_eq!(report.count_kind(WorktreeChangeKind::Untracked), 2);

    let _ = std::fs::remove_dir_all(root);
}

/// Append a raw, directly-constructed Patch envelope to the active WAL, bypassing ordinary
/// authoring entirely -- the only way to get a `RenamePath` or an arbitrary node id into the
/// queue. `commit` never authors `RenamePath` itself (`patch_replay.rs`'s own module doc: "renames
/// become delete+create"), so this is not a shortcut around a real path, it is the only path.
fn append_raw_patch(layout: &RepositoryLayout, operations: Vec<Operation>) {
    let payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let bytes = payload.to_canonical_bytes().unwrap();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, bytes);
    let id = envelope.object_id();
    envelope
        .add_signature(author_signature(&author_signer(), id).unwrap())
        .unwrap();
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
}

/// RFC 140 control 5, at the store level: an empty active WAL enumerates to an empty list, not an
/// error.
#[test]
fn enumerate_queued_patches_returns_empty_for_an_empty_queue() {
    let root = unique_temp_dir("enumerate-queued-patches-empty");
    let layout = RepositoryLayout::init(root.clone()).unwrap();

    let entries = enumerate_queued_patches(&layout).unwrap();
    assert!(entries.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 140 control 2's `rename-path` half: `RenamePath`'s `old_path`/`new_path` come straight from
/// the operation's own payload, never resolved and never unresolved -- demonstrated with a node id
/// that was never created anywhere, which would report `Unresolved` for any of the *node-addressed*
/// kinds (`edit-text`, `change-perm`, `replace-binary`) but must not affect a rename, since a
/// rename's own paths need no lookup at all. Not reachable via the CLI (`commit` never authors this
/// kind); appended directly, per this module's own `append_raw_patch` doc.
#[test]
fn enumerate_queued_patches_reports_rename_path_with_both_endpoints_verbatim() {
    let root = unique_temp_dir("enumerate-queued-patches-rename");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"first\n", "genesis");

    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id: NodeId::from_bytes([0x9A; 32]),
                old_path: "old-name.txt".to_string(),
                new_path: "new-name.txt".to_string(),
            }),
        }],
    );

    let entries = enumerate_queued_patches(&layout).unwrap();
    assert_eq!(entries.len(), 1);
    let operations = &entries.first().unwrap().operations;
    assert_eq!(operations.len(), 1);
    let operation = operations.first().unwrap();
    assert_eq!(operation.kind, "rename-path");
    assert_eq!(
        operation.paths,
        vec![
            QueuedPathResolution::Path("old-name.txt".to_string()),
            QueuedPathResolution::Path("new-name.txt".to_string()),
        ]
    );
    // RFC 144 §4o.6a control 1: the rename's asserting AUTHOR key id is recoverable in the same
    // answer, through the real `enumerate_queued_patches` path (`append_raw_patch` signs with
    // `author_signer()`, key id "worktree-status-author") -- not a hand-constructed
    // `QueuedOperationContent` value.
    assert_eq!(
        operation.content,
        super::QueuedOperationContent::RenamePath {
            author_key_id: "worktree-status-author".to_string()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 144 §4o.6a control 3: a `RenamePath` operand whose patch carries no AUTHOR signature at
/// all fails `enumerate_queued_patches` outright -- never a rendered rename with a blank or
/// absent signer. Constructed directly (bypassing `append_raw_patch`, which always signs), the
/// same way `enumerate_queued_patches_degrades_gracefully_when_active_ref_metadata_is_missing`
/// above constructs its own queue state directly.
#[test]
fn enumerate_queued_patches_fails_a_rename_whose_patch_has_no_author_signature() {
    let root = unique_temp_dir("enumerate-queued-patches-rename-no-author-signature");
    let layout = RepositoryLayout::init(root.clone()).unwrap();

    // Deliberately no active-ref metadata write (as
    // `enumerate_queued_patches_degrades_gracefully_when_active_ref_metadata_is_missing` above
    // also does not) -- `queued_operation_entry`'s `RenamePath` arm reads the AUTHOR key id
    // straight from the payload's own signed envelope, the same way it reads `old_path`/
    // `new_path`, regardless of whether the queue's owning ref (hence any lifecycle replay) can
    // be resolved. Keeping metadata absent here means only the missing-signature check is under
    // test, not the (unrelated) replay-validity of an invented node id.
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id: NodeId::from_bytes([0x9C; 32]),
                old_path: "old-name.txt".to_string(),
                new_path: "new-name.txt".to_string(),
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    // A signed envelope (the WAL itself refuses an entirely unsigned one), but deliberately no
    // AUTHOR-role signature -- unlike `append_raw_patch`, which always signs with one.
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    envelope
        .add_signature(crate::test_gates::test_support::maintainer_signature())
        .unwrap();
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();

    let err = enumerate_queued_patches(&layout).expect_err(
        "a RenamePath whose patch carries no AUTHOR signature must fail, not render a blank signer",
    );
    let message = err.to_string();
    assert!(
        message.contains("AUTHOR signature"),
        "expected an AUTHOR-signature error, got: {message}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 140 §4's principle, extended (this round's own judgement call, reported as such): a queue
/// whose owning-ref metadata is missing entirely still enumerates rather than failing the whole
/// read -- every node-addressed operation just cannot resolve, the same answer a resolvable queue
/// gives for one specifically-unresolvable node id. Constructed via `append_raw_patch`, which
/// (like a crash between WAL append and metadata write) leaves the WAL non-empty with no active-ref
/// metadata at all -- the same state `worktree_patch::tests::non_empty_wal_missing_active_ref_metadata_fails_closed`
/// names for `commit`'s own, deliberately stricter, fail-closed behaviour. `status` is a read, not
/// an author; RFC 140 §4 says a read must not inherit that refusal.
#[test]
fn enumerate_queued_patches_degrades_gracefully_when_active_ref_metadata_is_missing() {
    let root = unique_temp_dir("enumerate-queued-patches-missing-metadata");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"first\n", "genesis");

    // No active-ref metadata is ever written by `append_raw_patch` -- unlike `commit`, which
    // always writes it alongside the WAL append.
    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id: NodeId::from_bytes([0x9B; 32]),
                old_mode: 0o100_644,
                new_mode: 0o100_755,
            }),
        }],
    );

    let entries = enumerate_queued_patches(&layout).unwrap();
    assert_eq!(entries.len(), 1);
    let operations = &entries.first().unwrap().operations;
    assert_eq!(operations.len(), 1);
    let operation = operations.first().unwrap();
    assert_eq!(operation.kind, "change-perm");
    assert!(
        matches!(
            operation.paths.as_slice(),
            [QueuedPathResolution::Unresolved { .. }]
        ),
        "a node-addressed operation with no active-ref metadata to resolve against must report \
         unresolved, not fail the whole read: {:?}",
        operation.paths
    );

    let _ = std::fs::remove_dir_all(root);
}
