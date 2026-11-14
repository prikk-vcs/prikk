//! 0.58.0 S2a: the corpus that judges the fold (`sync seal` folds a batch once; S2b), written on today's code, before the fold exists.
//!
//! **What it holds.** Seven shapes, each a committed sender-side fixture (the sync artifacts, and a bundle for one shape) under
//! `tests/fixtures/s2a_sync_seal_corpus/` and an `expected.txt` recorded by running `sync accept` and `sync seal` at `cb1857d6` (the
//! commit S2a builds on). The test replays every shape against a fresh receiver that signs with the fixed maintainer seed and compares,
//! byte for byte: the `sync seal` exit and output (block ids, RefState ids, the `FAILED` line where there is one), the `sha256` of the
//! receiver's ref log and pointer index, `log --format json` (the repository path replaced), and `verify`'s exit.
//!
//! **Why the senders' output is the fixture, not a regenerated history.** A `commit` mints random NodeIds, so two runs of the same
//! script make different patch ids. A receiver signing with a fixed seed makes the same block and RefState ids from the same
//! artifact, so the artifact is what is fixed.
//!
//! **Regenerating it needs the architect's ruling.** `regenerate_corpus` (ignored) rebuilds the fixtures; it exists so the corpus is
//! reproducible in principle, not so a failing build can regenerate its way to green. A change to sealing output fails the test, and
//! the fixture is never regenerated to make a build pass.
//!
//! **The shapes:** text edits; large binaries (4 blocks of 64 KiB); a merge in the history; renames; several refs (the same claims
//! sealed onto two branches); a received checkpoint (a receiver pre-filled by `bundle import --adopt` of 66 blocks, then 2 more by sync);
//! a mid-batch refusal (a divergent claim at position 2); multi-patch claims (three blocks of three queued commits each, so a claim
//! names several patches and their order matters).
//!
//! **The refusal checklist `sync seal` keeps (S1 §3; S2b must keep every row, with the same message at the same claim; it may fire
//! earlier, never later).** Per claim, `seal_from_accepted.rs` (line numbers at `cb1857d6`), and where each is held:
//! - `:167` format not current: **no test, and none is possible**: the layout validates its format when it opens, so a layout that
//!   reaches this check always passes it;
//! - `:168` ref name: `seal_from_accepted/tests.rs::s2a_an_invalid_ref_name_refuses_before_anything` (new in S2a);
//! - `:169` / `:247` incomplete publication: `s2a_a_ref_pointing_at_a_missing_ref_state_refuses_as_an_incomplete_publication` (new). It
//!   also shows `:689-714` (a ref naming a missing RefState or Block) are defensive: this check refuses the same states first;
//! - `:179` derivation gate: `rfc136_derivation_gate.rs` (the sweep includes `sync seal`, `:159`, `:265`);
//! - `:186` claim missing: `s2a_a_claim_that_does_not_exist_refuses` (new);
//! - `:194` patch missing: store rows `row6_a_claim_naming_a_missing_patch_refuses`;
//! - `:233` ambiguous order: `row8_ambiguous_overlapping_claims_refuse`;
//! - `:238` untrusted signer: `row1_sealing_with_an_unadopted_signer_refuses`; `:241` claim signature: unreachable separately (`:238`
//!   loads the same policy first);
//! - `:287` witness, `:291` torn tail, `:298` damaged record: `s2a_an_active_wal_with_a_torn_tail_refuses` and
//!   `s2a_an_active_wal_with_a_damaged_record_refuses` (new). **The witness classification (`:287`) speaks first for a damaged
//!   record, so `:298`'s own sentence is behind it**; ownership refusals: `step3w3_r4_queue_ownership.rs`;
//! - `:308` non-empty WAL: `row10_a_non_empty_active_wal_refuses`; `:322` pointer-index tail: `rfc164_rule_d_refuse_before_first_write.rs`;
//! - `:346` parent not reached: store rows `divergent_grouping_*` and `the_refusal_names_a_held_claim_for_the_parent_*`;
//! - `block_state.rs:457-480` divergence and integrity: store rows 4 and 5 (`row4_*`, `row5_*`), and the corpus's mid-batch shape;
//! - the publication's own errors (CAS, tail, the P2b moved-directory refusal): `refs` publication tests and `p2b_*`;
//! - `:388` `update_seq` overflow: untested on purpose (AUD-08: reaching it needs routing around `:169`).
//!
//! **The five crash rows (S1 §4)** are the real-kill tests at the end of this file.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support;

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/s2a_sync_seal_corpus"
);

