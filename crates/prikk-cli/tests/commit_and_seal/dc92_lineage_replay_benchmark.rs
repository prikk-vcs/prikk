//! DC-92 lineage-replay benchmark harness.
//!
//! Measures wall-clock cost of `prikk verify` and `prikk seal` against **sealed-history length**
//! (block count), isolated from repository (tree) size — the axis DC-59/62/64's own harness never
//! varies, and DC-69's Axis D isolates for `commit` but not for `verify`/`seal`. This file answers
//! DC-92 handoff §2 (reproduce the O(N³) `verify` baseline) and §4.1 (measure `seal` against the same
//! axis to confirm or refute the O(N²)-per-seal hypothesis). It decides nothing about the fix; DC-92's
//! own report reads what this produces.
//!
//! `#[ignore]`d by default, matching `dc59_commit_benchmark.rs`'s precedent: a measurement
//! instrument, not a correctness test, and its dominant cost (growing a 160-block sealed history,
//! current-hypothesis O(N³) to do so if seal is itself O(N²) per step) does not belong in the default
//! suite.
//!
//! **One growing repository, not six independent ones.** DC-59's axes each build a fresh repository
//! per sample point. That would mean rebuilding history from scratch at N=5, again at N=10, ...,
//! again at N=160 — redundant work, since building to 160 already passes through every smaller
//! checkpoint. Instead: one repository is grown generation by generation to depth 160, `verify` is
//! timed non-destructively (read-only) at each checkpoint depth along the way, and **every** seal
//! call's duration is recorded as it happens, not only at checkpoints — giving the seal axis a full
//! curve at no extra cost.
//!
//! **Tree size held fixed via churn**, reusing DC-69 Axis D's exact technique (delete the oldest
//! tracked file, create one new file at a fresh path, each generation) rather than editing existing
//! files — editing the same text file across two separate commits is a pre-existing, independently
//! reported defect (`dc59_commit_benchmark.rs`'s own module doc), and churn avoids it while keeping
//! live tree size constant, which is what isolates history length from repository size.
//!
//! **Reduced sample count, stated rather than hidden.** Growing one repository to depth 160 is
//! expensive on its own (a `verify` call at depth 160 alone measured 34.2 s in the finding this
//! increment investigates, and this harness makes that many more calls building up to it). This file
//! uses `SAMPLES` independent full growth runs, deliberately fewer than DC-59's harness uses per
//! point — the report states the exact count and this module doc explains why, rather than silently
//! presenting a thin sample as if it were DC-59's depth of rigor.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::support;
#[cfg(target_os = "linux")]
use crate::support::prikk;
use crate::support::{init, seal, unique_repo, verify};

const REF_NAME: &str = "heads/main";

/// Independent full-growth-to-160 runs. See module doc for why this is small.
const SAMPLES: usize = 2;

/// Sealed-block-count checkpoints `verify` is timed at — the finding's own N values.
const N_VALUES: [usize; 6] = [5, 10, 20, 40, 80, 160];

/// Deepest checkpoint; the growth loop runs to this depth once per sample.
const MAX_DEPTH: usize = 160;

/// Live tree size held fixed throughout growth via churn (DC-69 Axis D's technique).
const TREE_SIZE: usize = 10;

const FILE_SIZE_BYTES: usize = 64;

/// Where `unique_repo` builds every fixture in this harness — `std::env::temp_dir()`, which honors
/// `TMPDIR`. Every timing figure in this report is `fsync`-dominated, so which filesystem this
/// resolves to (tmpfs vs. a real disk) changes the absolute numbers by an order of magnitude. RFC 111
/// Stage 2 implementation review v1 §5: a `TMPDIR` pointed at tmpfs for one run and at a real disk for
/// another produced numbers that looked like a code regression and were actually two different
/// filesystems. Reported once per run so two reports can be compared, or rejected, on sight.
fn fixture_root_dir() -> PathBuf {
    std::env::temp_dir()
}

/// Best-effort filesystem type at `path`, via `stat --file-system --format=%T` (coreutils, Linux).
/// Diagnostic only -- never fails the run; an undetectable filesystem is reported as such rather than
/// panicking, since this harness's value is the timing data, not this label.
#[cfg(target_os = "linux")]
fn filesystem_kind(path: &Path) -> String {
    let output = std::process::Command::new("stat")
        .args(["--file-system", "--format=%T"])
        .arg(path)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let kind = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if kind.is_empty() {
                "unknown (empty `stat` output)".to_string()
            } else {
                kind
            }
        }
        Ok(out) => format!(
            "unknown (`stat --file-system` exited {:?}: {})",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(err) => format!("unknown (`stat` unavailable: {err})"),
    }
}

