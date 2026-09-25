//! RFC 159 design round, handoff §2: **identity comes first.** Anchoring a `seal` at the nearest replay-verified
//! snapshot must change nothing a signature covers or a store holds. `#[ignore]`d: deliberately invoked
//! instruments, release build (`support::measurement_binary`), reports under `.git-exclude/measurements/rfc159/`.
//!
//! Instruments, each a different claim.
//!
//! 1. [`identity_probe_at_every_block`] -- **the derivation, at every block.** Independent histories are built
//!    commit-then-seal to `PRIKK_159_DEPTH` (default 1,024) and then every block of each is sealed *again*, in
//!    process, on its own parent, by the anchored derivation. Each result is compared with an **independent forward
//!    replay from genesis** (one pass, every root checked against the block's signed root): the whole lifecycle
//!    state (tombstones and `seen_ids` included), the state entries, the state root against the block's own signed
//!    `state_merkle_root`, and the text of every file whose content blob no one stored (what a checkpoint writer
//!    reads). Every `PRIKK_159_STRIDE`-th block is also compared with the literal full walk `seal` runs today.
//!    `PRIKK_159_PROBE_REPO` (colon-separated) probes repositories that already exist.
//! 2. [`identity_by_cli`] -- **the commands, end to end.** A repository is copied; `seal`, `merge` and
//!    seal-from-accepted (`sync accept` then `sync seal`) run on each copy under each **arm**: the binary as built,
//!    and the **baseline binary** `PRIKK_159_BASELINE_BINARY` names (a build from before RFC 159, whose `seal` walks
//!    the whole history -- there is no environment switch, RFC 159 handoff §1.7). The claim is that every file under
//!    `.prikk/` except the rebuildable `cache/` and the lock files -- so every block, signature, ref and snapshot --
//!    is **byte-identical** across arms.
//! 3. [`identity_of_every_snapshot`] -- **every snapshot the corpus writes.** The same history is built once by each
//!    binary and the whole durable store is compared: the snapshot writer's presence check (asked of the index,
//!    RFC 159 §8.6) must not change a byte of any of the checkpoints.
//!
//! **What byte-identity cannot see**: a *valid* restoration succeeds with or without the tombstone, so command output
//! is identical even if the anchored derivation dropped the history fields. The in-process control for that is
//! `block_state::anchored_parent::tests`' negative restoration, and the probe above.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_corpus::{Profile, execute};
use prikk_store::RepositoryLayout;

mod support;

const FLOOR_DEPTH: u64 = 2048;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn out_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc159");
    std::fs::create_dir_all(&dir).expect("creating the measurement directory");
    dir
}

fn require_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Grow one repository to `depth`, commit then seal per block, calling `at_block(depth, repo, queued)` after each
/// commit (before its seal) so a caller can copy the repository with that commit queued.
fn build_history(
    binary: &Path,
    profile: &Profile,
    manifest: &prikk_corpus::ActionManifest,
    depth: usize,
    label: &str,
    mut at_block: impl FnMut(usize, &Path),
) -> PathBuf {
    let repo_root = support::unique_dir(label);
    execute::init_repository(binary, &repo_root).expect("init");
    let mut trusted = false;
    for (index, commit) in manifest.commits.iter().take(depth).enumerate() {
        execute::materialize_commit(&repo_root, commit).expect("materializing commit");
        execute::run_commit(
            binary,
            &repo_root,
            profile,
            execute::REF_NAME,
            &format!("corpus commit {index}"),
        )
        .expect("commit");
        if !trusted {
            execute::trust_maintainer(binary, &repo_root, profile).expect("trust");
            trusted = true;
        }
        at_block(index + 1, &repo_root);
        execute::run_seal(binary, &repo_root, profile, execute::REF_NAME).expect("seal");
    }
    repo_root
}

