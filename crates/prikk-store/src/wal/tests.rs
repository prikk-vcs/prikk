//! WAL tests.

mod proptest_framing;

use crate::{DEFAULT_ACTIVE_NAME, RepositoryLayout, Wal};

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::{
    rollback_patch_envelope, signed_patch_envelope, unique_temp_dir,
};

/// RFC 102 Stage 1 acceptance criterion 4: the WAL exists after `init`, not only after the first
/// append. Behaviour-neutral per RFC 101 §5.1's inherited evidence -- `Wal::replay()` on a freshly
/// initialized layout must be byte-identical to what it already returns for a missing file
/// (`records: []`, `trailing_partial_bytes: 0`), confirmed directly against `decode_records`'s own
/// empty-input path rather than assumed from the inherited proof.
#[test]
fn wal_file_exists_after_init_and_replays_identically_to_missing() {
    let root = unique_temp_dir("wal-exists-after-init");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(layout.default_queue_wal_path().exists());
        assert!(std::fs::read(layout.default_queue_wal_path()).is_ok_and(|bytes| bytes.is_empty()));
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        let replay = wal.replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert!(replay.records.is_empty());
            assert_eq!(replay.trailing_partial_bytes, 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 108 increment 3a control 1: a non-UTF-8 session name must reach `Wal::for_layout` and
/// produce byte-exact paths, not a mangled or silently-dropped one -- the exact hazard this
/// increment closes. `wal.rs`'s old `format!("active/{name}/queue.wal")` could not even accept such
/// a name (`format!`/`Display` require valid UTF-8); the new `active_queue_wal_relative_path`
/// derivation is built from the same raw `OsStr` components as the absolute path, with no text
/// round-trip anywhere.
///
/// **Gated to `target_os = "linux"`, not `unix`**: RFC 108 increment 2 turned `main` red gating an
/// equivalent test on `unix` -- APFS rejects a directory name containing an invalid UTF-8 byte
/// outright (`EILSEQ`), so this exact byte sequence is not constructible on macOS at all.
#[cfg(target_os = "linux")]
#[test]
fn wal_for_layout_produces_byte_exact_paths_for_a_non_utf8_session_name() -> prikk_error::Result<()>
{
    use std::os::unix::ffi::OsStrExt;

    let root = unique_temp_dir("wal-non-utf8-active-name");
    let layout = RepositoryLayout::init(root.clone())?;
    let bad_name = std::ffi::OsStr::from_bytes(b"bad\xFFname");
    std::fs::create_dir_all(layout.active_session_dir(bad_name))?;
    // Mirrors what `init` does for the default session: the WAL file must already exist before
    // `append_patch` (which appends to an existing file, matching `default_queue_wal_path`'s own
    // init-time creation) -- nothing pre-creates it for a hand-planted second session.
    std::fs::write(layout.active_queue_wal_path(bad_name), b"")?;

    let wal = Wal::for_layout(&layout, bad_name);
    assert_eq!(wal.path(), layout.active_queue_wal_path(bad_name).as_path());

    let envelope = signed_patch_envelope();
    let seq = wal.append_patch(&envelope);
    assert_eq!(seq, Ok(1));
    assert!(
        layout.active_queue_wal_path(bad_name).is_file(),
        "the WAL file must exist at the exact byte-exact path, not a mangled one"
    );
    let replay = wal.replay()?;
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert_eq!(replay.records.len(), 1);
    assert_eq!(
        replay.records.first().map(|record| &record.envelope),
        Some(&envelope)
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn wal_roundtrips_signed_patch_envelope() {
    let root = unique_temp_dir("wal");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        let envelope = signed_patch_envelope();
        let seq = wal.append_patch(&envelope);
        assert_eq!(seq, Ok(1));
        let replay = wal.replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert_eq!(replay.trailing_partial_bytes, 0);
            assert_eq!(replay.records.len(), 1);
            let first = replay.records.first();
            assert!(first.is_some());
            if let Some(first) = first {
                assert_eq!(first.seq, 1);
                assert_eq!(first.envelope, envelope);
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wal_rejects_unsigned_patch_envelope() {
    let root = unique_temp_dir("wal-unsigned");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        let mut envelope = signed_patch_envelope();
        envelope.signatures.clear();
        let result = wal.append_patch(&envelope);
        assert!(result.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wal_file_sync_failure_retains_replayable_record() {
    let root = unique_temp_dir("wal-file-sync-failure");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        fail_once_for_test(TestFailPoint::RequiredFileSync);
        assert!(wal.append_patch(&signed_patch_envelope()).is_err());
        let replay = wal.replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert_eq!(replay.records.len(), 1);
            assert_eq!(replay.trailing_partial_bytes, 0);
        }
        assert_eq!(wal.append_patch(&signed_patch_envelope()), Ok(1));
        assert_eq!(wal.replay().map(|replay| replay.records.len()), Ok(1));
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn first_wal_directory_sync_failure_retains_replayable_record() {
    let root = unique_temp_dir("wal-directory-sync-failure");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        fail_once_for_test(TestFailPoint::RequiredDirectorySync);
        assert!(wal.append_patch(&signed_patch_envelope()).is_err());
        let replay = wal.replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert_eq!(replay.records.len(), 1);
            assert_eq!(replay.trailing_partial_bytes, 0);
        }
        assert_eq!(wal.append_patch(&signed_patch_envelope()), Ok(1));
        assert_eq!(wal.replay().map(|replay| replay.records.len()), Ok(1));
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wal_truncate_failure_retains_partial_tail_and_retry_repairs_it() -> prikk_error::Result<()> {
    use std::io::Write;

    let root = unique_temp_dir("wal-truncate-failure");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    assert!(wal.append_patch(&signed_patch_envelope()).is_ok());
    let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
    file.write_all(b"partial")?;
    drop(file);

    fail_once_for_test(TestFailPoint::Truncate);
    assert!(wal.truncate_trailing_partial().is_err());
    assert_eq!(wal.replay()?.trailing_partial_bytes, 7);
    assert!(wal.truncate_trailing_partial().is_ok());
    let replay = wal.replay()?;
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.trailing_partial_bytes, 0);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-66 criterion 5: a torn queue of N > 1 must preserve every complete record and say *which*
/// patches survived, not just how many. `decode_records`/`truncate_trailing_partial` already looped
/// generically before DC-66 (nothing above them ever produced N > 1 to prove it against); this is the
/// first test to actually exercise that generality end to end, and the first to check the newly
/// reported `preserved_patch_ids`.
#[test]
fn wal_truncate_preserves_all_complete_records_in_a_torn_queue_and_reports_their_ids()
-> prikk_error::Result<()> {
    use std::io::Write;

    let root = unique_temp_dir("wal-torn-queue");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let first = signed_patch_envelope();
    let second = rollback_patch_envelope();
    assert_eq!(wal.append_patch(&first)?, 1);
    assert_eq!(wal.append_patch(&second)?, 2);

    let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
    file.write_all(b"partial")?;
    drop(file);

    assert_eq!(wal.replay()?.trailing_partial_bytes, 7);
    let repair = wal.truncate_trailing_partial()?;
    assert_eq!(repair.preserved_records, 2);
    assert_eq!(repair.truncated_bytes, 7);
    assert_eq!(
        repair.preserved_patch_ids,
        vec![first.object_id(), second.object_id()],
        "both complete queued patches must be identified, in append order"
    );

    let replay = wal.replay()?;
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert_eq!(
        replay
            .records
            .iter()
            .map(|record| &record.envelope)
            .collect::<Vec<_>>(),
        vec![&first, &second]
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 163 §4 (N6): a damaged **last** record -- its own bytes all present, checksum failed -- is,
/// by rule 3, indistinguishable from a genuine crash-torn tail once nothing sound follows it: the
/// repair truncates it and keeps its bytes, same as any other tail. The output must not be silent
/// about *what* it removed: this asserts `complete_records_removed` names it, distinguishing a real
/// (if damaged) write from a genuine interrupted append.
#[test]
fn wal_truncate_reports_when_the_removed_tail_includes_a_complete_damaged_record()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("wal-damaged-last-record");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let first = signed_patch_envelope();
    let second = rollback_patch_envelope();
    assert_eq!(wal.append_patch(&first)?, 1);
    assert_eq!(wal.append_patch(&second)?, 2);

    // Flip the file's own last byte: inside the second record's checksum or body, never its header
    // (the header ends well before the file's own end for any record with a real payload), so the
    // record's own claimed length is untouched -- exactly what "the bytes were all present, but
    // damaged" means.
    let mut bytes = std::fs::read(wal.path())?;
    if let Some(last) = bytes.last_mut() {
        *last ^= 0x01;
    }
    std::fs::write(wal.path(), &bytes)?;

    // RFC 162 rule 3: `verify`'s own replay must accept this as a tail (nothing sound follows the
    // damaged record), not damage -- the precondition this test's own claim depends on.
    let replay = wal.replay()?;
    assert_eq!(
        replay.records.len(),
        1,
        "only the first, undamaged record replays"
    );
    assert!(
        !replay.has_item_failure(),
        "rule 3: nothing sound follows the damaged record, so it is a tail, not an item failure"
    );
    assert_ne!(
        replay.trailing_partial_bytes, 0,
        "the damaged last record must be classified as tail bytes"
    );

    let repair = wal.truncate_trailing_partial()?;
    assert_eq!(repair.preserved_records, 1);
    assert_eq!(
        repair.complete_records_removed, 1,
        "the removed tail is one whole, if damaged, record -- not a genuine short fragment"
    );
    assert_eq!(
        repair.preserved_patch_ids,
        vec![first.object_id()],
        "only the first patch survives; the damaged second is what was removed"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **Control, run for real**: a genuinely short (structurally incomplete) tail must never be reported
/// as a complete record removed -- `complete_records_removed` is `0` for the exact fixture
/// [`wal_truncate_preserves_all_complete_records_in_a_torn_queue_and_reports_their_ids`] above
/// already builds (a real interrupted append, `b"partial"` appended after two sound records).
#[test]
fn wal_truncate_reports_zero_complete_records_for_a_genuine_short_tail() -> prikk_error::Result<()>
{
    use std::io::Write;

    let root = unique_temp_dir("wal-genuine-short-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    assert_eq!(wal.append_patch(&signed_patch_envelope())?, 1);
    let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
    file.write_all(b"partial")?;
    drop(file);

    let repair = wal.truncate_trailing_partial()?;
    assert_eq!(repair.truncated_bytes, 7);
    assert_eq!(
        repair.complete_records_removed, 0,
        "7 bytes of a torn append is not a complete record, whatever its content"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **Control, run for real**: M3's own zeros/garbage tail rows (100 zero bytes, 100 random bytes --
/// too long to be a structurally-incomplete fragment, but no plausible header either) must never be
/// miscounted as "complete records removed" -- they are noise, not writes.
#[test]
fn wal_truncate_reports_zero_complete_records_for_zeros_and_garbage_tails()
-> prikk_error::Result<()> {
    use std::io::Write;

    for (label, tail_bytes) in [
        ("100 zero bytes", vec![0_u8; 100]),
        (
            "100 deterministic non-zero bytes",
            (0..100_u32).map(|n| (n % 251) as u8 + 1).collect(),
        ),
    ] {
        let root = unique_temp_dir(&format!("wal-garbage-tail-{}", label.replace(' ', "-")));
        let layout = RepositoryLayout::init(root.clone())?;
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert_eq!(wal.append_patch(&signed_patch_envelope())?, 1);
        let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
        file.write_all(&tail_bytes)?;
        drop(file);

        let repair = wal.truncate_trailing_partial()?;
        assert_eq!(
            repair.complete_records_removed, 0,
            "{label}: garbage must never be counted as a complete record"
        );

        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn existing_wal_append_write_failure_is_retryable() -> prikk_error::Result<()> {
    let root = unique_temp_dir("wal-append-write-failure");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    assert_eq!(wal.append_patch(&signed_patch_envelope()), Ok(1));
    let mut second = signed_patch_envelope();
    if let Some(signature) = second.signatures.first_mut() {
        signature.created_at = signature.created_at.saturating_add(1);
    }

    fail_once_for_test(TestFailPoint::AppendWrite);
    assert!(wal.append_patch(&second).is_err());
    assert_eq!(wal.replay()?.records.len(), 1);
    assert_eq!(wal.append_patch(&second), Ok(2));
    assert_eq!(wal.replay()?.records.len(), 2);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn wal_replay_and_append_remain_on_retained_repository_root() -> prikk_error::Result<()> {
    let root = unique_temp_dir("wal-root-replacement");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    assert_eq!(wal.append_patch(&signed_patch_envelope()), Ok(1));
    let displaced = root.join(".prikk-displaced");
    std::fs::rename(layout.prikk_dir(), &displaced)?;
    std::fs::create_dir_all(root.join(".prikk/active/default"))?;
    std::fs::write(root.join(".prikk/active/default/queue.wal"), b"replacement")?;

    assert_eq!(wal.replay()?.records.len(), 1);
    assert_eq!(wal.append_patch(&signed_patch_envelope()), Ok(1));
    assert_eq!(wal.replay()?.records.len(), 1);
    assert_eq!(
        std::fs::read(root.join(".prikk/active/default/queue.wal"))?,
        b"replacement"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// ---- RFC 160 F3 Addendum 1: a repair keeps every byte it removes ------------------------------------------------------------------

fn recovery_bytes(layout: &RepositoryLayout, file: &std::path::Path) -> Vec<u8> {
    std::fs::read(layout.prikk_dir().join(file)).unwrap_or_default()
}

/// **A true torn tail: the recovery file holds exactly the removed bytes.** Two queued records and seven bytes of a torn third: the
/// repair saves those seven bytes, byte for byte, to `recovery/` **before** it truncates, names the file in its report, and leaves
/// the two records.
/// **Perturb:** truncate without saving (skip `save_removed_bytes`): `recovery_file` is `None` and this goes red.
#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn a_repair_saves_exactly_the_torn_bytes_it_removes() -> prikk_error::Result<()> {
    use std::io::Write;

    let root = unique_temp_dir("wal-repair-saves-torn-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope())?;
    wal.append_patch(&rollback_patch_envelope())?;
    let intact = std::fs::read(wal.path())?;
    let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
    file.write_all(b"partial")?;
    drop(file);

    let repair = wal.truncate_trailing_partial()?;
    let saved = repair
        .recovery_file
        .expect("the repair names its recovery file");
    assert_eq!(
        recovery_bytes(&layout, &saved),
        b"partial",
        "the file holds exactly the removed bytes"
    );
    assert!(
        saved.to_string_lossy().contains("recovery")
            && !saved.to_string_lossy().contains("quarantine"),
        "{saved:?}"
    );
    assert_eq!(
        std::fs::read(wal.path())?,
        intact,
        "and the WAL is the two records"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **A lone damaged record: the repair truncates it (it cannot tell it from a torn append), and the recovery file holds it whole.**
/// One queued record whose length field is set to 2^62. The repair removes every byte of the file and the recovery file is that file,
/// byte for byte -- so a repair that was wrong about what it removed lost nothing.
/// **Perturb:** truncate without saving: red.
#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn a_repair_of_a_lone_damaged_record_keeps_the_record_byte_for_byte() -> prikk_error::Result<()> {
    let root = unique_temp_dir("wal-repair-saves-lone-record");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope())?;
    let mut bytes = std::fs::read(wal.path())?;
    bytes[18..26].copy_from_slice(&(1_u64 << 62).to_be_bytes());
    std::fs::write(wal.path(), &bytes)?;

    let repair = wal.truncate_trailing_partial()?;
    assert_eq!(repair.preserved_records, 0);
    assert_eq!(repair.truncated_bytes, bytes.len());
    let saved = repair.recovery_file.expect("named");
    assert_eq!(
        recovery_bytes(&layout, &saved),
        bytes,
        "the removed record is saved whole"
    );
    assert!(std::fs::read(wal.path())?.is_empty());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **A failpoint between the save and the truncation** leaves the WAL untouched and the recovery file complete; the retry succeeds
/// and rewrites the same file (its name carries the offset and a hash of the bytes).
/// **Perturb:** truncate before saving (swap the two statements): the failpoint fails the truncation first, no recovery file was
/// written, and this goes red.
#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn a_failure_between_the_save_and_the_truncation_leaves_the_wal_and_a_complete_recovery_file()
-> prikk_error::Result<()> {
    use std::io::Write;

    let root = unique_temp_dir("wal-repair-failpoint-after-save");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope())?;
    let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
    file.write_all(b"partial")?;
    drop(file);
    let before = std::fs::read(wal.path())?;

    fail_once_for_test(TestFailPoint::Truncate);
    assert!(wal.truncate_trailing_partial().is_err());
    assert_eq!(std::fs::read(wal.path())?, before, "the WAL is untouched");
    let recovery: Vec<_> = std::fs::read_dir(layout.prikk_dir().join("recovery"))?
        .flatten()
        .collect();
    assert_eq!(recovery.len(), 1, "one recovery file");
    assert_eq!(
        std::fs::read(recovery[0].path())?,
        b"partial",
        "and it is complete"
    );

    let repair = wal.truncate_trailing_partial()?;
    let saved = repair.recovery_file.expect("named");
    assert_eq!(recovery_bytes(&layout, &saved), b"partial");
    assert_eq!(
        std::fs::read_dir(layout.prikk_dir().join("recovery"))?.count(),
        1,
        "the retry rewrote the same file"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
