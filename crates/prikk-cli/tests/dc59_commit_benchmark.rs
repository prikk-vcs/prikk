//! DC-59 commit benchmark harness.
//!
//! Measures wall-clock cost of `prikk commit` across two axes, producing the evidence artifact
//! NFR-PERF-01 names. This increment **decides nothing** — DC-56 reads the report this test
//! produces and rules on compliance.
//!
//! `#[ignore]`d by default: it is a measurement instrument, not a correctness test, and its
//! dominant cost (generating up to 10,000-file repositories, several samples per point) does not
//! belong in the default suite. Run it deliberately with `--ignored` when a performance question is
//! open. Being excluded from routine runs means it can go stale between uses; that is accepted
//! deliberately rather than a maintenance oversight (see the RFC's Risks section).
//!
//! No new dev-dependency: `prikk-store` is already a normal dependency of this crate, reused here
//! only for `Ed25519MaintainerSigner` to derive the fixed benchmark maintainer key.
//!
//! `SplitMix64` below is a **deliberate duplicate** of the one in
//! `crates/prikk-hash/src/tests/hash_differential.rs`. That generator is a private struct in a
//! `#[cfg(test)]` module of a different crate and is not reachable from here; promoting it would
//! touch `prikk-hash` test material DC-55 froze on purpose. This is a second, independent copy of a
//! reviewed generator, not a new invented one.
//!
//! ## Why two commits per generated repository, not one
//!
//! A genesis commit (against an empty baseline) necessarily authors every file in the worktree —
//! there is no way to hold "repository size" and "changed-file count" independent on a first
//! commit, since every present file *is* the change. Both axes therefore need an established
//! baseline: generate the full repository, commit it (genesis), and seal it — all **untimed** setup
//! — then mutate exactly the files under test and time only the second `commit`, which must
//! reconcile the worktree against a real baseline of the target size. This is also why the fixed
//! maintainer key exists: only the untimed setup seal needs it, not the timed measurement itself.
//!
//! `commit` cannot run twice against one repository without a seal between attempts
//! (`node_authoring.rs`'s active-WAL guard). The one seal used here happens entirely inside setup,
//! before the timing window opens, and is not "sealing between trials" in the sense the design
//! forbids — there is exactly one timed trial per generated repository, never two.
//!
//! ## Peak-memory measurement (DC-62)
//!
//! Added as a **separate, third pass** after Axis A and Axis B, never interleaved with them:
//! Axis A/B keep DC-59's exact `time_commit` (`.output()`, no polling), so their figures are
//! produced by unmodified code and stay comparable to the original report. The memory pass reuses
//! the same repository-generation methodology at Axis A's sizes (1 file changed) but times a
//! `.spawn()`'d commit instead, polling `/proc/<pid>/status` for `VmHWM` — the kernel's own
//! peak-RSS high-water mark — at a fixed interval while the child runs, keeping the maximum
//! observed. This requires `.spawn()` rather than `.output()`, which is why it cannot share Axis
//! A/B's timing loop and why it runs separately instead.
//!
//! **No new dependency.** `rustix` is workspace-declared with `features = ["fs"]` only — no
//! `getrusage`/`wait4` — and `std` exposes neither, so `/proc` reading is the only route available
//! without adding `libc` or widening `rustix`'s features (which would also touch DC-51's dependency
//! placement surface). `/proc` is Linux-only; the memory pass is compiled and skipped cleanly on
//! other platforms (a one-line notice, not a failure), consistent with DC-37's existing
//! Linux-only-mutation-support boundary for this harness.
//!
//! **A missed sample is reported as "not measured," never as zero.** Sampling cannot catch the
//! peak of a run shorter than the polling interval — at 10 files a commit takes on the order of a
//! few milliseconds, so a sample may not land. That is expected and stated in the report per point,
//! never silently defaulted to a number: a fabricated zero would make DC-56's later memory-fix
//! evidence look like it improved from a baseline that was never actually measured.
//!
//! **The report also measures a memory floor** and reports each repository-size point's peak both
//! raw and as a delta above it, so growth against repository size — the actual claim under test —
//! is legible without a reader doing the subtraction themselves. Read raw, peak VmHWM can look
//! dominated by a fixed process cost and understate how closely it tracks repository size; the
//! floor-relative delta is the content-proportional figure DC-56 must actually flatten. The floor
//! is a real `commit` against a `MEMORY_FLOOR_FILE_COUNT`-file repository — deliberately **not**
//! `prikk --version` (the timing floor's basis): `--version` exits fast enough that `VmHWM`
//! sampling mostly caught it before the resident set reached its natural size, understating the
//! floor by two to three orders of magnitude and making it useless as a subtraction reference. A
//! minimal real commit exercises the same code path as the measured points and gives a floor that
//! actually represents the fixed cost folded into every other row.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;
use std::time::{Duration, Instant};

use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner};

const FIXED_AUTHOR_KEY_ID: &str = "dc59-bench-author";
const FIXED_AUTHOR_SEED: [u8; 32] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
const FIXED_MAINTAINER_KEY_ID: &str = "dc59-bench-maintainer";
const FIXED_MAINTAINER_SEED: [u8; 32] = [
    0x21, 0x21, 0x32, 0x32, 0x43, 0x43, 0x54, 0x54, 0x65, 0x65, 0x76, 0x76, 0x87, 0x87, 0x98, 0x98,
    0xa9, 0xa9, 0xba, 0xba, 0xcb, 0xcb, 0xdc, 0xdc, 0xed, 0xed, 0xfe, 0xfe, 0x0f, 0x0f, 0x10, 0x10,
];
/// Deterministic content seed. Distinct from DC-41/DC-55's constant — this generator produces
/// worktree file content, not hash-differential inputs, and the two should not be confused as the
/// same domain.
const CONTENT_SEED: u64 = 0x1234_5678_9abc_def0;

