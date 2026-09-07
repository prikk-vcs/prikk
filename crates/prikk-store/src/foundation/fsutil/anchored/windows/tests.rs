//! DC-87 Stage 2: what is realistically demonstrable on Windows from a unit test running on this
//! host. This is not the full proof the handoff's acceptance criteria ask for -- see the review
//! submission for what remains CI-only (cross-platform history divergence, the reparse-point
//! refusal's dependence on Developer Mode / admin privilege on the CI runner). DC-98 built and
//! wired the Windows failpoint injection mechanism this comment used to say did not exist -- the
//! tests below using `TestFailPoint` are DC-98's, demonstrating DC-76's negative controls that
//! wiring makes possible here for the first time.

use std::path::Path;
use std::sync::{Arc, Barrier};

use crate::RepositoryLayout;
use crate::foundation::fsutil::{
    MutationRoot, TestFailPoint, append_file_required, create_new_file_required,
    ensure_directory_required, fail_after_for_test, fail_once_for_test, read_file_if_exists,
    remove_file_required, set_directory_create_barrier_for_test, set_regular_file_mode_required,
    sync_directory_required, truncate_existing_file_required, truncate_file_empty_required,
    write_file_atomically,
};
use crate::test_gates::test_support::unique_temp_dir;

fn mutation_root(path: &Path) -> MutationRoot {
    match MutationRoot::open(path) {
        Ok(root) => root,
        Err(error) => panic!("test mutation root failed: {error}"),
    }
}

#[test]
fn create_exclusive_then_read_round_trips() {
    let root_path = unique_temp_dir("windows-create-read");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(create_new_file_required(&root, relative, b"first").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"first".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

#[test]
fn create_exclusive_refuses_an_already_occupied_path() {
    let root_path = unique_temp_dir("windows-create-exclusive-refuse");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(create_new_file_required(&root, relative, b"first").is_ok());
    assert!(create_new_file_required(&root, relative, b"second").is_err());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"first".to_vec()),
        "a refused create must not have touched the existing content"
    );

    let _ = std::fs::remove_dir_all(root_path);
}