fn tip_of(binary: &Path, repo: &Path, ref_name: &str) -> String {
    support::block_ids(binary, repo, ref_name, 1)
        .into_iter()
        .next()
        .expect("a sealed tip")
}

#[test]
#[ignore = "RFC 159 identity probe at every block; expensive, run deliberately"]
fn identity_probe_at_every_block() {
    let profile = self_profile();
    let depth = env_usize("PRIKK_159_DEPTH", 1024);
    let builds = env_usize("PRIKK_159_BUILDS", 3);
    let stride = env_usize("PRIKK_159_STRIDE", 61);
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    eprintln!(
        "build: {build}; binary {} ({}), sha256 {}",
        identity.path, identity.version_output, identity.sha256
    );
    let mut manifest = prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning");
    manifest.commits.truncate(depth);

    let mut repos: Vec<(String, PathBuf, bool)> = std::env::var("PRIKK_159_PROBE_REPO")
        .ok()
        .map(|list| {
            list.split(':')
                .filter(|part| !part.is_empty())
                .map(|part| (format!("existing:{part}"), PathBuf::from(part), false))
                .collect()
        })
        .unwrap_or_default();
    if repos.is_empty() {
        for sample in 0..builds {
            let start = std::time::Instant::now();
            let repo = build_history(
                binary,
                &profile,
                &manifest,
                depth,
                &format!("id159-{sample}"),
                |_, _| {},
            );
            eprintln!(
                "built sample {sample} to depth {depth} in {:.0} s",
                start.elapsed().as_secs_f64()
            );
            repos.push((format!("built-{sample}"), repo, true));
        }
    }

    let mut report = format!(
        "# RFC 159 -- identity of the anchored derivation, every block ({build})\n\n\
         **{build} build**: `{}` (`{}`), sha256 `{}`. Depth {depth}; every block of each history is sealed again on \
         its own parent and compared (module documentation). Literal full-walk comparison every {stride}th block.\n\n\
         | history | blocks | anchored | fell back | most blocks folded | id-only blocks read (sum) | compared with the literal full walk | anchored derivation, all blocks (s) | literal full walk, compared blocks (s) | anchored, same blocks (s) | differences |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
        identity.path, identity.version_output, identity.sha256
    );
    let mut all_identical = true;
    for (label, repo, built_here) in &repos {
        let layout = RepositoryLayout::open(repo.clone()).expect("opening the repository");
        let tip = tip_of(binary, repo, execute::REF_NAME);
        let start = std::time::Instant::now();
        let probe =
            prikk_store::rfc159_identity_probe(&layout, &tip, stride).expect("identity probe");
        eprintln!(
            "[{label}] {} blocks: {} anchored, {} fell back, max folded {}, {} literal comparisons, {} differences ({:.0} s)",
            probe.blocks,
            probe.anchored,
            probe.fell_back,
            probe.max_folded,
            probe.literal_full_compared,
            probe.differences.len(),
            start.elapsed().as_secs_f64()
        );
        for difference in &probe.differences {
            eprintln!("  DIFFERENCE: {difference}");
        }
        all_identical &= probe.differences.is_empty();
        report.push_str(&format!(
            "| {label} | {} | {} | {} | {} | {} | {} | {:.1} | {:.1} | {:.2} | {} |\n",
            probe.blocks,
            probe.anchored,
            probe.fell_back,
            probe.max_folded,
            probe.id_only_blocks_total,
            probe.literal_full_compared,
            probe.anchored_seconds,
            probe.literal_full_seconds,
            probe.anchored_seconds_at_compared,
            if probe.differences.is_empty() {
                "**none**".to_string()
            } else {
                probe.differences.join("; ")
            }
        ));
        if *built_here && std::env::var("PRIKK_159_KEEP").is_err() {
            let _ = std::fs::remove_dir_all(repo);
        }
    }
    std::fs::write(
        out_dir().join(format!("identity-probe-{build}-{depth}.md")),
        &report,
    )
    .expect("writing the report");
    eprintln!("{report}");
    assert!(
        all_identical,
        "the anchored derivation differs from full replay"
    );
}

