#![allow(clippy::indexing_slicing)]

use prikk_crypto::Ed25519KeyPair;
use prikk_error::Result;

use super::{
    compact_received_index, compact_ref_pointer_index, compact_trust_policy,
    plan_compact_ref_pointer_index,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test};
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::{ContainerSlot, LockableContainer};
use crate::lock::acquire_container_locks;
use crate::refs::{decode_pointer_index_entries_for_resolver, fold_one_pointer_index_entry};
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, unique_temp_dir,
};
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    add_trusted_maintainer, load_maintainer_trust_policy, remove_trusted_maintainer,
};

fn public_key_hex(seed: &[u8; 32]) -> String {
    Ed25519KeyPair::from_seed(seed)
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn publish_update(
    store: &RefStore,
    objects: &mut FileObjectStore,
    ref_name: &str,
    expected_previous: Option<prikk_object::ObjectId>,
    seq: u64,
) -> Result<prikk_object::ObjectId> {
    let target = objects.write_object(&signed_empty_block_envelope())?;
    let ref_state = signed_ref_state_envelope(ref_name, expected_previous, target, seq);
    let ref_state_id = ref_state.object_id();
    store.publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: expected_previous,
        ref_update: crate::test_gates::test_support::signed_ref_update_envelope(
            ref_name,
            expected_previous,
            ref_state_id,
            target,
            seq,
        ),
        ref_state,
    })
}

/// Acceptance criterion 1 (compactor publishes by appending a generation record, after the new
/// slot's bytes are durable) proven end to end: three updates to the same ref plus one update to an
/// unrelated ref leaves 4 raw entries but only 2 live pointers; compaction must reclaim exactly the
/// 2 stale ones while every lookup still resolves correctly afterward.
#[test]
fn compacting_the_ref_pointer_index_reclaims_stale_entries_and_preserves_current_pointers()
-> Result<()> {
    let root = unique_temp_dir("compact-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    let third = publish_update(&store, &mut objects, "heads/main", Some(second), 3)?;
    let other = publish_update(&store, &mut objects, "heads/topic", None, 1)?;

    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let live_before = resolve_live_slot(
        &layout,
        &generation_log_path,
        &layout.ref_pointer_index_slot_path(ContainerSlot::A),
        &layout.ref_pointer_index_slot_path(ContainerSlot::B),
        "ref pointer index has a damaged entry",
        "ref pointer index generation log is ambiguous",
        decode_pointer_index_entries_for_resolver,
        fold_one_pointer_index_entry,
    )?;
    assert_eq!(live_before, ContainerSlot::A);

    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_before, 4);
    assert_eq!(report.entries_after, 2);

    let live_after = resolve_live_slot(
        &layout,
        &generation_log_path,
        &layout.ref_pointer_index_slot_path(ContainerSlot::A),
        &layout.ref_pointer_index_slot_path(ContainerSlot::B),
        "ref pointer index has a damaged entry",
        "ref pointer index generation log is ambiguous",
        decode_pointer_index_entries_for_resolver,
        fold_one_pointer_index_entry,
    )?;
    assert_eq!(live_after, ContainerSlot::B);

    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(third));
    assert_eq!(store.read_current_ref_state_id("heads/topic")?, Some(other));

    // The retired slot's raw bytes are untouched by this compaction -- nothing is destroyed, only
    // superseded. It becomes the *next* compaction's own target, truncated only then.
    let retired_bytes = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
    assert!(!retired_bytes.is_empty());

    // The system stays fully functional post-compaction: a further update still publishes and
    // resolves correctly through the now-live slot.
    let fourth = publish_update(&store, &mut objects, "heads/main", Some(third), 4)?;
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(fourth));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// `--plan-only`'s own contract: the same counts a real run would report, with **nothing** on disk
/// touched -- both slots' bytes, and the generation log, exactly as before.
#[test]
fn plan_compact_reports_the_same_counts_as_a_real_run_and_touches_nothing() -> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-plan-only");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    publish_update(&store, &mut objects, "heads/main", Some(second), 3)?;
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;

    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let slot_a_before = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
    let slot_b_before = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?;
    let generation_log_before = std::fs::read(&generation_log_path)?;

    let report = plan_compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_before, 4);
    assert_eq!(report.entries_after, 2);

    assert_eq!(
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?,
        slot_a_before
    );
    assert_eq!(
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?,
        slot_b_before
    );
    assert_eq!(std::fs::read(&generation_log_path)?, generation_log_before);
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A
    );

    // A real run afterward still sees the same reduction -- the preview did not consume or disturb
    // anything a subsequent real compaction depends on.
    let real_report = compact_ref_pointer_index(&layout)?;
    assert_eq!(real_report.entries_before, report.entries_before);
    assert_eq!(real_report.entries_after, report.entries_after);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The preview holds the same container lock a real run does -- a stale-numbers preview is worse
/// than none, since an operator acts on what it reports.
#[test]
fn plan_compact_refuses_while_its_own_container_lock_is_externally_held() -> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-plan-only-lock-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;

    let held = acquire_container_locks(&layout, &[LockableContainer::RefPointerIndex])?;
    assert!(plan_compact_ref_pointer_index(&layout).is_err());
    drop(held);
    assert!(plan_compact_ref_pointer_index(&layout).is_ok());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Acceptance criterion 2, shown rather than argued: a crash between the new slot's bytes landing