/// File sizes and tree shape held constant across both axes.
const FILE_SIZE_BYTES: usize = 256;
const TREE_BREADTH: usize = 8;
const TREE_DEPTH: usize = 3;

/// Axis A: repository size (file count), held at 1 changed file.
const AXIS_A_SIZES: [usize; 4] = [10, 100, 1_000, 10_000];
const AXIS_A_SAMPLES: [usize; 4] = [5, 5, 5, 3];

/// Axis B: changed-file count, held at a fixed repository size.
const AXIS_B_REPO_SIZE: usize = 1_000;
const AXIS_B_CHANGE_COUNTS: [usize; 4] = [1, 10, 100, 1_000];
const AXIS_B_SAMPLES: usize = 5;

/// Axis C (DC-64): repository size varies, but each sample repository is carried through several
/// consecutive commit+seal cycles instead of a single prepared commit. Axis A's single-commit
/// methodology cannot show the incremental baseline cache's effect at all: the one commit it times
/// is always the *second* commit against a fresh ref, which is unconditionally a cold-cache full
/// replay (the incremental cache has nothing to be warm from yet). Only cycle 2 onward exercises the
/// warm, incremental path. Fewer samples than Axis A per point — each sample now costs
/// `CYCLE_COUNT` timed commits plus that many untimed seals, not one.
const CYCLE_AXIS_SIZES: [usize; 4] = [10, 100, 1_000, 10_000];
const CYCLE_AXIS_SAMPLES: [usize; 4] = [3, 3, 3, 2];
const CYCLE_COUNT: usize = 5;

const SPAWN_FLOOR_SAMPLES: usize = 10;

/// Polling interval for the memory pass's `/proc/<pid>/status` reads. Deliberately fine-grained
/// (sub-millisecond) to give the smallest, fastest commits the best available chance of landing at
/// least one sample; still expected to miss entirely at the smallest repository sizes, which the
/// report marks *not measured* rather than defaulting.
const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_micros(500);

/// Repository size and trial count for the memory pass's baseline reference. `prikk --version` was
/// tried first and rejected: it exits fast enough (sub-100 µs) that `VmHWM` sampling mostly catches
/// it before the process's resident set reaches its natural resting size, understating the floor by
/// two to three orders of magnitude and making it useless as a subtraction reference. A real
/// `commit` against a near-empty repository — same code path as the measured points, negligible
/// content (256 bytes) — gives a floor that actually represents the fixed cost folded into every
/// other row.
const MEMORY_FLOOR_FILE_COUNT: usize = 1;
// DC-71: only used by the Linux-only /proc-based sampling loop below.
#[cfg(target_os = "linux")]
const MEMORY_FLOOR_SAMPLES: usize = 5;

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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn maintainer_public_key_hex() -> String {
    let signer =
        Ed25519MaintainerSigner::from_seed(FIXED_MAINTAINER_KEY_ID, &FIXED_MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    hex(&signer.public_key_bytes())
}

fn unique_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "prikk-dc59-bench-{tag}-{}",
        support::unique_suffix()
    ));
    dir
}

/// Deterministically populate `root` with `file_count` files across a directory tree of the given
/// breadth and depth (files distributed round-robin across leaf directories, so the tree is
/// genuinely traversed rather than everything landing in one directory), each `file_size` bytes of
/// printable pseudo-random content so worktree edits later exercise the text-edit path. Returns the
/// created files' paths relative to `root`, in creation order.
fn generate_tree(
    root: &Path,
    file_count: usize,
    breadth: usize,
    depth: usize,
    file_size: usize,
    rng: &mut SplitMix64,
) -> Vec<PathBuf> {
    let mut leaf_dirs = Vec::new();
    build_leaf_dirs(PathBuf::new(), 0, depth, breadth, &mut leaf_dirs);
    if leaf_dirs.len() > file_count.max(1) {
        leaf_dirs.truncate(file_count.max(1));
    }
    for dir in &leaf_dirs {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
    }

    let mut paths = Vec::with_capacity(file_count);
    for index in 0..file_count {
        let dir = &leaf_dirs[index % leaf_dirs.len()];
        let relative = dir.join(format!("f{index}.txt"));
        let mut content = vec![0_u8; file_size];
        rng.fill_bytes(&mut content);
        for byte in &mut content {
            *byte = b'a' + (*byte % 26);
        }
        std::fs::write(root.join(&relative), &content).unwrap();
        paths.push(relative);
    }
    paths
}

fn build_leaf_dirs(
    prefix: PathBuf,
    level: usize,
    depth: usize,
    breadth: usize,
    out: &mut Vec<PathBuf>,
) {
    if level == depth {
        out.push(prefix);
        return;
    }
    for index in 0..breadth {
        build_leaf_dirs(
            prefix.join(format!("d{index}")),
            level + 1,
            depth,
            breadth,
            out,
        );
    }
}

