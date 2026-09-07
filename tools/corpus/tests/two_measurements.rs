//! RFC 139 increment 3's own deliverable (handoff §1, §3): the two measurements the corpus was
//! held for -- checkout cost at realistic history depth (RFC 136 §9 item 1) and baseline
//! reconstruction cost on the merge path (item 2) -- against `profiles/prikk-self.toml`, out to the
//! practical depth increment 2's own ruling accepted: **256**, not the RFC 139 §6 floor of 2,048
//! (increment 2 measured the floor at 23-101 extrapolated hours and it was ruled unreachable in
//! practice; see `rfcs/accepted/139-measurement-corpus.md`).
//!
//! **256 is 4 `REANCHOR_BOUND` intervals, not 32 -- this file's own numbers are not a cadence
//! curve.** Four points show direction, not shape (handoff §2.1). Do not read the table below as
//! evidence for tuning `REANCHOR_BOUND`.
//!
//! **One growing repository** (increment 2's own harness shape, dc92's technique): `heads/main`
//! grows continuously through the whole run. At each checkpoint depth:
//!
//! 1. **Item 1 (checkout).** `.prikk` is copied (never the worktree -- a real clone would not have
//!    one) into two fresh directories, and `checkout --patch-plan` / `--patch-materialize` are each
//!    timed there. RFC 136 §9 item 1 asks for the cost *today*; RFC 136 §5 already established every
//!    block-creating path writes `snapshot_blob_ref: None`, so the snapshot-backed checkout path is
//!    unreachable on any repository this corpus can build -- not attempted here (verified via
//!    `prepare_snapshot_checkout_plan`'s own `Integrity` refusal on a `None` snapshot ref, not
//!    assumed).
//! 2. **Item 2 (merge baseline reconstruction).** A branch is cut from `heads/main`'s current tip
//!    (the depth-profiled baseline), then **both** `heads/main` and the new branch grow by
//!    [`DIVERGENCE_SIZE`] commits each, one new branch-unique file per commit -- disjoint paths, not
//!    overlapping edits (§2.3's own invented-shape disclosure, below). `merge-evidence` is then timed
//!    against the frozen baseline/left/right block ids. `heads/main`'s own [`DIVERGENCE_SIZE`]
//!    commits are real, permanent growth (this file's own `actual_depth` tracks that, since the
//!    checkpoint-numbering loop index alone would understate true depth after the first divergence);
//!    the branch's are discarded from the worktree afterward so `heads/main`'s own continued growth
//!    stays worktree-consistent (see `crates/prikk-cli/tests/dc74_merge_execution.rs`'s own
//!    add-then-remove-file precedent for why this is necessary: one physical worktree, no
//!    branch-switch concept, `prikk commit --ref X` diffs the worktree against `X`'s own last sealed
//!    baseline only).
//!
//! ## §2.3's own disclosure: what is profiled, what is invented
//!
//! **The baseline depth is profiled** -- it is the real, `profiles/prikk-self.toml`-shaped history
//! `heads/main` grew to before the branch was cut, exactly as measured by increment 2.
//!
//! **The divergence shape is invented, not profiled, and is disjoint by choice.**
//! `profiles/prikk-self.toml`'s own extraction command carries `--no-merges`
//! (verified in the committed profile, not assumed), so the profile contains no merge data at all --
//! there is nothing to derive a realistic divergence shape from. Each side creates
//! [`DIVERGENCE_SIZE`] brand-new, branch-unique files; no path is touched by both sides. This is the
//! simplest shape to implement correctly without extending the planner's live-path bookkeeping across
//! a persisted-and-reloaded state boundary, and it still fully exercises `replay_derived_state`
//! (RFC 136 §9 item 2's actual target), which reads only the baseline's own depth -- not the
//! divergence's content. **What it does not exercise**: overlapping-edit divergence, which stresses
//! `analyze_merge_evidence`'s conflict detection differently and is a documented gap, not a silent
//! one -- a future round wanting that comparison needs its own divergence generator.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use prikk_corpus::{Profile, execute};

mod support;

/// Increment 2's ruling: 2,048 stands as RFC 139 §6's design requirement and is unreachable in
/// practice; 256 is the accepted working depth for this and future increments.
const PRACTICAL_DEPTH: u64 = 256;