/// and the generation record being appended must leave the *old* generation authoritative -- the
/// retry must be safe, and nothing observes the half-published state in between. Failpoint-gated to
/// Linux/macOS, matching `TestFailPoint`'s own availability (`fsutil.rs`).
/// 0.50.0 step 1 Part E briefly made this refuse (the log names no slot, slot B holds data -- file-
/// identical to a genuinely lost generation record): the team flagged that this exact crash window
/// is indistinguishable from a lost record *by file shape alone*, but not by content -- slot B here
/// is nothing but a re-encoding of the same entries slot A already has (compaction only ever copies
/// the live slot's own entries into the other one). Part E2's corrected ruling deduces exactly that:
/// every entry slot B holds is already in slot A, so A is live, and a bare retry heals it, restoring
/// this test to its original meaning.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_crash_before_the_generation_record_lands_leaves_the_old_generation_authoritative() -> Result<()>
{
    let root = unique_temp_dir("compact-pointer-index-crash-before-publish");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;

    let generation_log_path = layout.ref_pointer_index_generation_log_path();

    // Two `AppendWrite`s happen inside a successful run: the compacted slot's own bytes, then the
    // generation record. Skip 1 to fail on the second -- the generation record's own append -- so
    // the new slot's bytes are already durable when the crash hits.
    fail_after_for_test(TestFailPoint::AppendWrite, 1);
    assert!(compact_ref_pointer_index(&layout).is_err());

    // The log names no slot, but slot B holds data that deduces back to A (every entry it holds is
    // already in A) -- so A stays live, silently, and every ordinary read keeps working.
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));

    // A bare retry heals it exactly as in 0.49.0: the half-finished attempt left nothing for this
    // one to trip over.
    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_after, 1);
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::B
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E4, the case table's row 4 (the review): a compaction crash leaves A live, but A is not
/// frozen there -- it keeps taking ordinary writes (a new branch) until something records the
/// switch. `compaction(A_now)` then differs from B, which was made from an earlier A; Part E3's own
/// rule compared against the current A only and would wrongly resolve to B, losing the branch
/// written after the crash. The sound rule finds `P` = A as it stood at the crash among A's own
/// prefixes and still resolves to A.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_crash_then_ordinary_writes_to_a_still_resolve_to_a() -> Result<()> {
    let root = unique_temp_dir("part-e4-pointer-index-crash-then-writes-to-a");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;

    // The crash window: B becomes a sound copy of A as it stands now (one entry), but no record
    // lands.
    fail_after_for_test(TestFailPoint::AppendWrite, 1);
    assert!(compact_ref_pointer_index(&layout).is_err());

    // Ordinary work continues on A (the ambiguous state's own deduction already resolves reads and
    // writes to A) -- a second update to the *same* ref, landing only in A. This is the shape that
    // actually defeats a "compare against the full, current A" rule: the duplicate key makes
    // `compaction(A_now)` a single, newer entry that is no longer a prefix match for B at all,
    // even though B is still soundly derived from an earlier state of A.
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;

    assert_eq!(
        resolve_live_slot(
            &layout,
            &layout.ref_pointer_index_generation_log_path(),
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A,
        "A must stay live -- B was made from an earlier A, and the update written since must not \
         be lost to a stale read"
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E4, the case table's row 4 for the trust policy container -- security-relevant, mirroring
/// the review's own framing: a crash leaves A live, a maintainer is revoked afterward (an ordinary
/// write to A), and the rule must not read that revocation away by resolving to the stale B.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn trust_policy_crash_then_an_ordinary_revocation_still_resolves_to_a() -> Result<()> {
    let root = unique_temp_dir("part-e4-trust-policy-crash-then-writes-to-a");
    let layout = RepositoryLayout::init(root.clone())?;
    let first_key = public_key_hex(&[31_u8; 32]);
    let second_key = public_key_hex(&[32_u8; 32]);
    add_trusted_maintainer(&layout, "first", &first_key)?;
    add_trusted_maintainer(&layout, "second", &second_key)?;

    // The crash window: B becomes a sound copy of A's last snapshot as it stands now ({first,
    // second}), but no record lands.
    fail_after_for_test(TestFailPoint::AppendWrite, 1);
    assert!(compact_trust_policy(&layout).is_err());

    // Ordinary work continues on A: a revocation, landing only in A, which B was never made from.
    remove_trusted_maintainer(&layout, "first")?;

    let policy = load_maintainer_trust_policy(&layout)?;
    assert_eq!(
        policy.keys.len(),
        1,
        "the revocation written to A after the crash must not be lost to a stale read of B"
    );
    assert_eq!(policy.keys[0].key_id, "second");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-41 crash window 1: the compactor crashes *while writing the new slot's own bytes*, before they
/// are durable -- earlier than the previous test's window (which lets the slot write complete and
/// only fails the generation record). The old generation must still be authoritative and every read
/// still correct, exactly as when the crash lands later -- retrying from scratch is the same recovery
/// either way.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_crash_while_writing_the_new_slots_own_bytes_leaves_the_old_generation_authoritative()
-> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-crash-during-slot-write");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;

    let generation_log_path = layout.ref_pointer_index_generation_log_path();

    // Skip 0 to fail on the *first* `AppendWrite` -- the compacted slot's own bytes, before a single
    // byte of it is durable. The target slot was already truncated (a separate primitive, unaffected
    // by this failpoint) but never receives the new content.
    fail_after_for_test(TestFailPoint::AppendWrite, 0);
    assert!(compact_ref_pointer_index(&layout).is_err());

    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));

    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_after, 1);
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::B
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-41 crash window 3: the compactor crashes while truncating its *target* slot -- the retired slot
/// from a previous compaction, being reclaimed for reuse. This always happens before this run's own
/// generation switch, so the *previous* run's generation must still be authoritative regardless.
/// Exercised against a genuinely second compaction (slot `A` already retired once) rather than the
/// first, so the truncate is reclaiming real stale bytes, not a pristine empty file from `init`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_crash_while_truncating_the_retired_slot_leaves_the_previous_generation_authoritative()
-> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-crash-during-truncate");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;

    // First compaction: A (live) -> B (live), A now retired with stale bytes.
    compact_ref_pointer_index(&layout)?;
    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::B
    );

    let third = publish_update(&store, &mut objects, "heads/main", Some(second), 3)?;

    // Second compaction targets A (retired, stale) -- skip 0 to fail its own single `Truncate` call,
    // before the reclaim even starts.
    fail_after_for_test(TestFailPoint::Truncate, 0);
    assert!(compact_ref_pointer_index(&layout).is_err());

    // The first compaction's generation (B) is still authoritative -- the second never got far enough
    // to publish anything.
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::B
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(third));

    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_after, 1);
    assert_eq!(
        resolve_live_slot(
            &layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(third));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-41 crash window 5: a container lock held by one side (compactor or writer) makes the other side
/// fail immediately with no partial write, from *both* directions -- not just the writer-blocked-by-
/// compactor shape rounds 2-3 already proved, but the reverse too, so neither side can leave a torn
/// state regardless of which one is "in the way."
#[test]
fn writer_and_compactor_lock_contention_leaves_no_partial_write_from_either_side() -> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-lock-race-both-directions");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;

    // Direction 1: the compactor's own lock (simulated held, as a real compaction mid-flight would
    // hold it) blocks a writer. The writer's attempt must fail cleanly -- no new pointer entry lands.
    let entries_before_attempt =
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
    let compactor_lock = acquire_container_locks(&layout, &[LockableContainer::RefPointerIndex])?;
    assert!(publish_update(&store, &mut objects, "heads/main", Some(first), 2).is_err());
    assert_eq!(
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?,
        entries_before_attempt,
        "a writer blocked by the compactor's lock must not have appended anything"
    );
    drop(compactor_lock);

    // Direction 2: a writer's own lock (simulated held the same way) blocks the compactor. Its
    // attempt must fail cleanly -- neither slot nor the generation log changes.
    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let slot_a_before = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
    let slot_b_before = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?;
    let generation_log_before = std::fs::read(&generation_log_path)?;
    let writer_lock = acquire_container_locks(&layout, &[LockableContainer::RefPointerIndex])?;
    assert!(compact_ref_pointer_index(&layout).is_err());
    assert_eq!(
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?,
        slot_a_before
    );
    assert_eq!(
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?,
        slot_b_before
    );
    assert_eq!(std::fs::read(&generation_log_path)?, generation_log_before);
    drop(writer_lock);

    // Both sides work normally once uncontended.
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));
    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_after, 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The §15.3 ruling, non-negotiable: compaction refuses to run on a container with a known-corrupt