/// Untimed setup: init, generate `file_count` files, genesis-commit them, trust a fixed maintainer
/// key, and seal — establishing a real baseline of the target size before the timing window opens.
/// Returns the generated files' paths for the caller to mutate.
fn setup_baseline_repository(root: &Path, file_count: usize, seed: u64) -> Vec<PathBuf> {
    std::fs::create_dir_all(root).unwrap();
    ok(&prikk(root).arg("init").output().unwrap(), "init");

    let mut rng = SplitMix64::new(seed);
    let files = generate_tree(
        root,
        file_count,
        TREE_BREADTH,
        TREE_DEPTH,
        FILE_SIZE_BYTES,
        &mut rng,
    );

    let out = prikk(root)
        .env("PRIKK_AUTHOR_KEY_ID", FIXED_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(&hex(&FIXED_AUTHOR_SEED)),
        )
        .args(["commit", "-m", "dc59-bench: baseline"])
        .output()
        .unwrap();
    ok(&out, "baseline commit");

    let out = prikk(root)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            FIXED_MAINTAINER_KEY_ID,
            "--public-key",
            &maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
    ok(&out, "trust maintainer add");

    seal_active_wal(root);

    files
}

/// Seal the active WAL using the fixed maintainer key `setup_baseline_repository` already trusted.
/// Shared by the untimed baseline seal and DC-64's consecutive commit+seal cycle axis, which seals
/// after every timed commit (untimed, like the baseline seal) to make the next commit legal.
fn seal_active_wal(root: &Path) {
    let out = prikk(root)
        .env("PRIKK_MAINTAINER_KEY_ID", FIXED_MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&FIXED_MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");
}

/// Mutate the content of the first `count` files in `files` (deterministically, via `rng`),
/// producing text edits against the sealed baseline.
fn mutate_files(root: &Path, files: &[PathBuf], count: usize, rng: &mut SplitMix64) {
    for path in files.iter().take(count) {
        let mut content = std::fs::read(root.join(path)).unwrap();
        content.push(b'\n');
        let mut extra = [0_u8; 16];
        rng.fill_bytes(&mut extra);
        for byte in &mut extra {
            *byte = b'a' + (*byte % 26);
        }
        content.extend_from_slice(&extra);
        std::fs::write(root.join(path), content).unwrap();
    }
}

/// Time the one measured `commit` against an already-baselined, already-mutated repository.
fn time_commit(root: &Path) -> Duration {
    let start = Instant::now();
    let out = prikk(root)
        .env("PRIKK_AUTHOR_KEY_ID", FIXED_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(&hex(&FIXED_AUTHOR_SEED)),
        )
        .args(["commit", "-m", "dc59-bench: measured"])
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    ok(&out, "measured commit");
    elapsed
}

/// Axis C (DC-64): for each repository size, run `CYCLE_COUNT` consecutive mutate+commit+seal
/// cycles against the *same* sample repository, timing each commit separately. Returns, per
/// repository size, one `Point` per cycle number (so `points[size_index][cycle_index].samples` holds
/// that cycle's duration across all samples at that size) — the shape needed to show cycle 1 (cold)
/// against cycles 2+ (warm, incremental) both within one size and across sizes.
///
/// Each cycle mutates a **different** file (`files[cycle_index]`, never `files[0]` repeatedly):
/// editing the *same* text file twice across two separate commits currently fails
/// (`baseline content Blob ... is missing` — a pre-existing defect this axis's development
/// surfaced, reported independently and out of DC-64's scope; see the review-request package).
/// Distinct target files per cycle avoids it while still measuring exactly what Axis A measures —
/// one file changed per timed commit — repeated across a warming cache instead of a fresh repository
/// each time.
fn run_cycle_axis() -> Vec<(usize, Vec<Point>)> {
    let mut by_size = Vec::new();
    for (size, sample_count) in CYCLE_AXIS_SIZES.into_iter().zip(CYCLE_AXIS_SAMPLES) {
        assert!(
            size >= CYCLE_COUNT,
            "repository size must be at least CYCLE_COUNT so each cycle can target a distinct file"
        );
        let mut cycles: Vec<Point> = (0..CYCLE_COUNT)
            .map(|cycle| Point {
                label: format!("cycle {}", cycle + 1),
                samples: Vec::with_capacity(sample_count),
            })
            .collect();
        for sample_index in 0..sample_count {
            let root = unique_dir(&format!("axis-c-{size}-{sample_index}"));
            let seed = CONTENT_SEED
                .wrapping_add(0x4000_0000)
                .wrapping_add(size as u64)
                .wrapping_add(sample_index as u64);
            let files = setup_baseline_repository(&root, size, seed);
            let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
            for (cycle_index, cycle_point) in cycles.iter_mut().enumerate() {
                mutate_files(&root, &files[cycle_index..], 1, &mut rng);
                let elapsed = time_commit(&root);
                cycle_point.samples.push(elapsed);
                seal_active_wal(&root);
            }
            let _ = std::fs::remove_dir_all(&root);
        }
        by_size.push((size, cycles));
    }
    by_size
}

/// Outcome of one memory-measuring commit trial: the peak `VmHWM` observed (if any sample landed
/// while the child was alive), and how many polling attempts succeeded versus were made. A missed
/// sample is `peak_kb: None` — never zero.
// DC-71: only constructed by the Linux-only /proc-based sampling functions below.
#[cfg(target_os = "linux")]
struct MemoryTrial {
    peak_kb: Option<u64>,
    attempts: usize,
    successes: usize,
}

/// Aggregate of one or more `MemoryTrial`s at a fixed repository size: the maximum peak observed
/// across trials (`None` only if every trial's every poll missed), and the total samples obtained
/// versus attempted across all trials at this point.
struct MemoryPoint {
    label: String,
    trials: usize,
    peak_kb: Option<u64>,
    samples_obtained: usize,
    samples_attempted: usize,
}

/// The memory pass's full result: the process floor (`prikk --version`, no repository) and the
/// per-repository-size points, both `MemoryPoint`s so they render with identical fields. Kept as
/// one struct so `render_report` cannot receive one without the other.
struct MemoryAxisResult {
    floor: MemoryPoint,
    points: Vec<MemoryPoint>,
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
/// first, then checking whether it has exited, so a process that exits between iterations still
/// gets one more attempt), then collect its output and assert success under `what`. Shared by
/// `measure_commit_memory` and `measure_process_floor_memory` so both use identical polling logic
/// rather than two independent copies.
#[cfg(target_os = "linux")]
fn measure_process_memory(mut command: Command, interval: Duration, what: &str) -> MemoryTrial {
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
    ok(&output, what);

    MemoryTrial {
        peak_kb,
        attempts,
        successes,
    }
}

/// Time and peak-memory-measure one commit against an already-baselined, already-mutated
/// repository, via `.spawn()` and polling rather than `.output()` (see module docs for why this
/// cannot share `time_commit`'s loop). The elapsed time here is not used for Axis A/B's tables —
/// only the peak memory is retained by the caller.
#[cfg(target_os = "linux")]
fn measure_commit_memory(root: &Path, interval: Duration) -> MemoryTrial {
    let mut command = prikk(root);
    command
        .env("PRIKK_AUTHOR_KEY_ID", FIXED_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(&hex(&FIXED_AUTHOR_SEED)),
        )
        .args(["commit", "-m", "dc59-bench: measured (memory pass)"]);
    measure_process_memory(command, interval, "measured commit (memory pass)")
}

/// Run the memory axis (a process floor, then Axis A's repository sizes at 1 file changed) on
/// Linux; skip cleanly with a notice on every other platform, since `/proc` is Linux-only.
fn run_memory_axis() -> Option<MemoryAxisResult> {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!(
            "skipping peak-memory measurement: /proc/<pid>/status is Linux-only (DC-62; see module docs)"
        );
        None
    }
    #[cfg(target_os = "linux")]
    {
        let mut floor_peak_kb: Option<u64> = None;
        let mut floor_obtained = 0_usize;
        let mut floor_attempted = 0_usize;
        for sample_index in 0..MEMORY_FLOOR_SAMPLES {
            let root = unique_dir(&format!("axis-a-mem-floor-{sample_index}"));
            let seed = CONTENT_SEED
                .wrapping_add(0x3000_0000)
                .wrapping_add(sample_index as u64);
            let files = setup_baseline_repository(&root, MEMORY_FLOOR_FILE_COUNT, seed);
            let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
            mutate_files(&root, &files, 1, &mut rng);
            let trial = measure_commit_memory(&root, MEMORY_SAMPLE_INTERVAL);
            floor_attempted += trial.attempts;
            floor_obtained += trial.successes;
            if let Some(kb) = trial.peak_kb {
                floor_peak_kb = Some(floor_peak_kb.map_or(kb, |current| current.max(kb)));
            }
            let _ = std::fs::remove_dir_all(&root);
        }
        let floor = MemoryPoint {
            label: format!("floor ({MEMORY_FLOOR_FILE_COUNT} file, minimal content)"),
            trials: MEMORY_FLOOR_SAMPLES,
            peak_kb: floor_peak_kb,
            samples_obtained: floor_obtained,
            samples_attempted: floor_attempted,
        };

        let mut points = Vec::new();
        for (size, sample_count) in AXIS_A_SIZES.into_iter().zip(AXIS_A_SAMPLES) {
            let mut peak_kb: Option<u64> = None;
            let mut samples_obtained = 0_usize;
            let mut samples_attempted = 0_usize;
            for sample_index in 0..sample_count {
                let root = unique_dir(&format!("axis-a-mem-{size}-{sample_index}"));
                let seed = CONTENT_SEED
                    .wrapping_add(0x2000_0000)
                    .wrapping_add(size as u64)
                    .wrapping_add(sample_index as u64);
                let files = setup_baseline_repository(&root, size, seed);
                let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
                mutate_files(&root, &files, 1, &mut rng);
                let trial = measure_commit_memory(&root, MEMORY_SAMPLE_INTERVAL);
                samples_attempted += trial.attempts;
                samples_obtained += trial.successes;
                if let Some(kb) = trial.peak_kb {
                    peak_kb = Some(peak_kb.map_or(kb, |current| current.max(kb)));
                }
                let _ = std::fs::remove_dir_all(&root);
            }
            points.push(MemoryPoint {
                label: format!("{size} files"),
                trials: sample_count,
                peak_kb,
                samples_obtained,
                samples_attempted,
            });
        }
        Some(MemoryAxisResult { floor, points })
    }
}

fn spawn_floor_sample() -> Duration {
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .arg("--version")
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(out.status.success());
    elapsed
}

struct Point {
    label: String,
    samples: Vec<Duration>,
}

impl Point {
    fn median(&self) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort();
        sorted[sorted.len() / 2]
    }

    fn min(&self) -> Duration {
        *self.samples.iter().min().unwrap()
    }

    fn max(&self) -> Duration {
        *self.samples.iter().max().unwrap()
    }
}