fn fixture(name: &str) -> PathBuf {
    Path::new(CORPUS).join(name)
}

fn maintainer(command: &mut Command) -> &mut Command {
    command
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
}

fn hex_sha256(bytes: &[u8]) -> String {
    prikk_hash::to_hex(&prikk_hash::sha256(bytes))
}

fn scratch(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("s2a-{tag}-{}", support::unique_suffix()))
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The replay (the asserted side)
// ---------------------------------------------------------------------------------------------------------------------------------

/// What one `sync seal` run is recorded as.
fn record_run(receiver: &Path, label: &str, seal: &std::process::Output) -> String {
    let path = receiver.display().to_string();
    let normalize = |text: &str| text.replace(&path, "<repo>");
    let log = support::prikk(receiver)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    let verify = support::prikk(receiver).arg("verify").output().unwrap();
    let refs = receiver.join(".prikk/refs/containers");
    let sha = |name: &str| {
        std::fs::read(refs.join(name))
            .map_or_else(|_| "absent".to_string(), |bytes| hex_sha256(&bytes))
    };
    format!(
        "== {label}\nexit {}\nstdout:\n{}\nstderr:\n{}\nlog.sha256 {}\npointer-index.sha256 {}\nlog-json.sha256 {}\nverify.exit {}\n",
        seal.status
            .code()
            .map_or_else(|| "signal".to_string(), |code| code.to_string()),
        normalize(&String::from_utf8_lossy(&seal.stdout)),
        normalize(&String::from_utf8_lossy(&seal.stderr)),
        sha("log-a.container"),
        sha("pointer-index-a.container"),
        hex_sha256(normalize(&String::from_utf8_lossy(&log.stdout)).as_bytes()),
        verify
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |code| code.to_string()),
    )
}

/// A receiver that trusts the fixed maintainer key.
fn fresh_receiver(tag: &str) -> PathBuf {
    let receiver = support::unique_repo(tag);
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    receiver
}

/// `sync accept` of each artifact in order; the claim ids it printed, one per line, as `--claims-out` writes them.
fn accept_all(receiver: &Path, artifacts: &[PathBuf]) -> String {
    let mut claims = String::new();
    for (index, artifact) in artifacts.iter().enumerate() {
        let out = scratch(&format!("claims-{index}"));
        support::ok(
            &support::prikk(receiver)
                .args(["sync", "accept"])
                .arg(artifact)
                .arg("--claims-out")
                .arg(&out)
                .output()
                .unwrap(),
            "sync accept",
        );
        claims.push_str(&std::fs::read_to_string(&out).unwrap());
        let _ = std::fs::remove_file(out);
    }
    claims
}

fn seal(receiver: &Path, reference: &str, claims: &str) -> std::process::Output {
    let claims_file = scratch("claims");
    std::fs::write(&claims_file, claims).unwrap();
    let output = maintainer(&mut support::prikk(receiver))
        .args(["sync", "seal", reference, "--claims"])
        .arg(&claims_file)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(claims_file);
    output
}

