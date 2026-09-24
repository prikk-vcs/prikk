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

fn write_divergence_file(repo_root: &Path, path: &str, seed: u64) {
    let bytes = prikk_corpus::rng::generate_bytes(seed, 32);
    let full_path = repo_root.join(path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("creating divergence parent dir");
    }
    std::fs::write(full_path, bytes).expect("writing divergence file");
}

/// Item 1: copy `.prikk` (never the worktree) into a fresh directory per sample, and time `--patch-plan` and
/// `--patch-materialize` in it, `samples` times, alternating the two commands; the medians are returned.
fn measure_checkout(binary: &Path, repo_root: &Path, depth: u64, samples: usize) -> CheckoutSample {
    let mut plan = Vec::new();
    let mut materialize = Vec::new();
    let mut tree_files = 0;
    for round in 0..samples {
        let plan_dir = support::unique_dir(&format!("checkout-plan-{depth}-{round}"));
        support::copy_prikk_only(repo_root, &plan_dir);
        let (elapsed, peak, output) = run_measured({
            let mut command = Command::new(binary);
            command.current_dir(&plan_dir).args([
                "checkout",
                "--patch-plan",
                "--ref",
                execute::REF_NAME,
            ]);
            command
        });
        require_success(&output, "checkout --patch-plan");
        plan.push((elapsed.as_secs_f64() * 1000.0, peak));
        let _ = std::fs::remove_dir_all(&plan_dir);

        let dir = support::unique_dir(&format!("checkout-materialize-{depth}-{round}"));
        support::copy_prikk_only(repo_root, &dir);
        let (elapsed, peak, output) = run_measured({
            let mut command = Command::new(binary);
            command.current_dir(&dir).args([
                "checkout",
                "--patch-materialize",
                "--ref",
                execute::REF_NAME,
            ]);
            command
        });
        require_success(&output, "checkout --patch-materialize");
        tree_files = support::count_tree_files(&dir);
        materialize.push((elapsed.as_secs_f64() * 1000.0, peak));
        let _ = std::fs::remove_dir_all(&dir);
    }
    let peak = |samples: &[(f64, Option<u64>)]| samples.iter().filter_map(|s| s.1).max();
    CheckoutSample {
        depth,
        tree_files,
        plan_ms: median(plan.iter().map(|s| s.0).collect()),
        plan_peak_kb: peak(&plan),
        materialize_ms: median(materialize.iter().map(|s| s.0).collect()),
        materialize_peak_kb: peak(&materialize),
    }
}

/// The three blocks a `merge-evidence` names, and how many commits `heads/main` gained.
struct Divergence {
    baseline: String,
    left: String,
    right: String,
    left_growth: u64,
}

/// Item 2, **sides that carry no edits** (release re-measurement handoff §1.2): cut a branch from `heads/main`'s
/// current tip and grow both sides by [`DIVERGENCE_SIZE`] commits of brand-new files, so `merge-evidence` does
/// the baseline replay and nothing else. Restores the worktree to `heads/main`'s own real state (removes the
/// right side's files) so the caller can resume committing to `heads/main`. `heads/main` gains
/// [`DIVERGENCE_SIZE`] real commits.
fn prepare_divergence_without_edits(
    binary: &Path,
    repo_root: &Path,
    profile: &Profile,
    depth: u64,
) -> Divergence {
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
    Divergence {
        baseline: baseline_id,
        left: left_id,
        right: right_id,
        left_growth: DIVERGENCE_SIZE,
    }
}

