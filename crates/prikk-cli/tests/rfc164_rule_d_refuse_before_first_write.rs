//! RFC 164 Rule D: a publishing command refuses over its guarded file's own tail or damage at the
//! start, before any content object is written -- not only after, as `seal`, `branch create`, `tag
//! create`, and `merge` all did before this round (the pointer index), and a new author's first
//! commit did (the author-key container). `bundle import` and `trust maintainer add/remove` were
//! already compliant (RFC 163 §10/§9) and are not re-tested here.
//!
//! **Addendum 1 (round 2 review v1): `compact --all` and `rollback-draft --append-inverse` were not
//! compliant either**, found by the architect's own `rfc164_rule_d_writers_probe.sh` and `rfc164_
//! rule_d_sync_rollback_probe.sh`. `compact --all` compacted the pointer index durably before ever
//! reaching the received index's or trust policy's own tail/damage -- Rule D held per subsystem, not
//! across the whole command -- and separately, ANY single subsystem's own compaction silently dropped
//! a tail on its own LIVE slot (not only the generation log), with no recovery file and no line saying
//! so. `rollback-draft --append-inverse` wrote `active/default/ref-name` before reaching the
//! author-key check that could refuse it. Both fixed; both tested below.
//!
//! Each writer gets its own fixture (the setup each needs differs too much to share one driver --
//! the same reasoning `rfc163_write_never_buries_a_crash_state.rs` gives), but every row checks the
//! same two things: the command refuses, and the whole `.prikk/` tree is byte-for-byte identical to
//! before the attempt -- no object, no index entry, no cache write, nothing.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("target file exists")
}

fn append_torn_prefix(path: &Path) {
    let mut bytes = read_bytes(path);
    let prefix_len = 45.min(bytes.len());
    let prefix = bytes[..prefix_len].to_vec();
    bytes.extend(prefix);
    std::fs::write(path, &bytes).unwrap();
}

fn append_zeros_100(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend(vec![0_u8; 100]);
    std::fs::write(path, &bytes).unwrap();
}

fn append_random_100(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend((0..100_u32).map(|index| (index.wrapping_mul(2654435761) >> 24) as u8));
    std::fs::write(path, &bytes).unwrap();
}

/// Flip one byte in the body of the pointer index's own last complete record -- a checksum-mismatch
/// shape §9/§9.2 already classify as damage (interior, not a tail), the fourth fault shape the
/// handoff names.
fn flip_last_record_body(path: &Path) {
    let mut bytes = read_bytes(path);
    let magic = bytes[..8].to_vec();
    let mut offset = 0_usize;
    let mut frames = Vec::new();
    while offset + 50 <= bytes.len() && bytes[offset..offset + 8] == magic[..] {
        let body_len =
            u64::from_be_bytes(bytes[offset + 10..offset + 18].try_into().unwrap()) as usize;
        if offset + 50 + body_len > bytes.len() {
            break;
        }
        frames.push((offset, body_len));
        offset += 50 + body_len;
    }
    let (last_offset, last_len) = *frames.last().expect("at least one record");
    let flip_at = last_offset + 50 + (last_len / 2);
    bytes[flip_at] ^= 0xFF;
    std::fs::write(path, &bytes).unwrap();
}

type FaultFn = fn(&Path);

const FAULTS: [(&str, FaultFn); 4] = [
    ("torn prefix", append_torn_prefix as FaultFn),
    ("100 zero bytes", append_zeros_100 as FaultFn),
    ("100 random bytes", append_random_100 as FaultFn),
    (
        "a flipped byte in the last complete record",
        flip_last_record_body as FaultFn,
    ),
];