#[cfg(not(target_os = "linux"))]
fn filesystem_kind(_path: &Path) -> String {
    "unknown (filesystem detection is Linux-only in this harness)".to_string()
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut chunks = buf.chunks_exact_mut(8);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let remainder = chunks.into_remainder();
        if !remainder.is_empty() {
            let extra = self.next_u64().to_le_bytes();
            for (dst, byte) in remainder.iter_mut().zip(extra) {
                *dst = byte;
            }
        }
    }
}

fn random_content(rng: &mut SplitMix64) -> Vec<u8> {
    let mut content = vec![0_u8; FILE_SIZE_BYTES];
    rng.fill_bytes(&mut content);
    for byte in &mut content {
        *byte = b'a' + (*byte % 26);
    }
    content
}

/// Genesis: `tree_size` files, one commit, one seal -- sealed-block count 1 after this returns.
/// Untimed; this is setup, not a measured point (matching DC-59's own convention). Takes `tree_size`
/// as a parameter (rather than reading the `TREE_SIZE` constant directly) so the memory axis below can
/// build repositories at other, larger tree sizes than the timing axis's fixed 10 files.
fn build_genesis(root: &Path, seed: u64, tree_size: usize) -> Vec<PathBuf> {
    init(root);
    let mut rng = SplitMix64::new(seed);
    let mut files = Vec::with_capacity(tree_size);
    for index in 0..tree_size {
        let path = PathBuf::from(format!("f{index}.txt"));
        std::fs::write(root.join(&path), random_content(&mut rng)).unwrap();
        files.push(path);
    }
    let commit_output = support::commit(root, REF_NAME, "dc92-bench: genesis");
    support::ok(&commit_output, "genesis commit");
    let seal_output = seal(root, REF_NAME);
    support::ok(&seal_output, "genesis seal");
    files
}

/// One churn generation: delete the oldest tracked file, create a new one at a fresh path, commit
/// (untimed), then **time** the seal that produces the next sealed block. Live tree size is
/// unchanged (still `TREE_SIZE`) before and after every call.
fn churn_generation_timed(
    root: &Path,
    files: &mut Vec<PathBuf>,
    next_index: &mut usize,
    rng: &mut SplitMix64,
) -> Duration {
    let victim = files.remove(0);
    std::fs::remove_file(root.join(&victim)).unwrap();
    let new_path = PathBuf::from(format!("churn-{next_index}.txt"));
    *next_index += 1;
    std::fs::write(root.join(&new_path), random_content(rng)).unwrap();
    files.push(new_path);

    let commit_output = support::commit(root, REF_NAME, "dc92-bench: churn");
    support::ok(&commit_output, "churn commit");

    let start = Instant::now();
    let seal_output = seal(root, REF_NAME);
    let elapsed = start.elapsed();
    support::ok(&seal_output, "churn seal");
    elapsed
}

/// Time one `verify` invocation against the repository's current state. Read-only, so this does not
/// disturb the growing history -- the same repository continues to the next checkpoint afterward.
fn time_verify(root: &Path) -> Duration {
    let start = Instant::now();
    let output = verify(root);
    let elapsed = start.elapsed();
    support::ok(&output, "measured verify");
    elapsed
}

struct SampleRun {
    /// `seal_by_depth[i]` is the duration of the seal that produced sealed block `i + 2` (block 1 is
    /// genesis, untimed; the first timed seal produces block 2).
    seal_by_depth: Vec<Duration>,
    /// `verify_by_n[k]` is the duration of the `verify` call at sealed-block count `N_VALUES[k]`.
    verify_by_n: Vec<Duration>,
}

