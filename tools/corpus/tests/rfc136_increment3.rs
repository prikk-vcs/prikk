//! RFC 136 increment 3, handoff §1: the measurements that close RFC 136 §9 item 4 and §10.4, on RFC 139's
//! corpus (`profiles/prikk-self.toml`) at the five §9.3 depths, **three samples per cell** (median and
//! range reported).
//!
//! Two `#[ignore]`d measurements, run one after the other so a build never runs beside a timing:
//!
//! - [`storage_before_the_writer`]: the same history sealed by a build from before the checkpoint
//!   writer (`c786e7f9`, passed as `PRIKK_RFC136_PRE_WRITER_BIN`), recording `.prikk` bytes and the
//!   object count `verify` reports at each depth, and whether a checkpoint snapshot exists (it must
//!   not).
//! - [`cost_memory_storage_and_the_gate`]: the history sealed by this tree's binary. At each depth,
//!   storage (bytes, objects, the manifest Blob of every checkpoint so far), then three interleaved
//!   samples, per binary, of `checkout --patch-plan`, `--patch-materialize`, `branch switch`,
//!   `merge-evidence`, and `commit` with a warm and a cold DC-64 lifecycle cache (`bundle preview` is
//!   not measurable on this corpus; see `COMMANDS`). The
//!   "before" binary is `26e8c528` (no anchoring), passed as `PRIKK_RFC136_UNANCHORED_BIN`. At the
//!   deepest depth, the derivation gate end to end.
//!
//! **Both builds create the same branches** (`heads/cp-1`, `-65`, `-129`, `-193`, at each checkpoint
//! block), so the storage comparison is like for like, and the manifest of every checkpoint can be read
//! through `prepare_snapshot_checkout_plan`.
//!
//! **Hard stops (handoff §4), asserted:** any output difference between the two binaries (stdout, with
//! the copy's own path normalized, or the worktree's bytes and exec bits); an anchored median peak memory
//! above 1.5 x the unanchored one. The storage stop (a checkpoint above 10 % of the repository at depth
//! 64) needs both runs and is computed in the report.
//!
//! **Before binaries must be debug builds**, as the corpus support builds this tree's binary in debug.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use prikk_corpus::{Profile, execute};
use prikk_store::{
    FileObjectStore, ObjectReader, RepositoryLayout, prepare_snapshot_checkout_plan,
};

mod support;

const DEPTHS: [u64; 5] = [32, 64, 128, 192, 256];
const SAMPLES: usize = 3;
/// Checkpoint blocks: a ref's first block and every 64th after it (RFC 136 §10.2).
const CHECKPOINT_BLOCKS: [u64; 4] = [1, 65, 129, 193];
/// Commits per side of the merge-evidence divergence, as `two_measurements.rs`.
const DIVERGENCE_SIZE: u64 = 5;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

/// The depths to run: all five, or those up to `PRIKK_RFC136_INC3_MAX_DEPTH` (a smoke run before the real
/// one; a capped run's tables are not the measurement).
fn depths() -> Vec<u64> {
    let cap = std::env::var("PRIKK_RFC136_INC3_MAX_DEPTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(u64::MAX);
    DEPTHS
        .iter()
        .copied()
        .filter(|depth| *depth <= cap)
        .collect()
}

fn env_binary(var: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| panic!("{var} names a debug prikk binary")))
}

fn out_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc136");
    std::fs::create_dir_all(&dir).expect("creating the measurement directory");
    dir
}