fn fmt_ms(duration: Duration) -> String {
    format!("{:.2}", duration.as_secs_f64() * 1000.0)
}

fn fmt_us(duration: Duration) -> String {
    format!("{} \u{b5}s", duration.as_micros())
}

fn filesystem_kind(path: &Path) -> String {
    let Ok(output) = Command::new("df").arg("-T").arg(path).output() else {
        return "unknown (df unavailable)".to_owned();
    };
    if !output.status.success() {
        return "unknown (df failed)".to_owned();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().nth(1))
        .map(str::to_owned)
        .unwrap_or_else(|| "unknown (unparsed df output)".to_owned())
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("model name"))
                .and_then(|line| line.split(':').nth(1))
                .map(|value| value.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown (non-Linux or /proc/cpuinfo unavailable)".to_owned())
}

#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn commit_benchmark() {
    let temp_root = std::env::temp_dir();
    eprintln!(
        "filesystem under {}: {}",
        temp_root.display(),
        filesystem_kind(&temp_root)
    );

    let mut spawn_floor = Vec::with_capacity(SPAWN_FLOOR_SAMPLES);
    for _ in 0..SPAWN_FLOOR_SAMPLES {
        spawn_floor.push(spawn_floor_sample());
    }
    let spawn_floor_point = Point {
        label: "spawn floor".to_owned(),
        samples: spawn_floor,
    };

    let mut axis_a = Vec::new();
    for (size, sample_count) in AXIS_A_SIZES.into_iter().zip(AXIS_A_SAMPLES) {
        let mut samples = Vec::with_capacity(sample_count);
        for sample_index in 0..sample_count {
            let root = unique_dir(&format!("axis-a-{size}-{sample_index}"));
            let seed = CONTENT_SEED
                .wrapping_add(size as u64)
                .wrapping_add(sample_index as u64);
            let files = setup_baseline_repository(&root, size, seed);
            let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
            mutate_files(&root, &files, 1, &mut rng);
            samples.push(time_commit(&root));
            let _ = std::fs::remove_dir_all(&root);
        }
        axis_a.push(Point {
            label: format!("{size} files"),
            samples,
        });
    }

    let mut axis_b = Vec::new();
    for change_count in AXIS_B_CHANGE_COUNTS {
        let mut samples = Vec::with_capacity(AXIS_B_SAMPLES);
        for sample_index in 0..AXIS_B_SAMPLES {
            let root = unique_dir(&format!("axis-b-{change_count}-{sample_index}"));
            let seed = CONTENT_SEED
                .wrapping_add(0x1000_0000)
                .wrapping_add(change_count as u64)
                .wrapping_add(sample_index as u64);
            let files = setup_baseline_repository(&root, AXIS_B_REPO_SIZE, seed);
            let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
            mutate_files(&root, &files, change_count, &mut rng);
            samples.push(time_commit(&root));
            let _ = std::fs::remove_dir_all(&root);
        }
        axis_b.push(Point {
            label: format!("{change_count} changed"),
            samples,
        });
    }

    // Axis C (DC-64): consecutive commit+seal cycles, separate from Axis A/B's single-commit
    // methodology since it needs its own repeated-cycle repositories.
    let cycle_axis = run_cycle_axis();

    // Separate pass, deliberately after Axis A/B and never interleaved with them — see module docs.
    let memory_axis = run_memory_axis();

    let report = render_report(
        &spawn_floor_point,
        &axis_a,
        &axis_b,
        &cycle_axis,
        memory_axis.as_ref(),
    );
    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/DC-59-commit-benchmark-harness/benchmark-report-v1.md"
    );
    std::fs::write(report_path, report).unwrap();
    eprintln!("report written to {report_path}");
}