#[test]
fn durable_append_requires_an_existing_file_and_then_appends() {
    let root_path = unique_temp_dir("windows-append");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(
        append_file_required(&root, relative, b"x").is_err(),
        "append to a name nothing created yet must refuse, not create it"
    );
    assert!(create_new_file_required(&root, relative, b"first-").is_ok());
    assert!(append_file_required(&root, relative, b"second").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"first-second".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

#[test]
fn durable_truncate_and_truncate_to_empty() {
    let root_path = unique_temp_dir("windows-truncate");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(create_new_file_required(&root, relative, b"0123456789").is_ok());
    assert!(truncate_existing_file_required(&root, relative, 4).is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"0123".to_vec())
    );
    assert!(truncate_file_empty_required(&root, relative).is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(Vec::new())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

#[test]
fn remove_if_present_reports_removal_and_absence() {
    let root_path = unique_temp_dir("windows-remove");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(
        remove_file_required(&root, relative).is_ok(),
        "removing an absent file must not error -- absence is not a failure"
    );
    assert!(create_new_file_required(&root, relative, b"x").is_ok());
    assert!(remove_file_required(&root, relative).is_ok());
    assert!(
        read_file_if_exists(&root, relative)
            .ok()
            .flatten()
            .is_none()
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// `remove_if_present`'s guarantee depends on every open in this backend requesting
/// `FILE_SHARE_DELETE` (design-v1.md §3.1) -- demonstrated by holding a second, independent handle
/// open (via plain `std::fs::File`, `std` default share mode, which already includes
/// `FILE_SHARE_DELETE`) across the removal, rather than trusted from the doc comment alone.
#[test]
fn remove_if_present_succeeds_while_another_handle_holds_the_file_open() {
    let root_path = unique_temp_dir("windows-remove-share-delete");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");
    assert!(create_new_file_required(&root, relative, b"x").is_ok());

    let held_open = std::fs::File::open(root_path.join("state"));
    assert!(held_open.is_ok(), "test setup: could not open the file");

    assert!(remove_file_required(&root, relative).is_ok());

    drop(held_open);
    let _ = std::fs::remove_dir_all(root_path);
}

#[test]
fn atomic_replace_overwrites_existing_content() {
    let root_path = unique_temp_dir("windows-atomic-replace");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(write_file_atomically(&root, relative, b"first").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"first".to_vec())
    );
    assert!(write_file_atomically(&root, relative, b"second-longer").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"second-longer".to_vec()),
        "std::fs::rename must overwrite an existing destination on Windows, not refuse or leave \
         stale content -- verified here rather than only cited from documentation"
    );

    let _ = std::fs::remove_dir_all(root_path);
}

#[test]
fn ensure_directory_is_idempotent_under_a_concurrent_creator_shape() {
    let root_path = unique_temp_dir("windows-ensure-directory");
    let root = mutation_root(&root_path);
    let relative = Path::new("nested/deeper");

    assert!(ensure_directory_required(&root, relative).is_ok());
    assert!(
        ensure_directory_required(&root, relative).is_ok(),
        "G8: a second ensure over an already-created directory must validate and succeed, not error"
    );
    assert!(root_path.join("nested").join("deeper").is_dir());

    let _ = std::fs::remove_dir_all(root_path);
}

/// design-v1.md §3.3, ruled 2026-08-16: documented no-op. Demonstrated, not merely asserted by
/// comment -- the file's own bytes and existence are unaffected by the call.
#[test]
fn set_permission_bits_is_a_documented_noop() {
    let root_path = unique_temp_dir("windows-set-permission-noop");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");
    assert!(create_new_file_required(&root, relative, b"unchanged").is_ok());

    assert!(set_regular_file_mode_required(&root, relative, 0o100_755).is_ok());

    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"unchanged".to_vec()),
        "the documented no-op must not have touched the file's content"
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// design-v1.md §3.4: documented no-op, safe because the worktree marker brackets its only two
/// production callers. Demonstrated here only as "returns Ok and touches nothing" -- the marker's
/// own safety property is `worktree_marker.rs`'s test coverage, not re-proven here.
#[test]
fn durable_directory_entry_is_a_documented_noop() {
    let root_path = unique_temp_dir("windows-durable-directory-entry-noop");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");
    assert!(create_new_file_required(&root, relative, b"unchanged").is_ok());

    assert!(sync_directory_required(&root, relative).is_ok());

    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"unchanged".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// design-v1.md §4: an interrupted `init` has nothing to lose and a re-run completes it
/// idempotently -- the argument is stated to hold on Windows unchanged because it depends on
/// ordering (`FORMAT` written last), not on a durability primitive. Demonstrated the same way the
/// Linux/macOS equivalent is: `init` twice on the same root, both succeed. Not a literal
/// crash-injection -- Windows has no failpoint mechanism (§3.6-adjacent finding, reported
/// separately) -- but it is the same idempotent-completion property the exemption's argument rests
/// on.
#[test]
fn repository_init_is_idempotent() {
    let root_path = unique_temp_dir("windows-init-idempotent");
    let first = RepositoryLayout::init(root_path.clone());
    assert!(first.is_ok(), "first init failed: {first:?}");
    let second = RepositoryLayout::init(root_path.clone());
    assert!(
        second.is_ok(),
        "a second init over an already-initialized repository must complete idempotently, not \
         error: {second:?}"
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// design-v1.md §2: a reparse point substituted for a plain directory or file at any component
/// must be detected and refused. Requires `std::os::windows::fs::symlink_dir`, which needs either
/// Administrator privilege or Developer Mode enabled -- GitHub's hosted `windows-latest` runner is
/// expected to have this, but it was never actually confirmed.
///
/// **DC-97 §2 correction**: this test used to return silently (reporting `ok`) if either
/// precondition failed, on the stated intent that a missing precondition should be "reported as an
/// environment gap." **The code never did that -- it reported nothing, and the guarantee this test
/// exists to pin (G1, the anchoring property DC-96 hardened) could have been asserting nothing on
/// every run with no red test to notice.** Both preconditions now `panic!` with a diagnostic naming
/// exactly what failed, distinguishable from the refusal assertion itself failing. A control that
/// cannot run must fail loudly, not pass silently.
///
/// **DC-97 ordered-list-ruling-v1.md §1-§2 correction**: a probe that disabled `is_reparse_point`
/// entirely (making the reparse-point check itself a no-op) left this test **passing** -- diagnostic
/// evidence (run `31955440915`) showed `validate_directory_not_reparse_point`'s *second* check
/// (`!metadata.is_dir()`) caught the same case, because a no-follow handle on a directory symlink
/// reports `is_dir=false` on Windows. The guarantee holds -- nothing was created through the symlink
/// -- but the old `is_err()` assertion could not tell the two checks apart, so it proved less than
/// it looked like it proved. Tightened to assert the refusal specifically names the reparse point
/// (check 1's own message), not merely that some error occurred.
#[test]
fn a_reparse_point_substituted_for_a_directory_component_is_refused() {
    let root_path = unique_temp_dir("windows-reparse-refusal");
    let root = mutation_root(&root_path);

    let real_target = root_path.join("real-target");
    if let Err(error) = std::fs::create_dir(&real_target) {
        let _ = std::fs::remove_dir_all(&root_path);
        panic!(
            "DC-97 G1: could not create this test's own fixture directory (not the property under \
             test -- an ordinary directory create failed): {error}"
        );
    }
    let link_name = root_path.join("nested");
    if let Err(error) = std::os::windows::fs::symlink_dir(&real_target, &link_name) {
        let _ = std::fs::remove_dir_all(&root_path);
        panic!(
            "DC-97 G1: symlink_dir failed, so this test cannot demonstrate the reparse-point \
             refusal at all on this runner -- likely missing Developer Mode or Administrator \
             privilege: {error}. If this fires in CI, G1 currently has no working Windows control \
             and must be reclassified, not silently skipped."
        );
    }

    let result = ensure_directory_required(&root, Path::new("nested/deeper"));
    let _ = std::fs::remove_dir_all(&root_path);
    let Err(error) = result else {
        panic!(
            "a reparse point standing in for a plain directory component must be refused, but the \
             call succeeded"
        );
    };
    let message = error.to_string();
    assert!(
        message.contains("reparse point"),
        "the refusal must specifically identify the reparse point (`validate_directory_not_reparse_\
         point`'s first check), not merely occur for any reason -- a coincidental type-check \
         failure would satisfy a bare `is_err()` without proving this guarantee at all: {message}"
    );
}

// DC-98 Stage 2: the wiring pass. Each test below injects a failure at one of the boundaries
// `.git-exclude/reviewed/DC-98-stage-2-classification-ruling-v1.md` classified, mirroring the
// Linux/macOS test in `fsutil/tests.rs`/`fsutil/tests/directory.rs` that exercises the same
// `TestFailPoint` variant where a directly comparable one exists. None of these assert a specific
// `RequiredOpen` call ordinal (`fail_once_for_test` always fires on the *first* occurrence since
// arming, by construction) -- the ruling's §3 warning is about `fail_after`'s skip-count needing a
// counted ordinal, which none of these tests use.

/// Row #3. `fail_once_for_test` fires on the first `RequiredOpen` check after arming -- here, that
/// is `WindowsAuthority::verified_anchor_path`'s own re-verification open of the anchor itself,
/// which runs before any component-level open in `resolve_prepared`'s walk. No ordinal is asserted
/// or needed: the guarantee under test ("an open failure aborts cleanly, no side effect, retryable")
/// holds regardless of which of `open_no_follow`'s callers hits it first.
#[test]
fn required_open_failure_has_no_side_effect_and_is_retryable() {
    let root_path = unique_temp_dir("windows-required-open-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(create_new_file_required(&root, relative, b"candidate").is_err());
    assert!(!root_path.join("state").exists());
    assert!(create_new_file_required(&root, relative, b"retry").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"retry".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #1. Mirrors `fsutil/tests/directory.rs`'s `directory_create_failure_has_no_side_effect_and_is_retryable`:
/// the injected failure sits immediately before `fs::create_dir`, so nothing is created and a retry
/// completes normally.
#[test]
fn directory_create_failure_has_no_side_effect_and_is_retryable() {
    let root_path = unique_temp_dir("windows-directory-create-failure");
    let root = mutation_root(&root_path);

    fail_once_for_test(TestFailPoint::DirectoryCreate);
    assert!(ensure_directory_required(&root, Path::new("child")).is_err());
    assert!(!root_path.join("child").exists());
    assert!(ensure_directory_required(&root, Path::new("child")).is_ok());
    assert!(root_path.join("child").is_dir());

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #2, closing the G8 gap `platform-support.md` named: the existing
/// `ensure_directory_is_idempotent_under_a_concurrent_creator_shape` test above calls the same
/// operation twice *sequentially in one thread* -- idempotency, not a proven race. This is the real
/// race, mirroring `fsutil/tests/directory.rs`'s `concurrent_required_directory_creation_is_idempotent`
/// exactly: eight threads all barrier-synchronized to reach `fs::create_dir` at the same instant, so
/// exactly one observes success and the rest must observe and validate the winner's directory
/// rather than erroring.
#[test]
fn concurrent_required_directory_creation_is_idempotent() -> prikk_error::Result<()> {
    let root_path = unique_temp_dir("windows-concurrent-directory");
    let root = mutation_root(&root_path);
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let thread_root = root.clone();
        let thread_barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            set_directory_create_barrier_for_test(thread_barrier);
            ensure_directory_required(&thread_root, Path::new("shared/shard"))
        }));
    }
    for handle in handles {
        handle.join().map_err(|_| prikk_error::PrikkError::Io {
            kind: None,
            context: "directory race thread panicked".to_string(),
        })??;
    }
    assert!(root_path.join("shared").join("shard").is_dir());
    let _ = std::fs::remove_dir_all(root_path);
    Ok(())
}

/// Row #4. Mirrors `fsutil/tests.rs`'s `failed_mutable_file_sync_keeps_only_non_authoritative_temp`:
/// the injected failure sits after the temp file is written but before it is synced, so the rename
/// that would publish it is never reached and the destination is untouched.
#[test]
fn atomic_replace_file_sync_failure_leaves_destination_unchanged_and_is_retryable() {
    let root_path = unique_temp_dir("windows-atomic-replace-file-sync-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(write_file_atomically(&root, relative, b"first").is_ok());
    fail_once_for_test(TestFailPoint::MutableFileSync);
    assert!(write_file_atomically(&root, relative, b"second").is_err());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"first".to_vec()),
        "a crash before the temp file's own sync must not let the rename that follows be reached"
    );
    assert!(write_file_atomically(&root, relative, b"second").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"second".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #5. Mirrors `fsutil/tests.rs`'s `failed_mutable_rename_keeps_previous_authoritative_state`:
/// the temp file is fully written and synced, but the injected failure sits before the rename that
/// would publish it, so the destination's previous content survives untouched.
#[test]
fn atomic_replace_rename_failure_leaves_destination_unchanged_and_is_retryable() {
    let root_path = unique_temp_dir("windows-atomic-replace-rename-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(write_file_atomically(&root, relative, b"first").is_ok());
    fail_once_for_test(TestFailPoint::MutableRename);
    assert!(write_file_atomically(&root, relative, b"second").is_err());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"first".to_vec()),
        "a crash between the temp file's durable sync and the rename must leave the destination \
         untouched"
    );
    assert!(write_file_atomically(&root, relative, b"second").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"second".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #6 -- RFC criterion 2's own named minimum bar: G3 demonstrated on Windows, a crash injected
/// at a sync boundary propagated rather than swallowed. `durable_truncate` is the vehicle rather
/// than `durable_append`: the `set_len` this injection follows has already run when the sync fails,
/// and truncating to a length the file is already at is idempotent, so a retry's correctness is
/// actually assertable here -- unlike `durable_append`, where a retry after the write already
/// landed would append the bytes a second time, a property this test does not need and should not
/// claim.
#[test]
fn durable_truncate_sync_failure_is_retryable_and_idempotent() {
    let root_path = unique_temp_dir("windows-truncate-sync-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(create_new_file_required(&root, relative, b"0123456789").is_ok());
    fail_once_for_test(TestFailPoint::RequiredFileSync);
    assert!(
        truncate_existing_file_required(&root, relative, 4).is_err(),
        "a crash injected at the sync boundary must be propagated, not swallowed"
    );
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"0123".to_vec()),
        "the truncate syscall itself already ran before the injected sync failure"
    );
    assert!(
        truncate_existing_file_required(&root, relative, 4).is_ok(),
        "truncating to a length the file is already at is idempotent, so retry after a sync \
         failure must succeed without further change"
    );
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"0123".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #7. Mirrors `fsutil/tests.rs`'s `failed_append_write_is_retryable`: the injected failure
/// sits before the write itself, so the file's prior (empty) content is unchanged and a retry
/// appends normally.
#[test]
fn append_write_failure_has_no_side_effect_and_is_retryable() {
    let root_path = unique_temp_dir("windows-append-write-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("log");

    assert!(create_new_file_required(&root, relative, b"").is_ok());
    fail_once_for_test(TestFailPoint::AppendWrite);
    assert!(append_file_required(&root, relative, b"record").is_err());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(Vec::new())
    );
    assert!(append_file_required(&root, relative, b"record").is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"record".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #8. Mirrors `fsutil/tests.rs`'s `failed_truncate_retains_previous_state_and_is_retryable`:
/// the injected failure sits before `set_len`, so the file's prior length survives untouched and a
/// retry truncates normally.
#[test]
fn truncate_failure_retains_previous_state_and_is_retryable() {
    let root_path = unique_temp_dir("windows-truncate-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("wal");

    assert!(create_new_file_required(&root, relative, b"complete-partial").is_ok());
    fail_once_for_test(TestFailPoint::Truncate);
    assert!(truncate_existing_file_required(&root, relative, 8).is_err());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"complete-partial".to_vec())
    );
    assert!(truncate_existing_file_required(&root, relative, 8).is_ok());
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"complete".to_vec())
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// Row #9. Mirrors `fsutil/tests.rs`'s `failed_unlink_retains_file_and_cleanup_sync_reports_removed_state`,
/// minus its `cleanup_directory_sync` half: Windows performs no parent-directory sync after unlink
/// (row #14, no operation), so only the unlink-itself half applies here. The injected failure sits
/// before `fs::remove_file`, so the file survives untouched and a retry removes it normally.
#[test]
fn unlink_failure_retains_file_and_is_retryable() {
    let root_path = unique_temp_dir("windows-unlink-failure");
    let root = mutation_root(&root_path);
    let relative = Path::new("entry");

    assert!(create_new_file_required(&root, relative, b"state").is_ok());
    fail_once_for_test(TestFailPoint::Unlink);
    assert!(remove_file_required(&root, relative).is_err());
    assert!(root_path.join("entry").is_file());
    assert!(remove_file_required(&root, relative).is_ok());
    assert!(!root_path.join("entry").exists());

    let _ = std::fs::remove_dir_all(root_path);
}

/// Windows twin of `caller_tests::sync_matrix::object_write_sync_failure_retains_and_classifies`
/// (row #6's own caller-level control, the object-write path). `caller_tests` as a whole stays
/// Unix-only (17 of its 18 tests depend on a directory-entry-sync point Windows has no operation
/// for -- `fsutil.rs`'s own gate comment), so this test lives here instead of moving one test out
/// of that module's shared per-file imports, which would need splitting three files' worth of
/// `use` statements for a single test's sake. Same body, same `skip` values, reusing
/// `windows.rs`'s own `ACTIVE_DURABILITY.durable_append` call path unchanged.
///
/// **The two `RequiredFileSync` ordinals below are an established fact, not a call-graph
/// assumption**: a probe ported this exact body to Windows unchanged and it passed
/// (`.git-exclude/reviewed/DC-98-stage-2-followups-ruling-v1.md` §2, CI run `31983187612`) --
/// `RequiredFileSync` fires at the same two ordinals on Windows as on Linux/macOS: the container
/// append's own sync (skip 0), then the index append's own sync (skip 1).
#[test]
fn object_write_sync_failure_retains_and_classifies_windows() -> prikk_error::Result<()> {
    use crate::{FileObjectStore, ObjectWriter};
    use prikk_object::{ObjectEnvelope, ObjectType};

    for (skip, indexed_after_error) in [(0, false), (1, true)] {
        let root = unique_temp_dir("windows-object-sync-matrix");
        let layout = RepositoryLayout::init(root.clone())?;
        let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"sync".to_vec());
        object.add_signature(crate::test_gates::test_support::dummy_signature())?;
        let object_id = object.object_id();
        let mut store = FileObjectStore::new(layout);
        fail_after_for_test(TestFailPoint::RequiredFileSync, skip);
        assert!(store.write_object(&object).is_err());
        assert_eq!(
            store.contains_object(ObjectType::Blob, object_id),
            indexed_after_error
        );
        assert_eq!(store.write_object(&object)?, object_id);
        assert!(store.contains_object(ObjectType::Blob, object_id));
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