fn require(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn prikk(binary: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(dir).args(args);
    command
}

fn cp_branch(block: u64) -> String {
    format!("heads/cp-{block}")
}

/// Build one planned commit, sealed, creating the checkpoint branch when the new block is a checkpoint.
fn grow(
    binary: &Path,
    repo: &Path,
    profile: &Profile,
    index: usize,
    commit: &prikk_corpus::PlannedCommit,
    trusted: &mut bool,
) {
    execute::materialize_commit(repo, commit).expect("materializing commit");
    execute::run_commit(
        binary,
        repo,
        profile,
        execute::REF_NAME,
        &format!("corpus commit {index}"),
    )
    .expect("commit");
    if !*trusted {
        execute::trust_maintainer(binary, repo, profile).expect("trust");
        *trusted = true;
    }
    execute::run_seal(binary, repo, profile, execute::REF_NAME).expect("seal");
    let block = (index + 1) as u64;
    if CHECKPOINT_BLOCKS.contains(&block) {
        execute::branch_create(binary, repo, profile, &cp_branch(block), execute::REF_NAME)
            .expect("checkpoint branch");
    }
}

/// `(checkpoint block, manifest Blob bytes, entries, content bytes named)` for every checkpoint so far,
/// or `None` where the block carries no snapshot.
fn manifests(repo: &Path, depth: u64) -> Vec<(u64, Option<(usize, usize, u64)>)> {
    let layout = RepositoryLayout::open(repo.to_path_buf()).expect("open");
    CHECKPOINT_BLOCKS
        .iter()
        .filter(|block| **block <= depth)
        .map(|block| {
            let manifest = prepare_snapshot_checkout_plan(&layout, &cp_branch(*block))
                .ok()
                .map(|plan| {
                    let envelope = FileObjectStore::new(layout.clone())
                        .read_object(plan.snapshot_blob_id)
                        .expect("read snapshot Blob")
                        .expect("snapshot Blob present");
                    (
                        envelope.canonical_payload.len(),
                        plan.file_count,
                        plan.total_content_bytes,
                    )
                });
            (*block, manifest)
        })
        .collect()
}

struct StorageRow {
    depth: u64,
    prikk_bytes: u64,
    objects: Option<u64>,
    manifests: Vec<(u64, Option<(usize, usize, u64)>)>,
}

fn storage_row(verify_binary: &Path, repo: &Path, depth: u64) -> StorageRow {
    StorageRow {
        depth,
        prikk_bytes: support::dir_bytes(&repo.join(".prikk")),
        objects: support::verified_object_count(verify_binary, repo),
        manifests: manifests(repo, depth),
    }
}

fn storage_table(title: &str, identity: &execute::BinaryIdentity, rows: &[StorageRow]) -> String {
    let mut text = format!(
        "## {title}\n\nSealed by `{}` (`{}`), sha256 `{}`.\n\n| depth | .prikk bytes | objects | checkpoint manifests (block: Blob bytes / entries / content bytes named) |\n|---:|---:|---:|---|\n",
        identity.path, identity.version_output, identity.sha256
    );
    for row in rows {
        let manifests = row
            .manifests
            .iter()
            .map(|(block, manifest)| match manifest {
                Some((bytes, entries, content)) => {
                    format!("{block}: {bytes} / {entries} / {content}")
                }
                None => format!("{block}: none"),
            })
            .collect::<Vec<_>>()
            .join("; ");
        text.push_str(&format!(
            "| {} | {} | {} | {manifests} |\n",
            row.depth,
            row.prikk_bytes,
            row.objects
                .map_or_else(|| "?".to_string(), |count| count.to_string())
        ));
    }
    text
}

#[test]
#[ignore = "RFC 136 increment 3 §1.1: builds the corpus with the pre-writer binary; run with PRIKK_RFC136_PRE_WRITER_BIN"]
fn storage_before_the_writer() {
    let pre_writer = env_binary("PRIKK_RFC136_PRE_WRITER_BIN");
    let identity = execute::binary_identity(&pre_writer).expect("binary identity");
    let verifier = support::prikk_binary_path();
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, *depths().last().unwrap()).expect("planning");
    let repo = support::unique_dir("rfc136-inc3-pre-writer");
    execute::init_repository(&pre_writer, &repo).expect("init");
    let mut trusted = false;
    let mut rows = Vec::new();
    for (index, commit) in manifest.commits.iter().enumerate() {
        grow(&pre_writer, &repo, &profile, index, commit, &mut trusted);
        let depth = (index + 1) as u64;
        if depths().contains(&depth) {
            let row = storage_row(verifier, &repo, depth);
            eprintln!(
                "pre-writer depth {depth}: {} bytes, {:?} objects",
                row.prikk_bytes, row.objects
            );
            assert!(
                row.manifests.iter().all(|(_, manifest)| manifest.is_none()),
                "premise check: the pre-writer build wrote a snapshot at depth {depth}"
            );
            rows.push(row);
        }
    }
    let _ = std::fs::remove_dir_all(&repo);
    let text = format!(
        "# RFC 136 increment 3 §1.1 -- storage before the writer\n\nObject counts from `verify` of `{}`.\n\n{}",
        verifier.display(),
        storage_table("Pre-writer build", &identity, &rows)
    );
    std::fs::write(out_dir().join("increment3-storage-pre-writer.md"), &text).expect("writing");
    eprintln!("{text}");
}