/// Item 2, **sides that carry edits**: each side makes [`DIVERGENCE_SIZE`] commits appending a line to a file
/// the corpus had already edited (its content is an `EditText` result nobody stored, so `merge-evidence` needs its
/// baseline text for every one of those operations). The two sides edit different files. Runs in the **throwaway
/// copy** `repo_root` (the caller discards it), so `heads/main` proper is not perturbed: the worktree ends
/// inconsistent with `heads/main`, which does not matter to a read-only `merge-evidence`.
fn prepare_divergence_with_edits(
    binary: &Path,
    repo_root: &Path,
    profile: &Profile,
    depth: u64,
    edited_paths: &[String],
) -> Divergence {
    let [left_path, right_path] = edited_paths else {
        panic!("two edited files are needed, got {edited_paths:?}");
    };
    let branch_name = format!("heads/divergence-edits-{depth}");
    let baseline_id = support::block_ids(binary, repo_root, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("heads/main has a sealed tip");
    execute::branch_create(binary, repo_root, profile, &branch_name, execute::REF_NAME)
        .expect("branch create");
    let original_left = std::fs::read(repo_root.join(left_path)).expect("left file");
    let append = |path: &str, line: String| {
        let full = repo_root.join(path);
        let mut bytes = std::fs::read(&full).expect("reading the file to edit");
        bytes.extend_from_slice(line.as_bytes());
        std::fs::write(full, bytes).expect("editing");
    };
    for index in 0..DIVERGENCE_SIZE {
        append(left_path, format!("left edit {depth}-{index}\n"));
        execute::run_commit(
            binary,
            repo_root,
            profile,
            execute::REF_NAME,
            &format!("left edit {depth}-{index}"),
        )
        .expect("left edit commit");
        execute::run_seal(binary, repo_root, profile, execute::REF_NAME).expect("left edit seal");
    }
    let left_id = support::block_ids(binary, repo_root, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("main tip");
    // The branch's baseline does not have the left side's edits: put the file back before committing on it.
    std::fs::write(repo_root.join(left_path), original_left).expect("restoring the left file");
    for index in 0..DIVERGENCE_SIZE {
        append(right_path, format!("right edit {depth}-{index}\n"));
        execute::run_commit(
            binary,
            repo_root,
            profile,
            &branch_name,
            &format!("right edit {depth}-{index}"),
        )
        .expect("right edit commit");
        execute::run_seal(binary, repo_root, profile, &branch_name).expect("right edit seal");
    }
    let right_id = support::block_ids(binary, repo_root, &branch_name, 1)
        .into_iter()
        .next()
        .expect("branch tip");
    Divergence {
        baseline: baseline_id,
        left: left_id,
        right: right_id,
        left_growth: 0,
    }
}

/// One timed `merge-evidence` over `divergence`, in `dir`.
fn time_merge_evidence(binary: &Path, dir: &Path, divergence: &Divergence) -> (f64, Option<u64>) {
    let (elapsed, peak_kb, output) = run_measured({
        let mut command = Command::new(binary);
        command.current_dir(dir).args([
            "merge-evidence",
            "--baseline-block",
            &divergence.baseline,
            "--left-block",
            &divergence.left,
            "--right-block",
            &divergence.right,
        ]);
        command
    });
    require_success(&output, "merge-evidence");
    (elapsed.as_secs_f64() * 1000.0, peak_kb)
}

/// The last two distinct files the plan edited up to commit `upto` that are still on disk in `repo_root`.
fn recently_edited_files(
    manifest: &prikk_corpus::ActionManifest,
    upto: usize,
    repo_root: &Path,
) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for commit in manifest.commits[..upto].iter().rev() {
        for action in commit.actions.iter().rev() {
            if let prikk_corpus::PlannedAction::EditText { path, .. } = action {
                if !found.contains(path) && repo_root.join(path).is_file() {
                    found.push(path.clone());
                    if found.len() == 2 {
                        return found;
                    }
                }
            }
        }
    }
    found
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

fn out_dir() -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc139");
    std::fs::create_dir_all(&dir).expect("creating the measurement directory");
    dir
}

/// One row of the merge-evidence table: `(depth, no-edit samples ms, edit samples ms, edit ops per side)`.
struct MergeRow {
    depth: u64,
    without_edits: Vec<(f64, Option<u64>)>,
    with_edits: Vec<(f64, Option<u64>)>,
    edited_files: Vec<String>,
}

#[test]
#[ignore = "RFC 139 increment 3's own measurement instrument, in release; expensive, run deliberately"]
fn two_measurements() {
    let profile = self_profile();
    let mut manifest =
        prikk_corpus::plan(&profile, PRACTICAL_DEPTH).expect("planning to depth 256");
    // For trying the instrument out: `PRIKK_TM_MAX_DEPTH` cuts the plan short.
    if let Some(cap) = std::env::var("PRIKK_TM_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        manifest.commits.truncate(cap);
    }
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let samples: usize = std::env::var("PRIKK_TM_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let label = std::env::var("PRIKK_TM_LABEL").unwrap_or_else(|_| build.to_string());
    let repo_root = support::unique_dir("two-measurements");

    eprintln!(
        "build: {build}; binary: {} ({})",
        identity.path, identity.version_output
    );
    eprintln!("binary sha256: {}", identity.sha256);

    execute::init_repository(binary, &repo_root).expect("init");

    let mut trusted = false;
    let mut actual_depth: u64 = 0;
    let mut checkout_samples = Vec::new();
    let mut merge_rows: Vec<MergeRow> = Vec::new();

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
            let (load, _) = (
                std::fs::read_to_string("/proc/loadavg").unwrap_or_default(),
                (),
            );
            eprintln!("  load at the checkpoint: {}", load.trim());

            let checkout_sample = measure_checkout(binary, &repo_root, depth, samples);
            eprintln!(
                "  checkout: tree {} files, plan {:.2} ms, materialize {:.2} ms",
                checkout_sample.tree_files, checkout_sample.plan_ms, checkout_sample.materialize_ms,
            );
            checkout_samples.push(checkout_sample);

            // The edit variant first, in a throwaway copy at exactly this depth; then the no-edit variant on the
            // real repository (which gains DIVERGENCE_SIZE commits, as before).
            let edited_files = recently_edited_files(&manifest, index + 1, &repo_root);
            let copy = support::unique_dir(&format!("two-measurements-edits-{depth}"));
            support::copy_dir_all(&repo_root, &copy);
            let with_edits =
                prepare_divergence_with_edits(binary, &copy, &profile, depth, &edited_files);
            let without_edits =
                prepare_divergence_without_edits(binary, &repo_root, &profile, depth);
            let mut row = MergeRow {
                depth,
                without_edits: Vec::new(),
                with_edits: Vec::new(),
                edited_files,
            };
            for _ in 0..samples {
                row.without_edits
                    .push(time_merge_evidence(binary, &repo_root, &without_edits));
                row.with_edits
                    .push(time_merge_evidence(binary, &copy, &with_edits));
            }
            eprintln!(
                "  merge-evidence: no edits {:?} ms, with edits {:?} ms",
                row.without_edits
                    .iter()
                    .map(|s| s.0.round())
                    .collect::<Vec<_>>(),
                row.with_edits
                    .iter()
                    .map(|s| s.0.round())
                    .collect::<Vec<_>>(),
            );
            merge_rows.push(row);
            let _ = std::fs::remove_dir_all(&copy);
            actual_depth += without_edits.left_growth;
        }
    }

    let mut report = String::new();
    report.push_str(&format!(
        "# RFC 139 increment 3 -- the two measurements ({build})\n\n"
    ));
    report.push_str(&format!(
        "Generated by `cargo test -p prikk-corpus --locked --test two_measurements -- --ignored --nocapture two_measurements`. \
         Profile: `profiles/prikk-self.toml`. Depth cap {PRACTICAL_DEPTH}. **{build} build**: `{}` (`{}`), sha256 `{}`. \
         {samples} samples per cell, medians; peak memory is the maximum over samples of a `VmHWM` poll (500 us), reported as such. \
         Divergence: {DIVERGENCE_SIZE} commits per side.\n\n\
         **256 / 64 = 4 `REANCHOR_BOUND` intervals -- this is not a cadence curve.**\n\n",
        identity.path, identity.version_output, identity.sha256,
    ));
    report.push_str("## Item 1 -- checkout cost\n\n| Baseline depth | Tree files | patch-plan (ms) | plan peak (KB, VmHWM) | patch-materialize (ms) | materialize peak (KB, VmHWM) |\n|---:|---:|---:|---:|---:|---:|\n");
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
    report.push_str(&format!(
        "\n## Item 2 -- `merge-evidence`\n\nBaseline = the block `heads/main` was at when the divergence was cut. **Without edits:** both sides create brand-new files, so \
         the cost is the baseline replay only. **With edits:** each side appends to a file the corpus had already edited (different files), so every one of its \
         {DIVERGENCE_SIZE} operations needs a baseline text nobody stored.\n\n\
         | Baseline depth | without edits (ms, median (min–max)) | with edits (ms, median (min–max)) | files edited (left, right) | peak KB without / with |\n|---:|---|---|---|---|\n"
    ));
    for row in &merge_rows {
        let range = |values: &[(f64, Option<u64>)]| {
            let ms: Vec<f64> = values.iter().map(|v| v.0).collect();
            let lo = ms.iter().cloned().fold(f64::MAX, f64::min);
            let hi = ms.iter().cloned().fold(0.0, f64::max);
            format!("{:.1} ({lo:.1}–{hi:.1})", median(ms))
        };
        let peak = |values: &[(f64, Option<u64>)]| {
            values
                .iter()
                .filter_map(|v| v.1)
                .max()
                .map_or("-".to_string(), |kb| kb.to_string())
        };
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} / {} |\n",
            row.depth,
            range(&row.without_edits),
            range(&row.with_edits),
            row.edited_files.join(", "),
            peak(&row.without_edits),
            peak(&row.with_edits),
        ));
    }
    std::fs::write(
        out_dir().join(format!("two-measurements-{label}.md")),
        &report,
    )
    .expect("writing report");
    eprintln!("{report}");

    let _ = std::fs::remove_dir_all(&repo_root);
}