// ---- The commands, end to end -----------------------------------------------------------------------------

/// Every file under `.prikk/` that is not the rebuildable cache or a lock, by relative path: `(length, sha256)`.
fn store_digest(repo: &Path) -> BTreeMap<String, (u64, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, String)>) {
        for entry in std::fs::read_dir(dir).expect("reading directory") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if entry.file_type().unwrap().is_dir() {
                if relative == "cache" {
                    continue;
                }
                walk(root, &path, out);
            } else if !relative.contains("lock") {
                let bytes = std::fs::read(&path).expect("reading file");
                out.insert(
                    relative,
                    (
                        bytes.len() as u64,
                        prikk_hash::to_hex(&prikk_hash::sha256(&bytes)),
                    ),
                );
            }
        }
    }
    let root = repo.join(".prikk");
    let mut out = BTreeMap::new();
    walk(&root, &root, &mut out);
    out
}

/// One arm: a binary and the environment it runs under.
#[derive(Clone)]
struct Arm {
    name: &'static str,
    binary: PathBuf,
    envs: Vec<(&'static str, &'static str)>,
}

fn arms(binary: &Path) -> Vec<Arm> {
    let baseline = std::env::var("PRIKK_159_BASELINE_BINARY").expect(
        "PRIKK_159_BASELINE_BINARY must name a pre-RFC-159 build: there is no environment switch to turn anchoring off",
    );
    vec![
        Arm {
            name: "this build (anchored)",
            binary: binary.to_path_buf(),
            envs: Vec::new(),
        },
        Arm {
            name: "baseline build (full walk)",
            binary: PathBuf::from(baseline),
            envs: Vec::new(),
        },
    ]
}

fn run_arm(arm: &Arm, profile: &Profile, repo: &Path, args: &[&str]) -> Output {
    let mut command = execute::keyed_command(&arm.binary, repo, profile).expect("keyed command");
    command.args(args);
    for (key, value) in &arm.envs {
        command.env(key, value);
    }
    command.output().expect("running prikk")
}