/// One command's samples for one binary.
#[derive(Default)]
struct Cell {
    ms: Vec<Option<f64>>,
    kb: Vec<Option<u64>>,
}

impl Cell {
    fn push(&mut self, elapsed: Duration, peak: Option<u64>) {
        self.ms.push(Some(elapsed.as_secs_f64() * 1000.0));
        self.kb.push(peak);
    }
    fn ms(&self) -> String {
        support::median_range(&self.ms).map_or_else(
            || "n/a".into(),
            |(m, lo, hi)| format!("{m:.0} ({lo:.0}–{hi:.0})"),
        )
    }
    fn kb(&self) -> String {
        support::median_range(&self.kb).map_or_else(
            || "not measured".into(),
            |(m, lo, hi)| format!("{m} ({lo}–{hi})"),
        )
    }
    fn median_ms(&self) -> f64 {
        support::median_range(&self.ms).map_or(f64::NAN, |(m, _, _)| m)
    }
    fn median_kb(&self) -> Option<u64> {
        support::median_range(&self.kb).map(|(m, _, _)| m)
    }
}

/// `bundle preview` is not measured, and cannot be on this corpus today: `bundle export` of `heads/main`
/// fails at every measured depth with `missing blob object`, on this tree and on `26e8c528` and
/// `c786e7f9` alike. The export closure (`bundle.rs`) collects a deleted file's preimage Blob, and a text
/// file edited before its deletion has no stored preimage Blob (DC-65). A pre-existing product defect,
/// reported in the increment 3 report and held for review rather than fixed here. Its sample arm below is
/// kept, so the measurement is one line away once export works.
const COMMANDS: [&str; 6] = [
    "checkout --patch-plan",
    "checkout --patch-materialize",
    "branch switch",
    "merge-evidence",
    "commit (warm cache)",
    "commit (cold cache)",
];

/// What one sample of `command` produced, beside its timing: normalized stdout or a worktree digest, so
/// the two binaries' outputs can be compared.
type Observed = String;

fn normalize(output: &Output, dir: &Path) -> String {
    String::from_utf8_lossy(&output.stdout).replace(&dir.display().to_string(), "<dir>")
}

fn digest_text(dir: &Path) -> String {
    let digest = support::worktree_digest(dir);
    let mut text = String::new();
    for (path, (bytes, executable)) in digest {
        text.push_str(&format!(
            "{} {} {}\n",
            path.display(),
            prikk_hash::to_hex(&prikk_hash::sha256(&bytes)),
            executable
        ));
    }
    text
}