/// The live slot's own suffix ('a' or 'b'), read from the generation log's own last record --
/// never guessed by file size, which breaks down the moment compaction has run once: both slots
/// then hold content of comparable size, and a size-based guess can silently pick the retired one
/// (confirmed: it did, for every one of the three `compact --all` fixtures below, before this fix --
/// the fault landed on the slot the live-slot check never even reads, and the test passed for the
/// wrong reason: nothing changed because nothing was ever faulted). Mirrors the architect's own
/// shell `live()` helper exactly: the generation log's own record is 51 bytes, and the live slot is
/// named by the last record's own body byte (0 = a, 1 = b); no record at all means the slot has
/// never been switched, i.e. still 'a' -- the state every pre-compaction fixture in this file is in.
fn live_slot_suffix(generation_log_path: &Path) -> char {
    let Ok(bytes) = std::fs::read(generation_log_path) else {
        return 'a';
    };
    if bytes.len() < 51 {
        return 'a';
    }
    let last_record_start = (bytes.len() / 51 - 1) * 51;
    if bytes[last_record_start + 50] == 1 {
        'b'
    } else {
        'a'
    }
}

fn pointer_index_path(repo: &Path) -> PathBuf {
    let suffix =
        live_slot_suffix(&repo.join(".prikk/refs/containers/pointer-index-generation.log"));
    repo.join(format!(
        ".prikk/refs/containers/pointer-index-{suffix}.container"
    ))
}

fn author_key_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/author-keys.container")
}

/// A side file kept as a SIBLING of `repo` rather than inside it -- anything written at the
/// worktree root is fair game for a command's own tree materialization to remove (confirmed for
/// `merge`'s own baseline: a file written at `repo.join(...)` right after the first seal was gone
/// by the time `merge_writer` read it back, wiped by the `branch switch` calls in between).
fn side_path(repo: &Path, suffix: &str) -> PathBuf {
    let mut path = repo.as_os_str().to_owned();
    path.push(".rfc164-ruleD-");
    path.push(suffix);
    PathBuf::from(path)
}

