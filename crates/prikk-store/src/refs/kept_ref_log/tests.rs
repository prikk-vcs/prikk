//! 0.58.0 S5: the ref log a hold keeps (`refs/kept_ref_log.rs`).
//!
//! What these hold: nothing is kept outside a hold and the last release drops it; under a hold the log is read whole once and every later use
//! reads the canary and what was appended (this command's own publication is picked up by the next use's tail read and kept from then on); the
//! copy equals a fresh decode after every kind of change (another ref's publication, a replaced file, a truncate-and-regrow, a damaged or torn
//! tail); and a publication meeting a torn tail does under a hold exactly what it does without one. **Linux and macOS only**: the identity
//! is `(device, inode)`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;

use super::{KeptRefLog, load_ref_log_shared};
use crate::foundation::fsutil::read_tally;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::lock::acquire_container_locks;
use crate::refs::container::decode_ref_container_records;
use crate::refs::kept_pointer_index::tests::publish_ref;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{LockableContainer, RefStore};

fn log_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.ref_log_container_slot_path(ContainerSlot::A)
}

fn log_relative(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout
        .repository_relative(&log_path(layout))
        .expect("relative")
}

fn bytes_read(layout: &RepositoryLayout) -> u64 {
    read_tally::bytes_read(&log_relative(layout))
}

fn file_len(layout: &RepositoryLayout) -> u64 {
    std::fs::metadata(log_path(layout)).expect("log").len()
}

/// The byte length of the log's last record, from the decoded frames' offsets.
fn last_record_len(layout: &RepositoryLayout) -> u64 {
    let bytes = std::fs::read(log_path(layout)).expect("read");
    let replay = decode_ref_container_records(&bytes).expect("decode");
    let last = replay.record_outcomes.last().expect("a record").offset;
    (bytes.len() - last) as u64
}

fn another_process(layout: &RepositoryLayout) -> RepositoryLayout {
    RepositoryLayout::open(layout.root().to_path_buf()).expect("reopen")
}

fn hold(layout: &RepositoryLayout) -> crate::lock::ContainerLockGuard {
    acquire_container_locks(layout, &[LockableContainer::TrustPolicy]).expect("hold")
}

fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    (RepositoryLayout::init(root.clone()).expect("init"), root)
}

/// The shared replay must equal a fresh decode of the file, every time.
fn assert_equals_a_fresh_decode(layout: &RepositoryLayout, label: &str) {
    let shared = load_ref_log_shared(layout)
        .expect("shared")
        .expect("a log exists");
    let fresh = decode_ref_container_records(&std::fs::read(log_path(layout)).expect("read"))
        .expect("decode");
    assert_eq!(
        *shared.replay, fresh,
        "{label}: kept and fresh decode differ"
    );
}