/// Take one sample of `command` with `binary` against the repository at `repo` (never modified).
/// `setup` is the prepared per-depth material (bundle, divergence).
fn sample(
    command: &str,
    binary: &Path,
    after: &Path,
    repo: &Path,
    profile: &Profile,
    prepared: &Prepared,
    tag: &str,
) -> (Duration, Option<u64>, Observed) {
    let dir = support::unique_dir(tag);
    support::copy_prikk_only(repo, &dir);
    let observed;
    let measured = match command {
        "checkout --patch-plan" => {
            let m = support::run_measured(prikk(
                binary,
                &dir,
                &["checkout", "--patch-plan", "--ref", execute::REF_NAME],
            ));
            require(&m.2, command);
            observed = normalize(&m.2, &dir);
            m
        }
        "checkout --patch-materialize" => {
            let m = support::run_measured(prikk(
                binary,
                &dir,
                &[
                    "checkout",
                    "--patch-materialize",
                    "--ref",
                    execute::REF_NAME,
                ],
            ));
            require(&m.2, command);
            observed = digest_text(&dir);
            m
        }
        "branch switch" => {
            // Untimed: the worktree as heads/main, then switched to heads/cp-1, both by the after binary.
            require(
                &prikk(
                    after,
                    &dir,
                    &[
                        "checkout",
                        "--patch-materialize",
                        "--ref",
                        execute::REF_NAME,
                    ],
                )
                .output()
                .unwrap(),
                "untimed materialize",
            );
            require(
                &prikk(after, &dir, &["branch", "switch", &cp_branch(1)])
                    .output()
                    .unwrap(),
                "untimed switch to heads/cp-1",
            );
            let m = support::run_measured(prikk(
                binary,
                &dir,
                &["branch", "switch", execute::REF_NAME],
            ));
            require(&m.2, command);
            observed = digest_text(&dir);
            m
        }
        "bundle preview" => {
            let m = support::run_measured(prikk(
                binary,
                &dir,
                &[
                    "bundle",
                    "preview",
                    "--input",
                    prepared.bundle.to_str().unwrap(),
                    "--ref",
                    execute::REF_NAME,
                ],
            ));
            require(&m.2, command);
            observed =
                normalize(&m.2, &dir).replace(&prepared.bundle.display().to_string(), "<bundle>");
            m
        }
        "merge-evidence" => {
            let _ = std::fs::remove_dir_all(&dir);
            let m = support::run_measured(prikk(
                binary,
                &prepared.divergence_dir,
                &[
                    "merge-evidence",
                    "--baseline-block",
                    &prepared.baseline,
                    "--left-block",
                    &prepared.left,
                    "--right-block",
                    &prepared.right,
                ],
            ));
            require(&m.2, command);
            observed = normalize(&m.2, &prepared.divergence_dir);
            return (m.0, m.1, observed);
        }
        "commit (warm cache)" | "commit (cold cache)" => {
            require(
                &prikk(
                    after,
                    &dir,
                    &[
                        "checkout",
                        "--patch-materialize",
                        "--ref",
                        execute::REF_NAME,
                    ],
                )
                .output()
                .unwrap(),
                "untimed materialize",
            );
            std::fs::create_dir_all(dir.join("bench")).unwrap();
            std::fs::write(dir.join("bench/sample.txt"), b"a sample commit\n").unwrap();
            if command == "commit (cold cache)" {
                let _ = std::fs::remove_file(dir.join(".prikk/cache/lifecycle-state.v1"));
            }
            let m = support::run_measured(
                execute::commit_command(binary, &dir, profile, execute::REF_NAME, "sample")
                    .expect("commit command"),
            );
            require(&m.2, command);
            // Patch ids carry fresh node ids, so commit output is not compared; the queued count is.
            observed = String::from_utf8_lossy(&m.2.stdout)
                .lines()
                .filter(|line| line.contains("operation"))
                .map(|line| {
                    line.split_whitespace()
                        .filter(|word| word.len() < 32)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("\n");
            m
        }
        other => panic!("unknown command {other}"),
    };
    let _ = std::fs::remove_dir_all(&dir);
    (measured.0, measured.1, observed)
}

/// Per-depth material shared by every sample: a bundle of heads/main, and a divergence for
/// merge-evidence built in its own copy (so heads/main's history, and its checkpoint cadence, is never
/// changed by the measurement).
struct Prepared {
    bundle: PathBuf,
    divergence_dir: PathBuf,
    baseline: String,
    left: String,
    right: String,
}

fn prepare(after: &Path, repo: &Path, profile: &Profile, depth: u64) -> Prepared {
    let bundle = support::unique_dir(&format!("inc3-bundle-{depth}")).with_extension("bundle");
    // No bundle export: it fails on this corpus (see `COMMANDS`). The path is kept for the sample arm.
    let dir = support::unique_dir(&format!("inc3-divergence-{depth}"));
    support::copy_prikk_only(repo, &dir);
    require(
        &prikk(
            after,
            &dir,
            &[
                "checkout",
                "--patch-materialize",
                "--ref",
                execute::REF_NAME,
            ],
        )
        .output()
        .unwrap(),
        "divergence materialize",
    );
    let baseline = support::block_ids(after, &dir, execute::REF_NAME, 1).remove(0);
    let branch = format!("heads/divergence-{depth}");
    execute::branch_create(after, &dir, profile, &branch, execute::REF_NAME)
        .expect("divergence branch");
    for index in 0..DIVERGENCE_SIZE {
        let path = dir.join(format!("divergence/left/f{index}.txt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            prikk_corpus::rng::generate_bytes(
                profile.builder_inputs.generator_seed ^ (depth << 32) ^ index,
                32,
            ),
        )
        .unwrap();
        execute::run_commit(
            after,
            &dir,
            profile,
            execute::REF_NAME,
            &format!("left {index}"),
        )
        .expect("left commit");
        execute::run_seal(after, &dir, profile, execute::REF_NAME).expect("left seal");
    }
    let left = support::block_ids(after, &dir, execute::REF_NAME, 1).remove(0);
    let mut right_paths = Vec::new();
    for index in 0..DIVERGENCE_SIZE {
        let path = dir.join(format!("divergence/right/f{index}.txt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            prikk_corpus::rng::generate_bytes(
                profile.builder_inputs.generator_seed ^ (depth << 32) ^ (1_000_000 + index),
                32,
            ),
        )
        .unwrap();
        execute::run_commit(after, &dir, profile, &branch, &format!("right {index}"))
            .expect("right commit");
        execute::run_seal(after, &dir, profile, &branch).expect("right seal");
        right_paths.push(path);
    }
    let right = support::block_ids(after, &dir, &branch, 1).remove(0);
    for path in right_paths {
        std::fs::remove_file(path).unwrap();
    }
    Prepared {
        bundle,
        divergence_dir: dir,
        baseline,
        left,
        right,
    }
}

/// Growth exponent between depths 64 and 256: `ln(t256 / t64) / ln(4)`.
fn exponent(at64: f64, at256: f64) -> String {
    if at64.is_finite() && at256.is_finite() && at64 > 0.0 {
        format!("{:.2}", (at256 / at64).ln() / 4_f64.ln())
    } else {
        "n/a".into()
    }
}

#[test]
#[ignore = "RFC 136 increment 3 §1.2-§1.5: builds the corpus and times both binaries; run with PRIKK_RFC136_UNANCHORED_BIN"]
fn cost_memory_storage_and_the_gate() {
    let before = env_binary("PRIKK_RFC136_UNANCHORED_BIN");
    let after = support::prikk_binary_path().to_path_buf();
    let before_identity = execute::binary_identity(&before).expect("before identity");
    let after_identity = execute::binary_identity(&after).expect("after identity");
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, *depths().last().unwrap()).expect("planning");
    let repo = support::unique_dir("rfc136-inc3");
    execute::init_repository(&after, &repo).expect("init");

    let mut trusted = false;
    let mut storage = Vec::new();
    // results[depth index][command index] = (before cell, after cell, tree files)
    let mut results: Vec<(u64, u64, Vec<(Cell, Cell)>)> = Vec::new();
    let mut gate_rows: Vec<(&str, Cell)> = Vec::new();

    for (index, commit) in manifest.commits.iter().enumerate() {
        grow(&after, &repo, &profile, index, commit, &mut trusted);
        let depth = (index + 1) as u64;
        if !depths().contains(&depth) {
            continue;
        }
        let row = storage_row(&after, &repo, depth);
        eprintln!(
            "depth {depth}: {} bytes, {:?} objects",
            row.prikk_bytes, row.objects
        );
        storage.push(row);

        let prepared = prepare(&after, &repo, &profile, depth);
        let mut cells: Vec<(Cell, Cell)> = COMMANDS
            .iter()
            .map(|_| (Cell::default(), Cell::default()))
            .collect();
        let mut tree_files = 0;
        for (command_index, command) in COMMANDS.iter().enumerate() {
            let mut observed_before = None;
            let mut observed_after = None;
            for round in 0..SAMPLES {
                // Interleaved, before then after, so drift touches both.
                let tag = format!("inc3-{depth}-{command_index}-{round}");
                let (elapsed, peak, observed) = sample(
                    command,
                    &before,
                    &after,
                    &repo,
                    &profile,
                    &prepared,
                    &format!("{tag}-b"),
                );
                cells[command_index].0.push(elapsed, peak);
                observed_before.get_or_insert(observed);
                let (elapsed, peak, observed) = sample(
                    command,
                    &after,
                    &after,
                    &repo,
                    &profile,
                    &prepared,
                    &format!("{tag}-a"),
                );
                cells[command_index].1.push(elapsed, peak);
                if *command == "checkout --patch-materialize" {
                    tree_files = observed.lines().count() as u64;
                }
                observed_after.get_or_insert(observed);
            }
            assert_eq!(
                observed_before, observed_after,
                "HARD STOP: `{command}` output differs between the unanchored and anchored binaries at depth {depth}"
            );
            eprintln!(
                "  {command}: before {} ms / {} KB, after {} ms / {} KB",
                cells[command_index].0.ms(),
                cells[command_index].0.kb(),
                cells[command_index].1.ms(),
                cells[command_index].1.kb()
            );
        }
        let _ = std::fs::remove_file(&prepared.bundle);
        let _ = std::fs::remove_dir_all(&prepared.divergence_dir);
        results.push((depth, tree_files, cells));

        if depth == *depths().last().unwrap() {
            gate_rows = derivation_gate(&after, &repo, &profile, depth);
        }
    }
    let _ = std::fs::remove_dir_all(&repo);

    // ---- the report ----
    let mut text = String::new();
    text.push_str("# RFC 136 increment 3 §1.2–§1.5 -- cost, what is still unanchored, peak memory, the derivation gate\n\n");
    text.push_str(&format!(
        "Generated by `PRIKK_RFC136_UNANCHORED_BIN=<path> cargo test -p prikk-corpus --locked --test rfc136_increment3 -- --ignored --nocapture cost_memory_storage_and_the_gate`. \
         Profile `profiles/prikk-self.toml`; one repository sealed by the after binary; {SAMPLES} interleaved samples per cell, median (min–max). \
         Every command's output (stdout with the copy's path normalized, or the written worktree's bytes and exec bits) was identical between the two binaries at every depth.\n\n\
         - before (no anchoring): `{}` (`{}`), sha256 `{}`\n- after: `{}` (`{}`), sha256 `{}`\n\n",
        before_identity.path, before_identity.version_output, before_identity.sha256,
        after_identity.path, after_identity.version_output, after_identity.sha256
    ));
    text.push_str(&storage_table(
        "§1.1 Storage, this tree's writer",
        &after_identity,
        &storage,
    ));
    text.push_str("\n## §1.2 and §1.4 Wall time (ms) and peak memory (KB)\n\n| command | depth | tree files | before ms | after ms | after/before | before KB | after KB | after/before |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for (command_index, command) in COMMANDS.iter().enumerate() {
        for (depth, files, cells) in &results {
            let (b, a) = &cells[command_index];
            let kb_ratio = match (b.median_kb(), a.median_kb()) {
                (Some(bk), Some(ak)) if bk > 0 => format!("{:.2}", ak as f64 / bk as f64),
                _ => "n/a".into(),
            };
            text.push_str(&format!(
                "| `{command}` | {depth} | {files} | {} | {} | {:.2} | {} | {} | {kb_ratio} |\n",
                b.ms(),
                a.ms(),
                a.median_ms() / b.median_ms(),
                b.kb(),
                a.kb()
            ));
        }
    }
    text.push_str("\n## §1.3 Growth exponent against depth, 64 → 256 (median wall time)\n\n| command | before | after |\n|---|---:|---:|\n");
    for (command_index, command) in COMMANDS.iter().enumerate() {
        let at = |depth: u64, pick: fn(&(Cell, Cell)) -> f64| {
            results
                .iter()
                .find(|(d, _, _)| *d == depth)
                .map_or(f64::NAN, |(_, _, cells)| pick(&cells[command_index]))
        };
        text.push_str(&format!(
            "| `{command}` | {} | {} |\n",
            exponent(at(64, |c| c.0.median_ms()), at(256, |c| c.0.median_ms())),
            exponent(at(64, |c| c.1.median_ms()), at(256, |c| c.1.median_ms())),
        ));
    }
    text.push_str("\n## §1.5 The derivation gate at the deepest depth (after binary)\n\nThe snapshot of the deepest checkpoint branch (`heads/cp-193` at depth 256; block 256 is not a checkpoint), with the replay-verified record deleted first.\n\n| step | ms, median (min–max) | peak KB |\n|---|---:|---:|\n");
    for (step, cell) in &gate_rows {
        text.push_str(&format!("| {step} | {} | {} |\n", cell.ms(), cell.kb()));
    }
    std::fs::write(out_dir().join("increment3-cost-memory-gate.md"), &text).expect("writing");
    eprintln!("{text}");

    // Hard stop, asserted after the table is written so the numbers are kept.
    for (command_index, command) in COMMANDS.iter().enumerate() {
        for (depth, _, cells) in &results {
            if let (Some(bk), Some(ak)) = (
                cells[command_index].0.median_kb(),
                cells[command_index].1.median_kb(),
            ) {
                assert!(
                    (ak as f64) <= 1.5 * bk as f64,
                    "HARD STOP: `{command}` anchored peak memory {ak} KB > 1.5 x unanchored {bk} KB at depth {depth}"
                );
            }
        }
    }
}

/// §1.5: delete the record, materialize the snapshot of `heads/cp-193`, `commit` refuses, `verify`,
/// `commit` proceeds. Three runs, each in a fresh copy.
fn derivation_gate(
    after: &Path,
    repo: &Path,
    profile: &Profile,
    depth: u64,
) -> Vec<(&'static str, Cell)> {
    let steps = [
        "checkout --snapshot-materialize",
        "commit (refused by the gate)",
        "verify (clears the marker)",
        "commit (proceeds)",
    ];
    let mut cells: Vec<Cell> = steps.iter().map(|_| Cell::default()).collect();
    // The deepest checkpoint at this depth: 193 at depth 256 (block 256 itself is not a checkpoint).
    let block = CHECKPOINT_BLOCKS
        .iter()
        .copied()
        .filter(|block| *block <= depth)
        .max()
        .expect("block 1");
    let branch = cp_branch(block);
    for round in 0..SAMPLES {
        let dir = support::unique_dir(&format!("inc3-gate-{round}"));
        support::copy_prikk_only(repo, &dir);
        let _ = std::fs::remove_file(dir.join(".prikk/cache/replay-verified-blocks.v1"));

        let m = support::run_measured(prikk(
            after,
            &dir,
            &["checkout", "--snapshot-materialize", "--ref", &branch],
        ));
        require(&m.2, "snapshot-materialize");
        assert!(
            String::from_utf8_lossy(&m.2.stdout).contains("provisional: yes"),
            "the record was deleted, so the worktree is provisional"
        );
        cells[0].push(m.0, m.1);

        std::fs::write(dir.join("gate-sample.txt"), b"after a snapshot\n").unwrap();
        let m = support::run_measured(
            execute::commit_command(after, &dir, profile, &branch, "refused").unwrap(),
        );
        assert!(
            !m.2.status.success(),
            "commit must refuse while the worktree is provisional"
        );
        assert!(
            String::from_utf8_lossy(&m.2.stderr).contains("not replay-verified"),
            "{}",
            String::from_utf8_lossy(&m.2.stderr)
        );
        cells[1].push(m.0, m.1);

        let m = support::run_measured(prikk(after, &dir, &["verify"]));
        assert!(
            String::from_utf8_lossy(&m.2.stdout).contains("marker cleared"),
            "{}",
            String::from_utf8_lossy(&m.2.stdout)
        );
        cells[2].push(m.0, m.1);

        let m = support::run_measured(
            execute::commit_command(after, &dir, profile, &branch, "proceeds").unwrap(),
        );
        require(&m.2, "commit after verify");
        cells[3].push(m.0, m.1);
        let _ = std::fs::remove_dir_all(&dir);
    }
    steps.into_iter().zip(cells).collect()
}
