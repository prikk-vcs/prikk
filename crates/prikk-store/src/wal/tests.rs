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