fn render_report(
    spawn_floor: &Point,
    axis_a: &[Point],
    axis_b: &[Point],
    cycle_axis: &[(usize, Vec<Point>)],
    memory_axis: Option<&MemoryAxisResult>,
) -> String {
    let mut out = String::new();
    out.push_str("# DC-59 Commit Benchmark Report v1\n\n");
    out.push_str("Generated by `cargo test -p prikk --locked --test dc59_commit_benchmark -- --ignored --nocapture commit_benchmark`.\n");
    out.push_str("Re-running that exact command regenerates this file. The *numbers* are hardware-dependent; the **shape** of Axis A is the claim under test.\n\n");
    out.push_str("DC-62 added the peak-memory section below as a third, separate pass; DC-64 added Axis C (consecutive commit+seal cycles). Axis A and Axis B above them are produced by DC-59's original, unmodified timing code and are unaffected by either addition.\n\n");

    out.push_str("## Scope\n\n");
    out.push_str("This report states what was measured. It does not conclude whether `prikk commit` complies with NFR-PERF-01 — that determination belongs to DC-56. The peak-memory section states what was measured; it does not conclude whether the footprint is acceptable — that determination also belongs to DC-56.\n\n");

    out.push_str("## Machine and filesystem context\n\n");
    out.push_str(&format!("- CPU: {}\n", cpu_model()));
    out.push_str(&format!(
        "- Temp directory filesystem: {}\n",
        filesystem_kind(&std::env::temp_dir())
    ));
    out.push_str("- Commit includes fsync; NFR-PERF-01 names fsync in its bound, so the filesystem above is part of what these numbers claim.\n\n");

    out.push_str("## Generator parameters\n\n");
    out.push_str(&format!(
        "- File size: {FILE_SIZE_BYTES} bytes, printable pseudo-random content\n"
    ));
    out.push_str(&format!("- Tree shape: breadth {TREE_BREADTH}, depth {TREE_DEPTH} (files distributed round-robin across leaf directories, not concentrated in one directory)\n"));
    out.push_str(&format!("- Content seed: `{CONTENT_SEED:#x}` (SplitMix64, a deliberate duplicate of the generator in `crates/prikk-hash/src/tests/hash_differential.rs`, documented in this file's module docs)\n"));
    out.push_str(&format!("- Fixed author key id: `{FIXED_AUTHOR_KEY_ID}`, seed: `{}` (benchmark material, not a credential)\n", hex(&FIXED_AUTHOR_SEED)));
    out.push_str(&format!("- Fixed maintainer key id: `{FIXED_MAINTAINER_KEY_ID}`, seed: `{}` (benchmark material, not a credential; used only by the untimed setup seal, never by the timed commit)\n\n", hex(&FIXED_MAINTAINER_SEED)));

    out.push_str("## Methodology\n\n");
    out.push_str("Each sample generates a fresh repository, commits its full file set (genesis, untimed), seals it (untimed), mutates the target number of files, then times exactly one subsequent `commit`. Generation, the baseline commit, and the seal are all outside the timing window; only the measured `commit` invocation is timed. A repository is used for exactly one timed trial and then discarded — `commit` cannot run twice against one repository without an intervening seal, and repeating that cycle within a trial would let seal cost contaminate the measurement, so variance instead comes from sampling independently generated repositories per point.\n\n");
    out.push_str("Signing cost: Ed25519 author signing happens inside every timed `commit` and scales with the change set, which is exactly what NFR-PERF-01 permits. It therefore contributes to Axis B's growth, not Axis A's — Axis A holds the change set at 1 file throughout, so any growth there is not attributable to signing.\n\n");

    out.push_str("## Process-spawn floor\n\n");
    out.push_str(&format!(
        "`prikk --version`, {} samples: median {} ms, range {}-{} ms.\n",
        spawn_floor.samples.len(),
        fmt_ms(spawn_floor.median()),
        fmt_ms(spawn_floor.min()),
        fmt_ms(spawn_floor.max()),
    ));
    out.push_str("Every measurement below drives the binary through `Command`, so this floor is included in every figure below as a roughly constant additive offset. It does not hide Axis A's shape but may dominate at the smallest repository size.\n\n");

    out.push_str("## Axis A — cost against repository size, 1 file changed\n\n");
    out.push_str("Repository size varies; exactly 1 of the baseline's files is modified before each timed commit.\n\n");
    out.push_str("| Repository size | Samples | Median (ms) | Min (ms) | Max (ms) |\n");
    out.push_str("|---:|---:|---:|---:|---:|\n");
    for point in axis_a {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            point.label,
            point.samples.len(),
            fmt_ms(point.median()),
            fmt_ms(point.min()),
            fmt_ms(point.max()),
        ));
    }
    out.push('\n');
    out.push_str("If cost grows with repository size here despite the change set staying fixed at 1 file, that growth is not explained by patch construction or signing — both scale with the change set, not the baseline — and points at a full-tree scan.\n\n");

    out.push_str(&format!(
        "## Axis B — cost against changed-file count, fixed {AXIS_B_REPO_SIZE}-file repository\n\n"
    ));
    out.push_str("Repository size is held fixed; the number of modified files varies before each timed commit.\n\n");
    out.push_str("| Changed files | Samples | Median (ms) | Min (ms) | Max (ms) |\n");
    out.push_str("|---:|---:|---:|---:|---:|\n");
    for point in axis_b {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            point.label,
            point.samples.len(),
            fmt_ms(point.median()),
            fmt_ms(point.min()),
            fmt_ms(point.max()),
        ));
    }
    out.push('\n');
    out.push_str("This is the cost NFR-PERF-01 permits: patch construction and signing scale with the change set.\n\n");

    out.push_str("## Axis C (DC-64) — cost across consecutive commit+seal cycles\n\n");
    out.push_str("Axis A's single-commit methodology cannot show DC-64's incremental baseline cache at all: the one commit it times is always the *second* commit against a fresh ref, which is unconditionally a cold-cache full replay — the cache has nothing to be warm from yet. This axis instead carries each sample repository through several consecutive mutate+commit+seal cycles, timing every commit separately, so cycle 1 (cold) and cycles 2+ (warm, incremental) are both visible.\n\n");
    for (size, cycles) in cycle_axis {
        out.push_str(&format!("### {size} files\n\n"));
        out.push_str("| Cycle | Samples | Median (ms) | Min (ms) | Max (ms) |\n");
        out.push_str("|---:|---:|---:|---:|---:|\n");
        for point in cycles {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                point.label,
                point.samples.len(),
                fmt_ms(point.median()),
                fmt_ms(point.min()),
                fmt_ms(point.max()),
            ));
        }
        out.push('\n');
    }
    out.push_str("**Summary — cycle 1 (cold) vs cycles 2+ (warm), median of medians:**\n\n");
    out.push_str("| Repository size | Cycle 1 (ms) | Cycles 2+ median (ms) |\n");
    out.push_str("|---:|---:|---:|\n");
    for (size, cycles) in cycle_axis {
        let cycle_one = cycles
            .first()
            .map_or(0.0, |point| point.median().as_secs_f64() * 1000.0);
        let mut warm: Vec<f64> = cycles
            .iter()
            .skip(1)
            .map(|point| point.median().as_secs_f64() * 1000.0)
            .collect();
        warm.sort_by(f64::total_cmp);
        let warm_median = warm.get(warm.len() / 2).copied().unwrap_or(0.0);
        out.push_str(&format!(
            "| {size} files | {cycle_one:.2} | {warm_median:.2} |\n"
        ));
    }
    out.push('\n');
    out.push_str("**If cycles 2+ stop tracking repository size while cycle 1 still does, the incremental cache is doing its job.** If cycles 2+ still grow with repository size at a rate comparable to cycle 1, the incremental path is either not engaging (falling back to full replay every cycle) or not helping.\n\n");

    out.push_str("## Peak memory (DC-62) — repository size, 1 file changed, separate pass\n\n");
    out.push_str("Same repository sizes and change count as Axis A, but timed and measured by a **separate pass**: each trial `.spawn()`s the commit (rather than `.output()`, as Axis A/B use) and polls `/proc/<pid>/status` for `VmHWM` — the kernel's own peak-RSS high-water mark — ");
    out.push_str(&format!(
        "every {} while the child runs, keeping the maximum observed across all trials at a point. Axis A/B's own timing figures above are produced by the original, unmodified `.output()`-based code and are not affected by this pass.\n\n",
        fmt_us(MEMORY_SAMPLE_INTERVAL)
    ));
    match memory_axis {
        None => {
            out.push_str("**Not measured on this platform.** `/proc/<pid>/status` is Linux-only; this run was on a non-Linux platform, so no peak-memory data is available. Re-run on Linux to populate this section.\n\n");
        }
        Some(result) => {
            let floor_peak_str = result
                .floor
                .peak_kb
                .map_or_else(|| "not measured".to_owned(), |kb| format!("{kb} KB"));
            out.push_str(&format!(
                "**Memory floor:** a real `commit` against a {MEMORY_FLOOR_FILE_COUNT}-file repository (negligible content, {FILE_SIZE_BYTES} bytes) — same code path as the measured points below, not `prikk --version` (tried first and rejected: it exits fast enough that `VmHWM` sampling mostly caught it before the resident set reached its natural size, understating the floor by two to three orders of magnitude). {} trials, samples {}/{}: peak VmHWM {}. This is the fixed process-and-minimal-commit cost folded into every figure below as a roughly constant additive offset, and the reference the **Above floor** column subtracts.\n\n",
                result.floor.trials,
                result.floor.samples_obtained,
                result.floor.samples_attempted,
                floor_peak_str,
            ));
            out.push_str("A missed sample is reported as **not measured**, never as zero: a run shorter than the polling interval may complete before any poll lands, which is expected at the smallest repository sizes and does not mean memory usage was zero.\n\n");
            out.push_str(
                "| Repository size | Trials | Samples obtained / attempted | Peak VmHWM (KB) | Above floor (KB) |\n",
            );
            out.push_str("|---:|---:|---:|---:|---:|\n");
            for point in &result.points {
                let peak = point
                    .peak_kb
                    .map_or_else(|| "not measured".to_owned(), |kb| kb.to_string());
                let above_floor = match (point.peak_kb, result.floor.peak_kb) {
                    (Some(peak_kb), Some(floor_kb)) => peak_kb.saturating_sub(floor_kb).to_string(),
                    _ => "n/a".to_owned(),
                };
                out.push_str(&format!(
                    "| {} | {} | {} / {} | {} | {} |\n",
                    point.label,
                    point.trials,
                    point.samples_obtained,
                    point.samples_attempted,
                    peak,
                    above_floor,
                ));
            }
            out.push('\n');
            out.push_str("**Above floor is the content-proportional component — what DC-56 must flatten.** Read against raw peak VmHWM alone, growth from small to large repository sizes can look sub-linear, because the fixed process floor dominates at small sizes and shrinks in relative share as content grows; the delta against the floor removes that effect. If the **Above floor** column grows roughly linearly with repository size here despite only 1 file changing, that is the O(total worktree bytes) full-tree-read DC-56 exists to eliminate; this report states the measurement, not whether the resulting footprint is acceptable.\n\n");
        }
    }

    out.push_str("## Reproduction\n\n");
    out.push_str("```\ncargo test -p prikk --locked --test dc59_commit_benchmark -- --ignored --nocapture commit_benchmark\n```\n");
    out
}

