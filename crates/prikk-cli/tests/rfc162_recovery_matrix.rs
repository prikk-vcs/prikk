//! RFC 162 §5: the crash-and-corruption matrix -- every framed file this round's rules cover (object
//! container, object index, WAL, pointer index) x a fixed set of faults x the commands that follow,
//! checked against the review's four invariants:
//! - **I1.** `verify` exit 0 implies the next `seal`, and every read of committed work, succeeds.
//! - **I2.** No repair turns a failing `verify` into a passing one unless what was damaged is restored.
//! - **I3.** From every state a crash can leave, a documented command sequence reaches a repository
//!   that accepts a commit.
//! - **I4.** Every repair is idempotent, removes no sound record, and keeps what it removes.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// magic(8) version(2) body_len(8) checksum(32) -- the object container, the object index, and the
/// pointer index all share this header shape.
const OBJECT_HEADER_LEN: usize = 8 + 2 + 8 + 32;
/// magic(8) version(2) seq(8) body_len(8) checksum(32) -- the WAL's own header, one field longer.
const WAL_HEADER_LEN: usize = 8 + 2 + 8 + 8 + 32;

/// One of the four framed files RFC 162 covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetFile {
    ObjectContainer,
    ObjectIndex,
    Wal,
    PointerIndex,
}

impl TargetFile {
    const ALL: [Self; 4] = [
        Self::ObjectContainer,
        Self::ObjectIndex,
        Self::Wal,
        Self::PointerIndex,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::ObjectContainer => "object container",
            Self::ObjectIndex => "object index",
            Self::Wal => "WAL",
            Self::PointerIndex => "pointer index",
        }
    }

    fn relative_path(self) -> &'static str {
        match self {
            Self::ObjectContainer => "containers/blob/a.container",
            Self::ObjectIndex => "containers/index.container",
            Self::Wal => "active/default/queue.wal",
            Self::PointerIndex => "refs/containers/pointer-index-a.container",
        }
    }

    fn header_len(self) -> usize {
        match self {
            Self::Wal => WAL_HEADER_LEN,
            _ => OBJECT_HEADER_LEN,
        }
    }

    /// Byte offset of the big-endian `u64` body length field within the header.
    fn body_len_offset(self) -> usize {
        match self {
            Self::Wal => 8 + 2 + 8,
            _ => 8 + 2,
        }
    }

    /// `doctor`'s own repair verb for this file, per rule 1 (object container/index -- a rebuild is
    /// the same verb for both, since the container is never repaired, only the index cache built from
    /// it) and rule 3 (WAL, pointer index -- a positional tail truncation).
    fn repair_args(self) -> &'static [&'static str] {
        match self {
            Self::ObjectContainer | Self::ObjectIndex => &["doctor", "--repair-index"],
            Self::Wal => &["doctor", "--repair-wal-tail"],
            Self::PointerIndex => &["doctor", "--repair-pointer-index-tail"],
        }
    }
}

fn path_of(repo: &Path, target: TargetFile) -> PathBuf {
    repo.join(".prikk").join(target.relative_path())
}

fn read_target(repo: &Path, target: TargetFile) -> Vec<u8> {
    std::fs::read(path_of(repo, target)).expect("target file exists")
}

fn write_target(repo: &Path, target: TargetFile, bytes: &[u8]) {
    std::fs::write(path_of(repo, target), bytes).expect("target file is writable");
}

/// One of the fixed faults RFC 162 §5 names, plus a "no fault" baseline for the control checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    TornPrefix,
    Zeros30,
    Zeros100,
    Zeros4096,
    Random100,
    FlippedBodyByte,
    FlippedLength,
}