fn run_one_sample(sample_index: usize) -> SampleRun {
    let root = unique_repo(&format!("dc92-lineage-{sample_index}"));
    let seed = 0xDC92_0000_0000_0000_u64
        .wrapping_add(sample_index as u64)
        .rotate_left(7);
    let mut files = build_genesis(&root, seed, TREE_SIZE);
    let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
    let mut next_index = TREE_SIZE;

    let mut seal_by_depth = Vec::with_capacity(MAX_DEPTH - 1);
    let mut verify_by_n = Vec::with_capacity(N_VALUES.len());

    // Depth 1 (genesis) already exists, untimed. Grow to MAX_DEPTH, timing every seal and checking
    // verify at each N_VALUES checkpoint as depth reaches it.
    for depth in 2..=MAX_DEPTH {
        let elapsed = churn_generation_timed(&root, &mut files, &mut next_index, &mut rng);
        seal_by_depth.push(elapsed);
        if N_VALUES.contains(&depth) {
            verify_by_n.push(time_verify(&root));
        }
    }

    let _ = std::fs::remove_dir_all(&root);
    SampleRun {
        seal_by_depth,
        verify_by_n,
    }
}

fn fmt_ms(duration: Duration) -> String {
    format!("{:.2}", duration.as_secs_f64() * 1000.0)
}

fn median(durations: &[Duration]) -> Duration {
    let mut sorted = durations.to_vec();
    sorted.sort();
    sorted[sorted.len() / 2]
}

// --- Memory axis (DC-92 implementation review v1, §4 condition) ---------------------------------
//
// `LineageStateMemo` retains one `(NodeLifecycleState, TextCache)` clone per verified block for the
// whole `verify` run, never evicted. The timing axes above hold tree size fixed at `TREE_SIZE` (10)
// files specifically to isolate history depth from repository size -- exactly the review's point:
// that is the wrong instrument for a memory question that plausibly depends on *both* axes at once
// (`NodeLifecycleState::seen_ids` never shrinks, DC-69; each memo entry additionally carries a
// `TextCache` of materialized file content). This section measures peak `VmHWM` (the kernel's own
// resident-set high-water mark) via DC-62's technique -- `.spawn()` + polling `/proc/<pid>/status` --
// against depth and tree size independently, so an observed growth can be attributed to the right
// axis rather than conflated.
//
// Point-sampled, not grown continuously like the timing axis above: crossing two independent
// variables (depth, tree size) over one growth path would make it impossible to tell which axis
// produced an observed change, so each grid point (or short checkpoint run) gets its own repository.
//
// 1 trial per point, not `SAMPLES`. This measures a structural growth *shape* (linear vs.
// superlinear), which is visible at n=1 unlike wall-clock timing's run-to-run scheduling noise --
// stated here rather than silently reusing the timing axis's own stated-thin-sample precedent as if
// it justified the same count for a different reason.

/// Sampling interval for `/proc/<pid>/status` polling, identical to DC-62's own harness.
#[cfg(target_os = "linux")]
const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_micros(500);

/// Fixed tree size for the depth-sensitivity axis: a "realistic" file count per the review's
/// condition, not the churn-fixed 10 files the timing axis uses to isolate depth alone.
#[cfg(target_os = "linux")]
const MEMORY_DEPTH_TREE_SIZE: usize = 1_000;
#[cfg(target_os = "linux")]
const MEMORY_DEPTH_VALUES: [usize; 4] = [5, 40, 100, 160];

/// Fixed depth (the deepest checkpoint measured elsewhere in this harness) for the
/// tree-size-sensitivity axis.
#[cfg(target_os = "linux")]
const MEMORY_TREE_DEPTH: usize = 160;
/// `1_000` is deliberately omitted -- the depth axis above already measures that exact grid point
/// (tree size 1,000, depth 160) as its own last checkpoint, so it is reused rather than rebuilt.
#[cfg(target_os = "linux")]
const MEMORY_TREE_VALUES: [usize; 3] = [10, 100, 10_000];

#[cfg(target_os = "linux")]
const MEMORY_FLOOR_TREE_SIZE: usize = 1;
#[cfg(target_os = "linux")]
const MEMORY_FLOOR_SAMPLES: usize = 5;

/// Outcome of one memory-measuring `verify` trial: the peak `VmHWM` observed (if any sample landed
/// while the child was alive), and how many polling attempts succeeded versus were made. A missed
/// sample is `peak_kb: None` -- never zero. Mirrors `dc59_commit_benchmark.rs`'s `MemoryTrial`
/// exactly (DC-62's own established shape); duplicated rather than shared since sharing would mean
/// touching that already-reviewed file's structure for an unrelated increment.
#[cfg(target_os = "linux")]
struct MemoryTrial {
    peak_kb: Option<u64>,
    attempts: usize,
    successes: usize,
}