/// record, rather than silently compacting around the damage. Damage a record's checksum-covered
/// body, observe the refusal, restore the bytes, observe compaction then succeeds.
#[test]
fn compaction_refuses_on_a_corrupt_container_and_touches_nothing() -> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-corrupt");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;

    let live_path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    // RFC 162 rule 3: a corrupted entry with nothing sound after it is now a repairable tail, not
    // damage -- so a second, genuinely sound entry follows the corrupted first one, keeping this
    // fixture interior damage (a sound entry follows it), which is what this test is about.
    let first_entry_len = std::fs::read(&live_path)?.len();
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    let sound_bytes = std::fs::read(&live_path)?;
    let mut damaged = sound_bytes.clone();
    let last = first_entry_len - 1;
    damaged[last] ^= 0x01;
    std::fs::write(&live_path, &damaged)?;

    assert!(compact_ref_pointer_index(&layout).is_err());
    // Nothing touched: the live slot is exactly as this test left it (still damaged, not repaired
    // or partially rewritten), the retired slot is still empty, and no generation record exists.
    assert_eq!(std::fs::read(&live_path)?, damaged);
    assert!(std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?.is_empty());
    assert_eq!(
        resolve_live_slot(
            &layout,
            &layout.ref_pointer_index_generation_log_path(),
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A
    );

    std::fs::write(&live_path, &sound_bytes)?;
    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_after, 2);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 164 round 2 Addendum 1, item 1's "quieter shape": a tail on the live slot itself, not only