/// One shape: its fixtures replayed against a fresh receiver.
fn replay(shape: &str) -> String {
    let receiver = fresh_receiver(&format!("s2a-{shape}"));
    let artifacts = |names: &[&str]| -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| fixture(&format!("{shape}.{name}")))
            .collect()
    };
    match shape {
        "text-edits" | "large-binaries" | "merge" | "renames" | "multi-patch" => {
            let claims = accept_all(&receiver, &artifacts(&["artifact"]));
            record_run(&receiver, shape, &seal(&receiver, "heads/main", &claims))
        }
        "several-refs" => {
            let claims = accept_all(&receiver, &artifacts(&["artifact"]));
            let first = record_run(
                &receiver,
                "several-refs: heads/main",
                &seal(&receiver, "heads/main", &claims),
            );
            let second = record_run(
                &receiver,
                "several-refs: heads/other",
                &seal(&receiver, "heads/other", &claims),
            );
            format!("{first}{second}")
        }
        "received-checkpoint" => {
            support::ok(
                &maintainer(&mut support::prikk(&receiver))
                    .args(["bundle", "import", "--adopt", "--input"])
                    .arg(fixture("received-checkpoint.bundle"))
                    .output()
                    .unwrap(),
                "bundle import --adopt",
            );
            let claims = accept_all(&receiver, &artifacts(&["artifact"]));
            record_run(&receiver, shape, &seal(&receiver, "heads/main", &claims))
        }
        "mid-batch-refusal" => {
            let claims = accept_all(&receiver, &artifacts(&["artifact-1", "artifact-2"]));
            record_run(&receiver, shape, &seal(&receiver, "heads/main", &claims))
        }
        other => panic!("unknown shape {other}"),
    }
}

const SHAPES: &[&str] = &[
    "text-edits",
    "large-binaries",
    "merge",
    "renames",
    "several-refs",
    "received-checkpoint",
    "mid-batch-refusal",
    "multi-patch",
];