impl Fault {
    const ALL: [Self; 7] = [
        Self::TornPrefix,
        Self::Zeros30,
        Self::Zeros100,
        Self::Zeros4096,
        Self::Random100,
        Self::FlippedBodyByte,
        Self::FlippedLength,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::TornPrefix => "torn prefix",
            Self::Zeros30 => "30 zero bytes",
            Self::Zeros100 => "100 zero bytes",
            Self::Zeros4096 => "4096 zero bytes",
            Self::Random100 => "100 random bytes",
            Self::FlippedBodyByte => "flipped body byte",
            Self::FlippedLength => "flipped length",
        }
    }

    /// The tail-shaped faults RFC 162 rule 3 redefines positionally (a torn prefix, or garbage
    /// appended after the last sound record). The other two are interior damage: a sound record
    /// follows them, so they stay refused regardless of shape.
    fn is_tail_shaped(self) -> bool {
        matches!(
            self,
            Self::TornPrefix | Self::Zeros30 | Self::Zeros100 | Self::Zeros4096 | Self::Random100
        )
    }

    fn deterministic_bytes(n: usize) -> Vec<u8> {
        // Fixed xorshift64*, not a nondeterministic RNG: a failing row must reproduce exactly.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut bytes = Vec::with_capacity(n);
        for _ in 0..n {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.push((state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8);
        }
        bytes
    }

    fn apply(self, repo: &Path, target: TargetFile) {
        let mut bytes = read_target(repo, target);
        match self {
            Self::TornPrefix => {
                // A genuine interrupted append: a copy of this format's own magic+version, cut short
                // before a full header lands -- appended after the existing sound content, which stays
                // intact (a real crash never erases what came before the append it interrupted).
                let prefix_len = (target.header_len() - 5).min(bytes.len());
                let header_prefix = bytes[..prefix_len].to_vec();
                bytes.extend(header_prefix);
            }
            Self::Zeros30 => bytes.extend(vec![0_u8; 30]),
            Self::Zeros100 => bytes.extend(vec![0_u8; 100]),
            Self::Zeros4096 => bytes.extend(vec![0_u8; 4096]),
            Self::Random100 => bytes.extend(Self::deterministic_bytes(100)),
            Self::FlippedBodyByte => {
                // A few bytes into the first record's body -- well past its header -- leaving a sound
                // second record behind it.
                let flip_at = target.header_len() + 4;
                bytes[flip_at] ^= 0x01;
            }
            Self::FlippedLength => {
                let offset = target.body_len_offset();
                bytes[offset] ^= 0xFF;
            }
        }
        write_target(repo, target, &bytes);
    }
}

/// A repository with two sealed generations (so every sealed container -- object, index, pointer
/// index -- holds two or more records) and two queued, unsealed commits behind them (so the WAL also
/// holds two records). Every target file therefore has a sound record for an interior fault to leave
/// standing behind the one it damages.
fn matrix_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("one.txt"), "one\n".repeat(50)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    std::fs::write(repo.join("two.txt"), "two\n".repeat(50)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "second"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    std::fs::write(repo.join("three.txt"), "three\n".repeat(50)).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "third (queued)"),
        "commit",
    );
    std::fs::write(repo.join("four.txt"), "four\n".repeat(50)).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "fourth (queued)"),
        "commit",
    );
    repo
}

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = support::prikk(repo).args(args).output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn verify_exit(repo: &Path) -> (Option<i32>, String) {
    run(repo, &["verify"])
}

/// Whether `verify` refuses (exit non-zero) a given `(file, fault)` pair, as the round measured it
/// empirically against the real binary (see the round's report). RFC 162's three rules give each row
/// its own reason:
/// - a **tail-shaped** fault on any of the four files is a repairable tail (rule 3, WAL/pointer index)
///   or a warning-only remnant (rule 1's "never evidence of anything" for the object index; the
///   object container's own unparseable-frame classification defaults to a warning too, rule 2's
///   "index membership is no longer the witness" -- see `verify/objects.rs`) -- `verify` exits 0.
/// - an **interior** fault (a sound record follows it) on the object container, WAL, or pointer index
///   is damage and stays refused -- `verify` exits non-zero.
/// - an **interior** fault on the object index itself is not damage at all: the index is a pure cache
///   (rule 1), so garbage inside it, with sound records on either side, changes nothing a reader
///   trusts -- `verify` exits 0, uniquely among the four files.
fn verify_should_refuse(target: TargetFile, fault: Fault) -> bool {
    if fault.is_tail_shaped() {
        return false;
    }
    target != TargetFile::ObjectIndex
}

