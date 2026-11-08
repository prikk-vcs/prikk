//! CLI end-to-end regression for the DC-64 incremental baseline lifecycle-state cache.
//!
//! Covers the two evidence obligations the RFC names explicitly: deletion/rebuild must leave commit
//! outcomes unchanged (NFR-PERF-04), and cache/replay divergence must be detectable and reported
//! (`verify`), not silently trusted. See
//! `rfcs/handoffs/DC-64-baseline-reconstruction-cost/incremental-baseline-cache-design-v1.md` for
//! the trust condition these tests exercise.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    // RFC 148: prikk now reads key material from the user's own config directory, so a test that
    // does not neutralise it measures whoever is running it. See `support::isolate_key_environment`.
    support::isolate_key_environment(&mut cmd);
    cmd
}

fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fail(output: &Output, what: &str) {
    assert!(
        !output.status.success(),
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc64-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const AUTHOR_KEY_ID: &str = "dc64-test-author";
const AUTHOR_SEED_HEX: &str = "0011223344556677889900112233445566778899001122334455667788990011";
const MAINTAINER_KEY_ID: &str = "dc64-test-maintainer";
const MAINTAINER_SEED: [u8; 32] = [
    0x31, 0x31, 0x42, 0x42, 0x53, 0x53, 0x64, 0x64, 0x75, 0x75, 0x86, 0x86, 0x97, 0x97, 0xa8, 0xa8,
    0xb9, 0xb9, 0xca, 0xca, 0xdb, 0xdb, 0xec, 0xec, 0xfd, 0xfd, 0x0e, 0x0e, 0x1f, 0x1f, 0x20, 0x20,
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn maintainer_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    hex(&signer.public_key_bytes())
}

fn init(repo: &Path) {
    ok(&prikk(repo).arg("init").output().unwrap(), "init");
}

fn commit(repo: &Path, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", message])
        .output()
        .unwrap()
}

fn seal(repo: &Path) {
    let out = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            MAINTAINER_KEY_ID,
            "--public-key",
            &maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
    // Trusting the same key twice (once per copied repo in the deletion/rebuild test) is fine;
    // only assert success the first time each repo sees it, so allow either outcome here.
    let _ = out;

    let out = prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");
}

fn patch_id_line(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .find(|line| line.starts_with("patch id: "))
        .expect("commit output must include a patch id line")
        .to_string()
}

fn lifecycle_cache_path(repo: &Path) -> PathBuf {
    repo.join(".prikk").join("cache").join("lifecycle-state.v1")
}

fn write_two_files(repo: &Path, a: &str, b: &str) {
    std::fs::write(repo.join("a.txt"), a).unwrap();
    std::fs::write(repo.join("b.txt"), b).unwrap();
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path);
        } else {
            std::fs::copy(entry.path(), &dst_path).unwrap();
        }
    }
}