// ---------------------------------------------------------------------------------------------
// DC-69 §3.4: Axis D — long history, small tree.
//
// A separate, self-contained pass, deliberately never interleaved with `commit_benchmark`'s
// Axis A/B/C or the memory pass above (same precedent as DC-62's memory pass: a distinct question
// gets a distinct trial, not a modification to code another increment's report depends on).
// `commit_benchmark`, `render_report`, and Axis A/B/C above are untouched by this addition.
//
// Every prior axis in this file varies **file count** (repository size) at a short lineage — at
// most `CYCLE_COUNT = 5` sealed generations (Axis C). None isolates **cumulative history** from
// tree size: a repository that has existed for a long time but always stayed small. DC-69 asks
// exactly this question, because `NodeLifecycleState::seen_ids`/`latest_tombstone_by_id` grow with
// total operations ever performed, never with the current tree's size — so if their cost is real,
// it should appear here even though Axis A/B/C's own repositories never grow large enough or live
// long enough to show it.
//
// **Churn, not edits.** `mutate_files` (Axis A/B/C) only edits existing content, which never mints
// or tombstones a node id — `seen_ids` and `latest_tombstone_by_id` would stay flat under it
// regardless of cycle count, proving nothing about this axis's question. Each Axis D generation
// instead deletes the oldest tracked file and creates a new one at a fresh path: live tree size
// stays fixed at `AXIS_D_TREE_SIZE` every generation, while `seen_ids` grows by one and
// `latest_tombstone_by_id` grows by one, per generation, forever.

