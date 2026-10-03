//! RFC 166 D1: `ref-name` is written only by a session's first commit. A *second* commit never
//! touches the file at all, so no failpoint ordinal of a second commit can leave it torn -- swept
//! here across every write ordinal that could plausibly interrupt a write to it, confirming the
//! file is byte-identical before and after every attempt, successful or not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::commit_boundary::worktree_patch::commit_worktree_changes_with_generator;
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::node::node_id_gen::{NodeIdGenerator, SequenceEntropySource};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-d1-ref-name-once", &[0x61_u8; 32]).unwrap()
}

fn generator() -> NodeIdGenerator<SequenceEntropySource> {
    let candidates: Vec<[u8; 32]> =
        (0..16u8).map(|i| { let mut b = [0x62_u8; 32]; b[31] = i; b }).collect();
    NodeIdGenerator::with_source(SequenceEntropySource::new(&candidates))
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_with_generator(
        layout, "heads/main", "d1", WorktreePatchCommitOptions::file_level(),
        &mut generator(), &signer(),
    ).unwrap();
}

/// Crash a *second* commit at the named failpoint's `skip`-th occurrence, and return whether
/// `ref-name` is byte-identical to what it was before the second commit was even attempted.
fn second_commit_leaves_ref_name_untouched_at(point: TestFailPoint, skip: usize) -> bool {
    let root = unique_temp_dir(&format!("rfc166-d1-{point:?}-{skip}"));
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let ref_name_path = layout.active_session_dir(DEFAULT_ACTIVE_NAME).join("ref-name");
    let before = std::fs::read(&ref_name_path).unwrap();
    std::fs::write(layout.root().join("b.txt"), b"two").unwrap();
    fail_after_for_test(point, skip);
    // May succeed or fail at this ordinal; both are checked the same way (the file's own bytes),
    // which is why this does not go through the panicking `commit()` helper above.
    let _ = commit_worktree_changes_with_generator(
        &layout, "heads/main", "d1", WorktreePatchCommitOptions::file_level(),
        &mut generator(), &signer(),
    );
    clear_failpoint_for_test();
    let after = std::fs::read(&ref_name_path).unwrap();
    std::fs::remove_dir_all(&root).ok();
    before == after
}

#[test]
fn second_commit_never_touches_ref_name_at_any_write_ordinal() {
    // Sweeps the full width the project's own established convention uses (RFC 166 design round's
    // own `SWEEP_WIDTH`): every failpoint family a commit's own write path can hit.
    for point in [
        TestFailPoint::AppendWrite,
        TestFailPoint::RequiredFileSync,
        TestFailPoint::MutableFileSync,
        TestFailPoint::MutableRename,
        TestFailPoint::Truncate,
    ] {
        for ordinal in 0..40 {
            assert!(
                second_commit_leaves_ref_name_untouched_at(point, ordinal),
                "{point:?}@{ordinal}: ref-name changed across a second commit -- D1 requires it \
                 untouched at every ordinal, successful or not"
            );
        }
    }
}

/// K3's own "a control must be able to fail": failing this failpoint at the *first* occurrence of
/// each point, unconditionally, inside `prepare_empty_active_ref_for_append` called directly against
/// a non-empty WAL (bypassing D1's own guard, exactly reproducing what `author_inner` did before
/// this round) does leave `ref-name` torn -- confirming the sweep above is actually sensitive to the
/// defect it claims to rule out, not vacuously passing because nothing ever touches the file.
#[test]
fn the_control_the_old_unconditional_write_can_still_be_made_to_tear() {
    let root = unique_temp_dir("rfc166-d1-control");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let ref_name_path = layout.active_session_dir(DEFAULT_ACTIVE_NAME).join("ref-name");
    assert_eq!(std::fs::read(&ref_name_path).unwrap(), b"heads/main");
    // The exact primitive D1's own guard now skips on a non-empty WAL, called directly and crashed
    // between its own truncate (which succeeds, emptying the file) and its own append (which does
    // not) -- the pre-D1 crash shape, exactly.
    fail_after_for_test(TestFailPoint::AppendWrite, 0);
    let result = crate::commit_boundary::active::prepare_empty_active_ref_for_append(
        &layout, "heads/main",
    );
    clear_failpoint_for_test();
    assert!(result.is_err(), "the injected failure must actually fire");
    let after = std::fs::read(&ref_name_path).unwrap();
    assert!(
        after.is_empty(),
        "the control must fail: a crashed unconditional rewrite must leave ref-name empty (the \
         torn, pre-D1 state), got {after:?} -- or this sweep is not testing anything real"
    );
    std::fs::remove_dir_all(&root).ok();
}
