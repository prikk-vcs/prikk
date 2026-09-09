//! RFC 133 §6b.3 step 1 — incremental commit memory against node count.
//!
//! **This instrument measures. It optimises nothing** (the handoff's own words). §2's genesis-vs-
//! file-count series read "flat" from three points, but its own fourth point (8,000 -> 21.7 MiB, a
//! 55% jump for a 2x path-count increase while content grew only 1 MiB) departs from that reading —
//! §6a.3 found the series stops exactly where it departs. This harness re-takes those points with
//! more than one sample each, extends well past 8,000, and — the axis §2 never measured at all —
//! does the same for the **incremental** commit, which is `NFR-PERF-01`'s in-scope steady-state
//! property, not genesis.
//!
//! ## Why `getrusage(RUSAGE_CHILDREN)`, not `/proc/<pid>/status` `VmHWM` — and why via Python
//!
//! `dc59_commit_benchmark.rs`'s own memory pass polls `/proc/<pid>/status` because `.output()`
//! cannot read anything from a child after it exits and `rustix` (this workspace's only fs-syscall
//! dependency) does not expose `getrusage`/`wait4`. That polling can miss the peak of a run shorter
//! than the sampling interval. `getrusage(RUSAGE_CHILDREN)` has no such gap: the kernel accumulates
//! it at `wait()` time regardless of how long the child ran, which is why the handoff requires it
//! specifically ("an exact kernel figure, not `dc59`'s sampled `VmHWM`").
//!
//! **Calling it from this crate's own Rust would need `unsafe` — the workspace forbids that**
//! (`unsafe_code = "forbid"` at the workspace `[lints]` table; only `prikk-ffi` is exempted, for its
//! own narrow, reviewed reason, and this is not that). So this harness shells out to
//! `tests/support/rusage_child.py` (see its own doc comment): a few lines over Python's standard-
//! library `resource` module, which wraps the identical syscall safely on Python's own side. This
//! is a new environmental prerequisite (`python3`), not a new Cargo dependency and not a lint
//! exemption — the same category of documented, deliberate-instrument-only prerequisite `/proc`
//! already is for `dc59`'s own memory pass (DC-62).
//!
//! ## Why one process per measured commit
//!
//! `RUSAGE_CHILDREN`'s `ru_maxrss` is a **running maximum across every child the calling process has
//! ever reaped**, not the last child's own figure. A harness that spawned `init`, a baseline commit,
//! `trust`, `seal`, *and* the measured commit all from one process — Python or Rust — would have the
//! measured commit's reading silently floored by whichever earlier step used more memory, worse,
//! contaminated across repository sizes as the loop moves from a small `N` to a large one, since the
//! maximum never resets. This is exactly what the handoff's own §2.1 method note guards against
//! ("measured in a fresh process so `RUSAGE_CHILDREN`'s running maximum could not contaminate it").
//! `rusage_child.py`'s own lifetime is "start, spawn exactly one child, wait, read, exit" — a fresh
//! interpreter process per invocation, invoked once per sample here. All *untimed* setup (`init`,
//! the baseline commit, `trust`, `seal`) still runs directly in this harness's own process via
//! `support`'s helpers; nothing ever reads *this* process's own `RUSAGE_CHILDREN`, so it does not
//! matter that they share it with each other.
//!
//! ## What is measured, and how it maps to §6b.1's decomposition
//!
//! Two series, at the same node-count points, reported separately (never merged, per the handoff):
//!
//! - **Genesis**: `init`, generate `N` 256-byte files on disk (untimed), then the measured commit
//!   *is* the genesis commit. There is no published baseline yet, so `resolve_baseline_state`
//!   (`lifecycle_cache/incremental.rs`) is never called on this path — confirmed empirically (no
//!   `lifecycle-state.v1` file exists after a genesis commit) — so this series has no cache-file
//!   control; only peak RSS.
//! - **Incremental**: `init`, generate `N` files, commit, trust, seal (all untimed, via
//!   `support::init`/`support::commit`/`support::seal`), then mutate exactly one file and measure
//!   the second commit. This is `NFR-PERF-01`'s in-scope property. Peak RSS **and** the persisted
//!   `.prikk/cache/lifecycle-state.v1` file's byte size are recorded (§4's REQUIRED control): if the
//!   cache file grows with `N` on the same shape as RSS, the state structure (§6b.1's
//!   `NodeLifecycleState`) is implicated; if RSS departs while the cache file does not, the cause is
//!   elsewhere and the `seen_ids` hypothesis (§6b.2) is not it. Agreement between the two series is
//!   reported as *consistent with* the hypothesis, never as proof of it, per the handoff's own
//!   instruction.
//!
//! File size is held at the constant §2 used (256 bytes) so only the node-count axis moves.
//!
//! ## §6c.2 — attribution (`rfc133_node_count_memory_attribution`)
//!
//! Step 1's own control found that `lifecycle-state.v1` is linear in `N` from the very first point
//! with **no** departure, and is ~11x smaller per node than the RSS growth it was checked against —
//! so it does not explain either the shape or the magnitude of the departure. This second driver
//! test (same file, same shared helpers, per the attribution handoff's own instruction not to build
//! a third instrument) measures three named candidates and reports what is left over:
//!
//! - **(a) `NodeLifecycleState`'s own in-memory cost, isolated.** `lifecycle_state_probe_worker`
//!   (below) is a second self-reexec worker, invoked the same way the measured `commit` is (a fresh
//!   process per sample, via `rusage_child.py`) — except what it does is build a synthetic state of
//!   exactly `N` live nodes through the type's own public `create_node` API
//!   (`prikk_replay::NodeLifecycleState`, hence this file's new `[dev-dependencies]` edge) and exit.
//!   Nothing else runs in that process, so its peak RSS *is* the structure's own in-memory cost —
//!   measured against a same-process `N=0` floor, never described as the structure's theoretical
//!   packed size (the handoff's own caution: allocator behaviour, page granularity, and
//!   `BTreeMap`/`BTreeSet` node overhead all sit between "bytes the structure logically needs" and
//!   "RSS the process shows", and this measures the latter, honestly).
//! - **(b) `.prikk/cache/commit-index.v1`'s own growth.** Read off the same measured commits step 1's
//!   own incremental series already runs — no new repositories, no new commits.
//! - **(c) A third grower, found by sweeping rather than assumed absent.** `containers/index.container`
//!   (`FileObjectStore`'s own object index, `foundation/index.rs`) is loaded **wholesale** into a
//!   `Vec<IndexEntry>` on every write session (`object_store.rs`'s `IndexSnapshot::open`) — confirmed
//!   at source, the same standard `AUD-01` was originally read at — and its file size scales with `N`
//!   too. `IndexEntry` itself is `pub(crate)` inside `prikk-store`; reaching it from here would need
//!   either a production visibility change (forbidden this round) or a second instrument inside that
//!   crate (disproportionate for a sweep), so this gets the same treatment (b) gets — a real measured
//!   file-size series — not an isolated RSS probe. A full recursive walk of `.prikk/` after one
//!   measured commit at each `N` (`sweep_prikk_tree`) backs the claim that nothing else scales
//!   meaningfully beyond what is named here, rather than leaving it assumed.
//!
//! **The residual is computed honestly, not maximally.** Only (a) is RSS-commensurate (both sides
//! are `getrusage` readings), so only (a) is subtracted from measured RSS growth to produce the
//! residual row. (b) and (c) are disk-byte series, reported alongside for context, never subtracted
//! from an RSS quantity as though the two units were interchangeable without further evidence.
//!
//! ## Running it
//!
//! **Must be run with `--release`** — the handoff requires a release build, matching §2.1's own
//! method, and this is a real behaviour difference: a debug build's allocator/bounds-checking
//! overhead would not be comparable to §2's figures at all. Needs `python3` on `PATH` (see above)
//! and, like `dc59`'s own memory pass, is Linux-only (`resource.getrusage` is POSIX and exists on
//! more than Linux, but this harness is not verified anywhere else and skips cleanly elsewhere).
//!
//! ```text
//! cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory
//! cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_attribution
//! ```
//!
//! `#[ignore]`d: these are measurement instruments, not correctness tests, and their dominant cost
//! (repositories up to tens of thousands of files, three samples per point) does not belong in the
//! default suite.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use prikk_object::{NodeId, NodeKind, ObjectId};
use prikk_replay::{LiveNode, NodeContent, NodeLifecycleState, RepoPath};