/// Tree size held fixed across every point on this axis — the variable under test is history
/// length, not repository size, which is exactly what distinguishes this from Axis A/B/C.
const AXIS_D_TREE_SIZE: usize = 20;
/// History depth (sealed generations before the timed commit) at each point.
const AXIS_D_GENERATION_DEPTHS: [usize; 4] = [10, 50, 100, 200];
const AXIS_D_SAMPLES: usize = 3;

/// One churn generation: delete the oldest tracked file, create a new one at a fresh path, commit,
/// seal. Untimed — used for every generation except the last at each depth, which
/// `run_axis_d` times separately via `time_commit`.
fn churn_generation(
    root: &Path,
    files: &mut Vec<PathBuf>,
    next_index: &mut usize,
    rng: &mut SplitMix64,
) {
    let (path, content) = prepare_churn_step(root, files, next_index, rng);
    std::fs::write(root.join(&path), &content).unwrap();
    let out = prikk(root)
        .env("PRIKK_AUTHOR_KEY_ID", FIXED_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(&hex(&FIXED_AUTHOR_SEED)),
        )
        .args(["commit", "-m", "dc69-bench: churn"])
        .output()
        .unwrap();
    ok(&out, "churn commit");
    seal_active_wal(root);
}

/// Shared step for both the untimed churn loop and the final, timed generation: delete the oldest
/// tracked file from disk and the tracking list, choose a fresh path for its replacement, write its
/// content, and return that path so the caller decides how to time the commit. Keeps live tree size
/// at exactly `files.len()` before and after every call.
fn prepare_churn_step(
    root: &Path,
    files: &mut Vec<PathBuf>,
    next_index: &mut usize,
    rng: &mut SplitMix64,
) -> (PathBuf, Vec<u8>) {
    let victim = files.remove(0);
    std::fs::remove_file(root.join(&victim)).unwrap();
    let new_path = PathBuf::from(format!("churn-{next_index}.txt"));
    *next_index += 1;
    let mut content = vec![0_u8; FILE_SIZE_BYTES];
    rng.fill_bytes(&mut content);
    for byte in &mut content {
        *byte = b'a' + (*byte % 26);
    }
    files.push(new_path.clone());
    (new_path, content)
}