/// `(repo tree snapshot, exit code, combined output)` for one attempted writer invocation.
fn try_writer(repo: &Path, writer: fn(&Path) -> std::process::Output) -> (Option<i32>, String) {
    let output = writer(repo);
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// Run the whole matrix (4 faults) for one writer against one guarded file.
fn run_matrix(
    label: &str,
    target_path: fn(&Path) -> PathBuf,
    repo_builder: fn(&str) -> PathBuf,
    writer: fn(&Path) -> std::process::Output,
) {
    let mut failures = Vec::new();
    for (fault_name, fault) in FAULTS {
        let repo = repo_builder(&format!(
            "rfc164-ruleD-{}-{}",
            label.replace(' ', "-"),
            fault_name.replace(' ', "-")
        ));
        let path = target_path(&repo);
        fault(&path);
        let before = support::store_bytes(&repo);
        let cell = format!("{label} / {fault_name}");

        let (code, text) = try_writer(&repo, writer);
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{cell}: the writer must refuse, but exited 0\n{text}"
            ));
        }
        let after = support::store_bytes(&repo);
        if before != after {
            let mut changed: Vec<String> = before
                .keys()
                .chain(after.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter(|path| before.get(*path) != after.get(*path))
                .map(|path| path.display().to_string())
                .collect();
            changed.sort();
            changed.dedup();
            failures.push(format!(
                "{cell}: every file under .prikk/ must be byte-identical after the refusal; changed: {changed:?}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

// ---------------------------------------------------------------------------------------------
// seal -- one queued commit, so a seal attempt has something to seal.
// ---------------------------------------------------------------------------------------------

fn seal_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    std::fs::write(repo.join("b.txt"), "b\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "second"), "queue");
    repo
}

fn seal_writer(repo: &Path) -> std::process::Output {
    support::seal(repo, "heads/main")
}

#[test]
fn seal_refuses_on_every_pointer_index_fault_before_any_write() {
    run_matrix("seal", pointer_index_path, seal_repository, seal_writer);
}

// ---------------------------------------------------------------------------------------------
// branch create -- an existing main to branch from.
// ---------------------------------------------------------------------------------------------

fn branch_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

fn branch_create_writer(repo: &Path) -> std::process::Output {
    support::branch_create(repo, "heads/topic", "heads/main")
}

#[test]
fn branch_create_refuses_on_every_pointer_index_fault_before_any_write() {
    run_matrix(
        "branch create",
        pointer_index_path,
        branch_repository,
        branch_create_writer,
    );
}

// ---------------------------------------------------------------------------------------------
// tag create -- an existing main to target.
// ---------------------------------------------------------------------------------------------

fn tag_create_writer(repo: &Path) -> std::process::Output {
    support::tag_create(repo, "tags/v1", "heads/main")
}

#[test]
fn tag_create_refuses_on_every_pointer_index_fault_before_any_write() {
    run_matrix(
        "tag create",
        pointer_index_path,
        branch_repository,
        tag_create_writer,
    );
}

// ---------------------------------------------------------------------------------------------
// merge -- two diverged branches, proven confluent.
// ---------------------------------------------------------------------------------------------

fn merge_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    // The baseline is the common ancestor -- captured here, right after the shared first seal and
    // before either branch moves past it, and saved alongside the fixture so `merge_writer` reads it
    // back rather than re-deriving it from `log` after the fault is applied (the fault itself can
    // make the pointer index unreadable, including for `log`'s own ref resolution).
    let baseline = merge_baseline_block_id(&repo);
    std::fs::write(side_path(&repo, "baseline"), &baseline).unwrap();

    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create",
    );
    let switch = support::prikk(&repo)
        .args(["branch", "switch", "heads/topic"])
        .output()
        .unwrap();
    assert!(switch.status.success(), "branch switch heads/topic");
    std::fs::write(repo.join("b.txt"), "b\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/topic", "second"), "commit");
    support::ok(&support::seal(&repo, "heads/topic"), "seal");
    let switch_back = support::prikk(&repo)
        .args(["branch", "switch", "heads/main"])
        .output()
        .unwrap();
    assert!(switch_back.status.success(), "branch switch heads/main");
    // A second seal on `main` itself (a change independent of topic's own "b.txt", so confluence
    // against the shared baseline above still holds) -- so `heads/main`'s own pointer-index entry,
    // not `heads/topic`'s, is the file's actual last record. Without this, the fault below lands on
    // `heads/topic`'s own entry, which `into_ref == heads/main`'s own early check never reads --
    // exercising nothing about Rule D's own early check for the ref the merge is actually advancing.
    std::fs::write(repo.join("c.txt"), "c\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "third"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

fn merge_baseline_block_id(repo: &Path) -> String {
    let output = support::prikk(repo)
        .args(["log", "--ref", "heads/main", "--format", "json"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let value = support::json::parse(&text);
    value.get("blocks").as_array()[0]
        .get("block_id")
        .as_str()
        .to_string()
}

fn merge_writer(repo: &Path) -> std::process::Output {
    let baseline = std::fs::read_to_string(side_path(repo, "baseline")).unwrap();
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline,
            "--into",
            "heads/main",
            "--from",
            "heads/topic",
        ])
        .output()
        .unwrap()
}

#[test]
fn merge_refuses_on_every_pointer_index_fault_before_any_write() {
    run_matrix("merge", pointer_index_path, merge_repository, merge_writer);
}

// ---------------------------------------------------------------------------------------------
// sync seal -- an accepted claim sealed onto `heads/main` (`seal_from_accepted_claim`).
// ---------------------------------------------------------------------------------------------

fn sync_seal_repository(tag: &str) -> PathBuf {
    let sender = support::unique_repo(&format!("{tag}-sender"));
    support::init(&sender);
    support::generation(&sender, "heads/main", "a.txt", b"first\n", "first");

    let repo = support::unique_repo(tag);
    support::init(&repo);
    // `heads/main` itself gets its very first pointer-index entry only when the accepted claim is
    // sealed below -- an unrelated ref is sealed here first, so the shared pointer-index container
    // already has a real framed record for the fault functions to corrupt (the tail check reads the
    // whole container's own replay state, not a per-ref slice of it).
    support::generation(&repo, "heads/other", "seed.txt", b"seed\n", "seed");

    let have_file = side_path(&repo, "have");
    support::ok(
        &support::prikk(&repo)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = side_path(&sender, "artifact");
    support::ok(
        &support::prikk(&sender)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                artifact_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    let claims_file = side_path(&repo, "claims");
    support::ok(
        &support::prikk(&repo)
            .args([
                "sync",
                "accept",
                artifact_file.to_str().unwrap(),
                "--claims-out",
                claims_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync accept",
    );
    let claim_id = std::fs::read_to_string(&claims_file)
        .unwrap()
        .lines()
        .next()
        .expect("accept produced exactly one claim id")
        .to_string();
    std::fs::write(side_path(&repo, "claim"), claim_id).unwrap();

    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
    let _ = std::fs::remove_file(&claims_file);
    repo
}

fn sync_seal_writer(repo: &Path) -> std::process::Output {
    let claim_id = std::fs::read_to_string(side_path(repo, "claim")).unwrap();
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", claim_id.trim()])
        .output()
        .unwrap()
}

#[test]
fn sync_seal_refuses_on_every_pointer_index_fault_before_any_write() {
    run_matrix(
        "sync seal",
        pointer_index_path,
        sync_seal_repository,
        sync_seal_writer,
    );
}

// ---------------------------------------------------------------------------------------------
// commit by a new author -- the author-key container, not the pointer index.
// ---------------------------------------------------------------------------------------------

const NEW_AUTHOR_KEY_ID: &str = "rfc164-ruleD-new-author";
const NEW_AUTHOR_SEED_HEX: &str =
    "c3a1f5e7b9d28460c3a1f5e7b9d28460c3a1f5e7b9d28460c3a1f5e7b9d28460";

fn author_key_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    repo
}

fn commit_by_new_author_writer(repo: &Path) -> std::process::Output {
    std::fs::write(repo.join("new.txt"), "new\n".repeat(20)).unwrap();
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", NEW_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(NEW_AUTHOR_SEED_HEX),
        )
        .args(["commit", "--ref", "heads/main", "-m", "by a new author"])
        .output()
        .unwrap()
}

#[test]
fn a_new_authors_first_commit_refuses_on_every_author_key_fault_before_any_write() {
    run_matrix(
        "commit by a new author",
        author_key_path,
        author_key_repository,
        commit_by_new_author_writer,
    );
}

// ---------------------------------------------------------------------------------------------
// compact --all -- Addendum 1 item 1. Pointer index, received index, and trust policy all
// populated and compacted once already, so each has both a live-slot record and a generation-log
// record for the fault functions to act on.
// ---------------------------------------------------------------------------------------------

fn compact_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    // Pointer index: a real sealed commit.
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // Received index: import a bundle from an independent sender.
    let sender = support::unique_repo(&format!("{tag}-sender"));
    support::init(&sender);
    std::fs::write(sender.join("s.txt"), "s\n".repeat(20)).unwrap();
    support::ok(&support::commit(&sender, "heads/main", "s"), "commit");
    support::ok(&support::seal(&sender, "heads/main"), "seal");
    let bundle = side_path(&repo, "bundle");
    support::ok(
        &support::prikk(&sender)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle.to_str().unwrap(),
                "--force",
            ])
            .output()
            .unwrap(),
        "bundle export",
    );
    support::ok(
        &support::prikk(&repo)
            .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
            .output()
            .unwrap(),
        "bundle import",
    );
    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_file(&bundle);

    // Trust policy: adopt the fixed maintainer key.
    support::trust_maintainer(&repo);

    // Compact once: builds the first generation-log record for all three subsystems, and switches
    // each one's own live slot from the empty "a" starting state to a populated "b" -- every fault
    // below lands on a slot (and a log) with real content, not an empty file.
    support::ok(
        &support::prikk(&repo)
            .args(["compact", "--all"])
            .output()
            .unwrap(),
        "compact --all (building the fixture)",
    );
    repo
}

fn compact_all_writer(repo: &Path) -> std::process::Output {
    support::prikk(repo)
        .args(["compact", "--all"])
        .output()
        .unwrap()
}

fn pointer_index_generation_log_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/pointer-index-generation.log")
}

fn received_index_path(repo: &Path) -> PathBuf {
    let suffix =
        live_slot_suffix(&repo.join(".prikk/refs/containers/received-index-generation.log"));
    repo.join(format!(
        ".prikk/refs/containers/received-index-{suffix}.container"
    ))
}

fn received_index_generation_log_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/received-index-generation.log")
}