#[cfg(target_os = "linux")]
fn read_vm_hwm_kb(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let mut parts = rest.split_whitespace();
            let number = parts
                .next()
                .unwrap_or_else(|| panic!("VmHWM line has no value: {line:?}"));
            let unit = parts
                .next()
                .unwrap_or_else(|| panic!("VmHWM line has no unit: {line:?}"));
            assert_eq!(unit, "kB", "unexpected VmHWM unit in line: {line:?}");
            let kb: u64 = number
                .parse()
                .unwrap_or_else(|err| panic!("VmHWM value {number:?} is not an integer: {err}"));
            return Some(kb);
        }
    }
    None
}

/// Spawn `command`, poll `/proc/<pid>/status` for `VmHWM` at `interval` while it runs (sampling
/// first, then checking whether it has exited, so a process that exits between iterations still gets
/// one more attempt), then collect its output and assert success under `what`.
#[cfg(target_os = "linux")]
fn measure_process_memory(
    mut command: std::process::Command,
    interval: Duration,
    what: &str,
) -> MemoryTrial {
    use std::process::Stdio;

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();

    let mut peak_kb: Option<u64> = None;
    let mut attempts = 0_usize;
    let mut successes = 0_usize;
    loop {
        attempts += 1;
        if let Some(kb) = read_vm_hwm_kb(pid) {
            successes += 1;
            peak_kb = Some(peak_kb.map_or(kb, |current| current.max(kb)));
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(interval);
    }
    let output = child.wait_with_output().unwrap();
    support::ok(&output, what);

    MemoryTrial {
        peak_kb,
        attempts,
        successes,
    }
}

#[cfg(target_os = "linux")]
fn measure_verify_memory(root: &Path, interval: Duration) -> MemoryTrial {
    let mut command = prikk(root);
    command.arg("verify");
    measure_process_memory(command, interval, "measured verify (memory pass)")
}

/// Grow a repository to `target_depth` at a fixed `tree_size` via churn, measuring `verify`'s peak
/// memory at each of `checkpoints` (sealed-block counts) along the way. Returns `(depth, trial)`
/// pairs in checkpoint order.
#[cfg(target_os = "linux")]
fn measure_verify_memory_by_depth(
    tree_size: usize,
    target_depth: usize,
    checkpoints: &[usize],
) -> Vec<(usize, MemoryTrial)> {
    let root = unique_repo(&format!("dc92-mem-tree{tree_size}-depth{target_depth}"));
    let seed = 0xDC92_5EED_0000_0000_u64
        .wrapping_add(tree_size as u64)
        .rotate_left(11)
        .wrapping_add(target_depth as u64);
    let mut files = build_genesis(&root, seed, tree_size);
    let mut rng = SplitMix64::new(seed ^ 0xAAAA_AAAA_5555_5555);
    let mut next_index = tree_size;

    let mut results = Vec::with_capacity(checkpoints.len());
    for depth in 2..=target_depth {
        churn_generation_timed(&root, &mut files, &mut next_index, &mut rng);
        if checkpoints.contains(&depth) {
            results.push((depth, measure_verify_memory(&root, MEMORY_SAMPLE_INTERVAL)));
        }
    }

    let _ = std::fs::remove_dir_all(&root);
    results
}

/// One generation of the edit-heavy variant: overwrite one already-tracked file's content in place
/// (round-robin through `files`, never creating or deleting) and commit, producing an `EditText`
/// patch (DC-65's own pattern — `dc65_text_edit_baseline.rs`'s
/// `editing_the_same_text_file_across_five_sealed_commits_succeeds` confirms overwriting a tracked
/// path is all `commit` needs to classify it as an edit, no separate API). Unlike churn, live tree
/// membership and `NodeLifecycleState`'s `seen_ids`/`live_by_id`/`path_to_id` sizes stay exactly
/// constant across every generation -- isolating the `TextCache` term the depth/tree-size axes above
/// cannot see (their churn technique never edits, so `TextCache` stayed empty in every trial there).
#[cfg(target_os = "linux")]
fn edit_generation(root: &Path, files: &[PathBuf], edit_index: &mut usize, rng: &mut SplitMix64) {
    let path = &files[*edit_index % files.len()];
    *edit_index += 1;
    std::fs::write(root.join(path), random_content(rng)).unwrap();
    let commit_output = support::commit(root, REF_NAME, "dc92-bench: edit");
    support::ok(&commit_output, "edit commit");
    let seal_output = seal(root, REF_NAME);
    support::ok(&seal_output, "edit seal");
}

/// Grow a repository to `target_depth` at a fixed `tree_size` via **edits** (see [`edit_generation`]),
/// measuring `verify`'s peak memory at each of `checkpoints`. The edit-axis counterpart to
/// [`measure_verify_memory_by_depth`], which uses churn.
#[cfg(target_os = "linux")]
fn measure_verify_memory_by_depth_with_edits(
    tree_size: usize,
    target_depth: usize,
    checkpoints: &[usize],
) -> Vec<(usize, MemoryTrial)> {
    let root = unique_repo(&format!(
        "dc92-mem-edit-tree{tree_size}-depth{target_depth}"
    ));
    let seed = 0xDC92_ED17_0000_0000_u64
        .wrapping_add(tree_size as u64)
        .rotate_left(13)
        .wrapping_add(target_depth as u64);
    let files = build_genesis(&root, seed, tree_size);
    let mut rng = SplitMix64::new(seed ^ 0x5555_5555_AAAA_AAAA);
    let mut edit_index = 0_usize;

    let mut results = Vec::with_capacity(checkpoints.len());
    for depth in 2..=target_depth {
        edit_generation(&root, &files, &mut edit_index, &mut rng);
        if checkpoints.contains(&depth) {
            results.push((depth, measure_verify_memory(&root, MEMORY_SAMPLE_INTERVAL)));
        }
    }

    let _ = std::fs::remove_dir_all(&root);
    results
}

#[cfg(target_os = "linux")]
struct MemoryAxisResult {
    floor_peak_kb: Option<u64>,
    floor_samples: (usize, usize),
    by_depth: Vec<(usize, MemoryTrial)>,
    by_tree_size: Vec<(usize, MemoryTrial)>,
    by_depth_with_edits: Vec<(usize, MemoryTrial)>,
}

#[cfg(target_os = "linux")]
fn run_memory_axis() -> MemoryAxisResult {
    let mut floor_peak_kb: Option<u64> = None;
    let mut floor_obtained = 0_usize;
    let mut floor_attempted = 0_usize;
    for sample_index in 0..MEMORY_FLOOR_SAMPLES {
        let root = unique_repo(&format!("dc92-mem-floor-{sample_index}"));
        let seed = 0xDC92_F100_0000_0000_u64.wrapping_add(sample_index as u64);
        build_genesis(&root, seed, MEMORY_FLOOR_TREE_SIZE);
        let trial = measure_verify_memory(&root, MEMORY_SAMPLE_INTERVAL);
        floor_attempted += trial.attempts;
        floor_obtained += trial.successes;
        if let Some(kb) = trial.peak_kb {
            floor_peak_kb = Some(floor_peak_kb.map_or(kb, |current| current.max(kb)));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
    eprintln!("memory axis: floor done ({floor_obtained}/{floor_attempted} samples)");

    eprintln!(
        "memory axis: depth sweep (tree size {MEMORY_DEPTH_TREE_SIZE}, depths {MEMORY_DEPTH_VALUES:?})..."
    );
    let by_depth =
        measure_verify_memory_by_depth(MEMORY_DEPTH_TREE_SIZE, MAX_DEPTH, &MEMORY_DEPTH_VALUES);

    let mut by_tree_size = Vec::with_capacity(MEMORY_TREE_VALUES.len() + 1);
    // Reuse the depth sweep's own deepest checkpoint (tree size 1,000, depth 160) as this axis's
    // tree-size-1,000 point instead of rebuilding it.
    if let Some((_, last_trial)) = by_depth.last() {
        by_tree_size.push((
            MEMORY_DEPTH_TREE_SIZE,
            MemoryTrial {
                peak_kb: last_trial.peak_kb,
                attempts: last_trial.attempts,
                successes: last_trial.successes,
            },
        ));
    }
    for &tree_size in &MEMORY_TREE_VALUES {
        eprintln!("memory axis: tree size {tree_size} at depth {MEMORY_TREE_DEPTH}...");
        let mut points =
            measure_verify_memory_by_depth(tree_size, MEMORY_TREE_DEPTH, &[MEMORY_TREE_DEPTH]);
        if let Some((_, trial)) = points.pop() {
            by_tree_size.push((tree_size, trial));
        }
    }
    by_tree_size.sort_by_key(|(tree_size, _)| *tree_size);

    eprintln!(
        "memory axis: edit-heavy depth sweep (tree size {MEMORY_DEPTH_TREE_SIZE}, depths {MEMORY_DEPTH_VALUES:?})..."
    );
    let by_depth_with_edits = measure_verify_memory_by_depth_with_edits(
        MEMORY_DEPTH_TREE_SIZE,
        MAX_DEPTH,
        &MEMORY_DEPTH_VALUES,
    );

    MemoryAxisResult {
        floor_peak_kb,
        floor_samples: (floor_obtained, floor_attempted),
        by_depth,
        by_tree_size,
        by_depth_with_edits,
    }
}

#[cfg(target_os = "linux")]
fn fmt_kb(kb: Option<u64>) -> String {
    kb.map_or_else(|| "not measured".to_owned(), |value| value.to_string())
}

#[cfg(target_os = "linux")]
fn fmt_above_floor(kb: Option<u64>, floor_kb: Option<u64>) -> String {
    match (kb, floor_kb) {
        (Some(kb), Some(floor)) => kb.saturating_sub(floor).to_string(),
        _ => "not measured".to_owned(),
    }
}

fn render_memory_axis(out: &mut String) {
    #[cfg(not(target_os = "linux"))]
    {
        out.push_str("## verify — peak memory\n\n");
        out.push_str(
            "**Not measured on this platform.** `/proc/<pid>/status` is Linux-only (DC-62); this run \
             was on a non-Linux platform, so no peak-memory data is available. Re-run on Linux to \
             populate this section.\n\n",
        );
    }
    #[cfg(target_os = "linux")]
    {
        let result = run_memory_axis();
        out.push_str("## verify — peak memory\n\n");
        out.push_str(&format!(
            "Peak `VmHWM` (the kernel's own resident-set high-water mark), measured via `.spawn()` + \
             `/proc/<pid>/status` polling every {} µs (DC-62's technique, no new dependency). 1 trial \
             per point (see module doc for why this differs from the timing axes' {SAMPLES}). \
             Investigates the implementation review's §4 condition: `LineageStateMemo` retains one \
             state clone per verified block for the whole run, never evicted, so this measures whether \
             that grows unboundedly against history depth, tree size, or both.\n\n",
            MEMORY_SAMPLE_INTERVAL.as_micros(),
        ));
        out.push_str(&format!(
            "**Floor:** `verify` against a {MEMORY_FLOOR_TREE_SIZE}-file, 1-block repository, \
             {MEMORY_FLOOR_SAMPLES} trials, {}/{} samples landed: peak VmHWM {}. Subtracted from every \
             figure below as a roughly constant additive offset (the **Above floor** column).\n\n",
            result.floor_samples.0,
            result.floor_samples.1,
            fmt_kb(result.floor_peak_kb),
        ));

        out.push_str(&format!(
            "### Depth axis (tree size fixed at {MEMORY_DEPTH_TREE_SIZE} files)\n\n"
        ));
        out.push_str("| Sealed blocks (N) | Peak VmHWM (KB) | Above floor (KB) |\n");
        out.push_str("|---:|---:|---:|\n");
        for (depth, trial) in &result.by_depth {
            out.push_str(&format!(
                "| {depth} | {} | {} |\n",
                fmt_kb(trial.peak_kb),
                fmt_above_floor(trial.peak_kb, result.floor_peak_kb),
            ));
        }
        out.push('\n');

        out.push_str(&format!(
            "### Tree-size axis (depth fixed at {MEMORY_TREE_DEPTH})\n\n"
        ));
        out.push_str("| Live tree files | Peak VmHWM (KB) | Above floor (KB) |\n");
        out.push_str("|---:|---:|---:|\n");
        for (tree_size, trial) in &result.by_tree_size {
            out.push_str(&format!(
                "| {tree_size} | {} | {} |\n",
                fmt_kb(trial.peak_kb),
                fmt_above_floor(trial.peak_kb, result.floor_peak_kb),
            ));
        }
        out.push('\n');

        out.push_str(&format!(
            "### Depth axis, edit-heavy variant (tree size fixed at {MEMORY_DEPTH_TREE_SIZE} files, \
             review §4.4)\n\n"
        ));
        out.push_str(
            "Every generation **overwrites one already-tracked file's content** (round-robin, DC-65's \
             `EditText` pattern) instead of churn's create+delete -- live tree membership and \
             `NodeLifecycleState`'s `seen_ids`/`live_by_id`/`path_to_id` sizes stay exactly constant \
             across every generation, isolating `TextCache`'s own contribution from the depth axis \
             above (which never edits, so `TextCache` stayed empty there).\n\n",
        );
        out.push_str("| Sealed blocks (N) | Peak VmHWM (KB) | Above floor (KB) |\n");
        out.push_str("|---:|---:|---:|\n");
        for (depth, trial) in &result.by_depth_with_edits {
            out.push_str(&format!(
                "| {depth} | {} | {} |\n",
                fmt_kb(trial.peak_kb),
                fmt_above_floor(trial.peak_kb, result.floor_peak_kb),
            ));
        }
        out.push('\n');
    }
}

#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn lineage_replay_benchmark() {
    let mut runs = Vec::with_capacity(SAMPLES);
    for sample_index in 0..SAMPLES {
        eprintln!(
            "sample {}/{SAMPLES}: growing to depth {MAX_DEPTH}...",
            sample_index + 1
        );
        runs.push(run_one_sample(sample_index));
    }

    let fixture_root = fixture_root_dir();
    let fs_kind = filesystem_kind(&fixture_root);

    let mut out = String::new();
    out.push_str("# DC-92 Lineage Replay Benchmark Report v1\n\n");
    out.push_str(&format!(
        "Generated by `cargo test -p prikk --locked --test dc92_lineage_replay_benchmark -- --ignored --nocapture lineage_replay_benchmark`. {SAMPLES} independent full-growth-to-{MAX_DEPTH} runs (see module doc for why this is fewer than DC-59's per-point sample counts). Tree size held fixed at {TREE_SIZE} files throughout via churn.\n\n"
    ));
    out.push_str(&format!(
        "**Fixture root:** `{}` — filesystem: **{fs_kind}**. Every timing figure below is `fsync`-dominated, so this filesystem changes the absolute numbers by an order of magnitude (RFC 111 Stage 2 implementation review v1 §5) — compare two reports only when this line matches, and re-run rather than trust a comparison across different values here.\n\n",
        fixture_root.display(),
    ));

    out.push_str("## verify — cost against sealed-block count\n\n");
    out.push_str("| Sealed blocks (N) | Samples | Median (ms) |\n");
    out.push_str("|---:|---:|---:|\n");
    for (index, &n) in N_VALUES.iter().enumerate() {
        let samples: Vec<Duration> = runs.iter().map(|run| run.verify_by_n[index]).collect();
        out.push_str(&format!(
            "| {n} | {} | {} |\n",
            samples.len(),
            fmt_ms(median(&samples))
        ));
    }
    out.push('\n');

    out.push_str("## seal — cost against sealed-block count already present\n\n");
    out.push_str(
        "Each row is the seal that produced the block at the stated position, i.e. the ancestor \
         chain already sealed *before* this call. `N=2` means the seal producing the second block, \
         with 1 ancestor already sealed.\n\n",
    );
    out.push_str("| Sealed blocks (N, after this seal) | Samples | Median (ms) |\n");
    out.push_str("|---:|---:|---:|\n");
    for &n in &N_VALUES {
        if n < 2 {
            continue;
        }
        let index = n - 2;
        let samples: Vec<Duration> = runs
            .iter()
            .filter_map(|run| run.seal_by_depth.get(index).copied())
            .collect();
        if samples.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "| {n} | {} | {} |\n",
            samples.len(),
            fmt_ms(median(&samples))
        ));
    }
    out.push('\n');
    out.push_str(
        "**Full per-depth seal curve** (every generation, not only checkpoints), first sample run \
         only, for inspecting the shape between checkpoints:\n\n",
    );
    out.push_str("| Sealed blocks (N) | Duration (ms) |\n");
    out.push_str("|---:|---:|\n");
    if let Some(first) = runs.first() {
        for (index, duration) in first.seal_by_depth.iter().enumerate() {
            let n = index + 2;
            if n % 10 == 0 || n == 2 || n == MAX_DEPTH {
                out.push_str(&format!("| {n} | {} |\n", fmt_ms(*duration)));
            }
        }
    }
    out.push('\n');

    eprintln!("timing axes done; starting memory axis...");
    render_memory_axis(&mut out);

    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/DC-92-lineage-replay-memoization/benchmark-report-v1.md"
    );
    std::fs::write(report_path, &out).unwrap();
    eprintln!("report written to {report_path}");
}