#[test]
fn nothing_is_kept_outside_a_hold_and_the_last_release_drops_it() -> Result<()> {
    let (layout, root) = repository("s5l-absent");
    publish_ref(&layout, "heads/main", 1)?;
    let mutation = layout.repository_mutation_root();
    let _ = load_ref_log_shared(&layout)?;
    assert!(!mutation.kept_value_is_held_for_test::<KeptRefLog>());
    let first = hold(&layout);
    let _ = load_ref_log_shared(&layout)?;
    assert!(mutation.kept_value_is_held_for_test::<KeptRefLog>());
    drop(first);
    assert!(!mutation.kept_value_is_held_for_test::<KeptRefLog>());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The reads: whole once, then the canary (the last record) per use; after this command's own publication one use reads the canary and the new
/// record, and the use after reads the canary alone -- the copy was kept current by the first.
#[test]
fn a_hold_reads_the_log_whole_once_then_the_canary_and_what_was_appended() -> Result<()> {
    let (layout, root) = repository("s5l-reads");
    for seq in 1..=20 {
        publish_ref(&layout, "heads/main", seq)?;
    }
    let whole = file_len(&layout);
    let record = last_record_len(&layout);

    read_tally::reset();
    for _ in 0..3 {
        let _ = load_ref_log_shared(&layout)?;
    }
    assert_eq!(bytes_read(&layout), 3 * whole);

    let _hold = hold(&layout);
    read_tally::reset();
    let _ = load_ref_log_shared(&layout)?;
    assert_eq!(bytes_read(&layout), whole);
    read_tally::reset();
    for _ in 0..5 {
        let _ = load_ref_log_shared(&layout)?;
    }
    assert_eq!(bytes_read(&layout), 5 * record);

    publish_ref(&layout, "heads/main", 21)?;
    let record = last_record_len(&layout);
    read_tally::reset();
    let loaded = load_ref_log_shared(&layout)?.expect("log");
    assert_eq!(loaded.replay.records.len(), 21);
    assert!(loaded.bytes.is_empty(), "the kept copy carries no bytes");
    read_tally::reset();
    let _ = load_ref_log_shared(&layout)?;
    assert_eq!(
        bytes_read(&layout),
        record,
        "after the tail was read once, a use reads the canary only"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 2: another command publishes a **different** ref between two of this command's publications, and the next use sees it.
#[test]
fn another_commands_publication_of_a_different_ref_is_seen_by_the_next_use() -> Result<()> {
    let (layout, root) = repository("s5l-other-ref");
    publish_ref(&layout, "heads/main", 1)?;
    let other = another_process(&layout);
    let _hold = hold(&layout);
    let store = RefStore::new(layout.clone());
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    assert!(store.replay_log("heads/other")?.records.is_empty());

    publish_ref(&other, "heads/other", 1)?;
    assert_eq!(
        store.replay_log("heads/other")?.records.len(),
        1,
        "the next use reads the appended tail"
    );
    // This command's own next publication of its ref is classified against the log the other command extended.
    publish_ref(&layout, "heads/main", 2)?;
    assert_equals_a_fresh_decode(&layout, "after both publications");
    assert_eq!(store.replay_log("heads/main")?.records.len(), 2);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 4: damage or a torn tail appearing between two uses. Nothing damaged is kept, and the call returns what a fresh decode returns.
#[test]
fn damage_and_a_torn_tail_between_two_uses_behave_as_a_fresh_read_does() -> Result<()> {
    for torn in [false, true] {
        let (layout, root) = repository(if torn { "s5l-torn" } else { "s5l-damaged" });
        publish_ref(&layout, "heads/main", 1)?;
        let other = another_process(&layout);
        let _hold = hold(&layout);
        let _ = load_ref_log_shared(&layout)?;
        assert!(
            layout
                .repository_mutation_root()
                .kept_value_is_held_for_test::<KeptRefLog>()
        );
        publish_ref(&other, "heads/other", 1)?;
        let path = log_path(&layout);
        if torn {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            file.set_len(std::fs::metadata(&path)?.len() - 9)?;
        } else {
            let mut bytes = std::fs::read(&path)?;
            let flip = bytes.len() - 20;
            bytes[flip] ^= 0xff;
            std::fs::write(&path, &bytes)?;
        }
        let loaded = load_ref_log_shared(&layout)?.expect("log");
        let fresh = decode_ref_container_records(&std::fs::read(&path)?)?;
        assert_eq!(*loaded.replay, fresh);
        assert!(
            !layout
                .repository_mutation_root()
                .kept_value_is_held_for_test::<KeptRefLog>(),
            "nothing damaged or torn is kept"
        );
        assert_eq!(
            loaded.bytes,
            std::fs::read(&path)?,
            "the whole read carries its bytes"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

/// The file replaced by a copy (another inode), and truncated in place below the copy and regrown **to the same length** with other records of
/// the same size: same inode, same size, and a boundary at the copy's end -- only the record there (the canary) can tell.
#[test]
fn a_replaced_file_and_a_truncate_and_regrow_are_read_whole() -> Result<()> {
    let (layout, root) = repository("s5l-replaced");
    for seq in 1..=4 {
        publish_ref(&layout, "heads/main", seq)?;
    }
    // Two first states of refs whose names are as long as `heads/main`'s: records of one size.
    publish_ref(&layout, "heads/aaaa", 1)?;
    publish_ref(&layout, "heads/bbbb", 1)?;
    let other = another_process(&layout);
    let path = log_path(&layout);
    let _hold = hold(&layout);
    let _ = load_ref_log_shared(&layout)?;

    let bytes = std::fs::read(&path)?;
    let temporary = path.with_extension("swap");
    std::fs::write(&temporary, &bytes)?;
    std::fs::rename(&temporary, &path)?;
    read_tally::reset();
    let _ = load_ref_log_shared(&layout)?;
    assert_eq!(bytes_read(&layout), bytes.len() as u64);
    assert_equals_a_fresh_decode(&layout, "after the file was replaced");

    let known = std::fs::metadata(&path)?.len();
    let replay = decode_ref_container_records(&std::fs::read(&path)?)?;
    let cut = replay.record_outcomes[replay.record_outcomes.len() - 2].offset as u64;
    let file = std::fs::OpenOptions::new().write(true).open(&path)?;
    file.set_len(cut)?;
    drop(file);
    publish_ref(&other, "heads/cccc", 1)?;
    publish_ref(&other, "heads/dddd", 1)?;
    assert_eq!(
        std::fs::metadata(&path)?.len(),
        known,
        "fixture sanity: the same length, on a record boundary"
    );
    read_tally::reset();
    assert_equals_a_fresh_decode(&layout, "after the truncate and regrow");
    assert!(
        !decode_ref_container_records(&std::fs::read(&path)?)?
            .records
            .iter()
            .any(|record| record.ref_name_key
                == crate::foundation::layout::ref_name_key_bytes("heads/bbbb")),
        "fixture sanity: the cut records are gone"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The kept copy equals a fresh decode after every step of a mixed sequence of this command's and another command's publications.
#[test]
fn the_kept_copy_equals_a_fresh_decode_after_every_publication() -> Result<()> {
    let (layout, root) = repository("s5l-sequence");
    let other = another_process(&layout);
    let _hold = hold(&layout);
    let mut seqs = std::collections::BTreeMap::<String, u64>::new();
    for step in 0..40_usize {
        let name = format!("heads/r{}", step % 5);
        let seq = seqs.entry(name.clone()).or_insert(0);
        *seq += 1;
        let writer = if step % 3 == 0 { &other } else { &layout };
        publish_ref(writer, &name, *seq)?;
        assert_equals_a_fresh_decode(&layout, &format!("step {step}"));
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 3/4 together: a publication meeting a torn log tail does, inside an outer hold that kept the log, exactly what it does with no hold
/// at all -- the same refusal or the same success, and the same bytes in both containers afterwards.
#[test]
fn a_publication_meeting_a_torn_tail_does_the_same_under_a_hold_as_without_one() -> Result<()> {
    let run = |with_hold: bool| -> Result<(String, Vec<u8>, Vec<u8>)> {
        let (layout, root) = repository(if with_hold {
            "s5l-torn-hold"
        } else {
            "s5l-torn-bare"
        });
        publish_ref(&layout, "heads/main", 1)?;
        publish_ref(&layout, "heads/other", 1)?;
        let _hold = with_hold.then(|| hold(&layout));
        let _ = load_ref_log_shared(&layout)?;
        let path = log_path(&layout);
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;
        file.set_len(std::fs::metadata(&path)?.len() - 11)?;
        drop(file);
        let outcome = match publish_ref(&layout, "heads/main", 2) {
            Ok(id) => format!("ok {id}"),
            Err(error) => format!("error {error}"),
        };
        let log = std::fs::read(&path)?;
        let pointer = std::fs::read(layout.ref_pointer_index_slot_path(ContainerSlot::A))?;
        let _ = std::fs::remove_dir_all(root);
        Ok((outcome, log, pointer))
    };
    let (bare_outcome, bare_log, bare_pointer) = run(false)?;
    let (hold_outcome, hold_log, hold_pointer) = run(true)?;
    // The repositories live in different directories and sign with the same fixtures, so ids and bytes agree.
    assert_eq!(hold_outcome, bare_outcome);
    assert_eq!(hold_log, bare_log);
    assert_eq!(hold_pointer, bare_pointer);
    Ok(())
}