/// Run `args` on a fresh copy of `repo` under every arm; assert the durable store is byte-identical across arms.
/// Returns the arm's stderr (for the trace line) of the first arm.
fn identical_across_arms(
    label: &str,
    arms: &[Arm],
    profile: &Profile,
    repo: &Path,
    args: &[&str],
    checks: &mut Vec<String>,
) -> String {
    let mut digests: Vec<(&'static str, BTreeMap<String, (u64, String)>)> = Vec::new();
    let mut first_stderr = String::new();
    for arm in arms {
        let copy = support::unique_dir("id159-arm");
        support::copy_dir_all(repo, &copy);
        let output = run_arm(arm, profile, &copy, args);
        require_success(&output, &format!("{label} under `{}`", arm.name));
        if digests.is_empty() {
            first_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        }
        digests.push((arm.name, store_digest(&copy)));
        let _ = std::fs::remove_dir_all(&copy);
    }
    let (first_name, first) = &digests[0];
    for (name, digest) in &digests[1..] {
        assert_eq!(
            first, digest,
            "{label}: the store under `{first_name}` differs from the store under `{name}`"
        );
    }
    checks.push(format!(
        "- **{label}**: {} durable files, byte-identical across {} arms ({})",
        first.len(),
        arms.len(),
        arms.iter()
            .map(|arm| arm.name)
            .collect::<Vec<_>>()
            .join("; ")
    ));
    first_stderr
}

#[test]
#[ignore = "RFC 159 identity by CLI (seal, merge, seal-from-accepted); run deliberately"]
fn identity_by_cli() {
    let profile = self_profile();
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let depth = env_usize("PRIKK_159_CLI_DEPTH", 200);
    let arms = arms(binary);
    let mut manifest = prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning");
    manifest.commits.truncate(depth);
    let mut checks: Vec<String> = Vec::new();

    // `seal`: an ordinary block and a checkpoint block, at the boundaries of the 64-block cadence.
    let depths: [usize; 14] = [1, 2, 3, 63, 64, 65, 66, 127, 128, 129, 130, 192, 193, 200];
    let mut sealed_at = Vec::new();
    let repo = build_history(
        binary,
        &profile,
        &manifest,
        depth,
        "id159-cli",
        |block, repo| {
            if depths.contains(&block) {
                identical_across_arms(
                    &format!("seal of block {block}"),
                    &arms,
                    &profile,
                    repo,
                    &["seal", "--allow-no-audit", "--ref", execute::REF_NAME],
                    &mut checks,
                );
                sealed_at.push(block);
            }
        },
    );
    checks.push(format!("- `seal` compared at blocks {sealed_at:?}"));

    // `merge`: a divergent pair. The right side is built first, on its own branch, then removed from the worktree,
    // so the left side (on `heads/main`) does not author it.
    let baseline = support::block_ids(binary, &repo, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("a tip");
    execute::branch_create(binary, &repo, &profile, "heads/topic", execute::REF_NAME)
        .expect("branch");
    let side_files = |side: &str, count: usize| -> Vec<String> {
        (0..count)
            .map(|i| format!("divergence/{side}/f{i}.txt"))
            .collect()
    };
    for (index, path) in side_files("right", 3).iter().enumerate() {
        let full = repo.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, format!("right {index}\n")).unwrap();
        execute::run_commit(
            binary,
            &repo,
            &profile,
            "heads/topic",
            &format!("right {index}"),
        )
        .expect("right commit");
        execute::run_seal(binary, &repo, &profile, "heads/topic").expect("right seal");
    }
    std::fs::remove_dir_all(repo.join("divergence/right")).unwrap();
    for (index, path) in side_files("left", 3).iter().enumerate() {
        let full = repo.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, format!("left {index}\n")).unwrap();
        execute::run_commit(
            binary,
            &repo,
            &profile,
            execute::REF_NAME,
            &format!("left {index}"),
        )
        .expect("left commit");
        execute::run_seal(binary, &repo, &profile, execute::REF_NAME).expect("left seal");
    }
    identical_across_arms(
        "merge of a divergent pair",
        &arms,
        &profile,
        &repo,
        &[
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline,
            "--into",
            execute::REF_NAME,
            "--from",
            "heads/topic",
        ],
        &mut checks,
    );

    // seal-from-accepted: a receiver at `depth` (a copy of the repository before the sender's extra blocks), a
    // sender with `catchup` more blocks, `sync have` / `sync build` / `sync accept`, then `sync seal` under each arm.
    let catchup = env_usize("PRIKK_159_CATCHUP", 8);
    let receiver = support::unique_dir("id159-recv");
    support::copy_dir_all(&repo, &receiver);
    for index in 0..catchup {
        let path = repo.join(format!("catchup/f{index}.txt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("catch-up {index}\n")).unwrap();
        // The sender's heads/main is the merged one; catch-up blocks extend it.
        execute::run_commit(
            binary,
            &repo,
            &profile,
            execute::REF_NAME,
            &format!("catch-up {index}"),
        )
        .expect("sender commit");
        execute::run_seal(binary, &repo, &profile, execute::REF_NAME).expect("sender seal");
    }
    let have = support::unique_dir("id159-have");
    std::fs::create_dir_all(&have).unwrap();
    let have_file = have.join("have.bin");
    let output = execute::keyed_command(binary, &receiver, &profile)
        .unwrap()
        .args(["sync", "have", execute::REF_NAME, "--output"])
        .arg(&have_file)
        .output()
        .unwrap();
    require_success(&output, "sync have");
    let artifact = have.join("exchange.bin");
    let output = execute::keyed_command(binary, &repo, &profile)
        .unwrap()
        .args(["sync", "build", execute::REF_NAME, "--have"])
        .arg(&have_file)
        .arg("--output")
        .arg(&artifact)
        .output()
        .unwrap();
    require_success(&output, "sync build");
    let claims = have.join("claims.txt");
    let output = execute::keyed_command(binary, &receiver, &profile)
        .unwrap()
        .args(["sync", "accept"])
        .arg(&artifact)
        .arg("--claims-out")
        .arg(&claims)
        .output()
        .unwrap();
    require_success(&output, "sync accept");
    let claims_arg = claims.to_string_lossy().to_string();
    identical_across_arms(
        "seal-from-accepted (sync seal --claims)",
        &arms,
        &profile,
        &receiver,
        &["sync", "seal", execute::REF_NAME, "--claims", &claims_arg],
        &mut checks,
    );

    let report = format!(
        "# RFC 159 -- identity of the commands, end to end ({build})\n\n**{build} build**: `{}` (`{}`), sha256 `{}`. \
         History depth {depth}. Each check copies the repository once per arm, runs the command under the arm, and \
         compares every file under `.prikk/` except `cache/` and locks.\n\n{}\n",
        identity.path,
        identity.version_output,
        identity.sha256,
        checks.join("\n")
    );
    std::fs::write(out_dir().join(format!("identity-cli-{build}.md")), &report)
        .expect("writing the report");
    eprintln!("{report}");
    for path in [&repo, &receiver, &have] {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// **Every snapshot the corpus writes, byte for byte** (handoff §1.6): the same commit-then-seal history built by each
/// binary, the whole durable store compared. The history crosses the checkpoints at blocks 1, 65 and 129; the blocks
/// after the first are sealed by the anchored derivation in one arm and by the full walk in the other, and the
/// snapshot writer asks the index in one and reads every blob in the other.
#[test]
#[ignore = "RFC 159 identity of every corpus snapshot; run deliberately"]
fn identity_of_every_snapshot() {
    let profile = self_profile();
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let depth = env_usize("PRIKK_159_SNAPSHOT_DEPTH", 200);
    let arms = arms(binary);
    let mut manifest = prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning");
    manifest.commits.truncate(depth);
    let mut built = Vec::new();
    for arm in &arms {
        let repo = build_history(
            &arm.binary,
            &profile,
            &manifest,
            depth,
            "id159-snap",
            |_, _| {},
        );
        built.push((arm.name, store_digest(&repo), repo));
    }
    let checkpoints = 1 + (depth.saturating_sub(1)) / 64;
    let (first_name, first, _) = &built[0];
    for (name, digest, _) in &built[1..] {
        assert_eq!(
            first, digest,
            "the store built by `{first_name}` differs from the store built by `{name}`"
        );
    }
    let report = format!(
        "# RFC 159 -- every snapshot the corpus writes ({build})\n\n**{build} build**: `{}` (`{}`), sha256 `{}`. \
         Depth {depth} ({checkpoints} checkpoints), the same history built by each of {} binaries; {} durable files, \
         **byte-identical**.\n",
        identity.path,
        identity.version_output,
        identity.sha256,
        arms.len(),
        first.len()
    );
    std::fs::write(
        out_dir().join(format!("identity-snapshots-{build}.md")),
        &report,
    )
    .expect("writing the report");
    eprintln!("{report}");
    for (_, _, repo) in &built {
        let _ = std::fs::remove_dir_all(repo);
    }
}

// ---- Shapes the corpus does not have ---------------------------------------------------------------------

/// The corpus turns a rename into a delete and a create, has no symlinks, no `mv`, and no deletion the history later
/// undoes. This instrument builds the histories that do, across the 64-block checkpoint boundaries, and checks the
/// anchored derivation on each: **rename runs** (`prikk mv`), **mode changes**, **binary replacement**, a **symlink**
/// (if `commit` accepts one), **deletions whose own node is restored by the next block** (`rollback-draft
/// --append-inverse`, the one operation that reintroduces a deleted node id, and so the one that consults the
/// tombstone) with the deletion **at the block before a checkpoint, at a checkpoint, and after one**, and a **merge
/// block below and above the anchor**. Every seal the instrument makes is compared across the three arms of
/// [`identity_by_cli`] at the points that matter, and the finished history is probed at every block.
#[test]
#[ignore = "RFC 159 identity on shapes the corpus lacks (deletions restored); run deliberately"]
fn identity_on_shapes_with_restorations() {
    shapes_history(false);
}

/// `rollback-draft --append-inverse` refuses a history that holds a `RenamePath` ("inverse planning for RenamePath
/// awaits a rename authoring path"), so the restorations and the renames live in two histories.
#[test]
#[ignore = "RFC 159 identity on shapes the corpus lacks (renames); run deliberately"]
fn identity_on_shapes_with_renames() {
    shapes_history(true);
}

fn shapes_history(renames: bool) {
    let profile = self_profile();
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let arms = arms(binary);
    let mut checks: Vec<String> = Vec::new();
    let repo = support::unique_dir("id159-shapes");
    execute::init_repository(binary, &repo).expect("init");
    let write = |path: &str, bytes: &[u8]| {
        let full = repo.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    };
    let blocks = std::cell::Cell::new(0_usize);
    let trusted = std::cell::Cell::new(false);
    let seal_args = ["seal", "--allow-no-audit", "--ref", execute::REF_NAME];
    // Commit, optionally compare the seal across arms, then seal.
    let step = |message: &str, compare: Option<&str>, checks: &mut Vec<String>| {
        execute::run_commit(binary, &repo, &profile, execute::REF_NAME, message).expect("commit");
        if !trusted.get() {
            execute::trust_maintainer(binary, &repo, &profile).expect("trust");
            trusted.set(true);
        }
        if let Some(label) = compare {
            identical_across_arms(
                &format!("{label} (block {})", blocks.get() + 1),
                &arms,
                &profile,
                &repo,
                &seal_args,
                checks,
            );
        }
        execute::run_seal(binary, &repo, &profile, execute::REF_NAME).expect("seal");
        blocks.set(blocks.get() + 1);
    };

    // Block 1 (a checkpoint): text, binary, an executable, files to delete later, and a symlink if it is accepted.
    let long_text: Vec<u8> = (1..=40)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    write("a.txt", &long_text);
    write("b.txt", b"b\n");
    write("x.txt", b"x\n");
    write("y.txt", b"y\n");
    write("z.txt", b"z\n");
    write("bin.dat", &[0xff, 0x00, 0x01, 0x02, 0x03]);
    write("run.sh", b"#!/bin/sh\n");
    #[cfg(unix)]
    let symlink_made = std::os::unix::fs::symlink("a.txt", repo.join("link")).is_ok();
    #[cfg(not(unix))]
    let symlink_made = false;
    let first = execute::run_commit(binary, &repo, &profile, execute::REF_NAME, "shapes");
    if first.is_err() && symlink_made {
        // `commit` refuses a symlink: say so, drop it, and go on without one.
        std::fs::remove_file(repo.join("link")).unwrap();
        checks.push(
            "- symlink: `commit` refuses one in this build; the history has none".to_string(),
        );
    } else if symlink_made {
        checks.push(
            "- symlink: created in block 1 and carried through every later block".to_string(),
        );
    }
    if first.is_err() {
        execute::run_commit(binary, &repo, &profile, execute::REF_NAME, "shapes").expect("commit");
    }
    execute::trust_maintainer(binary, &repo, &profile).expect("trust");
    trusted.set(true);
    execute::run_seal(binary, &repo, &profile, execute::REF_NAME).expect("seal");
    blocks.set(blocks.get() + 1);

    // Blocks 2-6: text edits, a mode change, a binary replacement, a rename, a deletion.
    for i in 0..3 {
        let mut text = long_text.clone();
        text.extend_from_slice(format!("edit {i}\n").as_bytes());
        write("a.txt", &text);
        step(&format!("edit a {i}"), None, &mut checks);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(repo.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    step("chmod", None, &mut checks);
    write("bin.dat", &[0xfe, 0x00, 0x09, 0x08]);
    step("replace binary", None, &mut checks);
    if renames {
        std::fs::create_dir_all(repo.join("moved")).unwrap();
        let output = execute::keyed_command(binary, &repo, &profile)
            .unwrap()
            .args(["mv", "b.txt", "moved/b.txt"])
            .output()
            .unwrap();
        require_success(&output, "mv");
        step("rename", None, &mut checks);
    }
    std::fs::remove_file(repo.join("x.txt")).unwrap();
    step("delete x", None, &mut checks);

    // Filler to block 63; the deletions sit at exactly blocks 64 and 129.
    let filler = |index: usize| {
        write(
            &format!("filler/f{index}.txt"),
            format!("filler {index}\n").as_bytes(),
        );
        if index % 5 == 0 {
            let mut text = long_text.clone();
            text.extend_from_slice(format!("filler edit {index}\n").as_bytes());
            write("a.txt", &text);
        }
    };
    while blocks.get() < 63 {
        filler(blocks.get());
        step(&format!("filler {}", blocks.get()), None, &mut checks);
    }
    // Block 64 (the block before a checkpoint) deletes y; block 65 (a checkpoint) restores it: the inverse of the
    // deletion is a CreateFile of the same node id, checked against the tombstone.
    let restore = |checks: &mut Vec<String>, label: &str, blocks_now: usize| {
        let output = execute::keyed_command(binary, &repo, &profile)
            .unwrap()
            .args([
                "rollback-draft",
                "--append-inverse",
                "--ref",
                execute::REF_NAME,
                "-m",
                "restore",
            ])
            .output()
            .unwrap();
        require_success(&output, "rollback-draft --append-inverse");
        identical_across_arms(
            &format!("{label} (block {})", blocks_now + 1),
            &arms,
            &profile,
            &repo,
            &seal_args,
            checks,
        );
        execute::run_seal(binary, &repo, &profile, execute::REF_NAME).expect("seal");
    };
    std::fs::remove_file(repo.join("y.txt")).unwrap();
    step(
        "delete y at the block before a checkpoint",
        Some("seal of a deletion"),
        &mut checks,
    );
    assert_eq!(blocks.get(), 64);
    if !renames {
        restore(
            &mut checks,
            "seal of the restoration that is itself a checkpoint",
            blocks.get(),
        );
        blocks.set(blocks.get() + 1);
    }
    // Filler to block 128; block 129 (a checkpoint) deletes z, block 130 restores it: the anchor is the deletion.
    while blocks.get() < 128 {
        filler(blocks.get());
        step(&format!("filler {}", blocks.get()), None, &mut checks);
    }
    std::fs::remove_file(repo.join("z.txt")).unwrap();
    step(
        "delete z at a checkpoint",
        Some("seal of a deletion that is a checkpoint"),
        &mut checks,
    );
    assert_eq!(blocks.get(), 129);
    if !renames {
        restore(
            &mut checks,
            "seal of a restoration whose anchor is the deletion",
            blocks.get(),
        );
        blocks.set(blocks.get() + 1);
    }
    // A rename after the anchor, then filler.
    if renames {
        std::fs::create_dir_all(repo.join("renamed")).unwrap();
        let output = execute::keyed_command(binary, &repo, &profile)
            .unwrap()
            .args(["mv", "filler/f10.txt", "renamed/f10.txt"])
            .output()
            .unwrap();
        require_success(&output, "mv");
        step(
            "rename after the anchor",
            Some("seal of a rename above the anchor"),
            &mut checks,
        );
    }
    while blocks.get() < 145 {
        filler(blocks.get());
        step(&format!("filler {}", blocks.get()), None, &mut checks);
    }

    // A merge block: right side on its own branch first, removed from the worktree, then the left side on main.
    let baseline = support::block_ids(binary, &repo, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .unwrap();
    execute::branch_create(binary, &repo, &profile, "heads/topic", execute::REF_NAME)
        .expect("branch");
    for i in 0..2 {
        write(
            &format!("divergence/right/f{i}.txt"),
            format!("right {i}\n").as_bytes(),
        );
        execute::run_commit(
            binary,
            &repo,
            &profile,
            "heads/topic",
            &format!("right {i}"),
        )
        .expect("right");
        execute::run_seal(binary, &repo, &profile, "heads/topic").expect("right seal");
    }
    std::fs::remove_dir_all(repo.join("divergence/right")).unwrap();
    for i in 0..2 {
        write(
            &format!("divergence/left/f{i}.txt"),
            format!("left {i}\n").as_bytes(),
        );
        step(&format!("left {i}"), None, &mut checks);
    }
    let merge_args = [
        "merge",
        "--allow-no-audit",
        "--baseline-block",
        &baseline,
        "--into",
        execute::REF_NAME,
        "--from",
        "heads/topic",
    ];
    identical_across_arms(
        "merge of a divergent pair",
        &arms,
        &profile,
        &repo,
        &merge_args,
        &mut checks,
    );
    let output = run_arm(&arms[0], &profile, &repo, &merge_args);
    require_success(&output, "merge");
    blocks.set(blocks.get() + 1);
    // Seals that fold across the merge block (anchor 129 below it), then one whose anchor (193) is above it.
    while blocks.get() < 160 {
        filler(blocks.get());
        step(&format!("filler {}", blocks.get()), None, &mut checks);
    }
    filler(blocks.get());
    step(
        "filler",
        Some("seal folding across a merge block"),
        &mut checks,
    );
    while blocks.get() < 199 {
        filler(blocks.get());
        step(&format!("filler {}", blocks.get()), None, &mut checks);
    }
    filler(blocks.get());
    step(
        "filler",
        Some("seal anchored above a merge block"),
        &mut checks,
    );

    // The finished history, every block, in process.
    let layout = RepositoryLayout::open(repo.clone()).expect("layout");
    let tip = tip_of(binary, &repo, execute::REF_NAME);
    let probe = prikk_store::rfc159_identity_probe(&layout, &tip, 7).expect("probe");
    checks.push(format!(
        "- **every block of the finished history, in process**: {} blocks, {} anchored, {} fell back, most folded {}, {} compared with the literal full walk, differences: {}",
        probe.blocks,
        probe.anchored,
        probe.fell_back,
        probe.max_folded,
        probe.literal_full_compared,
        if probe.differences.is_empty() { "**none**".to_string() } else { probe.differences.join("; ") }
    ));
    let report = format!(
        "# RFC 159 -- identity on shapes the corpus lacks ({build})\n\n**{build} build**: `{}` (`{}`), sha256 `{}`. {} blocks ({}): a mode change, a binary replacement, deletions at the block before a checkpoint and at a checkpoint, a merge block below and above the anchor.\n\n{}\n",
        identity.path,
        identity.version_output,
        identity.sha256,
        probe.blocks,
        if renames {
            "rename runs by `prikk mv`, before and after the anchor"
        } else {
            "each deletion restored by the next block: the restoration is at a checkpoint, and with the deletion itself as the anchor"
        },
        checks.join("\n")
    );
    std::fs::write(
        out_dir().join(format!(
            "identity-shapes-{}-{build}.md",
            if renames { "renames" } else { "restorations" }
        )),
        &report,
    )
    .expect("writing the report");
    eprintln!("{report}");
    assert!(probe.differences.is_empty(), "{:?}", probe.differences);
    let _ = std::fs::remove_dir_all(&repo);
}