/// The cache is scoped to the commit path only and never touched by a genesis commit (baseline is
/// empty, nothing to replay); it first appears after the *second* real commit, which resolves a
/// `Published` baseline for the first time.
#[test]
fn genesis_commit_does_not_create_the_lifecycle_cache_but_the_second_commit_does() {
    let repo = unique_repo("appears-on-second-commit");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    assert!(!lifecycle_cache_path(&repo).exists());

    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");

    let cache_path = lifecycle_cache_path(&repo);
    assert!(cache_path.exists());
    let contents = std::fs::read(&cache_path).unwrap();
    assert!(contents.starts_with(b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// The RFC's stated NFR-PERF-04 evidence obligation: deleting the lifecycle cache and then
/// committing must produce a result identical to committing with the cache intact.
///
/// As with DC-56, comparing two independently-run repositories directly is unsound (fresh node ids
/// come from real OS randomness in production), so the comparison instead byte-for-byte copies one
/// shared lineage before diverging on cache presence.
#[test]
fn deleting_the_lifecycle_cache_does_not_change_commit_outcome() {
    let origin = unique_repo("rebuild-origin");
    init(&origin);
    write_two_files(&origin, "alpha\n", "bravo\n");
    ok(&commit(&origin, "genesis"), "genesis commit");
    seal(&origin);
    // Second commit: populates the lifecycle cache via a full replay (cache was cold).
    std::fs::write(origin.join("a.txt"), "alpha, edited once\n").unwrap();
    ok(&commit(&origin, "second"), "second commit");
    seal(&origin);
    // Third commit's mutation is what gets timed/compared below, with and without the cache.
    std::fs::write(origin.join("b.txt"), "bravo, edited\n").unwrap();

    let with_cache = unique_repo("rebuild-with-cache");
    let without_cache = unique_repo("rebuild-without-cache");
    copy_dir_recursive(&origin, &with_cache);
    copy_dir_recursive(&origin, &without_cache);

    assert!(lifecycle_cache_path(&with_cache).exists());
    std::fs::remove_file(lifecycle_cache_path(&without_cache)).unwrap();
    assert!(!lifecycle_cache_path(&without_cache).exists());

    // RFC 123: the message is now inside object identity, so it must be held constant across
    // both branches -- the cache is the only variable this test is isolating (see the doc comment
    // above). Before RFC 123 the two branches used different literal messages here and still
    // produced the same patch id, since the message was validated and discarded; that is no
    // longer true, correctly.
    let with_cache_output = commit(&with_cache, "third commit");
    ok(&with_cache_output, "third commit with cache intact");
    let without_cache_output = commit(&without_cache, "third commit");
    ok(&without_cache_output, "third commit with cache deleted");

    assert_eq!(
        patch_id_line(&with_cache_output.stdout),
        patch_id_line(&without_cache_output.stdout),
        "deleting the lifecycle cache must not change the committed patch"
    );

    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&with_cache);
    let _ = std::fs::remove_dir_all(&without_cache);
}

/// The ordinary case: a repository whose cache was populated normally has no lifecycle-cache
/// divergence.
#[test]
fn verify_reports_no_lifecycle_cache_divergence_for_an_untouched_repository() {
    let repo = unique_repo("verify-clean");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lifecycle-cache divergences: 0"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// The divergence-detection test the RFC asks for explicitly: a deliberately stale cached state —
/// still structurally decodable and checksum-valid, but disagreeing with what an independent replay
/// of the block it claims to represent produces — must be reported by `verify`, not silently
/// trusted by the next commit.
#[test]
fn verify_reports_a_deliberately_stale_lifecycle_cache_as_divergence() {
    let repo = unique_repo("verify-stale-lifecycle-cache");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");

    let cache_path = lifecycle_cache_path(&repo);
    let mut bytes = std::fs::read(&cache_path).unwrap();
    const MAGIC: &[u8] = b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0";
    assert!(bytes.len() > MAGIC.len() + 32);

    // Flip the last byte of the encoded body (inside the last node record's trailing field —
    // `mode` for a file entry). This keeps the wire structure and checksum-covered length intact;
    // only the checksum itself needs recomputing to stay internally consistent, exactly the
    // "persistence fault that a checksum alone would have caught if it hadn't been recomputed to
    // match" scenario a real corruption would not produce, but which exercises the same downstream
    // comparison against ground truth this check exists for.
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    let body = bytes[MAGIC.len() + 32..].to_vec();
    let checksum = prikk_hash::sha256(&body);
    bytes[MAGIC.len()..MAGIC.len() + 32].copy_from_slice(&checksum);
    std::fs::write(&cache_path, &bytes).unwrap();

    let out = prikk(&repo).arg("verify").output().unwrap();
    fail(&out, "verify with a deliberately stale lifecycle cache");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lifecycle-cache divergences: 1"));
    assert!(stdout.contains("lifecycle-cache [divergence] block"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// 0.57.0 step 1a: the same stale lifecycle cache, read by `doctor`. `verify` fails on it, and
/// `doctor` used to say nothing; it now reports `PRIKK-DOCTOR-LIFECYCLE-CACHE-DIVERGENCE` as a
/// **warning** (derived state, no history: exit stays 0, so no `--repair-*` is refused over it) and
/// says what is true of the cache.
///
/// **Perturb (red control):** drop the `lifecycle_cache_divergences` loop from `doctor_repository`.
#[test]
fn doctor_reports_a_deliberately_stale_lifecycle_cache_as_a_warning() {
    let repo = unique_repo("doctor-stale-lifecycle-cache");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");

    let cache_path = lifecycle_cache_path(&repo);
    let mut bytes = std::fs::read(&cache_path).unwrap();
    const MAGIC: &[u8] = b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0";
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    let body = bytes[MAGIC.len() + 32..].to_vec();
    let checksum = prikk_hash::sha256(&body);
    bytes[MAGIC.len()..MAGIC.len() + 32].copy_from_slice(&checksum);
    std::fs::write(&cache_path, &bytes).unwrap();

    fail(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify with a stale lifecycle cache",
    );
    let doctor = prikk(&repo).arg("doctor").output().unwrap();
    ok(&doctor, "doctor: a derived-state warning keeps exit 0");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        stdout.contains("warning [PRIKK-DOCTOR-LIFECYCLE-CACHE-DIVERGENCE]"),
        "{stdout}"
    );
    assert!(stdout.contains("holds no history"), "{stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// DC-95 Stage 1, round 12: `lifecycle_cache::incremental::verify_divergence`'s "could not be
/// independently verified" arm -- the deliberate inversion (prerequisite report §3.3) that converts
/// what would otherwise be a replay `Err` into a `Vec` divergence entry instead of propagating it,
/// so `verify` completes with a full picture rather than aborting. Distinct from the row above: that
/// one exercises the `Ok(_)` branch (structurally valid cache, disagreeing content); this one
/// exercises the `Err(_)` branch (the cache's own independent replay itself fails).
///
/// **A first construction attempt (delete the cache's claimed baseline block from the object store)
/// does not isolate this check.** `verify_objects` unconditionally re-derives every persisted V2
/// block's `state_merkle_root` via `block_state.rs`'s own full parent-lineage walk
/// (`validate_v2_lineage`/`resolved_parent_state`), which shares its patch/operation-application fold
/// with `lifecycle_cache/replay.rs` (`apply_candidate_patches` is defined there and imported by
/// `block_state.rs`). Deleting a block or patch anywhere in a currently-relevant lineage therefore
/// makes `verify_objects`' own state-root re-derivation fail first, with a hard `Err`, well before
/// `verify_lifecycle_cache_divergence` is ever reached -- structurally the same "earlier gate
/// intercepts the defect" pattern this module's own doc comment now names generally (rounds 10-11).
///
/// **The one place the two independent replays are not equivalent**: `lifecycle_cache/replay.rs`'s
/// walk is horizon-anchored -- it requires the parentless block the walk terminates on to equal the
/// cache's own claimed `horizon_id`, or it fails with `HorizonNotInLineage`
/// (`replay.rs::walk_single_parent_chain_inner`). `block_state.rs`'s own lineage walk has no horizon
/// concept at all; it simply walks to genesis and accepts whatever root it lands on. The cache file's
/// `horizon_id` field is read only by `lifecycle_cache::verify_divergence`, from the persisted cache
/// file itself (protected only by a plain SHA-256 over the body, recomputable) -- nothing else in
/// `verify_repository` reads or cross-checks it. Rewriting it to an arbitrary `ObjectId` therefore
/// changes nothing `verify_objects` or the ref checks look at: the object store stays byte-identical,
/// so this reaches the target check with no other category confounding the report -- unlike the first
/// attempt, which also flagged a dangling ref target (round 5's already-load-bearing check).
///
/// Byte offset for `horizon_id` (field 3, `WireType::ObjectId`) is computed the same way round 11's
/// op_seq surgery was: from `CanonicalWriter::field_raw`'s own fixed wire format (`tag: u16 BE` +
/// `wire_type: u8` + `len: u64 BE` + value), not by guessing, following
/// `verify_reports_a_deliberately_stale_lifecycle_cache_as_divergence`'s own established checksum-
/// recompute technique above.
#[test]
fn verify_reports_a_lifecycle_cache_with_a_horizon_not_in_its_own_lineage_as_divergence() {
    let repo = unique_repo("verify-lifecycle-horizon-mismatch");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");

    let cache_path = lifecycle_cache_path(&repo);
    let mut bytes = std::fs::read(&cache_path).unwrap();
    const MAGIC: &[u8] = b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0";
    // Field 1 (schema_version, U32): 2 (tag) + 1 (wire) + 8 (len) + 4 (value) = 15 bytes.
    // Field 2 (baseline_block_id, ObjectId): 2 + 1 + 8 + 32 = 43 bytes.
    // Field 3 (horizon_id, ObjectId) starts immediately after both, inside the checksummed body.
    const OBJECT_ID_FIELD_HEADER_LEN: usize = 11;
    const OBJECT_ID_LEN: usize = 32;
    let body_start = MAGIC.len() + 32;
    let field1_len = 2 + 1 + 8 + 4;
    let field2_len = OBJECT_ID_FIELD_HEADER_LEN + OBJECT_ID_LEN;
    let horizon_header = [0x00u8, 0x03, 0x12, 0, 0, 0, 0, 0, 0, 0, 0x20];
    let horizon_header_start = body_start + field1_len + field2_len;
    assert_eq!(
        &bytes[horizon_header_start..horizon_header_start + OBJECT_ID_FIELD_HEADER_LEN],
        &horizon_header,
        "expected the horizon_id field header at the computed offset"
    );
    let horizon_value_start = horizon_header_start + OBJECT_ID_FIELD_HEADER_LEN;
    bytes[horizon_value_start..horizon_value_start + OBJECT_ID_LEN].copy_from_slice(&[0xAB; 32]);
    let body = bytes[body_start..].to_vec();
    let checksum = prikk_hash::sha256(&body);
    bytes[MAGIC.len()..MAGIC.len() + 32].copy_from_slice(&checksum);
    std::fs::write(&cache_path, &bytes).unwrap();

    let out = prikk(&repo).arg("verify").output().unwrap();
    fail(
        &out,
        "verify with a lifecycle cache whose horizon isn't in its own lineage",
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lifecycle-cache divergences: 1"));
    assert!(stdout.contains("could not be independently verified"));
    assert!(stdout.contains("without crossing the claimed horizon"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// Damage the lifecycle cache so it stays structurally valid and checksum-correct but disagrees with a
/// replay (the persistence fault `verify` exists to catch, as `verify_reports_a_deliberately_stale_…`).
fn corrupt_lifecycle_cache(repo: &Path) {
    let cache_path = lifecycle_cache_path(repo);
    let mut bytes = std::fs::read(&cache_path).unwrap();
    const MAGIC: &[u8] = b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0";
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    let body = bytes[MAGIC.len() + 32..].to_vec();
    let checksum = prikk_hash::sha256(&body);
    bytes[MAGIC.len()..MAGIC.len() + 32].copy_from_slice(&checksum);
    std::fs::write(&cache_path, &bytes).unwrap();
}

fn rebuild(repo: &Path, extra: &[&str]) -> Output {
    prikk(repo)
        .args(["doctor", "--rebuild-caches"])
        .args(extra)
        .output()
        .unwrap()
}

/// **0.57.0 step 1d: one verb rebuilds every derived cache.** A stale lifecycle cache makes `verify`
/// fail, and both `verify` and `doctor` now say what to do **before the next commit** (a divergent cache
/// feeds `commit`'s incremental step) and name the verb. `--plan-only` names the cache and the count it
/// would read and leaves the file byte-identical; the real run replays the lineage from scratch, names the
/// cache and the count it read and wrote, leaves `verify` clean, and the next `commit` goes through and
/// leaves `verify` clean.
///
/// **Perturb (red controls):** skip the lifecycle rebuild's write -- the cure assertions go red; let the
/// plan write (`rebuild_lifecycle_cache_inner(.., true)` always) -- the byte-identical assertion goes red;
/// drop "before your next commit" from the advice -- the advice assertions go red.
#[test]
fn rebuild_caches_cures_a_stale_lifecycle_cache_and_names_what_it_read() {
    let repo = unique_repo("rebuild-caches-lifecycle");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");
    corrupt_lifecycle_cache(&repo);

    let verify = prikk(&repo).arg("verify").output().unwrap();
    fail(&verify, "verify with a stale lifecycle cache");
    let advice_marks = ["before your next commit", "prikk doctor --rebuild-caches"];
    let stdout = String::from_utf8_lossy(&verify.stdout);
    for mark in advice_marks {
        assert!(stdout.contains(mark), "verify names {mark:?}: {stdout}");
    }
    let doctor = prikk(&repo).arg("doctor").output().unwrap();
    ok(&doctor, "doctor: a derived-state warning keeps exit 0");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    for mark in advice_marks {
        assert!(stdout.contains(mark), "doctor names {mark:?}: {stdout}");
    }

    let before = std::fs::read(lifecycle_cache_path(&repo)).unwrap();
    let plan = rebuild(&repo, &["--plan-only"]);
    ok(&plan, "plan");
    let stdout = String::from_utf8_lossy(&plan.stdout);
    assert!(stdout.contains("commit-index:"), "{stdout}");
    assert!(
        stdout.contains("lifecycle-state cache: 1 block would be replayed from scratch"),
        "names the cache and the count: {stdout}"
    );
    assert!(stdout.contains("plan only -- nothing written"), "{stdout}");
    assert_eq!(
        std::fs::read(lifecycle_cache_path(&repo)).unwrap(),
        before,
        "a plan writes nothing"
    );

    let run = rebuild(&repo, &[]);
    ok(&run, "rebuild");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("lifecycle-state cache: 1 block replayed from scratch"),
        "{stdout}"
    );
    assert!(
        stdout.contains("lifecycle-state cache rebuilt:") && stdout.contains("nodes written"),
        "{stdout}"
    );
    assert!(stdout.contains("hold no history"), "{stdout}");
    assert_ne!(std::fs::read(lifecycle_cache_path(&repo)).unwrap(), before);

    let verify = prikk(&repo).arg("verify").output().unwrap();
    ok(&verify, "verify after the rebuild");
    assert!(
        String::from_utf8_lossy(&verify.stdout).contains("lifecycle-cache divergences: 0"),
        "clean after the rebuild"
    );
    std::fs::write(repo.join("b.txt"), "bravo, edited\n").unwrap();
    ok(&commit(&repo, "third"), "the next commit goes through");
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after the next commit",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Both caches damaged at once: one run cures both, and `verify` is clean. (The commit-index is damaged by
/// a fabricated hash with its stat untouched, as `dc56_commit_index` does.)
#[test]
fn one_rebuild_caches_run_cures_both_caches() {
    let repo = unique_repo("rebuild-caches-both");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    support::backdate_mtime(&repo.join("a.txt"));
    support::backdate_mtime(&repo.join("b.txt"));
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    support::backdate_mtime(&repo.join("a.txt"));
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    support::backdate_mtime(&repo.join("a.txt"));
    ok(&commit(&repo, "second"), "second commit");
    corrupt_lifecycle_cache(&repo);
    let index_path = repo.join(".prikk").join("cache").join("commit-index.v1");
    let original = std::fs::read_to_string(&index_path).unwrap();
    let damaged: String = original
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                format!("{line}\n")
            } else {
                let mut fields: Vec<&str> = line.split('\t').collect();
                let zeros = "0".repeat(64);
                *fields.last_mut().unwrap() = &zeros;
                format!("{}\n", fields.join("\t"))
            }
        })
        .collect();
    std::fs::write(&index_path, damaged).unwrap();

    let verify = prikk(&repo).arg("verify").output().unwrap();
    fail(&verify, "verify with both caches damaged");
    let stdout = String::from_utf8_lossy(&verify.stdout);
    assert!(
        stdout.contains("lifecycle-cache divergences: 1"),
        "{stdout}"
    );
    assert!(stdout.contains("commit-index divergences: "), "{stdout}");
    assert!(!stdout.contains("commit-index divergences: 0"), "{stdout}");

    ok(&rebuild(&repo, &[]), "one run");
    let verify = prikk(&repo).arg("verify").output().unwrap();
    ok(&verify, "verify after the single rebuild");
    let stdout = String::from_utf8_lossy(&verify.stdout);
    assert!(
        stdout.contains("lifecycle-cache divergences: 0"),
        "{stdout}"
    );
    assert!(stdout.contains("commit-index divergences: 0"), "{stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// A branch with no published history has no lifecycle cache to rebuild: the verb says so plainly, leaves
/// the file alone, and still rebuilds the commit-index. The old flag name is gone (it never shipped).
#[test]
fn rebuild_caches_on_an_unborn_branch_and_the_old_flag_name() {
    let repo = unique_repo("rebuild-caches-unborn");
    init(&repo);
    let run = rebuild(&repo, &[]);
    ok(&run, "rebuild on an unborn branch");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("has no published history, so there is nothing to rebuild"),
        "{stdout}"
    );
    assert!(stdout.contains("commit-index rebuilt:"), "{stdout}");
    let old = prikk(&repo)
        .args(["doctor", "--rebuild-commit-index"])
        .output()
        .unwrap();
    assert_eq!(old.status.code(), Some(2), "{old:?}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// The lifecycle half is under the lock `commit` takes too: with the active lock held elsewhere, the
/// verb exits 1 with `lock conflict` and **neither** cache file changes; a plan still answers.
#[test]
fn rebuild_caches_conflicts_with_a_held_lock_and_changes_neither_cache() {
    let repo = unique_repo("rebuild-caches-lock");
    init(&repo);
    write_two_files(&repo, "alpha\n", "bravo\n");
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
    ok(&commit(&repo, "second"), "second commit");
    corrupt_lifecycle_cache(&repo);
    let cache = std::fs::read(lifecycle_cache_path(&repo)).unwrap();
    let index_path = repo.join(".prikk").join("cache").join("commit-index.v1");
    let index = std::fs::read(&index_path).ok();

    let layout = prikk_store::RepositoryLayout::open(&repo).unwrap();
    let held = prikk_store::ActiveLock::acquire(&layout, prikk_store::DEFAULT_ACTIVE_NAME).unwrap();
    let blocked = rebuild(&repo, &[]);
    assert_eq!(blocked.status.code(), Some(1), "{blocked:?}");
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("lock conflict"));
    assert_eq!(std::fs::read(lifecycle_cache_path(&repo)).unwrap(), cache);
    assert_eq!(std::fs::read(&index_path).ok(), index);
    ok(&rebuild(&repo, &["--plan-only"]), "a plan takes no lock");
    drop(held);
    ok(
        &rebuild(&repo, &[]),
        "the rebuild runs once the lock is free",
    );
    let _ = std::fs::remove_dir_all(&repo);
}