/// Depths this run measures in detail. 256 / 64 = 4 `REANCHOR_BOUND` intervals -- direction, not a
/// cadence curve (module doc).
const CHECKPOINTS: [u64; 5] = [32, 64, 128, 192, 256];

/// Commits each side of the divergence performs, invented per §2.3 -- small and fixed across every
/// checkpoint so it does not confound the baseline-depth trend either measurement is actually about.
const DIVERGENCE_SIZE: u64 = 5;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

fn fmt_ms(duration: Duration) -> String {
    format!("{:.2}", duration.as_secs_f64() * 1000.0)
}

fn fmt_kb(kb: Option<u64>) -> String {
    kb.map_or_else(|| "not measured".to_owned(), |value| format!("{value} KB"))
}

#[cfg(target_os = "linux")]
fn read_vm_hwm_kb(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb);
        }
    }
    None
}

/// Spawn `command`, poll `/proc/<pid>/status` for `VmHWM` while it runs, and return
/// `(elapsed, peak_kb, output)`. Does not assert success -- callers decide (some of these calls are
/// expected to be read-only and cheap to check themselves). Linux-only; see [`run_measured`] for the
/// portable fallback. Polling interval matches DC-62's own harness.
#[cfg(target_os = "linux")]
fn spawn_and_measure(mut command: Command) -> (Duration, Option<u64>, Output) {
    const INTERVAL: Duration = Duration::from_micros(500);
    let start = Instant::now();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut peak_kb: Option<u64> = None;
    loop {
        if let Some(kb) = read_vm_hwm_kb(pid) {
            peak_kb = Some(peak_kb.map_or(kb, |current: u64| current.max(kb)));
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(INTERVAL);
    }
    let elapsed = start.elapsed();
    let output = child.wait_with_output().unwrap();
    (elapsed, peak_kb, output)
}

/// Run `command`, timed, with peak RSS on Linux (via [`spawn_and_measure`]) or `None` elsewhere
/// (DC-62's own discipline: a missed or unavailable sample is *not measured*, never zero).
fn run_measured(command: Command) -> (Duration, Option<u64>, Output) {
    #[cfg(target_os = "linux")]
    {
        spawn_and_measure(command)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut command = command;
        let start = Instant::now();
        let output = command.output().unwrap();
        (start.elapsed(), None, output)
    }
}

fn require_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct CheckoutSample {
    depth: u64,
    tree_files: u64,
    plan_ms: f64,
    plan_peak_kb: Option<u64>,
    materialize_ms: f64,
    materialize_peak_kb: Option<u64>,
}

struct MergeSample {
    depth: u64,
    elapsed_ms: f64,
    peak_kb: Option<u64>,
}

fn write_divergence_file(repo_root: &Path, path: &str, seed: u64) {
    let bytes = prikk_corpus::rng::generate_bytes(seed, 32);
    let full_path = repo_root.join(path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("creating divergence parent dir");
    }
    std::fs::write(full_path, bytes).expect("writing divergence file");
}

/// Item 1: copy `.prikk` twice (never the worktree) into fresh directories, time `--patch-plan` and
/// `--patch-materialize` in each.
fn measure_checkout(binary: &Path, repo_root: &Path, depth: u64) -> CheckoutSample {
    let plan_dir = support::unique_dir(&format!("checkout-plan-{depth}"));
    support::copy_prikk_only(repo_root, &plan_dir);
    let (plan_elapsed, plan_peak_kb, plan_output) = run_measured({
        let mut command = Command::new(binary);
        command.current_dir(&plan_dir).args([
            "checkout",
            "--patch-plan",
            "--ref",
            execute::REF_NAME,
        ]);
        command
    });
    require_success(&plan_output, "checkout --patch-plan");
    let _ = std::fs::remove_dir_all(&plan_dir);

    let materialize_dir = support::unique_dir(&format!("checkout-materialize-{depth}"));
    support::copy_prikk_only(repo_root, &materialize_dir);
    let (materialize_elapsed, materialize_peak_kb, materialize_output) = run_measured({
        let mut command = Command::new(binary);
        command.current_dir(&materialize_dir).args([
            "checkout",
            "--patch-materialize",
            "--ref",
            execute::REF_NAME,
        ]);
        command
    });
    require_success(&materialize_output, "checkout --patch-materialize");
    let tree_files = support::count_tree_files(&materialize_dir);
    let _ = std::fs::remove_dir_all(&materialize_dir);

    CheckoutSample {
        depth,
        tree_files,
        plan_ms: plan_elapsed.as_secs_f64() * 1000.0,
        plan_peak_kb,
        materialize_ms: materialize_elapsed.as_secs_f64() * 1000.0,
        materialize_peak_kb,
    }
}

/// Item 2: cut a branch from `heads/main`'s current tip, grow both sides by [`DIVERGENCE_SIZE`]
/// commits (disjoint new files), then time `merge-evidence`. Restores the worktree to
/// `heads/main`'s own real state before returning (removes the right side's files) so the caller can
/// safely resume committing to `heads/main`. Returns the sample plus how many additional real
/// commits `heads/main` gained (for the caller's own depth bookkeeping).
fn measure_merge(
    binary: &Path,
    repo_root: &Path,
    profile: &Profile,
    depth: u64,
) -> (MergeSample, u64) {
    let branch_name = format!("heads/divergence-{depth}");
    let baseline_id = support::block_ids(binary, repo_root, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("heads/main has a sealed tip");
    execute::branch_create(binary, repo_root, profile, &branch_name, execute::REF_NAME)
        .expect("branch create");

    for index in 0..DIVERGENCE_SIZE {
        let path = format!("divergence/{depth}/left/f{index}.txt");
        write_divergence_file(
            repo_root,
            &path,
            profile.builder_inputs.generator_seed ^ (depth << 32) ^ index,
        );
        execute::run_commit(
            binary,
            repo_root,
            profile,
            execute::REF_NAME,
            &format!("divergence left {depth}-{index}"),
        )
        .expect("left divergence commit");
        execute::run_seal(binary, repo_root, profile, execute::REF_NAME)
            .expect("left divergence seal");
    }
    let left_id = support::block_ids(binary, repo_root, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("heads/main has a sealed tip after left divergence growth");

    let mut right_paths = Vec::new();
    for index in 0..DIVERGENCE_SIZE {
        let path = format!("divergence/{depth}/right/f{index}.txt");
        write_divergence_file(
            repo_root,
            &path,
            profile.builder_inputs.generator_seed ^ (depth << 32) ^ (1_000_000 + index),
        );
        execute::run_commit(
            binary,
            repo_root,
            profile,
            &branch_name,
            &format!("divergence right {depth}-{index}"),
        )
        .expect("right divergence commit");
        execute::run_seal(binary, repo_root, profile, &branch_name).expect("right divergence seal");
        right_paths.push(path);
    }
    let right_id = support::block_ids(binary, repo_root, &branch_name, 1)
        .into_iter()
        .next()
        .expect("branch has a sealed tip after right divergence growth");

    for path in &right_paths {
        std::fs::remove_file(repo_root.join(path)).expect("removing right divergence file");
    }

    let (elapsed, peak_kb, output) = run_measured({
        let mut command = Command::new(binary);
        command.current_dir(repo_root).args([
            "merge-evidence",
            "--baseline-block",
            &baseline_id,
            "--left-block",
            &left_id,
            "--right-block",
            &right_id,
        ]);
        command
    });
    require_success(&output, "merge-evidence");

    (
        MergeSample {
            depth,
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
            peak_kb,
        },
        DIVERGENCE_SIZE,
    )
}

#[test]
#[ignore = "RFC 139 increment 3's own measurement instrument; expensive, run deliberately"]
fn two_measurements() {
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, PRACTICAL_DEPTH).expect("planning to depth 256");
    let binary = support::prikk_binary_path();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let repo_root = support::unique_dir("two-measurements");

    eprintln!("binary: {} ({})", identity.path, identity.version_output);
    eprintln!("binary sha256: {}", identity.sha256);
    eprintln!("planned commits: {}", manifest.commits.len());

    execute::init_repository(binary, &repo_root).expect("init");

    let mut trusted = false;
    let mut actual_depth: u64 = 0;
    let mut checkout_samples = Vec::new();
    let mut merge_samples = Vec::new();

    for (index, commit) in manifest.commits.iter().enumerate() {
        execute::materialize_commit(&repo_root, commit).expect("materializing commit");
        execute::run_commit(
            binary,
            &repo_root,
            &profile,
            execute::REF_NAME,
            &format!("corpus commit {index}"),
        )
        .expect("commit");
        if !trusted {
            execute::trust_maintainer(binary, &repo_root, &profile).expect("trust");
            trusted = true;
        }
        execute::run_seal(binary, &repo_root, &profile, execute::REF_NAME).expect("seal");
        actual_depth += 1;

        let nominal_depth = (index + 1) as u64;
        if CHECKPOINTS.contains(&nominal_depth) {
            let depth = actual_depth;
            eprintln!("checkpoint: nominal {nominal_depth}, actual {depth}");

            let checkout_sample = measure_checkout(binary, &repo_root, depth);
            eprintln!(
                "  checkout: tree {} files, plan {} ms (peak {}), materialize {} ms (peak {})",
                checkout_sample.tree_files,
                fmt_ms(Duration::from_secs_f64(checkout_sample.plan_ms / 1000.0)),
                fmt_kb(checkout_sample.plan_peak_kb),
                fmt_ms(Duration::from_secs_f64(
                    checkout_sample.materialize_ms / 1000.0
                )),
                fmt_kb(checkout_sample.materialize_peak_kb),
            );
            checkout_samples.push(checkout_sample);

            let (merge_sample, left_growth) = measure_merge(binary, &repo_root, &profile, depth);
            eprintln!(
                "  merge-evidence: {} ms (peak {})",
                fmt_ms(Duration::from_secs_f64(merge_sample.elapsed_ms / 1000.0)),
                fmt_kb(merge_sample.peak_kb),
            );
            merge_samples.push(merge_sample);
            actual_depth += left_growth;
        }
    }

    let mut report = String::new();
    report.push_str("# RFC 139 increment 3 -- the two measurements\n\n");
    report.push_str(&format!(
        "Generated by `cargo test -p prikk-corpus --locked --test two_measurements -- --ignored \
         --nocapture two_measurements`. Profile: `profiles/prikk-self.toml`. Practical depth cap: \
         {PRACTICAL_DEPTH} (increment 2's ruling; the RFC 139 §6 floor of 2,048 is unreachable in \
         practice). Divergence size: {DIVERGENCE_SIZE} commits per side, disjoint new files (see this \
         file's own module doc for what is profiled and what is invented). Binary: `{}` (`{}`), sha256 \
         `{}`.\n\n**256 / 64 = 4 `REANCHOR_BOUND` intervals -- this is not a cadence curve.**\n\n",
        identity.path, identity.version_output, identity.sha256,
    ));

    report.push_str("## Item 1 -- checkout cost (RFC 136 §9 item 1)\n\n");
    report.push_str(
        "| Baseline depth | Tree files | patch-plan (ms) | plan peak (KB) | patch-materialize (ms) | materialize peak (KB) |\n",
    );
    report.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for sample in &checkout_samples {
        report.push_str(&format!(
            "| {} | {} | {:.2} | {} | {:.2} | {} |\n",
            sample.depth,
            sample.tree_files,
            sample.plan_ms,
            sample
                .plan_peak_kb
                .map_or_else(|| "not measured".to_owned(), |kb| kb.to_string()),
            sample.materialize_ms,
            sample
                .materialize_peak_kb
                .map_or_else(|| "not measured".to_owned(), |kb| kb.to_string()),
        ));
    }

    report.push_str("\n## Item 2 -- merge baseline reconstruction cost (RFC 136 §9 item 2)\n\n");
    report.push_str(&format!(
        "Each row's baseline is the block `heads/main` was at when the divergence was cut; both \
         sides then grew by {DIVERGENCE_SIZE} commits (disjoint new files) before `merge-evidence` \
         was timed.\n\n",
    ));
    report.push_str("| Baseline depth | merge-evidence (ms) | peak (KB) |\n");
    report.push_str("|---:|---:|---:|\n");
    for sample in &merge_samples {
        report.push_str(&format!(
            "| {} | {:.2} | {} |\n",
            sample.depth,
            sample.elapsed_ms,
            sample
                .peak_kb
                .map_or_else(|| "not measured".to_owned(), |kb| kb.to_string()),
        ));
    }

    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/139-measurement-corpus/two-measurements-report-v1.md"
    );
    std::fs::write(report_path, &report).expect("writing report");
    eprintln!("report written to {report_path}");

    let _ = std::fs::remove_dir_all(&repo_root);
}