/// on the generation log. Before this fix, `compact_ref_pointer_index` silently dropped these
/// trailing bytes from the newly-written target slot -- no recovery file, no line saying so, and
/// `entries_after` reported as if the tail had never existed. Called directly, bypassing the CLI's
/// own early cross-subsystem precheck (`crates/prikk-cli/src/compact.rs`), so this proves the
/// in-function check itself is load-bearing for any caller, not only for `prikk compact --all`'s
/// own front door -- the control that found it: with this check removed, this test still passes
/// through the CLI alone, masked by that earlier check, and only fails when called this way.
#[test]
fn compaction_refuses_on_a_live_slot_tail_and_touches_nothing() -> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-live-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;

    let live_path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let sound_bytes = std::fs::read(&live_path)?;
    let mut tailed = sound_bytes.clone();
    tailed.extend(std::iter::repeat_n(0_u8, 100));
    std::fs::write(&live_path, &tailed)?;

    assert!(compact_ref_pointer_index(&layout).is_err());
    // Nothing touched: the live slot still carries the tail this test left it with, the retired
    // slot is still empty, and no generation record exists -- the refusal happened before any of
    // them could be written, not after a silent reduction that dropped the tail bytes.
    assert_eq!(std::fs::read(&live_path)?, tailed);
    assert!(std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?.is_empty());
    assert_eq!(
        resolve_live_slot(
            &layout,
            &layout.ref_pointer_index_generation_log_path(),
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A
    );

    std::fs::write(&live_path, &sound_bytes)?;
    let report = compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_after, 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Acceptance criterion 4: the compactor participates in the same container lock the writers do --
/// proven the same way round 2 proved the writers do, from the other direction: hold the lock
/// externally, observe the compactor refuse.
#[test]
fn compaction_refuses_while_its_own_container_lock_is_externally_held() -> Result<()> {
    let root = unique_temp_dir("compact-pointer-index-lock-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;

    let held = acquire_container_locks(&layout, &[LockableContainer::RefPointerIndex])?;
    assert!(compact_ref_pointer_index(&layout).is_err());
    drop(held);
    assert!(compact_ref_pointer_index(&layout).is_ok());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The received-index compactor, mirroring the ref-pointer-index coverage above at lighter weight:
/// two imports to the same received-ref name leave one stale entry, reclaimed by compaction.
#[test]
fn compacting_the_received_index_reclaims_a_superseded_import() -> Result<()> {
    let root = unique_temp_dir("compact-received-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let first = objects
        .write_object(&signed_empty_block_envelope())?
        .to_owned();
    let second_state = signed_ref_state_envelope("heads/main", None, first, 1);
    let second = second_state.object_id();
    let third_state = signed_ref_state_envelope("heads/main", None, first, 2);
    let third = third_state.object_id();

    crate::received::write_received_pointer(&layout, "remotes/heads/main", second)?;
    crate::received::write_received_pointer(&layout, "remotes/heads/main", third)?;

    let report = compact_received_index(&layout)?;
    assert_eq!(report.entries_before, 2);
    assert_eq!(report.entries_after, 1);
    let pointer = crate::received::read_received_pointer(&layout, "remotes/heads/main")?;
    assert!(pointer.is_some());
    if let Some(pointer) = pointer {
        assert_eq!(pointer.ref_state_id, third);
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 164 round 2 Addendum 1, item 1's "quieter shape", for the received index -- see
/// `compaction_refuses_on_a_live_slot_tail_and_touches_nothing`'s own doc for why this is called
/// directly rather than through the CLI.
#[test]
fn received_index_compaction_refuses_on_a_live_slot_tail_and_touches_nothing() -> Result<()> {
    let root = unique_temp_dir("compact-received-index-live-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let target = objects
        .write_object(&signed_empty_block_envelope())?
        .to_owned();
    let ref_state = signed_ref_state_envelope("heads/main", None, target, 1);
    crate::received::write_received_pointer(&layout, "remotes/heads/main", ref_state.object_id())?;

    let live_path = layout.received_index_slot_path(ContainerSlot::A);
    let sound_bytes = std::fs::read(&live_path)?;
    let mut tailed = sound_bytes.clone();
    tailed.extend(std::iter::repeat_n(0_u8, 100));
    std::fs::write(&live_path, &tailed)?;

    assert!(compact_received_index(&layout).is_err());
    assert_eq!(std::fs::read(&live_path)?, tailed);
    assert!(std::fs::read(layout.received_index_slot_path(ContainerSlot::B))?.is_empty());

    std::fs::write(&live_path, &sound_bytes)?;
    let report = compact_received_index(&layout)?;
    assert_eq!(report.entries_after, 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The trust-policy compactor: unlike the other two, reduction keeps only the *last* snapshot, not
/// one entry per key -- three snapshots (add, add, remove) collapse to the one live snapshot.
#[test]
fn compacting_the_trust_policy_container_keeps_only_the_last_snapshot() -> Result<()> {
    let root = unique_temp_dir("compact-trust-policy");
    let layout = RepositoryLayout::init(root.clone())?;
    let first_key = public_key_hex(&[7_u8; 32]);
    let second_key = public_key_hex(&[8_u8; 32]);
    add_trusted_maintainer(&layout, "first", &first_key)?;
    add_trusted_maintainer(&layout, "second", &second_key)?;
    remove_trusted_maintainer(&layout, "first")?;

    let report = compact_trust_policy(&layout)?;
    assert_eq!(report.entries_before, 3);
    assert_eq!(report.entries_after, 1);

    let policy = load_maintainer_trust_policy(&layout)?;
    assert_eq!(policy.keys.len(), 1);
    assert_eq!(policy.keys[0].key_id, "second");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 164 round 2 Addendum 1, item 1's "quieter shape", for the trust-policy container -- see
/// `compaction_refuses_on_a_live_slot_tail_and_touches_nothing`'s own doc for why this is called
/// directly rather than through the CLI.
#[test]
fn trust_policy_compaction_refuses_on_a_live_slot_tail_and_touches_nothing() -> Result<()> {
    let root = unique_temp_dir("compact-trust-policy-live-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = public_key_hex(&[9_u8; 32]);
    add_trusted_maintainer(&layout, "only", &key)?;

    let live_path = layout.trust_policy_container_slot_path(ContainerSlot::A);
    let sound_bytes = std::fs::read(&live_path)?;
    let mut tailed = sound_bytes.clone();
    tailed.extend(std::iter::repeat_n(0_u8, 100));
    std::fs::write(&live_path, &tailed)?;

    assert!(compact_trust_policy(&layout).is_err());
    assert_eq!(std::fs::read(&live_path)?, tailed);
    assert!(std::fs::read(layout.trust_policy_container_slot_path(ContainerSlot::B))?.is_empty());

    std::fs::write(&live_path, &sound_bytes)?;
    let report = compact_trust_policy(&layout)?;
    assert_eq!(report.entries_after, 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E2, rule 1 ("no write since"): the received index's generation log is lost right after a
/// compaction, with nothing written to the new slot afterward -- every entry slot B holds is exactly
/// what slot A already had, so the deduction resolves to A (equivalent content either way) and reads
/// keep working silently, rather than refuse a state content already decides.
#[test]
fn received_index_lost_generation_log_with_no_write_since_resolves_silently() -> Result<()> {
    let root = unique_temp_dir("part-e2-received-index-lost-generation-log-no-write-since");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = objects_target(&layout)?;
    let state = signed_ref_state_envelope("heads/main", None, target, 1);
    crate::received::write_received_pointer(&layout, "remotes/heads/main", state.object_id())?;

    let report = compact_received_index(&layout)?;
    assert_eq!(report.entries_after, 1);
    let slot_b = layout.received_index_slot_path(ContainerSlot::B);
    assert!(
        std::fs::metadata(&slot_b)?.len() > 0,
        "fixture: the compacted slot must hold real data"
    );

    // Lose the record of the switch -- nothing written to B since.
    std::fs::write(layout.received_index_generation_log_path(), b"")?;

    let pointer = crate::received::read_received_pointer(&layout, "remotes/heads/main")?;
    assert_eq!(
        pointer.map(|entry| entry.ref_state_id),
        Some(state.object_id()),
        "the deduction must resolve silently -- A and B agree, so there is nothing to refuse"
    );
    assert!(
        crate::received::write_received_pointer(&layout, "remotes/heads/topic", state.object_id())
            .is_ok(),
        "a writer must keep working too, appending behind whichever slot the deduction resolved"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E2, rule 2 ("writes after the switch"): the received index's generation log is lost *after*
/// the new slot took a real write the old slot never had -- the deduction must resolve to the new
/// slot, reading the newer entry rather than silently falling back to the stale one.
#[test]
fn received_index_lost_generation_log_after_a_write_resolves_to_the_newer_slot() -> Result<()> {
    let root = unique_temp_dir("part-e2-received-index-lost-generation-log-after-write");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = objects_target(&layout)?;
    let first_state = signed_ref_state_envelope("heads/main", None, target, 1);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/main",
        first_state.object_id(),
    )?;

    compact_received_index(&layout)?;

    // A real write after the switch, landing only in the now-live slot.
    let second_state = signed_ref_state_envelope("heads/topic", None, target, 1);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/topic",
        second_state.object_id(),
    )?;

    // Lose the record of the switch.
    std::fs::write(layout.received_index_generation_log_path(), b"")?;

    assert_eq!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")?
            .map(|entry| entry.ref_state_id),
        Some(first_state.object_id()),
        "the entry carried over by compaction must still read"
    );
    assert_eq!(
        crate::received::read_received_pointer(&layout, "remotes/heads/topic")?
            .map(|entry| entry.ref_state_id),
        Some(second_state.object_id()),
        "the entry written after the switch must read too -- only the new slot ever had it, so \
         resolving to the old one would silently lose it"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E2, rule 1 for the trust policy container: the identical "no write since" shape as the
/// received index's own test above.
#[test]
fn trust_policy_lost_generation_log_with_no_write_since_resolves_silently() -> Result<()> {
    let root = unique_temp_dir("part-e2-trust-policy-lost-generation-log-no-write-since");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = public_key_hex(&[11_u8; 32]);
    add_trusted_maintainer(&layout, "only", &key)?;

    let report = compact_trust_policy(&layout)?;
    assert_eq!(report.entries_after, 1);
    let slot_b = layout.trust_policy_container_slot_path(ContainerSlot::B);
    assert!(
        std::fs::metadata(&slot_b)?.len() > 0,
        "fixture: the compacted slot must hold real data"
    );

    // Lose the record of the switch -- nothing written to B since.
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    let policy = load_maintainer_trust_policy(&layout)?;
    assert_eq!(policy.keys.len(), 1);
    assert_eq!(policy.keys[0].key_id, "only");
    assert!(
        add_trusted_maintainer(&layout, "second", &public_key_hex(&[12_u8; 32])).is_ok(),
        "a writer must keep working too, appending behind whichever slot the deduction resolved"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E2, rule 2 for the trust policy container -- security-relevant (the review's own finding):
/// resolving to the stale slot here could silently bring back a policy from before a revocation. The
/// deduction must resolve to the slot holding the *newer* snapshot instead.
#[test]
fn trust_policy_lost_generation_log_after_a_write_resolves_to_the_newer_slot() -> Result<()> {
    let root = unique_temp_dir("part-e2-trust-policy-lost-generation-log-after-write");
    let layout = RepositoryLayout::init(root.clone())?;
    let first_key = public_key_hex(&[11_u8; 32]);
    let second_key = public_key_hex(&[12_u8; 32]);
    add_trusted_maintainer(&layout, "first", &first_key)?;
    add_trusted_maintainer(&layout, "second", &second_key)?;

    compact_trust_policy(&layout)?;

    // A real write after the switch -- revoking a maintainer the old slot still names as trusted.
    remove_trusted_maintainer(&layout, "first")?;

    // Lose the record of the switch.
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    let policy = load_maintainer_trust_policy(&layout)?;
    assert_eq!(
        policy.keys.len(),
        1,
        "the revocation written after the switch must be visible -- resolving to the stale slot \
         would silently bring the revoked maintainer back"
    );
    assert_eq!(policy.keys[0].key_id, "second");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E2, rule 3: when the deduction itself cannot be made -- one of the two slots is damaged --
/// it refuses rather than guessing, naming the container's own existing damage text.
#[test]
fn a_damaged_slot_refuses_the_deduction_rather_than_guessing() -> Result<()> {
    let root = unique_temp_dir("part-e2-pointer-index-lost-generation-log-damaged-slot");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?;

    // Lose the record of the switch, then damage the now-stale slot A -- the deduction needs to read
    // it too, and must refuse rather than guess once it cannot.
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;
    let slot_a_path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let mut damaged = std::fs::read(&slot_a_path)?;
    let last = damaged.len() - 1;
    damaged[last] ^= 0x01;
    std::fs::write(&slot_a_path, &damaged)?;

    assert!(
        store.read_current_ref_state_id("heads/main").is_err(),
        "a damaged slot must refuse the deduction, not silently prefer the other one"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E3, the corrected rule's own "partly written B" row: a sound prefix of `C` (the reduction
/// `compact` would write from A), not merely "every entry found somewhere in A" -- the review's own
/// distinction. Simulated by truncating a genuinely compacted slot B to its first record only, which
/// is exactly what an interrupted multi-record compaction write would leave behind.
#[test]
fn a_partly_written_slot_b_resolves_to_the_old_generation() -> Result<()> {
    let root = unique_temp_dir("part-e3-pointer-index-partly-written-slot-b");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let main_target = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;

    compact_ref_pointer_index(&layout)?;

    let slot_b_path = layout.ref_pointer_index_slot_path(ContainerSlot::B);
    let bytes = std::fs::read(&slot_b_path)?;
    let replay = crate::refs::decode_pointer_index_records(&bytes)?;
    assert_eq!(
        replay.entries.len(),
        2,
        "fixture: two distinct refs compact to two entries"
    );
    let first_record_len = crate::refs::encode_pointer_index_record(&replay.entries[0])?.len();
    std::fs::write(&slot_b_path, &bytes[..first_record_len])?;

    // Lose the record of the switch -- slot B now holds only the first of C's two records.
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    assert_eq!(
        resolve_live_slot(
            &layout,
            &layout.ref_pointer_index_generation_log_path(),
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &slot_b_path,
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::A,
        "a sound prefix of C must still resolve to A, the same as an interrupted write would"
    );
    assert_eq!(
        store.read_current_ref_state_id("heads/main")?,
        Some(main_target)
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Part E3's own un-revocation sequence (the review): a repeated snapshot defeats bare membership,
/// since `TrustPolicySnapshotEntry` carries no sequence, only a full `{key_ids}` set.
///
/// **Handoff 165 changes this test's own expectation, deliberately, not as a regression:** slot A's
/// *full* raw history is four snapshots, not three -- `{k}` (the very first add, before `l` ever
/// existed), `{k,l}`, `{k}` (revoked), `{k,l}` (re-trusted) -- and that first `{k}` happens to equal
/// `F(B)` after the post-compaction revoke (both are "no l"), the same coincidental-repeat shape row
/// 14 names. Under the one-directional E4/E3 rule this was never tested (only "is B derived from A"
/// was checked, and it held, resolving to B). Handoff 165's own rule also tests "is A derived from
/// B," which now ALSO holds here (`A` starts with `F(B) = [{k}]`, its own first entry) -- both
/// relations hold, and the folds disagree (`F(A) = [{k,l}]`, `F(B) = [{k}]`) on whether `l` is
/// trusted, so the rule now refuses rather than pick a side. This is a stricter, fail-safe answer to
/// the same security question Part E3 was already protecting (`l` must never silently read as
/// trusted) -- flagged for the architect in the Q1 report as a deliberate behavior change, not
/// discovered and silently absorbed.
#[test]
fn trust_policy_un_revocation_sequence_refuses_rather_than_pick_a_side() -> Result<()> {
    let root = unique_temp_dir("part-e3-trust-policy-un-revocation");
    let layout = RepositoryLayout::init(root.clone())?;
    let k_key = public_key_hex(&[21_u8; 32]);
    let l_key = public_key_hex(&[22_u8; 32]);

    // Slot A's full history: {K} (the very first add), {K,L}, {K} (L revoked), {K,L} (re-trusted).
    add_trusted_maintainer(&layout, "k", &k_key)?;
    add_trusted_maintainer(&layout, "l", &l_key)?;
    remove_trusted_maintainer(&layout, "l")?;
    add_trusted_maintainer(&layout, "l", &l_key)?;

    // Compaction keeps only the last snapshot: C = [{K,L}].
    compact_trust_policy(&layout)?;

    // A real write after the switch: L revoked again, in the now-live slot B -- B = [{K,L}, {K}].
    remove_trusted_maintainer(&layout, "l")?;

    // Lose the record of the switch.
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    assert!(
        matches!(
            load_maintainer_trust_policy(&layout),
            Err(prikk_error::PrikkError::AmbiguousGenerationLog(_))
        ),
        "both relations hold here (this test's own doc comment); L must never silently read as \
         trusted, and refusing -- typed as ambiguous, not damage, per the Q1 review -- is what \
         keeps that true when the evidence itself disagrees"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

fn objects_target(layout: &RepositoryLayout) -> Result<prikk_object::ObjectId> {
    FileObjectStore::new(layout.clone()).write_object(&signed_empty_block_envelope())
}

/// Handoff 165 row 7 (020's own reproduction, P/T/R): after **two** compactions, the slot retired by
/// the second holds a superseded entry from the first -- E4's one-directional rule tested only
/// whether the stale slot derives from the live one, never the other way around, and a superseded
/// entry defeats that test, so E4's `else` branch answered the *stale* slot, losing every write made
/// since. The pointer-index shape: compact once (`main` alone), publish a second update to the same
/// ref (superseding it in the now-live slot), compact again (the retired slot now holds that
/// superseded first-generation entry), then a real write (`topic`) lands in the new live slot before
/// the switch is lost.
#[test]
fn handoff_165_row_7_two_compactions_with_a_superseded_retired_slot_loses_nothing_pointer_index()
-> Result<()> {
    let root = unique_temp_dir("handoff-165-row7-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    // Superseded in the now-live slot.
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    compact_ref_pointer_index(&layout)?;
    // A real write after the second switch, landing only in the new live slot.
    let third = publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    assert_eq!(
        store.read_current_ref_state_id("heads/main")?,
        Some(second),
        "the superseded first generation must not come back"
    );
    assert_eq!(
        store.read_current_ref_state_id("heads/topic")?,
        Some(third),
        "the write made after the second switch must not be lost to the stale slot"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 7, the received-index shape (020's own reproduction): the same two-compactions-
/// then-a-write pattern, so a received tip does not silently go back to an earlier import.
#[test]
fn handoff_165_row_7_two_compactions_with_a_superseded_retired_slot_loses_nothing_received_index()
-> Result<()> {
    let root = unique_temp_dir("handoff-165-row7-received-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = objects_target(&layout)?;
    let first_state = signed_ref_state_envelope("heads/main", None, target, 1);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/main",
        first_state.object_id(),
    )?;
    compact_received_index(&layout)?;
    let second_state = signed_ref_state_envelope("heads/main", None, target, 2);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/main",
        second_state.object_id(),
    )?;
    compact_received_index(&layout)?;
    let third_state = signed_ref_state_envelope("heads/topic", None, target, 1);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/topic",
        third_state.object_id(),
    )?;
    std::fs::write(layout.received_index_generation_log_path(), b"")?;

    assert_eq!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")?
            .map(|entry| entry.ref_state_id),
        Some(second_state.object_id()),
        "a received tip must not go back to an earlier import"
    );
    assert_eq!(
        crate::received::read_received_pointer(&layout, "remotes/heads/topic")?
            .map(|entry| entry.ref_state_id),
        Some(third_state.object_id()),
        "the import made after the second switch must not be lost"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 7, the trust-policy shape (020's own reproduction): a revoked maintainer must not
/// read as trusted again after two compactions and a write.
#[test]
fn handoff_165_row_7_two_compactions_with_a_superseded_retired_slot_loses_nothing_trust_policy()
-> Result<()> {
    let root = unique_temp_dir("handoff-165-row7-trust-policy");
    let layout = RepositoryLayout::init(root.clone())?;
    let first_key = public_key_hex(&[41_u8; 32]);
    let second_key = public_key_hex(&[42_u8; 32]);
    add_trusted_maintainer(&layout, "first", &first_key)?;
    compact_trust_policy(&layout)?;
    // Superseded in the now-live slot: a second snapshot, adding a second maintainer.
    add_trusted_maintainer(&layout, "second", &second_key)?;
    compact_trust_policy(&layout)?;
    // A real write after the second switch: revoke the first maintainer.
    remove_trusted_maintainer(&layout, "first")?;
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    let policy = load_maintainer_trust_policy(&layout)?;
    assert_eq!(
        policy.keys.len(),
        1,
        "the revocation made after the second switch must not be lost to the stale slot, and the \
         revoked key must never come back from an earlier generation"
    );
    assert_eq!(policy.keys[0].key_id, "second");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 8: the same two-compactions shape as row 7, but with nothing written after the
/// second switch -- both slots fold to the same entries, so either reading is correct; this checks
/// that the real read keeps working (never refuses) and sees the right content.
#[test]
fn handoff_165_row_8_two_compactions_no_write_since_both_slots_agree() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row8-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    compact_ref_pointer_index(&layout)?;
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    assert_eq!(
        store.read_current_ref_state_id("heads/main")?,
        Some(second),
        "with nothing written since the second switch, either slot must read the current pointer"
    );
    assert!(
        publish_update(&store, &mut objects, "heads/topic", None, 1).is_ok(),
        "a writer must keep working too"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 6: one compaction, with the retired slot superseded *at* the compaction (two
/// raw entries for the same ref, before anything is compacted), then a write continuing into the
/// live slot. The write's own encoded bytes are appended directly to the slot the correct rule
/// resolves to -- `--repair-tails`'s own device for a cut-short write, not a live `publish` call --
/// because control 3 (relation 1 only) resolves this exact intermediate state to the *other* slot,
/// and a live write would land there instead, building a different row than the one under test.
#[test]
fn handoff_165_row_6_one_compaction_with_a_superseded_retired_slot_then_a_write() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row6-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let second = publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    compact_ref_pointer_index(&layout)?;
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    // The write after this point: appended directly to slot A, the slot the correct rule's "both
    // agree" case names here (A's own superseded raw history and B's clean compaction fold to the
    // same entries, with nothing written since).
    let third_target = objects.write_object(&signed_empty_block_envelope())?;
    let third_state = signed_ref_state_envelope("heads/topic", None, third_target, 1);
    let third = third_state.object_id();
    let entry = crate::refs::PointerIndexEntry {
        ref_name_key: crate::foundation::layout::ref_name_key_bytes("heads/topic"),
        ref_name: "heads/topic".to_string(),
        ref_state_id: third,
    };
    let mut slot_a = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
    slot_a.extend(crate::refs::encode_pointer_index_record(&entry)?);
    std::fs::write(layout.ref_pointer_index_slot_path(ContainerSlot::A), slot_a)?;

    assert_eq!(
        store.read_current_ref_state_id("heads/main")?,
        Some(second),
        "the superseded first generation must not come back"
    );
    assert_eq!(
        store.read_current_ref_state_id("heads/topic")?,
        Some(third),
        "the write made after the (uncommitted) switch must not be lost"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 9: two compactions where the slot retired by the second holds no supersession of
/// its own (it is a clean compaction, "B compact"), then a write after the second switch. Passes
/// under E4 too (the review's own note: a compact retired slot is literally a prefix of the live
/// one) -- kept as a non-regression alongside row 7's fix.
#[test]
fn handoff_165_row_9_two_compactions_clean_retired_slot_then_a_write() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row9-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    // A distinct ref -- no supersession, so the retired slot after the second compaction is clean.
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    let third = publish_update(&store, &mut objects, "heads/extra", None, 1)?;
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    assert_eq!(store.read_current_ref_state_id("heads/extra")?, Some(third));
    assert!(store.read_current_ref_state_id("heads/main")?.is_some());
    assert!(store.read_current_ref_state_id("heads/topic")?.is_some());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 10: the second compaction's own switch is lost before it is ever observed (a
/// crash or an immediate loss -- the two are byte-identical, see `resolve_or_deduce`'s own doc), and
/// only afterward does a real write land -- proving the ambiguous window itself resolves correctly
/// enough for ordinary work to keep going and for the write landing in it to still be read back.
#[test]
fn handoff_165_row_10_second_switch_lost_immediately_then_a_write_is_not_lost() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row10-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    publish_update(&store, &mut objects, "heads/main", Some(first), 2)?;
    compact_ref_pointer_index(&layout)?;
    // The second switch is lost before any further write observes it.
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;
    // Only now does a real write land -- wherever the ambiguous-state deduction above resolves it.
    let fourth = publish_update(&store, &mut objects, "heads/topic", None, 1)?;

    assert_eq!(
        store.read_current_ref_state_id("heads/topic")?,
        Some(fourth),
        "the write made into the ambiguous window must still read back"
    );
    assert!(
        store.read_current_ref_state_id("heads/main")?.is_some(),
        "the second compaction's own reduction must not have been lost either"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 11: the row-4 shape (a superseded retired slot, write after) one compaction
/// deeper -- three compactions instead of one, confirming the rule does not depend on which parity
/// the compaction count happens to land on.
#[test]
fn handoff_165_row_11_three_compactions_with_a_superseded_retired_slot_then_a_write() -> Result<()>
{
    let root = unique_temp_dir("handoff-165-row11-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let first = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?; // k=1
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    compact_ref_pointer_index(&layout)?; // k=2
    let third = publish_update(&store, &mut objects, "heads/main", Some(first), 3)?;
    compact_ref_pointer_index(&layout)?; // k=3, retires a slot that now supersedes the old "main" entry
    let fourth = publish_update(&store, &mut objects, "heads/extra", None, 1)?;
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(third));
    assert_eq!(
        store.read_current_ref_state_id("heads/extra")?,
        Some(fourth)
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 12: three compactions, the last switch lost immediately, then ordinary writes
/// continue into whichever slot that ambiguous state resolves to -- the same shape as row 10, one
/// compaction deeper.
#[test]
fn handoff_165_row_12_third_switch_lost_immediately_then_writes_are_not_lost() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row12-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?; // k=1
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    compact_ref_pointer_index(&layout)?; // k=2
    compact_ref_pointer_index(&layout)?; // k=3, nothing new since k=2 -- a clean third switch
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;
    let fourth = publish_update(&store, &mut objects, "heads/extra", None, 1)?;

    assert_eq!(
        store.read_current_ref_state_id("heads/extra")?,
        Some(fourth)
    );
    assert!(store.read_current_ref_state_id("heads/main")?.is_some());
    assert!(store.read_current_ref_state_id("heads/topic")?.is_some());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 13: the last compaction's own write to the retired slot is cut short at a record
/// boundary (an interrupted multi-record write, mirroring `a_partly_written_slot_b_resolves_to_the_
/// old_generation`'s own fixture one compaction deeper), then ordinary writes continue into the live
/// slot the deduction correctly keeps naming.
#[test]
fn handoff_165_row_13_a_cut_short_compaction_output_then_writes_to_the_live_slot() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row13-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let main_target = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    compact_ref_pointer_index(&layout)?; // k=1: B holds two distinct, compact entries

    let slot_b_path = layout.ref_pointer_index_slot_path(ContainerSlot::B);
    let bytes = std::fs::read(&slot_b_path)?;
    let replay = crate::refs::decode_pointer_index_records(&bytes)?;
    assert_eq!(
        replay.entries.len(),
        2,
        "fixture: two distinct refs compact to two entries"
    );
    let first_record_len = crate::refs::encode_pointer_index_record(&replay.entries[0])?.len();
    std::fs::write(&slot_b_path, &bytes[..first_record_len])?;
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    // The cut-short retired slot must not win -- the live slot (A) keeps taking writes.
    let extra = publish_update(&store, &mut objects, "heads/extra", None, 1)?;
    assert_eq!(
        store.read_current_ref_state_id("heads/main")?,
        Some(main_target)
    );
    assert_eq!(store.read_current_ref_state_id("heads/extra")?, Some(extra));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 14 (the trust-policy shape the review names explicitly): a key revoked, then
/// re-trusted, then revoked again, straddling two compactions, with the write after the second switch
/// repeating a snapshot the stale slot already holds.
///
/// **Q1 review ruled on this: row 14's answer is "refuse," not "the live slot."** The architect
/// proved it by commands (H1/H2, see [`row_14b_h1_and_h2_leave_byte_identical_slots_with_opposite_
/// answers`] below): two equally honest histories leave the identical two slot files with opposite
/// answers on whether `l` is trusted. Once the log is lost, no rule reading only the slots can be
/// right in both, so refusing is the only honest answer -- not a tie-breaker waiting to be found.
#[test]
fn handoff_165_row_14_trust_policy_revoke_retrust_revoke_across_two_compactions() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row14-trust-policy");
    let layout = RepositoryLayout::init(root.clone())?;
    let k_key = public_key_hex(&[51_u8; 32]);
    let l_key = public_key_hex(&[52_u8; 32]);

    add_trusted_maintainer(&layout, "k", &k_key)?;
    add_trusted_maintainer(&layout, "l", &l_key)?;
    remove_trusted_maintainer(&layout, "l")?; // revoked
    compact_trust_policy(&layout)?; // k=1: live snapshot {k}

    add_trusted_maintainer(&layout, "l", &l_key)?; // re-trusted, landing in the now-live slot
    compact_trust_policy(&layout)?; // k=2: live snapshot {k,l}, retired slot holds the {k} snapshot

    // The write after the second switch repeats the {k} snapshot the retired slot already holds.
    remove_trusted_maintainer(&layout, "l")?; // revoked again
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    assert!(
        matches!(
            load_maintainer_trust_policy(&layout),
            Err(prikk_error::PrikkError::AmbiguousGenerationLog(_))
        ),
        "both relations hold here (see this test's own doc comment) with disagreeing folds -- l \
         must never silently read as trusted, and refusing (typed as ambiguous, not damage) is \
         what keeps that true"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 Q1b item 2, row 14's twin: the architect's own proof that row 14 is genuinely
/// ambiguous, not merely unresolved (Q1 review, "the question: row 14 and the E3 test refuse. Is
/// that right?"). The base: setup, then one compaction -- B live. **H1**: add K2, compact (live
/// switches to A), remove K2 -- A live, K2 revoked; both slots saved aside here. **H2 continues H1**
/// from that exact point: compact again (live switches back to B), then add K2 back -- B live, K2
/// trusted. Two equally honest histories, continued from the same repository, yet slot a is
/// byte-identical between H1 and H2, and so is slot b -- only the generation log differs. Once that
/// log is lost, nothing in either slot can tell the two histories apart, so the deduction must
/// refuse, the architect's own point: no tie-breaker (length, raw order, anything else) can be right,
/// because H1 and H2 are the same bytes.
#[test]
fn row_14b_h1_and_h2_leave_byte_identical_slots_with_opposite_answers() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row14b-h1-h2");
    let layout = RepositoryLayout::init(root.clone())?;
    let k2_key = public_key_hex(&[53_u8; 32]);

    add_trusted_maintainer(&layout, "base", &public_key_hex(&[54_u8; 32]))?;
    compact_trust_policy(&layout)?; // the base: one compaction, B live

    // H1: add K2, compact (live -> A), remove K2 -- A live, K2 revoked.
    add_trusted_maintainer(&layout, "k2", &k2_key)?;
    compact_trust_policy(&layout)?;
    remove_trusted_maintainer(&layout, "k2")?;
    let h1_policy = load_maintainer_trust_policy(&layout)?;
    assert!(
        !h1_policy.keys.iter().any(|key| key.key_id == "k2"),
        "fixture: H1 must end with K2 revoked"
    );
    let h1_slot_a = std::fs::read(layout.trust_policy_container_slot_path(ContainerSlot::A))?;
    let h1_slot_b = std::fs::read(layout.trust_policy_container_slot_path(ContainerSlot::B))?;

    // H2 continues H1, in the same repository: compact again (live -> B), then add K2 back -- B
    // live, K2 trusted.
    compact_trust_policy(&layout)?;
    add_trusted_maintainer(&layout, "k2", &k2_key)?;
    let h2_policy = load_maintainer_trust_policy(&layout)?;
    assert!(
        h2_policy.keys.iter().any(|key| key.key_id == "k2"),
        "fixture: H2 must end with K2 trusted"
    );
    let h2_slot_a = std::fs::read(layout.trust_policy_container_slot_path(ContainerSlot::A))?;
    let h2_slot_b = std::fs::read(layout.trust_policy_container_slot_path(ContainerSlot::B))?;

    assert_eq!(
        h1_slot_a, h2_slot_a,
        "H1 and H2 must leave byte-identical slot a -- the architect's own proof"
    );
    assert_eq!(
        h1_slot_b, h2_slot_b,
        "H1 and H2 must leave byte-identical slot b -- the architect's own proof"
    );

    // Lose the log and confirm the ambiguous refusal: the same bytes that just answered "K2
    // trusted" under H2's own intact log answer nothing at all once it is lost, because H1's own
    // intact log, over the identical bytes, would have answered the opposite.
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;
    assert!(
        matches!(
            load_maintainer_trust_policy(&layout),
            Err(prikk_error::PrikkError::AmbiguousGenerationLog(_))
        ),
        "byte-identical slots with opposite answers must refuse once the log that disambiguates \
         them is lost"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 15: the degenerate shape where the live slot (as the log would have named it, had
/// it survived) is empty and the other slot holds the real data -- a structural edge the case table
/// marks as command-buildable only incidentally, exercised directly at the resolver (foundation
/// `generation` has its own synthetic coverage for rows 17-19; this one uses the real pointer-index
/// entry type and encoder, matching every other row in this file, just without a realistic compaction
/// history producing it).
#[test]
fn handoff_165_row_15_the_slot_the_log_would_have_named_is_empty() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row15-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    // Slot B holds real data; slot A (the default the log would otherwise name) stays empty.
    let only = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let bytes = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
    std::fs::write(layout.ref_pointer_index_slot_path(ContainerSlot::B), &bytes)?;
    std::fs::write(layout.ref_pointer_index_slot_path(ContainerSlot::A), b"")?;

    assert_eq!(
        resolve_live_slot(
            &layout,
            &layout.ref_pointer_index_generation_log_path(),
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry",
            "ref pointer index generation log is ambiguous",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?,
        ContainerSlot::B,
        "the only slot holding real data must be the one read, regardless of which letter it is"
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(only));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 16: the two slots are byte-identical (both agree trivially). Built by commands
/// alone: a single, non-superseded entry compacts to bytes identical to its own original encoding
/// (the same entry, the same deterministic encoder, on both sides), so a bare compaction with nothing
/// written since naturally leaves this shape -- no raw copy needed. Must read silently either way.
#[test]
fn handoff_165_row_16_byte_identical_slots_agree_trivially() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row16-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    let only = publish_update(&store, &mut objects, "heads/main", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    assert_eq!(
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?,
        std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::B))?,
        "fixture: a single, non-superseded entry compacts to identical bytes on both sides"
    );
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(only));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 17: neither relation holds -- slot B holds an entry sharing no compaction lineage
/// with slot A at all (another repository's own container, in the row's own framing). No command
/// sequence builds two genuinely unrelated slots; this uses the real encoder on a deliberately
/// unrelated entry, the same device row 18 uses.
#[test]
fn handoff_165_row_17_neither_relation_holds_refuses() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row17-pointer-index");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    let foreign = crate::refs::PointerIndexEntry {
        ref_name_key: crate::foundation::layout::ref_name_key_bytes("heads/foreign"),
        ref_name: "heads/foreign".to_string(),
        ref_state_id: crate::test_gates::test_support::sample_object_id("row17-foreign"),
    };
    std::fs::write(
        layout.ref_pointer_index_slot_path(ContainerSlot::B),
        crate::refs::encode_pointer_index_record(&foreign)?,
    )?;

    assert!(
        matches!(
            store.read_current_ref_state_id("heads/main"),
            Err(prikk_error::PrikkError::AmbiguousGenerationLog(_))
        ),
        "neither slot is a compaction of the other -- this must refuse, not guess, typed as \
         ambiguous rather than damage"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 row 18: both relations hold, but the two slots' folds differ -- not a shape any real
/// compaction sequence produces, built directly to prove it. The trust policy's own fold keeps only
/// the *last* entry (`fold_one_trust_policy_entry`: `running.clear(); running.push(entry)`), so with
/// `A = [s1, s2]` and `B = [s2, s1]`: `B` starts with `F(A) = [s2]` (relation 1, A stale beside B) and
/// `A` starts with `F(B) = [s1]` (relation 1, B stale beside A) -- both hold -- yet `F(A) = [s2] !=
/// [s1] = F(B)`. No real compaction produces this: a retired slot is never appended to again (only
/// truncated and rewritten by a later compaction), so a slot cannot read "the old live content,
/// followed by a fresh write" the way `B`'s own `[s2, s1]` would require once `s2` was the live
/// content.
#[test]
fn handoff_165_row_18_both_relations_hold_with_differing_folds_refuses() -> Result<()> {
    let root = unique_temp_dir("handoff-165-row18-trust-policy");
    let layout = RepositoryLayout::init(root.clone())?;
    // Real key material for both "k" and "l", so the only possible refusal is the deduction's own
    // -- not a missing-key-material error masking it. `add_trusted_maintainer` would also append a
    // policy snapshot; this test overwrites both slots wholesale right after, so that extra snapshot
    // never survives to be read.
    add_trusted_maintainer(&layout, "k", &public_key_hex(&[61_u8; 32]))?;
    add_trusted_maintainer(&layout, "l", &public_key_hex(&[62_u8; 32]))?;

    let snapshot_1 = crate::trust_index::TrustPolicySnapshotEntry {
        key_ids: vec!["k".to_string()],
    };
    let snapshot_2 = crate::trust_index::TrustPolicySnapshotEntry {
        key_ids: vec!["k".to_string(), "l".to_string()],
    };
    let mut slot_a = crate::trust_index::encode_trust_policy_record(&snapshot_1)?;
    slot_a.extend(crate::trust_index::encode_trust_policy_record(&snapshot_2)?);
    let mut slot_b = crate::trust_index::encode_trust_policy_record(&snapshot_2)?;
    slot_b.extend(crate::trust_index::encode_trust_policy_record(&snapshot_1)?);
    std::fs::write(
        layout.trust_policy_container_slot_path(ContainerSlot::A),
        slot_a,
    )?;
    std::fs::write(
        layout.trust_policy_container_slot_path(ContainerSlot::B),
        slot_b,
    )?;

    assert!(
        matches!(
            load_maintainer_trust_policy(&layout),
            Err(prikk_error::PrikkError::AmbiguousGenerationLog(_))
        ),
        "both relations hold (each slot's own fold is a prefix of the other's raw history) but the \
         folds themselves disagree on which snapshot is current -- this must refuse, not guess, \
         typed as ambiguous rather than damage"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 Q1b item 3: can a received tip return to an earlier value through any command? **Yes
/// -- `write_received_pointer`/`bundle import` enforce no fast-forward invariant** (`received.rs`'s
/// own `write_received_pointer`, and `bundle.rs`'s call to it, unconditionally record whatever tip
/// the bundle names; the comment above that call, "import records material, verify decides," is the
/// deliberate absence of a monotonicity check). Re-importing an earlier tip after a later one was
/// already received reproduces row 14's exact mechanism: two compactions plus a write that repeats a
/// value the stale slot already holds, with both `stale_beside` directions holding and the folds
/// disagreeing on which tip is current.
///
/// (The pointer index has no equivalent: `refs/publication.rs::log_position`'s own `expected_sequence
/// == index + 1` ties every `RefState`'s signed `update_seq` to its position in that ref's own,
/// permanent ref-log chain -- `compact.rs`'s module doc confirms the ref log is never pruned or
/// compacted -- so no two publications for the same ref can ever carry the same `update_seq`, and a
/// `RefState`'s object id is derived from its full signed payload, `update_seq` included. A pointer
/// value cannot recur.)
#[test]
fn handoff_165_q1b_item_3_a_received_tip_can_recur_and_the_ambiguity_follows() -> Result<()> {
    let root = unique_temp_dir("handoff-165-q1b3-received-index-recurs");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = objects_target(&layout)?;
    let tip1 = signed_ref_state_envelope("heads/main", None, target, 1).object_id();
    let tip2 = signed_ref_state_envelope("heads/main", None, target, 2).object_id();

    crate::received::write_received_pointer(&layout, "remotes/heads/main", tip1)?;
    compact_received_index(&layout)?; // k=1: live holds [tip1]
    crate::received::write_received_pointer(&layout, "remotes/heads/main", tip2)?; // a real, newer import
    compact_received_index(&layout)?; // k=2: retired slot holds [tip1, tip2]; live holds [tip2]
    // The tip "returns" to an earlier value -- re-receiving tip1 after tip2 was already current.
    crate::received::write_received_pointer(&layout, "remotes/heads/main", tip1)?;
    std::fs::write(layout.received_index_generation_log_path(), b"")?;

    assert!(
        matches!(
            crate::received::read_received_pointer(&layout, "remotes/heads/main"),
            Err(prikk_error::PrikkError::AmbiguousGenerationLog(_))
        ),
        "both relations hold (the live slot's fold is tip1, the retired slot's first entry is \
         tip1; the retired slot's fold is tip2, the live slot's first entry is tip2) -- this must \
         refuse, not silently pick either tip"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// Handoff 165 row 19 (either slot damaged) needs no new test: it is exactly
// `a_damaged_slot_refuses_the_deduction_rather_than_guessing`, above in this file, which already
// covers it under Part E2's own numbering.