mod support;

/// Node-count points. §2's original series stopped at 8,000, exactly where §6a.3 found it departs
/// from "flat" — this extends past it as the handoff requires (minimum 16,000/32,000) and one point
/// further (64,000) since build time allowed reaching it. **128,000 was assessed and not attempted**:
/// a single-sample probe at 64,000 (genesis commit alone) took ~59.5s against ~9s at 32,000 — a
/// ~6.6x cost increase for a 2x node-count increase, non-linear on its own. Extrapolating that same
/// ratio, one full 128,000 point (2 series x 3 samples x up to 3 timed/untimed steps each) would run
/// to the order of an hour for this one point alone. Reported here, not attempted, per the handoff's
/// own "report the wall and the cost that stopped you rather than extrapolating past it."
const NODE_COUNTS: [usize; 7] = [100, 1_000, 4_000, 8_000, 16_000, 32_000, 64_000];

/// At least 3, per the handoff — "a single number per point is what produced the reading now in
/// doubt."
const SAMPLES_PER_POINT: usize = 3;

const FILE_SIZE_BYTES: usize = 256;
const TREE_BREADTH: usize = 8;
const TREE_DEPTH: usize = 3;

/// Deterministic content seed, distinct from `dc59_commit_benchmark.rs`'s own `CONTENT_SEED` and
/// `tools/corpus`'s `generator_seed`s -- this generator produces this harness's own worktree
/// content and should not be confused with either.
const CONTENT_SEED: u64 = 0x1333_0006_b3b3_0001;