fn trust_policy_path(repo: &Path) -> PathBuf {
    let suffix = live_slot_suffix(&repo.join(".prikk/trust/policy-generation.log"));
    repo.join(format!(".prikk/trust/policy-{suffix}.container"))
}

fn trust_policy_generation_log_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/policy-generation.log")
}

#[test]
fn compact_all_refuses_on_every_pointer_index_live_slot_fault_before_any_write() {
    run_matrix(
        "compact --all / pointer index live slot",
        pointer_index_path,
        compact_repository,
        compact_all_writer,
    );
}

#[test]
fn compact_all_refuses_on_every_pointer_index_generation_log_fault_before_any_write() {
    run_matrix(
        "compact --all / pointer index generation log",
        pointer_index_generation_log_path,
        compact_repository,
        compact_all_writer,
    );
}

#[test]
fn compact_all_refuses_on_every_received_index_live_slot_fault_before_any_write() {
    run_matrix(
        "compact --all / received index live slot",
        received_index_path,
        compact_repository,
        compact_all_writer,
    );
}

#[test]
fn compact_all_refuses_on_every_received_index_generation_log_fault_before_any_write() {
    run_matrix(
        "compact --all / received index generation log",
        received_index_generation_log_path,
        compact_repository,
        compact_all_writer,
    );
}

