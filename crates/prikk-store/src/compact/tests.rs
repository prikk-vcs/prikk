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
use crate::refs::decode_pointer_index_entries_for_resolver;
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
        decode_pointer_index_entries_for_resolver,
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
        decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
        )?,
        ContainerSlot::B
    );
    assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(second));

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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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
            decode_pointer_index_entries_for_resolver,
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

fn objects_target(layout: &RepositoryLayout) -> Result<prikk_object::ObjectId> {
    FileObjectStore::new(layout.clone()).write_object(&signed_empty_block_envelope())
}