/// For each history depth, run `AXIS_D_SAMPLES` independent repositories through `depth - 1`
/// untimed churn generations, then time exactly the final (depth-th) commit. Comparing that timed
/// commit's cost **across depths**, with tree size fixed at `AXIS_D_TREE_SIZE` throughout, is what
/// isolates cumulative-history cost from repository-size cost.
fn run_axis_d() -> Vec<Point> {
    let mut points = Vec::with_capacity(AXIS_D_GENERATION_DEPTHS.len());
    for &depth in &AXIS_D_GENERATION_DEPTHS {
        let mut samples = Vec::with_capacity(AXIS_D_SAMPLES);
        for sample_index in 0..AXIS_D_SAMPLES {
            let root = unique_dir(&format!("axis-d-{depth}-{sample_index}"));
            let seed = CONTENT_SEED
                .wrapping_add(0x6000_0000)
                .wrapping_add(depth as u64)
                .wrapping_add(sample_index as u64);
            let mut files = setup_baseline_repository(&root, AXIS_D_TREE_SIZE, seed);
            let mut rng = SplitMix64::new(seed ^ 0xFFFF_FFFF_0000_0000);
            let mut next_index = AXIS_D_TREE_SIZE;
            for _ in 0..depth.saturating_sub(1) {
                churn_generation(&root, &mut files, &mut next_index, &mut rng);
            }
            let (path, content) = prepare_churn_step(&root, &mut files, &mut next_index, &mut rng);
            std::fs::write(root.join(&path), &content).unwrap();
            samples.push(time_commit(&root));
            let _ = std::fs::remove_dir_all(&root);
        }
        points.push(Point {
            label: format!("{depth} generations"),
            samples,
        });
    }
    points
}

fn render_axis_d_report(points: &[Point]) -> String {
    let mut out = String::new();
    out.push_str("# DC-69 §3.4 — Axis D: Cost at Long History, Small Tree\n\n");
    out.push_str(&format!(
        "Tree size held fixed at **{AXIS_D_TREE_SIZE} files** across every point. The varying \
         quantity is history depth: the number of sealed churn generations (delete oldest tracked \
         file, create one new file at a fresh path — net live tree size unchanged) before the \
         timed commit. `{AXIS_D_SAMPLES}` independent repositories per depth. See \
         `crates/prikk-cli/tests/dc59_commit_benchmark.rs`'s DC-69 section for the full method and \
         why churn (not edits) is required to exercise `seen_ids`/`latest_tombstone_by_id` growth.\n\n",
    ));
    out.push_str("| History depth | Live tree size | Median | Min | Max |\n");
    out.push_str("|---:|---:|---:|---:|---:|\n");
    for point in points {
        out.push_str(&format!(
            "| {} | {AXIS_D_TREE_SIZE} files | {} ms | {} ms | {} ms |\n",
            point.label,
            fmt_ms(point.median()),
            fmt_ms(point.min()),
            fmt_ms(point.max()),
        ));
    }
    out.push('\n');
    out.push_str(
        "**Reading this table:** if the timed commit's cost at depth 200 is materially higher \
         than at depth 10, with live tree size identical (20 files) at every row, that cost is \
         attributable to cumulative history — `seen_ids`/`latest_tombstone_by_id`'s unbounded \
         growth — not to repository size, which no prior DC-59/62/64 axis isolates.\n\n",
    );
    out.push_str("## Reproduction\n\n");
    out.push_str(
        "```\ncargo test -p prikk --locked --test dc59_commit_benchmark -- --ignored --nocapture axis_d_long_history_small_tree\n```\n",
    );
    out
}

#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn axis_d_long_history_small_tree() {
    let points = run_axis_d();
    let report = render_axis_d_report(&points);
    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/DC-69-lifecycle-state-retention/axis-d-benchmark-report-v1.md"
    );
    std::fs::write(report_path, report).unwrap();
    eprintln!("report written to {report_path}");
}