/// Deliberate duplicate of the `SplitMix64` already copied independently in
/// `dc59_commit_benchmark.rs` and `crates/prikk-hash/src/tests/hash_differential.rs`. Neither is
/// reachable from an integration test in a different compiled test binary; see either's own module
/// doc for why a third small, reviewed copy is preferred over a new shared dependency for this.
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

fn unique_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "prikk-rfc133-node-count-{tag}-{}",
        support::unique_suffix()
    ));
    dir
}

/// Deterministically populate `root` with `file_count` files across a directory tree of fixed
/// breadth/depth (round-robin leaf assignment, same shape `dc59_commit_benchmark.rs` uses), each
/// `FILE_SIZE_BYTES` bytes of printable pseudo-random content. Returns the created files' paths
/// relative to `root`, in creation order.
fn generate_tree(root: &Path, file_count: usize, rng: &mut SplitMix64) -> Vec<PathBuf> {
    let mut leaf_dirs = Vec::new();
    build_leaf_dirs(PathBuf::new(), 0, TREE_DEPTH, TREE_BREADTH, &mut leaf_dirs);
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
        let mut content = vec![0_u8; FILE_SIZE_BYTES];
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

/// Mutate the first file in `files` deterministically, producing a text edit against the sealed
/// baseline -- the "change one file" step both `dc59_commit_benchmark.rs`'s Axis A and this
/// harness's incremental series hold fixed while `N` moves.
fn mutate_one_file(root: &Path, files: &[PathBuf], rng: &mut SplitMix64) {
    let path = &files[0];
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

fn cache_file_size(root: &Path) -> Option<u64> {
    std::fs::metadata(root.join(".prikk/cache/lifecycle-state.v1"))
        .ok()
        .map(|metadata| metadata.len())
}

/// §6c.2(b): `.prikk/cache/commit-index.v1`, DC-56's own changed-path cache -- loaded wholesale by
/// `CommitIndex::load` on every commit (`commit_index.rs`), read off the same measured commit
/// `cache_file_size` reads its own file from, no new repository or commit needed.
fn commit_index_file_size(root: &Path) -> Option<u64> {
    std::fs::metadata(root.join(".prikk/cache/commit-index.v1"))
        .ok()
        .map(|metadata| metadata.len())
}

/// §6c.2(c): `.prikk/containers/index.container`, the object store's own location index --
/// confirmed at source (`object_store.rs`'s `IndexSnapshot::open`) to be decoded wholesale into a
/// `Vec<IndexEntry>` on every write session, the found-by-sweeping third candidate this round adds.
fn object_index_file_size(root: &Path) -> Option<u64> {
    std::fs::metadata(root.join(".prikk/containers/index.container"))
        .ok()
        .map(|metadata| metadata.len())
}

/// §3's own "sweep for a third": every file under `.prikk/`, path (relative to `.prikk/`) to byte
/// size. Backs the claim that (a)/(b)/(c) are not an arbitrarily curated list -- the report compares
/// this snapshot at the smallest and largest `N` and names every path whose size moved.
fn sweep_prikk_tree(root: &Path) -> BTreeMap<String, u64> {
    fn walk(dir: &Path, prikk_root: &Path, out: &mut BTreeMap<String, u64>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, prikk_root, out);
            } else if let Ok(metadata) = entry.metadata() {
                let relative = path
                    .strip_prefix(prikk_root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                out.insert(relative, metadata.len());
            }
        }
    }
    let mut out = BTreeMap::new();
    let prikk_dir = root.join(".prikk");
    walk(&prikk_dir, &prikk_dir, &mut out);
    out
}

const RUSAGE_CHILD_SCRIPT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rusage_child.py");