/// The corpus: every shape replays to the recorded bytes. **Do not regenerate the fixture to make a build pass.**
#[test]
fn every_shape_seals_to_the_recorded_corpus() {
    let mut failures = Vec::new();
    for shape in SHAPES {
        let expected = std::fs::read_to_string(fixture(&format!("{shape}.expected")))
            .unwrap_or_else(|error| panic!("{shape}: no recorded corpus: {error}"));
        let actual = replay(shape);
        if actual != expected {
            failures.push(format!(
                "--- {shape}: expected\n{expected}\n--- actual\n{actual}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The generator (ignored; needs the architect's ruling to run)
// ---------------------------------------------------------------------------------------------------------------------------------

fn sender() -> PathBuf {
    let repo = support::unique_repo("s2a-sender");
    support::init(&repo);
    repo
}

/// Build an artifact of `reference`'s history for `receiver` (what it lacks).
fn build_artifact(sender: &Path, receiver: &Path, reference: &str, to: &Path) {
    let have = scratch("have");
    support::ok(
        &support::prikk(receiver)
            .args(["sync", "have", reference, "--output"])
            .arg(&have)
            .output()
            .unwrap(),
        "sync have",
    );
    support::ok(
        &maintainer(&mut support::prikk(sender))
            .args(["sync", "build", reference, "--have"])
            .arg(&have)
            .arg("--output")
            .arg(to)
            .output()
            .unwrap(),
        "sync build",
    );
    let _ = std::fs::remove_file(have);
}

fn empty_receiver() -> PathBuf {
    let receiver = support::unique_repo("s2a-empty");
    support::init(&receiver);
    receiver
}

/// 64 KiB of reproducible bytes for block `seed`.
fn binary(seed: u8) -> Vec<u8> {
    let mut state = u64::from(seed)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(1);
    (0..65_536)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn write_all(repo: &Path, files: &[(&str, &[u8])]) {
    for (name, content) in files {
        std::fs::write(repo.join(name), content).unwrap();
    }
}

fn ensure_fixture_dir() {
    std::fs::create_dir_all(CORPUS).unwrap();
}

/// Regenerate every fixture: `cargo test -p prikk --test sync_and_exchange -- --ignored regenerate_corpus`. **Needs the architect's ruling.**
#[test]
#[ignore = "regenerating the S2a corpus needs the architect's ruling"]
fn regenerate_corpus() {
    ensure_fixture_dir();

    // 1. text edits: 10 pages, then 6 one-line edits, a block each.
    let text = sender();
    let mut pages: Vec<String> = (0..10).map(|i| format!("page {i}\n")).collect();
    support::generation(&text, "heads/main", "p0.md", pages[0].as_bytes(), "base-0");
    for (i, page) in pages.iter().enumerate().skip(1) {
        std::fs::write(text.join(format!("p{i}.md")), page).unwrap();
    }
    support::ok(&support::commit(&text, "heads/main", "base-rest"), "commit");
    support::ok(&support::seal(&text, "heads/main"), "seal");
    for k in 1..=6 {
        let index = k % 10;
        pages[index].push_str(&format!("line {k}\n"));
        support::generation(
            &text,
            "heads/main",
            &format!("p{index}.md"),
            pages[index].as_bytes(),
            &format!("e{k}"),
        );
    }
    build_artifact(
        &text,
        &empty_receiver(),
        "heads/main",
        &fixture("text-edits.artifact"),
    );

    // 2. large binaries: 4 blocks of 64 KiB, then a text edit.
    let bins = sender();
    write_all(&bins, &[("page.md", b"page\n")]);
    for k in 0..4u8 {
        support::generation(
            &bins,
            "heads/main",
            &format!("att{k}.bin"),
            &binary(k),
            &format!("a{k}"),
        );
    }
    support::generation(&bins, "heads/main", "page.md", b"page\nedit\n", "edit");
    build_artifact(
        &bins,
        &empty_receiver(),
        "heads/main",
        &fixture("large-binaries.artifact"),
    );

    // 3. a merge in the history.
    let merge = sender();
    support::generation(&merge, "heads/main", "a.txt", b"base\n", "c1");
    let base = first_block(&merge);
    support::ok(
        &maintainer(&mut support::prikk(&merge))
            .args(["branch", "create", "heads/topic", "--from", &base])
            .output()
            .unwrap(),
        "branch create",
    );
    support::generation(&merge, "heads/main", "m.txt", b"main\n", "c2");
    support::ok(
        &support::prikk(&merge)
            .args(["branch", "switch", "heads/topic"])
            .output()
            .unwrap(),
        "switch topic",
    );
    support::generation(&merge, "heads/topic", "t.txt", b"topic\n", "c3");
    support::ok(
        &support::prikk(&merge)
            .args(["branch", "switch", "heads/main"])
            .output()
            .unwrap(),
        "switch main",
    );
    support::ok(
        &maintainer(&mut support::prikk(&merge))
            .args([
                "merge",
                "--allow-no-audit",
                "--baseline-block",
                &base,
                "--into",
                "heads/main",
                "--from",
                "heads/topic",
            ])
            .output()
            .unwrap(),
        "merge",
    );
    build_artifact(
        &merge,
        &empty_receiver(),
        "heads/main",
        &fixture("merge.artifact"),
    );

    // 4. renames: a file, a declared rename, an edit of the renamed file.
    let rename = sender();
    support::generation(&rename, "heads/main", "a.txt", b"one\n", "r1");
    support::ok(
        &support::prikk(&rename)
            .args(["mv", "a.txt", "b.txt"])
            .output()
            .unwrap(),
        "mv",
    );
    support::ok(&support::commit(&rename, "heads/main", "r2"), "commit");
    support::ok(&support::seal(&rename, "heads/main"), "seal");
    support::generation(&rename, "heads/main", "b.txt", b"one\ntwo\n", "r3");
    build_artifact(
        &rename,
        &empty_receiver(),
        "heads/main",
        &fixture("renames.artifact"),
    );

    // 5. several refs: three small blocks, sealed onto two branches by the test.
    let refs = sender();
    for k in 0..3 {
        support::generation(
            &refs,
            "heads/main",
            "f.txt",
            format!("v{k}\n").as_bytes(),
            &format!("s{k}"),
        );
    }
    build_artifact(
        &refs,
        &empty_receiver(),
        "heads/main",
        &fixture("several-refs.artifact"),
    );

    // 6. a received checkpoint: 66 blocks adopted by bundle, then 2 more by sync.
    let checkpoint = sender();
    let mut body = String::new();
    for k in 0..66 {
        body.push_str(&format!("l{k}\n"));
        support::generation(
            &checkpoint,
            "heads/main",
            "f.txt",
            body.as_bytes(),
            &format!("k{k}"),
        );
    }
    support::ok(
        &support::prikk(&checkpoint)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(fixture("received-checkpoint.bundle"))
            .arg("--force")
            .output()
            .unwrap(),
        "bundle export",
    );
    for k in 66..68 {
        body.push_str(&format!("l{k}\n"));
        support::generation(
            &checkpoint,
            "heads/main",
            "f.txt",
            body.as_bytes(),
            &format!("k{k}"),
        );
    }
    let adopted = fresh_receiver("s2a-adopt");
    support::ok(
        &maintainer(&mut support::prikk(&adopted))
            .args(["bundle", "import", "--adopt", "--input"])
            .arg(fixture("received-checkpoint.bundle"))
            .output()
            .unwrap(),
        "bundle import --adopt",
    );
    build_artifact(
        &checkpoint,
        &adopted,
        "heads/main",
        &fixture("received-checkpoint.artifact"),
    );

    // 7. a mid-batch refusal: two senders that each create the same file differently; the second claim cannot apply.
    for (index, content) in [(1, "from a1\n"), (2, "from a2\n")] {
        let one = sender();
        support::generation(
            &one,
            "heads/main",
            "collide.txt",
            content.as_bytes(),
            &format!("a{index}"),
        );
        build_artifact(
            &one,
            &empty_receiver(),
            "heads/main",
            &fixture(&format!("mid-batch-refusal.artifact-{index}")),
        );
    }

    // 8. multi-patch claims: three blocks, each sealing three queued commits, so a claim names three patches in order.
    let multi = sender();
    for round in 0..3 {
        for file in 0..3 {
            std::fs::write(
                multi.join(format!("m{round}{file}.txt")),
                format!("round {round} file {file}\n"),
            )
            .unwrap();
            support::ok(
                &support::commit(&multi, "heads/main", &format!("m{round}{file}")),
                "commit",
            );
        }
        support::ok(&support::seal(&multi, "heads/main"), "seal");
    }
    build_artifact(
        &multi,
        &empty_receiver(),
        "heads/main",
        &fixture("multi-patch.artifact"),
    );

    // The recorded corpus: what `sync seal` produced on these fixtures at the commit this ran on.
    for shape in SHAPES {
        std::fs::write(fixture(&format!("{shape}.expected")), replay(shape)).unwrap();
    }
}

fn first_block(repo: &Path) -> String {
    let log = support::prikk(repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    let document = support::json::parse(&String::from_utf8_lossy(&log.stdout));
    document
        .get("blocks")
        .as_array()
        .last()
        .expect("a block")
        .get("block_id")
        .as_str()
        .to_string()
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The five crash rows (S1 §4), as real-kill tests. Linux and macOS only: a process abort leaves a lock file behind there, and a
// Windows abort has a different exit status; the platform difference is what produces the rows, so the gate is derived.
// ---------------------------------------------------------------------------------------------------------------------------------

/// **Row 5's one expected value.** Today a re-run that finds every claim already sealed clears the interrupted-materialization marker
/// without materializing (the worktree stays behind the ref; `worktree-status` names `branch switch`). S2b changes it: the re-run
/// finishes the materialization, and this constant becomes `true`.
#[cfg(unix)]
const ROW5_RERUN_MATERIALIZES: bool = false;

/// A 3-claim batch (`several-refs.artifact`: three blocks of one file) onto the current branch of a fresh receiver, killed at `point`
/// of the `k`-th claim sealed. Returns the receiver and the claims.
#[cfg(unix)]
fn killed_batch(tag: &str, point: &str, k: usize) -> (PathBuf, String) {
    use std::os::unix::process::ExitStatusExt;
    let receiver = fresh_receiver(tag);
    let claims = accept_all(&receiver, &[fixture("several-refs.artifact")]);
    let claims_file = scratch("kill-claims");
    std::fs::write(&claims_file, &claims).unwrap();
    let output = maintainer(&mut support::prikk(&receiver))
        .env("PRIKK_TEST_ABORT_AT", format!("{point}:{k}"))
        .args(["sync", "seal", "heads/main", "--claims"])
        .arg(&claims_file)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(claims_file);
    assert_eq!(
        output.status.signal(),
        Some(6),
        "the seal must abort at {point}:{k}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (receiver, claims)
}

/// Published blocks on the receiver's `heads/main`.
#[cfg(unix)]
fn published_blocks(receiver: &Path) -> usize {
    let log = support::prikk(receiver)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::json::parse(&String::from_utf8_lossy(&log.stdout))
        .get("blocks")
        .as_array()
        .len()
}

/// The no-crash end state of the same batch: the record every re-run must end in.
#[cfg(unix)]
fn clean_end_state() -> String {
    let receiver = fresh_receiver("s2a-kill-clean");
    let claims = accept_all(&receiver, &[fixture("several-refs.artifact")]);
    let output = seal(&receiver, "heads/main", &claims);
    support::ok(&output, "clean sync seal");
    end_state(&receiver)
}

/// The end state compared across runs: the ref log, the pointer index and `log --format json`, repository path removed.
#[cfg(unix)]
fn end_state(receiver: &Path) -> String {
    let path = receiver.display().to_string();
    let refs = receiver.join(".prikk/refs/containers");
    let sha = |name: &str| hex_sha256(&std::fs::read(refs.join(name)).unwrap());
    let log = support::prikk(receiver)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    format!(
        "log {} pointer-index {} log-json {}",
        sha("log-a.container"),
        sha("pointer-index-a.container"),
        hex_sha256(
            String::from_utf8_lossy(&log.stdout)
                .replace(&path, "<repo>")
                .as_bytes()
        )
    )
}

/// One row: the state after the kill (what is on disk; the exits of `status`, `verify`, `doctor`), then the next `sync seal` (refusing
/// on the lock the dead process left), `unlock --yes`, and the `sync seal` after that. `published` is how many blocks the ref holds
/// right after the kill.
#[cfg(unix)]
fn run_row(tag: &str, point: &str, k: usize, published: usize) -> (PathBuf, std::process::Output) {
    let (receiver, claims) = killed_batch(tag, point, k);

    // On disk and in the readers' words.
    let lock = receiver.join(".prikk/active/default/active.lock");
    assert!(
        lock.exists(),
        "the abort ran no destructor: the active lock is left"
    );
    assert_eq!(
        published_blocks(&receiver),
        published,
        "{tag}: blocks published at the kill"
    );
    let status = support::prikk(&receiver).arg("status").output().unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("interrupted materialization"),
        "{tag}: status names the interrupted materialization: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    assert_eq!(
        support::prikk(&receiver)
            .arg("verify")
            .output()
            .unwrap()
            .status
            .code(),
        Some(0),
        "{tag}: verify"
    );
    let doctor = support::prikk(&receiver).arg("doctor").output().unwrap();
    assert_eq!(doctor.status.code(), Some(0), "{tag}: doctor exits 0");
    let doctor_text = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        doctor_text.contains("PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION"),
        "{tag}: {doctor_text}"
    );
    assert!(
        doctor_text.contains("PRIKK-DOCTOR-HELD-LOCK"),
        "{tag}: {doctor_text}"
    );

    // The next sync seal meets the dead process's lock first.
    let first = seal(&receiver, "heads/main", &claims);
    assert_eq!(first.status.code(), Some(1), "{tag}: lock conflict");
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("lock conflict"),
        "{tag}: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    // `unlock --yes` clears it because the holder reads "does not appear to be running" (0.58.0 step 0a's rule).
    support::ok(
        &support::prikk(&receiver)
            .args(["unlock", "--lock"])
            .arg(&lock)
            .arg("--yes")
            .output()
            .unwrap(),
        "unlock --yes",
    );
    let second = seal(&receiver, "heads/main", &claims);
    (receiver, second)
}

#[cfg(unix)]
fn assert_ends_in_the_clean_state(tag: &str, receiver: &Path, clean: &str) {
    assert_eq!(
        end_state(receiver),
        clean,
        "{tag}: the re-run ends in the no-crash corpus's state"
    );
}

/// Rows 1–4: the re-run seals what is left, materializes, and ends in the clean state.
#[cfg(unix)]
fn assert_rerun_completes(tag: &str, receiver: &Path, second: &std::process::Output, clean: &str) {
    support::ok(second, "the re-run");
    assert_ends_in_the_clean_state(tag, receiver, clean);
    let status = support::prikk(receiver).arg("status").output().unwrap();
    assert!(
        !String::from_utf8_lossy(&status.stdout).contains("interrupted"),
        "{tag}: the marker is cleared"
    );
    assert_eq!(
        std::fs::read_to_string(receiver.join("f.txt")).unwrap(),
        "v2\n",
        "{tag}: the worktree is materialized at the last block"
    );
}

#[cfg(unix)]
#[test]
fn crash_row1_before_the_first_claims_first_write() {
    let clean = clean_end_state();
    let (receiver, second) = run_row("s2a-kill-1", "before-block", 1, 0);
    assert_rerun_completes("row 1", &receiver, &second, &clean);
}

#[cfg(unix)]
#[test]
fn crash_row2_between_claims() {
    let clean = clean_end_state();
    let (receiver, second) = run_row("s2a-kill-2", "before-block", 2, 1);
    assert_rerun_completes("row 2", &receiver, &second, &clean);
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("already fully sealed -- no-op"),
        "claim 1 is a no-op on the re-run"
    );
}

#[cfg(unix)]
#[test]
fn crash_row3_a_block_written_and_not_published() {
    let clean = clean_end_state();
    let (receiver, second) = run_row("s2a-kill-3", "after-block", 2, 1);
    assert_rerun_completes("row 3", &receiver, &second, &clean);
}

/// A point the table does not list: the first claim's Block written and not published. It is row 3's state with nothing published; the
/// re-run ends in the same clean state.
#[cfg(unix)]
#[test]
fn crash_a_block_written_for_the_first_claim_is_row_3_with_nothing_published() {
    let clean = clean_end_state();
    let (receiver, second) = run_row("s2a-kill-3b", "after-block", 1, 0);
    assert_rerun_completes("row 3 (first claim)", &receiver, &second, &clean);
}

#[cfg(unix)]
#[test]
fn crash_row4_a_claim_published_the_next_not_started() {
    let clean = clean_end_state();
    let (receiver, second) = run_row("s2a-kill-4", "after-publish", 2, 2);
    assert_rerun_completes("row 4", &receiver, &second, &clean);
}

/// Row 5: every claim published, the worktree not materialized. **Today:** the re-run is a no-op for every claim, clears the marker, and
/// leaves the worktree behind the ref (`worktree-status` names `branch switch`). `ROW5_RERUN_MATERIALIZES` is S2b's one expected value.
#[cfg(unix)]
#[test]
fn crash_row5_every_claim_published_the_worktree_not_materialized() {
    let clean = clean_end_state();
    let (receiver, second) = run_row("s2a-kill-5", "after-publish", 3, 3);
    support::ok(&second, "the re-run");
    assert_ends_in_the_clean_state("row 5", &receiver, &clean);
    assert!(
        !String::from_utf8_lossy(&second.stdout).contains("sealed 1 patch"),
        "every claim is already sealed"
    );
    let status = support::prikk(&receiver).arg("status").output().unwrap();
    assert!(
        !String::from_utf8_lossy(&status.stdout).contains("interrupted"),
        "the marker is cleared (today, without materializing)"
    );
    let worktree_status = support::prikk(&receiver)
        .arg("worktree-status")
        .output()
        .unwrap();
    let behind = String::from_utf8_lossy(&worktree_status.stdout).contains("behind heads/main");
    assert_eq!(
        receiver.join("f.txt").exists(),
        ROW5_RERUN_MATERIALIZES,
        "row 5: the worktree after the re-run"
    );
    assert_eq!(
        behind, !ROW5_RERUN_MATERIALIZES,
        "row 5: worktree-status says behind"
    );
}