/// **The matrix itself** (RFC 162 §5): every framed file this round covers, times every fault, checked
/// first against `verify`'s own exit code -- the shape [`verify_should_refuse`] documents, each
/// row explained by which of the three rules it exercises.
#[test]
fn the_matrix_verify_exit_matches_each_rules_own_shape() {
    let mut failures = Vec::new();
    for target in TargetFile::ALL {
        for fault in Fault::ALL {
            let repo = matrix_repository(&format!(
                "rfc162-matrix-{}-{}",
                target.name().replace(' ', "-"),
                fault.name().replace(' ', "-")
            ));
            fault.apply(&repo, target);
            let (code, text) = verify_exit(&repo);
            let refused = code.is_some_and(|code| code != 0);
            let expected = verify_should_refuse(target, fault);
            if refused != expected {
                failures.push(format!(
                    "{} / {}: expected verify to {} but it {} (exit {code:?})\n{text}",
                    target.name(),
                    fault.name(),
                    if expected { "refuse" } else { "accept" },
                    if refused { "refused" } else { "accepted" }
                ));
            }
            // Addendum 1 fix 3: "accepted" (the object index tolerating any of these faults, and the
            // pointer index tolerating its tail-shaped ones) must never mean unreported -- a report
            // line is asserted here per cell, not only the exit code above.
            match target {
                // Addendum 1 fix 3: the object index's own decode does not draw the same
                // tail-shaped/interior line rule 3 gives the WAL and pointer index (out of rule 3's
                // scope by design -- see `foundation/index.rs`'s own `Invalid` arm, unchanged) -- a
                // fault that fully fits a header's width but fails its magic or checksum is `Failed`
                // (the interior-damage line) even when this matrix calls it "tail-shaped," while one
                // that does not even fill a header is the trailing-partial line instead. Either is a
                // report; only both absent is silent.
                TargetFile::ObjectIndex
                    if text.contains("trailing partial object index bytes: 0")
                        && !text.contains(
                            "object index has an interior record that failed to decode",
                        ) =>
                {
                    failures.push(format!(
                        "{} / {}: accepted with no report line at all\n{text}",
                        target.name(),
                        fault.name()
                    ));
                }
                TargetFile::PointerIndex
                    if fault.is_tail_shaped()
                        && text.contains("trailing partial pointer index bytes: 0") =>
                {
                    failures.push(format!(
                        "{} / {}: accepted with no trailing-partial report line\n{text}",
                        target.name(),
                        fault.name()
                    ));
                }
                _ => {}
            }
            let _ = std::fs::remove_dir_all(repo);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// **I3 and I4, for every tail-shaped fault**: the repair reaches a repository that accepts a commit
/// (I3), is idempotent, and removes no sound record (I4) -- both records written before the fault
/// (`one.txt`'s and `two.txt`'s generations, or the two queued patches for the WAL) must still read
/// back, and a second repair run must find nothing left to do.
#[test]
fn tail_shaped_faults_repair_to_a_working_repository_idempotently() {
    let mut failures = Vec::new();
    for target in TargetFile::ALL {
        for fault in Fault::ALL
            .into_iter()
            .filter(|fault| fault.is_tail_shaped())
        {
            let repo = matrix_repository(&format!(
                "rfc162-repair-{}-{}",
                target.name().replace(' ', "-"),
                fault.name().replace(' ', "-")
            ));
            fault.apply(&repo, target);
            let label = format!("{} / {}", target.name(), fault.name());

            let (repair_code, repair_text) = run(&repo, target.repair_args());
            if repair_code != Some(0) {
                failures.push(format!(
                    "{label}: repair exit {repair_code:?}\n{repair_text}"
                ));
                let _ = std::fs::remove_dir_all(repo);
                continue;
            }

            // I4a: idempotent. A second run finds nothing left to repair.
            let (second_code, second_text) = run(&repo, target.repair_args());
            if second_code != Some(0) {
                failures.push(format!(
                    "{label}: second (idempotent) repair exit {second_code:?}\n{second_text}"
                ));
            }
            let nothing_left = second_text.contains("nothing to repair")
                || second_text.contains("truncated 0 byte")
                || second_text.contains("truncated 0 trailing byte");
            if !nothing_left {
                failures.push(format!(
                    "{label}: second repair did not report a no-op\n{second_text}"
                ));
            }

            // I1 (verify exit 0 implies the next seal and every read succeed) and I4b (no sound
            // record lost): both prior generations still read back.
            for path in ["one.txt", "two.txt"] {
                let (read_code, read_text) =
                    run(&repo, &["cat", "--path", path, "--ref", "heads/main"]);
                if read_code != Some(0) {
                    failures.push(format!("{label}: reading {path} after repair: {read_text}"));
                }
            }

            // I3: the repository accepts a new commit after the repair.
            std::fs::write(repo.join("after-repair.txt"), b"after repair\n").unwrap();
            let commit_output = support::commit(&repo, "heads/main", "after repair");
            if !commit_output.status.success() {
                failures.push(format!(
                    "{label}: I3, a commit after repair: exit {:?}\n{}{}",
                    commit_output.status.code(),
                    String::from_utf8_lossy(&commit_output.stdout),
                    String::from_utf8_lossy(&commit_output.stderr)
                ));
            }

            let (after_verify_code, after_verify_text) = verify_exit(&repo);
            if after_verify_code != Some(0) {
                failures.push(format!(
                    "{label}: verify after repair and a new commit: exit {after_verify_code:?}\n{after_verify_text}"
                ));
            }

            let _ = std::fs::remove_dir_all(repo);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// **I2, for every interior fault the object container, WAL, and pointer index still refuse**: the
/// file's own repair verb must not turn the failing `verify` into a passing one -- it is not a torn
/// tail, and the repair that truncates or rebuilds a tail must refuse it, leaving the repository
/// exactly as damaged as it found it. The object index is excluded: an interior fault there is not
/// damage at all (see [`verify_should_refuse`]), so there is nothing for I2 to guard.
#[test]
fn interior_faults_survive_their_files_own_repair_attempt() {
    let mut failures = Vec::new();
    for target in TargetFile::ALL
        .into_iter()
        .filter(|target| *target != TargetFile::ObjectIndex)
    {
        for fault in Fault::ALL
            .into_iter()
            .filter(|fault| !fault.is_tail_shaped())
        {
            let repo = matrix_repository(&format!(
                "rfc162-i2-{}-{}",
                target.name().replace(' ', "-"),
                fault.name().replace(' ', "-")
            ));
            fault.apply(&repo, target);
            let label = format!("{} / {}", target.name(), fault.name());

            let (before_code, _) = verify_exit(&repo);
            assert_eq!(
                before_code,
                Some(1),
                "{label}: this row is only meaningful when verify already refuses"
            );

            let before_bytes = read_target(&repo, target);
            let _ = run(&repo, target.repair_args());
            let after_bytes = read_target(&repo, target);

            let (after_code, after_text) = verify_exit(&repo);
            if after_code != Some(1) {
                failures.push(format!(
                    "{label}: I2, the repair turned a failing verify into a passing one \
                     (exit {after_code:?}) without restoring what was damaged\n{after_text}"
                ));
            }
            if target != TargetFile::ObjectContainer && before_bytes != after_bytes {
                // The object container is never touched by any repair (rule 1: only the index is
                // ever rewritten); the other three files' own repair must refuse and leave the file
                // untouched too.
                failures.push(format!(
                    "{label}: I2, the file changed even though the repair refused (or should have)"
                ));
            }

            let _ = std::fs::remove_dir_all(repo);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// **M1, at the CLI, reproduced exactly as the external review measured it.** A blob referenced only
/// by a *queued* (unsealed) commit is damaged: one byte flipped in its container frame. `verify`
/// fails. `doctor --repair-index` rebuilds the index (the blob's own entry cannot be re-derived, so it
/// is dropped and named as a lost id) -- but the queued commit still references the blob, so `verify`
/// must still fail afterward, naming the patch. Before this round's rule 2, the repair silently
/// reclassified the frame as "an interrupted append... nothing references it" and `verify` passed.
/// Since 0.49.0 step 5, D11/U5, this exact shape (nothing physically follows the one damaged frame in
/// its container) is a failed object item from the start, not an interrupted-append line at all --
/// see `foundation::container::parse_frame_at_reporting`'s own comment on `complete`.
#[test]
fn m1_a_queued_patch_referencing_a_damaged_blob_fails_verify_before_and_after_repair_index() {
    let repo = support::unique_repo("rfc162-m1");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(50)).unwrap();
    let commit = support::commit(&repo, "heads/main", "queued commit");
    support::ok(&commit, "queued commit");

    let container = repo.join(".prikk/containers/blob/a.container");
    let mut bytes = std::fs::read(&container).unwrap();
    *bytes.last_mut().unwrap() ^= 0x01;
    std::fs::write(&container, &bytes).unwrap();

    let (before_code, before_text) = verify_exit(&repo);
    assert_eq!(before_code, Some(1), "M1: verify must fail\n{before_text}");

    let (repair_code, repair_text) = run(&repo, &["doctor", "--repair-index"]);
    assert_eq!(
        repair_code,
        Some(1),
        "M1: the repair itself exits non-zero over the lost id\n{repair_text}"
    );
    assert!(
        repair_text.contains("could not be re-derived"),
        "M1: the repair names the lost id\n{repair_text}"
    );

    let (after_code, after_text) = verify_exit(&repo);
    assert_eq!(
        after_code,
        Some(1),
        "M1: verify must still fail after the repair -- the queued commit still references the \
         damaged blob\n{after_text}"
    );
    assert!(
        after_text.contains("connectivity") || after_text.contains("references"),
        "M1: the failure names the connectivity problem\n{after_text}"
    );
    assert!(
        !after_text.contains("is a harmless remnant")
            && !after_text.contains("connectivity finds nothing")
            && !after_text.contains("warning: interrupted append")
            && after_text.contains("interrupted appends: 0"),
        "M1: a complete, damaged frame with nothing after it in its container is a failed object \
         item, never worded as an interrupted append\n{after_text}"
    );
    assert!(
        after_text.contains("object items: 1 scanned, 1 failed")
            && after_text.contains(": failed: container checksum mismatch"),
        "M1: the failed object item itself is named\n{after_text}"
    );

    // Addendum 1 fix 1: I2 applies to `doctor` too -- the documented path is repair, then `doctor`,
    // and it must not read clean while the queued commit still references the damaged blob.
    let (doctor_code, doctor_text) = run(&repo, &["doctor"]);
    assert!(
        doctor_code.is_some_and(|code| code != 0),
        "M1: doctor must fail where verify fails on connectivity\n{doctor_text}"
    );
    assert!(
        doctor_text.contains("PRIKK-DOCTOR-OBJECT-CONNECTIVITY"),
        "M1: doctor names the connectivity problem\n{doctor_text}"
    );

    let _ = std::fs::remove_dir_all(repo);
}

/// **M2, at the CLI, reproduced exactly as the external review measured it.** A genuine 60-byte
/// prefix of a real index record, appended to the object index -- what an interrupted index append
/// leaves. Before this round's rule 1, every subsequent command refused ("object index has a damaged
/// entry") and `--repair-index` answered "nothing to repair" (the set comparison alone did not catch a
/// torn tail). Now: `verify` accepts it (a repairable tail is not damage), and -- rule 1's own point --
/// a plain **write does not need a prior `--repair-index` step at all**: the writer rebuilds under the
/// lock before appending.
#[test]
fn m2_a_torn_index_append_then_a_write_no_longer_refuses_everything() {
    let repo = support::unique_repo("rfc162-m2");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(50)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    Fault::TornPrefix.apply(&repo, TargetFile::ObjectIndex);

    let (verify_code, verify_text) = verify_exit(&repo);
    assert_eq!(
        verify_code,
        Some(0),
        "M2: verify must accept a torn index append as a repairable tail\n{verify_text}"
    );

    // The write itself, with no `--repair-index` step first: the writer rebuilds under its own lock.
    std::fs::write(repo.join("b.txt"), "b\n".repeat(50)).unwrap();
    let commit = support::commit(&repo, "heads/main", "after the torn append");
    assert!(
        commit.status.success(),
        "M2: a write must not need a prior repair\n{}{}",
        String::from_utf8_lossy(&commit.stdout),
        String::from_utf8_lossy(&commit.stderr)
    );

    let (after_code, after_text) = verify_exit(&repo);
    assert_eq!(
        after_code,
        Some(0),
        "M2: verify after the write\n{after_text}"
    );

    // The stronger claim: the write itself rebuilt the index cleanly (rule 1's own "a write never
    // buries a torn index tail"), not merely that `verify` tolerates whatever the write left behind --
    // a subsequent `--repair-index` must find nothing left to do.
    let (repair_code, repair_text) = run(&repo, &["doctor", "--repair-index"]);
    assert_eq!(
        repair_code,
        Some(0),
        "M2: a repair after the write\n{repair_text}"
    );
    assert!(
        repair_text.contains("nothing to repair"),
        "M2: the write must have already left the index clean, not merely tolerated\n{repair_text}"
    );

    let _ = std::fs::remove_dir_all(repo);
}

/// **M3, at the CLI, reproduced exactly as the external review measured it.** Bytes appended after
/// the last sound WAL record, at the review's own sizes: 30 zero bytes (already repairable before this
/// round), and 100 zero bytes / 4,096 zero bytes / 100 random bytes (fatal before this round, since a
/// tail was defined by shape, not position). Every one of the four now `verify`s clean and repairs to
/// a working repository.
#[test]
fn m3_wal_tails_at_every_size_the_review_measured_all_repair_to_a_working_repository() {
    for fault in [
        Fault::Zeros30,
        Fault::Zeros100,
        Fault::Zeros4096,
        Fault::Random100,
    ] {
        let repo = matrix_repository(&format!("rfc162-m3-{}", fault.name().replace(' ', "-")));
        fault.apply(&repo, TargetFile::Wal);

        let (verify_code, verify_text) = verify_exit(&repo);
        assert_eq!(
            verify_code,
            Some(0),
            "M3 {}: verify must accept the tail\n{verify_text}",
            fault.name()
        );

        let (repair_code, repair_text) = run(&repo, TargetFile::Wal.repair_args());
        assert_eq!(
            repair_code,
            Some(0),
            "M3 {}: the repair itself\n{repair_text}",
            fault.name()
        );

        std::fs::write(repo.join("after-m3.txt"), b"after m3\n").unwrap();
        let commit = support::commit(&repo, "heads/main", "after M3's repair");
        assert!(
            commit.status.success(),
            "M3 {}: a commit after the repair\n{}{}",
            fault.name(),
            String::from_utf8_lossy(&commit.stdout),
            String::from_utf8_lossy(&commit.stderr)
        );

        let _ = std::fs::remove_dir_all(repo);
    }
}

/// **Incident 7 stays fixed**: a crash-torn object frame that nothing references, then a later write.
/// `verify` exits 0 with a warning (an unreferenced remnant), never damage -- the object container's
/// own tail is not covered by rule 3's positional redefinition, but rule 2's "index membership is no
/// longer the witness" reaches the same outcome for the case incident 7 is about: nothing references
/// the torn frame, so it is never damage regardless of its shape.
#[test]
fn incident_7_a_crash_torn_object_frame_nothing_references_is_a_warning_not_damage() {
    let repo = support::unique_repo("rfc162-incident-7");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(50)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    Fault::TornPrefix.apply(&repo, TargetFile::ObjectContainer);

    let (verify_code, verify_text) = verify_exit(&repo);
    assert_eq!(
        verify_code,
        Some(0),
        "incident 7: an unreferenced torn frame is a warning, not damage\n{verify_text}"
    );
    assert!(
        verify_text.contains("interrupted append"),
        "incident 7: the warning is reported, not silent\n{verify_text}"
    );

    // A later write still succeeds -- the crash never makes verify fail for good.
    std::fs::write(repo.join("b.txt"), "b\n".repeat(50)).unwrap();
    let commit = support::commit(&repo, "heads/main", "after the crash");
    assert!(
        commit.status.success(),
        "incident 7: a later write must still succeed\n{}{}",
        String::from_utf8_lossy(&commit.stdout),
        String::from_utf8_lossy(&commit.stderr)
    );

    let _ = std::fs::remove_dir_all(repo);
}