/// Spawn `binary args...` at `cwd` via `rusage_child.py` (see module docs -- one fresh process per
/// measurement, so `RUSAGE_CHILDREN`'s running maximum never carries between samples), with `envs`
/// set on the `python3` invocation so they are inherited by whatever it spawns. Returns the child's
/// peak `RUSAGE_CHILDREN` RSS in KiB. Shared by the measured-commit and lifecycle-state-probe
/// measurements below -- one plumbing implementation, not two.
fn run_rusage_child(cwd: &Path, binary: &Path, args: &[&str], envs: &[(&str, &str)]) -> i64 {
    let mut command = Command::new("python3");
    command
        .arg(RUSAGE_CHILD_SCRIPT)
        .arg(cwd)
        .arg(binary)
        .args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .expect("spawning rusage_child.py -- is python3 on PATH?");
    assert!(
        output.status.success(),
        "measured child failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|err| {
            panic!(
                "rusage_child.py stdout {:?} is not an integer: {err}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
}

/// Run one measured `prikk commit --ref <ref_name> -m <message>` against `root` and return its peak
/// `RUSAGE_CHILDREN` RSS in KiB.
fn measure_commit_rss_kib(root: &Path, ref_name: &str, message: &str) -> i64 {
    run_rusage_child(
        root,
        Path::new(env!("CARGO_BIN_EXE_prikk")),
        &["commit", "--ref", ref_name, "-m", message],
        &[
            ("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID),
            ("PRIKK_AUTHOR_SEED", support::AUTHOR_SEED_HEX),
        ],
    )
}

/// Build a synthetic `NodeLifecycleState` with exactly `node_count` live text-file nodes, through
/// the type's own public `create_node` API (`prikk_replay::node_lifecycle::mutation`) -- the same
/// entry point real replay uses, not a second, ad hoc way to populate the structure. Content is
/// synthetic (a deterministic, non-zero `NodeId`/`ObjectId` pair per index) since this measures the
/// state's own bookkeeping cost, not blob content, which is never held here.
fn build_synthetic_lifecycle_state(node_count: usize) -> NodeLifecycleState {
    let mut state = NodeLifecycleState::new();
    for index in 0..node_count {
        let counter = (index as u64) + 1; // NodeId::from_bytes([0; 32]) is the reserved zero id
        let mut id_bytes = [0_u8; 32];
        id_bytes[..8].copy_from_slice(&counter.to_le_bytes());
        let node_id = NodeId::from_bytes(id_bytes);
        let mut blob_bytes = [0_u8; 32];
        blob_bytes[..8].copy_from_slice(&counter.to_le_bytes());
        blob_bytes[8] = 0x01; // distinct from node_id's own bytes; otherwise immaterial
        let blob_id = ObjectId::from_bytes(blob_bytes);
        let path = RepoPath::parse(&format!("f{index}.txt")).expect("valid synthetic path");
        let node = LiveNode {
            path,
            kind: NodeKind::TextFile,
            content: NodeContent::File {
                blob_id,
                mode: 0o100644,
            },
        };
        state
            .create_node(node_id, node)
            .expect("synthetic node creation must succeed");
    }
    state
}

/// Fresh-process worker for §6c.2(a)'s isolated `NodeLifecycleState` probe: does nothing unless
/// `PRIKK_LIFECYCLE_PROBE_NODE_COUNT` is set, in which case it builds a synthetic state of exactly
/// that many nodes and exits. `rusage_child.py` measures this process's own peak RSS from the
/// outside (see module docs) -- this function never reads `getrusage` itself, and needs no `unsafe`.
#[test]
#[ignore = "internal worker process for rfc133_node_count_memory_attribution; never run directly"]
fn lifecycle_state_probe_worker() {
    let Ok(node_count) = std::env::var("PRIKK_LIFECYCLE_PROBE_NODE_COUNT") else {
        return;
    };
    let node_count: usize = node_count
        .parse()
        .expect("PRIKK_LIFECYCLE_PROBE_NODE_COUNT must be a non-negative integer");
    let state = build_synthetic_lifecycle_state(node_count);
    // The peak RSS this process reaches while building `state` is what the caller measures; whether
    // `state` is still alive at the moment the process actually exits does not change that peak
    // (freed heap pages are not typically returned to the OS immediately). `black_box` only prevents
    // the optimizer from proving the loop above has no observable effect and eliding it entirely.
    std::hint::black_box(&state);
}

/// Run the lifecycle-state probe worker for `node_count` (`0` is the floor: same process startup,
/// zero synthetic nodes) and return its peak RSS in KiB.
fn measure_lifecycle_state_probe_rss_kib(node_count: usize) -> i64 {
    let current_exe = std::env::current_exe().expect("this test binary's own path");
    let node_count_arg = node_count.to_string();
    // The worker touches no files, so any valid directory works as its `cwd`.
    run_rusage_child(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &current_exe,
        &["lifecycle_state_probe_worker", "--exact", "--ignored"],
        &[("PRIKK_LIFECYCLE_PROBE_NODE_COUNT", &node_count_arg)],
    )
}

// ---- Reporting. ----

struct RssSeries {
    node_count: usize,
    peak_kib: Vec<i64>,
}

impl RssSeries {
    fn min(&self) -> i64 {
        *self.peak_kib.iter().min().unwrap()
    }
    fn median(&self) -> i64 {
        let mut sorted = self.peak_kib.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }
    fn max(&self) -> i64 {
        *self.peak_kib.iter().max().unwrap()
    }
}

struct CacheSeries {
    node_count: usize,
    bytes: Vec<u64>,
}

impl CacheSeries {
    fn min(&self) -> u64 {
        *self.bytes.iter().min().unwrap()
    }
    fn median(&self) -> u64 {
        let mut sorted = self.bytes.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }
    fn max(&self) -> u64 {
        *self.bytes.iter().max().unwrap()
    }
}

fn git_revision() -> String {
    Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn render_report(
    genesis: &[RssSeries],
    incremental_rss: &[RssSeries],
    incremental_cache: &[CacheSeries],
) -> String {
    let mut out = String::new();
    out.push_str("# RFC 133 §6b.3 step 1 — node-count memory measurement, report v1\n\n");
    out.push_str("Generated by `cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory`.\n");
    out.push_str("Re-running that exact command regenerates this file. The *numbers* are hardware-dependent; the **shape** of each series is the claim under test.\n\n");
    out.push_str(&format!("Revision measured at: `{}`. Release build, Linux, worktrees under a `tmpfs` temp directory, peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` (via `tests/support/rusage_child.py`, since this workspace forbids unsafe Rust), each measured commit in a fresh process. {SAMPLES_PER_POINT} samples per point.\n\n", git_revision()));

    out.push_str(
        "## Genesis series (secondary — for continuity with §2, not the in-scope property)\n\n",
    );
    out.push_str("The measured commit *is* the first commit; no published baseline exists yet, so the incremental lifecycle cache is never touched on this path (confirmed empirically — no `lifecycle-state.v1` file after a genesis commit).\n\n");
    out.push_str("| N | min (KiB) | median (KiB) | max (KiB) |\n|---|---|---|---|\n");
    for series in genesis {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## Incremental series (the in-scope property — `NFR-PERF-01` bounds steady-state, not genesis)\n\n");
    out.push_str("Repository already committed and sealed at N nodes, then exactly one file changed and committed. Peak RSS:\n\n");
    out.push_str("| N | min (KiB) | median (KiB) | max (KiB) |\n|---|---|---|---|\n");
    for series in incremental_rss {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## §4's REQUIRED control — persisted incremental-cache file size (`.prikk/cache/lifecycle-state.v1`)\n\n");
    out.push_str("A direct measurement of materialized `NodeLifecycleState` size, free of allocator behaviour, page granularity, and RSS noise. Recorded after the same measured commit the RSS table above reports:\n\n");
    out.push_str("| N | min (bytes) | median (bytes) | max (bytes) |\n|---|---|---|---|\n");
    for series in incremental_cache {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }
    out.push('\n');

    out
}

/// A snapshot of every file under `.prikk/` at one `N` (`sweep_prikk_tree`'s own output), kept
/// alongside the node count it was taken at so the report can compare the smallest and largest.
struct SweepSnapshot {
    node_count: usize,
    sizes: BTreeMap<String, u64>,
}

#[allow(clippy::too_many_arguments)]
fn render_attribution_report(
    floor_kib: i64,
    isolated_floor_kib: i64,
    incremental_rss: &[RssSeries],
    isolated_probe: &[RssSeries],
    commit_index: &[CacheSeries],
    object_index: &[CacheSeries],
    sweep_smallest: &SweepSnapshot,
    sweep_largest: &SweepSnapshot,
) -> String {
    let mut out = String::new();
    out.push_str("# RFC 133 §6c.2 — node-count memory attribution, report v1\n\n");
    out.push_str("Generated by `cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_attribution`.\n");
    out.push_str("Re-running that exact command regenerates this file. Follows step 1's own report (`node-count-memory-measurement-report-v1.md`); same method, same ladder.\n\n");
    out.push_str(&format!("Revision measured at: `{}`. Release build, Linux, worktrees under a `tmpfs` temp directory, peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` (via `tests/support/rusage_child.py`). {SAMPLES_PER_POINT} samples per point.\n\n", git_revision()));

    out.push_str("## Attribution table — the residual is the result\n\n");
    out.push_str(&format!(
        "Baseline (floor): incremental RSS at N=100, median = {floor_kib} KiB. Isolated-probe floor \
         (N=0 synthetic nodes, same process shape): median = {isolated_floor_kib} KiB. Only (a) is \
         subtracted from RSS growth below -- it is the only column in the same units \
         (`getrusage` KiB); (b)/(c) are disk-byte series, shown separately, never assumed \
         RSS-equivalent.\n\n"
    ));
    out.push_str("| N | RSS growth (KiB) | (a) isolated NodeLifecycleState growth (KiB) | residual = RSS growth − (a) (KiB) | (b) commit-index.v1 (bytes) | (c) object index.container (bytes) |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for node_count in NODE_COUNTS {
        let rss = incremental_rss
            .iter()
            .find(|series| series.node_count == node_count)
            .unwrap();
        let isolated = isolated_probe
            .iter()
            .find(|series| series.node_count == node_count)
            .unwrap();
        let commit_idx = commit_index
            .iter()
            .find(|series| series.node_count == node_count)
            .unwrap();
        let object_idx = object_index
            .iter()
            .find(|series| series.node_count == node_count)
            .unwrap();
        let rss_growth = rss.median() - floor_kib;
        let isolated_growth = isolated.median() - isolated_floor_kib;
        let residual = rss_growth - isolated_growth;
        out.push_str(&format!(
            "| {node_count} | {rss_growth} | {isolated_growth} | {residual} | {} | {} |\n",
            commit_idx.median(),
            object_idx.median(),
        ));
    }

    out.push_str("\n## (a) Isolated `NodeLifecycleState` probe — full series\n\n");
    out.push_str("Fresh process per sample, builds exactly N synthetic live nodes via `create_node` and exits; nothing else runs in that process. `N=0` is the same-shape floor.\n\n");
    out.push_str("| N | min (KiB) | median (KiB) | max (KiB) |\n|---|---|---|---|\n");
    out.push_str(&format!("| 0 (floor) | — | {isolated_floor_kib} | — |\n"));
    for series in isolated_probe {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## (c) Object index (`.prikk/containers/index.container`) — full series\n\n");
    out.push_str("`IndexSnapshot::open` (`object_store.rs`) decodes this file into a `Vec<IndexEntry>` on every write session -- confirmed at source, not assumed.\n\n");
    out.push_str("| N | min (bytes) | median (bytes) | max (bytes) |\n|---|---|---|---|\n");
    for series in object_index {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## §3's sweep — every file under `.prikk/`, smallest N vs largest N\n\n");
    out.push_str(&format!(
        "N={} vs N={}. Every path present in either snapshot; `-` means the path did not exist at \
         that N (its container pair had not been allocated yet).\n\n",
        sweep_smallest.node_count, sweep_largest.node_count
    ));
    out.push_str("| path | size at smallest N | size at largest N | ratio |\n|---|---|---|---|\n");
    let mut paths: std::collections::BTreeSet<&String> = sweep_smallest.sizes.keys().collect();
    paths.extend(sweep_largest.sizes.keys());
    for path in paths {
        let small = sweep_smallest.sizes.get(path).copied();
        let large = sweep_largest.sizes.get(path).copied();
        let ratio = match (small, large) {
            (Some(s), Some(l)) if s > 0 => format!("{:.1}x", l as f64 / s as f64),
            (Some(0), Some(l)) if l > 0 => "0->nonzero".to_string(),
            _ => "-".to_string(),
        };
        out.push_str(&format!(
            "| {path} | {} | {} | {ratio} |\n",
            small.map_or("-".to_string(), |v| v.to_string()),
            large.map_or("-".to_string(), |v| v.to_string()),
        ));
    }
    out.push('\n');

    out
}

#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn rfc133_node_count_memory() {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!(
            "skipping node-count memory measurement: verified on Linux only, matching dc59's own \
             memory pass (DC-62); see module docs"
        );
        return;
    }
    #[cfg(target_os = "linux")]
    rfc133_node_count_memory_linux();
}

#[cfg(target_os = "linux")]
fn rfc133_node_count_memory_linux() {
    if !Path::new(RUSAGE_CHILD_SCRIPT).exists() {
        panic!("rusage_child.py not found at {RUSAGE_CHILD_SCRIPT}");
    }
    let probe = Command::new("python3").arg("--version").output();
    if probe.is_err() || !probe.unwrap().status.success() {
        eprintln!(
            "skipping node-count memory measurement: python3 is not on PATH (see module docs -- \
             this instrument shells out to it for getrusage(RUSAGE_CHILDREN), since this \
             workspace forbids unsafe Rust)"
        );
        return;
    }

    let mut genesis_series = Vec::new();
    for &node_count in &NODE_COUNTS {
        let mut peak_kib = Vec::with_capacity(SAMPLES_PER_POINT);
        for sample_index in 0..SAMPLES_PER_POINT {
            let root = unique_dir(&format!("genesis-{node_count}-{sample_index}"));
            std::fs::create_dir_all(&root).unwrap();
            support::init(&root);
            let seed = CONTENT_SEED
                .wrapping_add(node_count as u64)
                .wrapping_add(sample_index as u64);
            let mut rng = SplitMix64::new(seed);
            generate_tree(&root, node_count, &mut rng);
            let kib = measure_commit_rss_kib(&root, "heads/main", "rfc133-bench: genesis");
            peak_kib.push(kib);
            let _ = std::fs::remove_dir_all(&root);
        }
        eprintln!("genesis N={node_count}: {peak_kib:?} KiB");
        genesis_series.push(RssSeries {
            node_count,
            peak_kib,
        });
    }

    let mut incremental_rss_series = Vec::new();
    let mut incremental_cache_series = Vec::new();
    for &node_count in &NODE_COUNTS {
        let mut peak_kib = Vec::with_capacity(SAMPLES_PER_POINT);
        let mut cache_bytes = Vec::with_capacity(SAMPLES_PER_POINT);
        for sample_index in 0..SAMPLES_PER_POINT {
            let root = unique_dir(&format!("incremental-{node_count}-{sample_index}"));
            std::fs::create_dir_all(&root).unwrap();
            support::init(&root);
            let seed = CONTENT_SEED
                .wrapping_add(0x8000_0000)
                .wrapping_add(node_count as u64)
                .wrapping_add(sample_index as u64);
            let mut rng = SplitMix64::new(seed);
            let files = generate_tree(&root, node_count, &mut rng);
            support::ok(
                &support::commit(&root, "heads/main", "rfc133-bench: baseline"),
                "baseline commit",
            );
            support::ok(&support::seal(&root, "heads/main"), "baseline seal");
            mutate_one_file(&root, &files, &mut rng);

            let kib = measure_commit_rss_kib(&root, "heads/main", "rfc133-bench: incremental");
            peak_kib.push(kib);
            cache_bytes.push(cache_file_size(&root).unwrap_or_else(|| {
                panic!(
                    "no lifecycle-state.v1 cache file after an incremental commit at N={node_count}"
                )
            }));
            let _ = std::fs::remove_dir_all(&root);
        }
        eprintln!("incremental N={node_count}: RSS {peak_kib:?} KiB, cache {cache_bytes:?} bytes");
        incremental_rss_series.push(RssSeries {
            node_count,
            peak_kib,
        });
        incremental_cache_series.push(CacheSeries {
            node_count,
            bytes: cache_bytes,
        });
    }

    let report = render_report(
        &genesis_series,
        &incremental_rss_series,
        &incremental_cache_series,
    );
    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/133-performance-cost-and-its-evidence/node-count-memory-measurement-report-v1.md"
    );
    std::fs::write(report_path, report).unwrap();
    eprintln!("report written to {report_path}");
}

/// §6c.2 — attribution. Does not re-run step 1's genesis series (out of scope here); reuses the
/// incremental series' own repositories and measured commits to also read (b)/(c), and adds the
/// isolated `NodeLifecycleState` probe series (a). See module docs for what each column means and
/// why only (a) is subtracted from RSS growth in the residual.
#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn rfc133_node_count_memory_attribution() {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!(
            "skipping node-count memory attribution: verified on Linux only, matching step 1's own \
             instrument; see module docs"
        );
        return;
    }
    #[cfg(target_os = "linux")]
    rfc133_node_count_memory_attribution_linux();
}

#[cfg(target_os = "linux")]
fn rfc133_node_count_memory_attribution_linux() {
    if !Path::new(RUSAGE_CHILD_SCRIPT).exists() {
        panic!("rusage_child.py not found at {RUSAGE_CHILD_SCRIPT}");
    }
    let probe = Command::new("python3").arg("--version").output();
    if probe.is_err() || !probe.unwrap().status.success() {
        eprintln!(
            "skipping node-count memory attribution: python3 is not on PATH (see module docs)"
        );
        return;
    }

    // (a): the isolated NodeLifecycleState probe, including the N=0 floor.
    let mut isolated_floor_samples = Vec::with_capacity(SAMPLES_PER_POINT);
    for _ in 0..SAMPLES_PER_POINT {
        isolated_floor_samples.push(measure_lifecycle_state_probe_rss_kib(0));
    }
    let isolated_floor_series = RssSeries {
        node_count: 0,
        peak_kib: isolated_floor_samples,
    };
    let isolated_floor_kib = isolated_floor_series.median();
    eprintln!(
        "isolated probe floor (N=0): {:?} KiB",
        isolated_floor_series.peak_kib
    );

    let mut isolated_probe_series = Vec::new();
    for &node_count in &NODE_COUNTS {
        let mut peak_kib = Vec::with_capacity(SAMPLES_PER_POINT);
        for _ in 0..SAMPLES_PER_POINT {
            peak_kib.push(measure_lifecycle_state_probe_rss_kib(node_count));
        }
        eprintln!("isolated probe N={node_count}: {peak_kib:?} KiB");
        isolated_probe_series.push(RssSeries {
            node_count,
            peak_kib,
        });
    }

    // (b), (c), and the sweep: reuse the incremental series' own repository generation, adding two
    // more file reads to each already-measured commit and one full directory sweep per N (taken
    // from sample 0 only -- §6b.3 step 1 already established these files are deterministic in size
    // given N and the touched-path shape, not sample noise).
    let mut incremental_rss_series = Vec::new();
    let mut commit_index_series = Vec::new();
    let mut object_index_series = Vec::new();
    let mut sweep_by_node_count: BTreeMap<usize, BTreeMap<String, u64>> = BTreeMap::new();
    for &node_count in &NODE_COUNTS {
        let mut peak_kib = Vec::with_capacity(SAMPLES_PER_POINT);
        let mut commit_index_bytes = Vec::with_capacity(SAMPLES_PER_POINT);
        let mut object_index_bytes = Vec::with_capacity(SAMPLES_PER_POINT);
        for sample_index in 0..SAMPLES_PER_POINT {
            let root = unique_dir(&format!("attribution-{node_count}-{sample_index}"));
            std::fs::create_dir_all(&root).unwrap();
            support::init(&root);
            let seed = CONTENT_SEED
                .wrapping_add(0x4000_0000)
                .wrapping_add(node_count as u64)
                .wrapping_add(sample_index as u64);
            let mut rng = SplitMix64::new(seed);
            let files = generate_tree(&root, node_count, &mut rng);
            support::ok(
                &support::commit(&root, "heads/main", "rfc133-attribution: baseline"),
                "baseline commit",
            );
            support::ok(&support::seal(&root, "heads/main"), "baseline seal");
            mutate_one_file(&root, &files, &mut rng);

            let kib =
                measure_commit_rss_kib(&root, "heads/main", "rfc133-attribution: incremental");
            peak_kib.push(kib);
            commit_index_bytes.push(commit_index_file_size(&root).unwrap_or_else(|| {
                panic!(
                    "no commit-index.v1 cache file after an incremental commit at N={node_count}"
                )
            }));
            object_index_bytes.push(object_index_file_size(&root).unwrap_or_else(|| {
                panic!(
                    "no containers/index.container after an incremental commit at N={node_count}"
                )
            }));
            if sample_index == 0 {
                sweep_by_node_count.insert(node_count, sweep_prikk_tree(&root));
            }
            let _ = std::fs::remove_dir_all(&root);
        }
        eprintln!(
            "attribution N={node_count}: RSS {peak_kib:?} KiB, commit-index {commit_index_bytes:?} bytes, object-index {object_index_bytes:?} bytes"
        );
        incremental_rss_series.push(RssSeries {
            node_count,
            peak_kib,
        });
        commit_index_series.push(CacheSeries {
            node_count,
            bytes: commit_index_bytes,
        });
        object_index_series.push(CacheSeries {
            node_count,
            bytes: object_index_bytes,
        });
    }

    let floor_kib = incremental_rss_series
        .iter()
        .find(|series| series.node_count == NODE_COUNTS[0])
        .expect("NODE_COUNTS[0] must have its own series")
        .median();

    let smallest_n = *NODE_COUNTS.iter().min().unwrap();
    let largest_n = *NODE_COUNTS.iter().max().unwrap();
    let sweep_smallest = SweepSnapshot {
        node_count: smallest_n,
        sizes: sweep_by_node_count.remove(&smallest_n).unwrap(),
    };
    let sweep_largest = SweepSnapshot {
        node_count: largest_n,
        sizes: sweep_by_node_count.remove(&largest_n).unwrap(),
    };

    let report = render_attribution_report(
        floor_kib,
        isolated_floor_kib,
        &incremental_rss_series,
        &isolated_probe_series,
        &commit_index_series,
        &object_index_series,
        &sweep_smallest,
        &sweep_largest,
    );
    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/133-performance-cost-and-its-evidence/node-count-memory-attribution-report-v1.md"
    );
    std::fs::write(report_path, report).unwrap();
    eprintln!("report written to {report_path}");
}