#[test]
fn compact_all_refuses_on_every_trust_policy_live_slot_fault_before_any_write() {
    run_matrix(
        "compact --all / trust policy live slot",
        trust_policy_path,
        compact_repository,
        compact_all_writer,
    );
}

#[test]
fn compact_all_refuses_on_every_trust_policy_generation_log_fault_before_any_write() {
    run_matrix(
        "compact --all / trust policy generation log",
        trust_policy_generation_log_path,
        compact_repository,
        compact_all_writer,
    );
}

// ---------------------------------------------------------------------------------------------
// rollback-draft --append-inverse -- Addendum 1 item 2. A sealed commit to invert, rolled back by
// a NEW author (so the author-key container's own tail check is actually exercised, not bypassed
// by the already-recorded fast path -- same reasoning as `commit_by_new_author_writer` above).
//
// Scope note: this file tests the author-key container specifically, since that is this
// Addendum's own fix. The WAL's own tail/damage check was already correctly ordered before this
// round (unchanged here) and is exhaustively covered, across all nine guarded files, by the
// architect's own `rfc164_rule_d_sync_rollback_probe.sh` (TOTAL findings: 0) -- not duplicated
// here as a Rust test, to avoid constructing a WAL fixture that risks exercising the pre-existing
// "requires an empty active WAL" precondition instead of the tail/damage check specifically.
// ---------------------------------------------------------------------------------------------

const ROLLBACK_AUTHOR_KEY_ID: &str = "rfc164-ruleD-rollback-author";
const ROLLBACK_AUTHOR_SEED_HEX: &str =
    "d4b20698d4b20698d4b20698d4b20698d4b20698d4b20698d4b20698d4b20698";

fn rollback_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

fn rollback_draft_writer(repo: &Path) -> std::process::Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", ROLLBACK_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(ROLLBACK_AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo"])
        .output()
        .unwrap()
}

#[test]
fn rollback_draft_refuses_on_every_author_key_fault_before_any_write() {
    run_matrix(
        "rollback-draft / author key container",
        author_key_path,
        rollback_repository,
        rollback_draft_writer,
    );
}
