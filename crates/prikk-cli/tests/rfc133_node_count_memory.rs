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
//! ## §6d.1 — the object index's resident cost (`rfc133_node_count_memory_object_index`)
//!
//! The attribution round filed `.prikk/containers/index.container` under "disk bytes, not RSS" —
//! **wrong**: `ObjectReadSnapshot::open`/`ObjectWriteSession::open` (`object_store.rs`) both hold the
//! whole decoded index (`IndexSnapshot`, a `Vec<IndexEntry>`) for the session's lifetime. Both types
//! are `pub`, re-exported from `prikk-store`'s crate root — no visibility change, no synthetic
//! reconstruction, no new production code: this probe opens a **real** repository's **real** index
//! through the same public API `commit`/`verify` use.
//!
//! **The indexed-object count is derived, not assumed.** `foundation/index.rs`'s own constants,
//! `INDEX_HEADER_LEN` (50) + `INDEX_BODY_LEN` (83) = 133 bytes, are a **fixed-width** record (no
//! length-prefixing inside the body) — confirmed at source and independently by the attribution
//! round's own 133–137 bytes/node figure for this same file. `object_index_file_size(root) /
//! INDEX_RECORD_BYTES` is therefore the exact indexed-object count, checked for a zero remainder
//! rather than trusted.
//!
//! ## §6d.3 — the probe was masked, and why fixing it needed a second binary AND a second measuring
//! process, not just the first
//!
//! §6d.1's own first attempt reused this file's self-reexec pattern (spawn `current_exe()` filtered
//! to one `#[test]`, exactly like `lifecycle_state_probe_worker`). The review found its readings
//! implausibly low below N=64,000 and traced it to that self-reexec binary's own ~11 MiB startup
//! cost (every linked crate, the full `libtest` harness) masking a multi-megabyte allocation inside
//! already-touched heap slack — the handoff's own fix asked for a minimal companion binary.
//!
//! **Built one (`rusage-object-index-probe`).** First landed as a `[[bin]]` on this crate,
//! `required-features`-gated so an ordinary `cargo install prikk` would not build it — but `prikk`
//! is a **published** crate, so that still shipped the binary's source in the `.crate` tarball and
//! put a public feature name on published surface (§6d.5). **Relocated to `tools/benchmarks`**
//! (`prikk-benchmarks`, `publish = false`, already depends on `prikk-store` alone and nothing
//! else — exactly what this probe needs, and already exists for precisely this purpose, so the move
//! needed no new workspace member). `env!("CARGO_BIN_EXE_*")` only resolves for test targets of the
//! crate declaring the `[[bin]]`, so it cannot name a binary in a different crate — the same wall
//! RFC 139 increment 2's own executor hit, resolved the same way here:
//! `rusage_object_index_probe_binary` takes the path from `PRIKK_RUSAGE_PROBE_BIN` explicitly and
//! `rusage_object_index_probe_identity` records the binary's own hash, rather than relying on a
//! macro that cannot reach it.
//!
//! **That alone did not fix it — confirmed empirically before committing to the rest of the fix,
//! not assumed.** Spawning the new minimal binary through `rusage_child.py` still read ~11-12 MiB.
//! So did spawning `/usr/bin/true`. So did spawning a bare `fn main(){}` Rust binary with no
//! dependencies at all. Measuring Python's own resident size immediately before it forks
//! (`resource.getrusage(RUSAGE_SELF)`) landed at the same ~10-12 MiB. **The floor tracks the
//! *spawning parent's* own RSS at fork time, not the child's** — a real Linux fork()+exec()
//! characteristic (the child's `mm` briefly shares the parent's pages via copy-on-write before
//! `exec()` replaces the address space, and `hiwater_rss` tracking can latch onto that). The
//! measured child's own content was never what mattered below N=64,000; the size of whatever
//! spawns it is.
//!
//! **So this one probe uses `tests/support/rusage_child.zsh` instead of `rusage_child.py`** — zsh's
//! own idle RSS is roughly 1.6-2 MiB, confirmed the same way, small enough that even the smallest
//! points now clear the standing control below. See that script's own doc comment for the exact
//! mechanism and why `set -e` at its top level does not work (hit during development, not assumed).
//! Every other measurement in this file keeps `rusage_child.py` unchanged, per the handoff's own
//! "do not re-run the other two rounds' series" instruction — their signal sizes (tens of MiB and
//! up) are not distorted by an 11 MiB floor the way this one's few-hundred-KiB-to-low-single-digit-
//! MiB signal was.
//!
//! **The standing control** (§6d.3's own REQUIRED addition): resident cost, over the floor, can
//! never read below `IndexEntry`'s own physical minimum size (88 bytes/entry — `object_id`(32) +
//! `object_type` + `slot` + `offset`(8) + `length`(8) + `container_checksum`(32), rounded up to
//! Rust's own 8-byte struct alignment; `IndexEntry` is `pub(crate)`, so this cannot be computed via
//! `size_of` from outside `prikk-store` and is instead stated here, the same bound the review's own
//! worked example used). A reading below it fails the test — a measurement failure, not a finding.
//!
//! ## Running it
//!
//! **Must be run with `--release`** — the handoff requires a release build, matching §2.1's own
//! method, and this is a real behaviour difference: a debug build's allocator/bounds-checking
//! overhead would not be comparable to §2's figures at all. `rfc133_node_count_memory` and
//! `rfc133_node_count_memory_attribution` need `python3` on `PATH`;
//! `rfc133_node_count_memory_object_index` needs `zsh` on `PATH` **and the companion binary built
//! and named**, since §6d.5 moved it out of this crate:
//!
//! ```text
//! cargo build --release -p prikk-benchmarks --locked --bin rusage-object-index-probe
//! PRIKK_RUSAGE_PROBE_BIN=target/release/rusage-object-index-probe \
//!   cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_object_index
//! ```
//!
//! All three are Linux-only (`resource.getrusage`/zsh's `TIMEFMT` are POSIX and likely work
//! elsewhere, but this harness is not verified anywhere else and skips cleanly elsewhere).
//!
//! ```text
//! cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory
//! cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_attribution
//! ```
//!
//! ## The release-gate profile (`rfc133_node_count_memory_release_gate`)
//!
//! The measurement-cost handoff (RFC 133 §6a, this round): the release-prep template's memory-ratio
//! step names **one number**, the incremental-commit peak-RSS ratio, and needs only
//! [`RELEASE_GATE_NODE_COUNTS`] to get it -- the full sweep above measures the whole **shape** across
//! seven points and two series, which the prep template does not need and could not afford every
//! release (the full sweep's own report clocked 77 minutes even before it gained its `tree`/`diff`
//! columns). `#[ignore]`d beside `rfc133_node_count_memory`, sharing all of its code
//! (`measure_incremental_point`); its own report says in its header that it is the release-gate
//! profile, never the full sweep, and where to run the full one:
//!
//! ```text
//! cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_release_gate
//! ```
//!
//! ## The measurement-budget round (measurement-budget handoff)
//!
//! - **Every step is timed, and the unit has a budget.** Both entry points (and `tools/corpus/tests/build_cost_curve.rs`) run under a
//!   [`budget::Unit`] (`tools/corpus/tests/support/budget.rs`, shared by inclusion): it declares its budget in source
//!   ([`RELEASE_GATE_BUDGET`], [`FULL_SWEEP_BUDGET`]), appends every step's start, end, elapsed time and the boot id to the report file as
//!   it happens, and at twice the budget kills what it started, writes `STOPPED: over budget` and fails. There is no environment
//!   variable for a budget.
//! - **The baseline is built once.** For each N the tree is generated and committed and sealed **once**
//!   ([`prepare_baseline`]); each sample copies that repository and makes its own one-file change from its own seed
//!   ([`measure_incremental_sample_from`]). Before, every sample rebuilt everything
//!   ([`measure_incremental_sample_rebuilding`], kept only for `rfc133_step_costs_one_sample` and the comparison below).
//!   `rfc133_gate_figure_old_vs_new` alternates the two forms at N = 32,000 and 64,000 and requires each new median to lie within the
//!   old run's min-max.
//! - **The release-gate profile measures only what release prep gates on**: the incremental commit's peak RSS and the cache-file
//!   control ([`Scope::ReleaseGate`]); the `tree` and `diff` rows and the second seal are in the full sweep ([`Scope::Full`]).
//!
//! `#[ignore]`d: these are measurement instruments, not correctness tests, and their dominant cost
//! (repositories up to tens of thousands of files, three samples per point) does not belong in the
//! default suite.
//!
//! **Linux-only, in full.** Every measurement here reads `getrusage(RUSAGE_CHILDREN).ru_maxrss`,
//! matching `dc59_commit_benchmark.rs`'s own memory pass (DC-62) — verified on Linux and not
//! assumed portable elsewhere. The whole file is gated rather than each helper: on other platforms
//! there is nothing here to be unused, so nothing needs `#[allow(dead_code)]` to stay quiet.
//! `dc59`/`dc92` gate per item because most of *their* code is genuinely cross-platform; none of
//! this file's is. `rfc133_node_count_memory_object_index` is no longer feature-gated (§6d.5): it
//! is already `#[ignore]`d, so a missing `PRIKK_RUSAGE_PROBE_BIN` panics loudly with build
//! instructions at run time instead of being refused at compile time — simpler, and it means the
//! driver shows up in an ordinary `cargo test --workspace` listing as `ignored` rather than
//! vanishing under default features.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use prikk_object::{NodeId, NodeKind, ObjectId};
use prikk_replay::{LiveNode, NodeContent, NodeLifecycleState, RepoPath};

mod support;

// The measurement watcher (measurement-budget handoff §4): one file, shared by inclusion with
// `tools/corpus/tests/build_cost_curve.rs`, so the two harnesses cannot drift.
#[path = "../../../tools/corpus/tests/support/budget.rs"]
mod budget;

/// **Unit budgets** (measurement-budget handoff §4): declared here, in source, never read from the environment, so nothing can raise
/// one for a run. A unit stops itself at twice its budget ([`budget::OVER_BUDGET_FACTOR`]), writing `STOPPED: over budget` into its
/// report. Each is set from a **measured** wall time with a **1.5x margin**, rounded up, unless its line says it is an estimate:
///
/// - step costs: measured 988 s (2026-09-26, one N = 64,000 sample, release, load 1.8) -> 1,500 s;
/// - the four-arm comparison ([`ARMS_BUDGET`]): 75 minutes as Addendum 1 declares it, stopped at 150. It is **not** from a measurement of this
///   unit: the round's earlier two-arm comparison took 2,306 s (2026-09-26, load 2.2) and this one runs four arms, five samples each;
/// - release-gate profile: **an estimate, not a measurement** -- about 730 s summed from the new form's own steps in that comparison run
///   (N = 100 negligible, 32,000 about 100 s, 64,000 about 620 s), x 1.6 -> 1,200 s. The round's §5 run of the profile itself has
///   **not** been made (held on the architect's ruling of the §2 acceptance); it replaces this figure when it is;
/// - full sweep: **an estimate, never measured under the watcher** -- the last full sweep took 77 minutes before it gained its `tree`/`diff`
///   columns, and one N = 64,000 sample alone is now 988 s; 4 h is a stated guess until its first run under the watcher sets it.
const STEP_COSTS_BUDGET: Duration = Duration::from_secs(1_500);
const RELEASE_GATE_BUDGET: Duration = Duration::from_secs(1_200);
const FULL_SWEEP_BUDGET: Duration = Duration::from_secs(4 * 60 * 60);

/// Node-count points. §2's original series stopped at 8,000, exactly where §6a.3 found it departs
/// from "flat" — this extends past it as the handoff requires (minimum 16,000/32,000) and one point
/// further (64,000) since build time allowed reaching it. **128,000 was assessed and not attempted**:
/// a single-sample probe at 64,000 (genesis commit alone) took ~59.5s against ~9s at 32,000 — a
/// ~6.6x cost increase for a 2x node-count increase, non-linear on its own. Extrapolating that same
/// ratio, one full 128,000 point (2 series x 3 samples x up to 3 timed/untimed steps each) would run
/// to the order of an hour for this one point alone. Reported here, not attempted, per the handoff's
/// own "report the wall and the cost that stopped you rather than extrapolating past it."
const NODE_COUNTS: [usize; 7] = [100, 1_000, 4_000, 8_000, 16_000, 32_000, 64_000];

/// The release-gate profile's own, fixed scope (the measurement-cost handoff §1): the incremental
/// series only, at one small point and the two largest. **100** because a *constant* overhead shows
/// up there as a large ratio (50 MB on a 12 MB baseline is 5x at N=100 and 1.4x at N=64,000) — a
/// diagnostic point, not the gate's own denominator. **32,000 and 64,000** because a *per-node*
/// overhead shows up there, uncontaminated by that constant: the gate ratio is computed between
/// these two. The small point costs almost nothing next to the two large ones, so it stays.
const RELEASE_GATE_NODE_COUNTS: [usize; 3] = [100, 32_000, 64_000];

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
            (
                "PRIKK_AUTHOR_SEED_FILE",
                &support::seed_file(support::AUTHOR_SEED_HEX)
                    .display()
                    .to_string(),
            ),
        ],
    )
}

/// RFC 157: run one measured `prikk tree --ref <ref_name> --format json` against `root` and return its peak
/// `RUSAGE_CHILDREN` RSS in KiB. Read-only; needs no key.
fn measure_tree_rss_kib(root: &Path, ref_name: &str) -> i64 {
    run_rusage_child(
        root,
        Path::new(env!("CARGO_BIN_EXE_prikk")),
        &["tree", "--ref", ref_name, "--format", "json"],
        &[],
    )
}

/// RFC 153: run one measured `prikk diff <args> --format json` against `root` and return its peak
/// `RUSAGE_CHILDREN` RSS in KiB. Read-only; needs no key.
fn measure_diff_rss_kib(root: &Path, args: &[&str]) -> i64 {
    let mut full = vec!["diff"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--format", "json"]);
    run_rusage_child(root, Path::new(env!("CARGO_BIN_EXE_prikk")), &full, &[])
}

/// The block id `prikk log --limit 1` prints for `root`'s current tip.
fn tip_block_id(root: &Path) -> String {
    let output = support::prikk(root)
        .args(["log", "--limit", "1"])
        .output()
        .unwrap();
    support::ok(&output, "log --limit 1");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .expect("log names a block")
        .trim()
        .to_string()
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

/// `foundation/index.rs`'s own record layout: `INDEX_HEADER_LEN` (8+2+8+32=50) +
/// `INDEX_BODY_LEN` (32+2+1+8+8+32=83) = 133, fixed-width, no length-prefixing inside the body.
/// Confirmed at source and cross-checked against the attribution round's own measured 133-137
/// bytes/node for this same file.
const INDEX_RECORD_BYTES: u64 = 133;

/// The exact count of objects `containers/index.container` currently indexes, derived from its
/// fixed-width record size rather than assumed equal to node count (§6d.1's own instruction: N nodes
/// produce blobs, patches, blocks, and ref states, so the index holds more entries than N).
fn indexed_object_count(root: &Path) -> u64 {
    let bytes = object_index_file_size(root)
        .unwrap_or_else(|| panic!("no containers/index.container under {}", root.display()));
    assert_eq!(
        bytes % INDEX_RECORD_BYTES,
        0,
        "containers/index.container size {bytes} is not a whole number of {INDEX_RECORD_BYTES}-byte records"
    );
    bytes / INDEX_RECORD_BYTES
}

/// §6d.3 -- the self-reexec pattern the rest of this file uses for its probes (spawn `current_exe()`
/// filtered to one `#[test]`) is exactly what made this probe's own floor too large to be usable:
/// the review found readings implausibly low below N=64,000, and it traced back to the whole
/// compiled test binary's own ~11 MiB startup cost (every linked crate, the full `libtest` harness)
/// masking a multi-megabyte allocation inside already-touched heap slack. Building a genuinely
/// minimal companion binary (`rusage-object-index-probe`) turned out **not** to be sufficient on its
/// own -- confirmed empirically before committing to the fix, not assumed: spawning that minimal
/// binary through `rusage_child.py` still read ~11-12 MiB, and so did spawning `/usr/bin/true` and a
/// bare `fn main(){}` Rust binary with no dependencies. The floor tracks **Python's own** resident
/// size at fork time, not the child's -- a real Linux fork()+exec() characteristic (see
/// `tests/support/rusage_child.zsh`'s own doc comment for the mechanism). So this probe alone uses
/// `rusage_child.zsh` (zsh's `TIMEFMT`, same underlying syscall, an order of magnitude smaller
/// spawning process) instead of `rusage_child.py`; every other measurement in this file keeps using
/// `rusage_child.py` unchanged, per the handoff's own "do not re-run the other two rounds' series"
/// instruction -- their signal sizes are not distorted by an 11 MiB floor the way this one was.
const RUSAGE_CHILD_ZSH_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/support/rusage_child.zsh"
);

/// §6d.5 -- the minimal companion binary moved to `tools/benchmarks` (`prikk-benchmarks`, `publish =
/// false`) because `prikk` itself is a published crate and a `[[bin]]` there, even
/// `required-features`-gated, still shipped the binary's source in the `.crate` tarball and put a
/// public feature name on published surface. `env!("CARGO_BIN_EXE_*")` only resolves for test
/// targets of the crate declaring the `[[bin]]`, so it cannot name a binary in a different crate --
/// the same wall RFC 139 increment 2's own executor hit, resolved the same way here: take the path
/// explicitly (`PRIKK_RUSAGE_PROBE_BIN`) and record its identity (see
/// `rusage_object_index_probe_identity` below), rather than relying on a macro that cannot reach it.
/// Fails loudly, naming the variable and the build command, rather than silently skipping -- an
/// unset variable here means the binary was never built, not that the environment lacks a tool (the
/// `zsh`/`python3` checks elsewhere in this file skip for the latter reason; this one panics because
/// it is the former).
fn rusage_object_index_probe_binary() -> PathBuf {
    let Ok(path) = std::env::var("PRIKK_RUSAGE_PROBE_BIN") else {
        panic!(
            "PRIKK_RUSAGE_PROBE_BIN is not set. Build the probe binary first:\n\n  \
             cargo build --release -p prikk-benchmarks --locked --bin rusage-object-index-probe\n\n\
             then set PRIKK_RUSAGE_PROBE_BIN to its path (e.g. \
             target/release/rusage-object-index-probe) and re-run."
        );
    };
    let path = PathBuf::from(path);
    if !path.exists() {
        panic!(
            "PRIKK_RUSAGE_PROBE_BIN={} does not exist. Build it first:\n\n  \
             cargo build --release -p prikk-benchmarks --locked --bin rusage-object-index-probe",
            path.display()
        );
    }
    path
}

/// The probed binary's own identity (RFC 139 increment 2's own convention for exactly this
/// situation: a measurement taken with one binary is not comparable to one taken with another) --
/// its path as given, plus the hex SHA-256 of its own bytes. No `--version` flag to also record:
/// unlike `prikk` itself, this is a small purpose-built tool with no version output.
fn rusage_object_index_probe_identity(binary: &Path) -> String {
    let bytes = std::fs::read(binary)
        .unwrap_or_else(|err| panic!("reading {} to record its identity: {err}", binary.display()));
    let sha256 = prikk_hash::to_hex(&prikk_hash::sha256(&bytes));
    format!("{} (sha256 {sha256})", binary.display())
}

/// Like `run_rusage_child`, but via `rusage_child.zsh` instead of `rusage_child.py` -- see that
/// script's own doc comment for why. No `envs` parameter: this probe's only caller passes
/// everything as argv, not environment.
fn run_rusage_child_zsh(cwd: &Path, binary: &Path, args: &[&str]) -> i64 {
    let output = Command::new("zsh")
        .arg(RUSAGE_CHILD_ZSH_SCRIPT)
        .arg(cwd)
        .arg(binary)
        .args(args)
        .output()
        .expect("spawning rusage_child.zsh -- is zsh on PATH?");
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
                "rusage_child.zsh stdout {:?} is not an integer: {err}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
}

/// Run the minimal companion binary against `repo_root` in `mode` (`"read"` or `"write"`) and
/// return its peak RSS in KiB.
fn measure_object_index_probe_rss_kib(binary: &Path, repo_root: &Path, mode: &str) -> i64 {
    run_rusage_child_zsh(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        binary,
        &[mode, &repo_root.to_string_lossy()],
    )
}

/// The floor: the same minimal binary, `mode = "floor"` (returns immediately, no repository) --
/// process-startup cost only.
fn measure_object_index_probe_floor_rss_kib(binary: &Path) -> i64 {
    run_rusage_child_zsh(Path::new(env!("CARGO_MANIFEST_DIR")), binary, &["floor"])
}

/// §3's own worked lower bound, stated here rather than re-derived silently: `IndexEntry`'s fields
/// (`object_id: ObjectId` 32 bytes, `object_type` code, `slot`, `offset: u64` 8, `length: u64` 8,
/// `container_checksum: [u8; 32]` 32 -- `foundation/index.rs`'s own `encode_entry_body`) sum to
/// 82-83 bytes before Rust's struct alignment rounds up to a multiple of 8: 88. `IndexEntry` itself
/// is `pub(crate)`, so this cannot be computed via `size_of` from outside the crate that owns it.
const INDEX_ENTRY_MIN_RESIDENT_BYTES: i64 = 88;

/// §6d.3's own REQUIRED standing control: measured resident cost can never read below the
/// structure's own physical minimum. `growth_kib` is resident cost *over the floor*, since the
/// floor itself is not part of what the structure costs.
fn object_index_resident_floor_holds(indexed_objects: u64, growth_kib: i64) -> bool {
    let resident_bytes = growth_kib * 1024;
    let minimum_bytes = indexed_objects as i64 * INDEX_ENTRY_MIN_RESIDENT_BYTES;
    resident_bytes >= minimum_bytes
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

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Where every report of this instrument is written: `.git-exclude/measurements/rfc133/`, untracked by
/// construction. **Never under `rfcs/`** -- the tracked reports in `rfcs/handoffs/133-…/` are the
/// historical record, and an instrument that rewrote them on every run, stamped with whatever HEAD was
/// when it finished, was one accidental commit away from a record naming a revision it never built (found
/// at the 0.42.0 release prep).
fn measurements_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc133")
}

/// The revision a run measured, and the state it must leave `rfcs/` in.
///
/// **The stamp is HEAD at the start of the run, held to the end.** This crate has no build script, so
/// the revision the test binary was built from is recorded nowhere; `cargo test` builds it from the tree
/// immediately before the run starts, so HEAD at start is that revision -- provided it has not moved by
/// the time the report is written, which [`RunRevision::write_report`] checks and refuses otherwise.
/// `dirty` marks a start with tracked changes against HEAD, where no revision fully names the tree.
struct RunRevision {
    head: String,
    dirty: bool,
    /// `git status --porcelain` under `rfcs/` at the start, so the teardown can assert the run left it
    /// exactly as it found it -- whatever the operator's own uncommitted edits there were.
    rfcs_status: String,
    /// Seconds since the Unix epoch at the start of the run -- the run-distinguishing filename suffix
    /// (the measurement-cost handoff Addendum 1 §2.4): two clean runs of the same committed revision
    /// used to write the same file name and silently overwrite each other's evidence.
    started_at: u64,
}

impl RunRevision {
    fn capture() -> Self {
        Self {
            head: git_output(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string()),
            dirty: git_output(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|status| !status.is_empty()),
            rfcs_status: git_output(&["status", "--porcelain", "--", ":(top)rfcs"])
                .unwrap_or_default(),
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
        }
    }

    fn stamp(&self) -> String {
        if self.dirty {
            format!("{} (with uncommitted tracked changes)", self.head)
        } else {
            self.head.clone()
        }
    }

    fn unchanged_at(&self, head_now: &str) -> Result<(), String> {
        if head_now == self.head {
            Ok(())
        } else {
            Err(format!(
                "HEAD moved during the run, from {} to {head_now}; refusing to write a report naming a \
                 revision the measured binary was not built from -- rerun on a settled tree",
                self.head
            ))
        }
    }

    fn file_name(&self, report: &str) -> String {
        let short: String = self.head.chars().take(12).collect();
        let suffix = if self.dirty { "-dirty" } else { "" };
        format!("{report}-{short}{suffix}-{}.md", self.started_at)
    }

    /// Write `content` as `<report>-<revision>.md` under [`measurements_dir`], print the path, and assert
    /// the run left `rfcs/` as it found it.
    fn write_report(&self, report: &str, content: &str) {
        let head_now = git_output(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
        if let Err(message) = self.unchanged_at(&head_now) {
            panic!("{message}");
        }
        let dir = measurements_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(self.file_name(report));
        std::fs::write(&path, content).unwrap();
        eprintln!("report written to {}", path.display());
        let rfcs_now =
            git_output(&["status", "--porcelain", "--", ":(top)rfcs"]).unwrap_or_default();
        assert_eq!(
            rfcs_now, self.rfcs_status,
            "the instrument must leave rfcs/ exactly as it found it"
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_report(
    revision: &str,
    genesis: &[RssSeries],
    incremental_rss: &[RssSeries],
    incremental_elapsed_secs: &[f64],
    incremental_cache: &[CacheSeries],
    tree_rss: &[RssSeries],
    diff_worktree_rss: &[RssSeries],
    diff_points_rss: &[RssSeries],
) -> String {
    assert_eq!(
        incremental_rss.len(),
        incremental_elapsed_secs.len(),
        "the incremental table's rows and elapsed times must be the same length -- `zip` below \
         silently drops a row otherwise, rather than failing"
    );
    let mut out = String::new();
    out.push_str("# RFC 133 §6b.3 step 1 — node-count memory measurement, report v1\n\n");
    out.push_str("Generated by `cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory`.\n");
    out.push_str("Each run writes a new file under `.git-exclude/measurements/rfc133/`, named for the revision it measured. The *numbers* are hardware-dependent; the **shape** of each series is the claim under test.\n\n");
    out.push_str(&format!("Revision measured at: `{}`. Release build, Linux, worktrees under a `tmpfs` temp directory, peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` (via `tests/support/rusage_child.py`, since this workspace forbids unsafe Rust), each measured commit in a fresh process. {SAMPLES_PER_POINT} samples per point.\n\n", revision));

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
    out.push_str("Repository already committed and sealed at N nodes, then exactly one file changed and committed. Peak RSS, and wall time for this point's own `SAMPLES_PER_POINT` samples across every series measured on the same repository (the measurement-cost handoff Addendum 1's per-point elapsed time):\n\n");
    out.push_str(
        "| N | min (KiB) | median (KiB) | max (KiB) | elapsed (s) |\n|---|---|---|---|---|\n",
    );
    for (series, elapsed_secs) in incremental_rss.iter().zip(incremental_elapsed_secs) {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {elapsed_secs:.1} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## `prikk tree` series (RFC 157 §3)\n\n");
    out.push_str("The incremental series' own repository, sealed at N nodes, listed with `prikk tree --ref heads/main --format json` in a fresh process: one anchored replay plus the exact text sizes. Peak RSS:\n\n");
    out.push_str("| N | min (KiB) | median (KiB) | max (KiB) |\n|---|---|---|---|\n");
    for series in tree_rss {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## `prikk diff` series (RFC 153 §5)\n\n");
    out.push_str("Two rows, both in a fresh process on the incremental series' own repository (N nodes, one file changed).\n\n");
    out.push_str("**Worktree** — a bare `prikk diff --format json` of the one-file change *before* it is committed: the folded baseline is read without refreshing the cache, every worktree file is read and compared by content hash, and only the changed file's baseline text is materialized. Peak RSS:\n\n");
    out.push_str("| N | min (KiB) | median (KiB) | max (KiB) |\n|---|---|---|---|\n");
    for series in diff_worktree_rss {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }
    out.push_str("\n**Two points** — the sealed baseline block against the sealed tip, `prikk diff --from <block> --to heads/main --format json`: two anchored replays over one read snapshot. Peak RSS:\n\n");
    out.push_str("| N | min (KiB) | median (KiB) | max (KiB) |\n|---|---|---|---|\n");
    for series in diff_points_rss {
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
    revision: &str,
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
    out.push_str("Each run writes a new file under `.git-exclude/measurements/rfc133/`, named for the revision it measured. Follows step 1's own report (`node-count-memory-measurement-report-v1.md`); same method, same ladder.\n\n");
    out.push_str(&format!("Revision measured at: `{}`. Release build, Linux, worktrees under a `tmpfs` temp directory, peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` (via `tests/support/rusage_child.py`). {SAMPLES_PER_POINT} samples per point.\n\n", revision));

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

/// One node-count's worth of the incremental series (RSS, cache-file size, and the `tree`/`diff`
/// context rows) — every measurement the incremental loop takes at one `N`, all read from the same
/// generated repository and the same measured commits, so splitting them into separate loops would
/// mean regenerating repositories for no reason.
///
/// **The one measurement step both `rfc133_node_count_memory` and
/// `rfc133_node_count_memory_release_gate` call** (the measurement-cost handoff §2's "the two entry
/// points share one implementation") — a full driver's own report and the release-gate profile's own
/// report are two different *scopes over the same points*, never two different ways of taking one.
struct IncrementalPoint {
    rss: RssSeries,
    cache: CacheSeries,
    tree_rss: RssSeries,
    diff_worktree_rss: RssSeries,
    diff_points_rss: RssSeries,
    /// Wall time for this whole point -- all [`SAMPLES_PER_POINT`] samples, every series, at this one
    /// `N`. The measurement-cost handoff Addendum 1 §2.1's per-point elapsed time: the same `N` timed
    /// in both entry points is a machine-drift check, and a point that finishes prints before the next
    /// one starts, so a run interrupted mid-sweep still leaves usable numbers.
    elapsed_secs: f64,
}

/// What one sample of the incremental series read.
struct SampleOutcome {
    peak_kib: i64,
    cache_bytes: u64,
    /// The context rows (`tree`, the two `diff`s): `None` when the scope does not measure them.
    tree_peak_kib: Option<i64>,
    diff_worktree_kib: Option<i64>,
    diff_points_kib: Option<i64>,
}

/// One sample, **built from nothing** -- the form this instrument had before the measurement-budget round: create a repository, write
/// `node_count` files, commit, seal, then measure. **Every step is timed** through the unit (`step`), so a report shows where a
/// sample's time goes; nothing else about the sample changed.
///
/// `through_measured_commit` ends the sample after the measured commit and the cache-file control: nothing after them can change
/// what the measured commit read, so the §2 comparison (which reads only that figure) skips the `tree`, second-seal and two-point
/// diff steps and keeps the worktree diff, which runs **before** the commit and could.
///
/// `worktree_diff` is whether the bare `prikk diff` runs before the measured commit (the old form always did; arm (ii) of the
/// Addendum 1 comparison does not). When it does not run the outcome's `diff_worktree_kib` is `None`.
fn measure_incremental_sample_rebuilding(
    unit: &budget::Unit,
    node_count: usize,
    sample_index: usize,
    through_measured_commit: bool,
    worktree_diff: bool,
) -> SampleOutcome {
    let label = format!("N={node_count} sample {sample_index}");
    let root = unique_dir(&format!("incremental-{node_count}-{sample_index}"));
    std::fs::create_dir_all(&root).unwrap();
    unit.step(&format!("{label}: init"), || support::init(&root));
    let seed = CONTENT_SEED
        .wrapping_add(0x8000_0000)
        .wrapping_add(node_count as u64)
        .wrapping_add(sample_index as u64);
    let mut rng = SplitMix64::new(seed);
    let files = unit.step(&format!("{label}: generate the tree"), || {
        generate_tree(&root, node_count, &mut rng)
    });
    unit.step(&format!("{label}: baseline commit"), || {
        support::ok(
            &support::commit(&root, "heads/main", "rfc133-bench: baseline"),
            "baseline commit",
        );
    });
    unit.step(&format!("{label}: baseline seal"), || {
        support::ok(&support::seal(&root, "heads/main"), "baseline seal");
    });
    mutate_one_file(&root, &files, &mut rng);
    let baseline_block = tip_block_id(&root);
    // RFC 153's worktree row: a bare `prikk diff` of the same one-file change, before it is committed --
    // the baseline read plus a read of every worktree file, in a fresh process.
    let diff_worktree_kib = worktree_diff.then(|| {
        unit.step(&format!("{label}: worktree diff"), || {
            measure_diff_rss_kib(&root, &[])
        })
    });
    let peak_kib = unit.step(&format!("{label}: measured commit"), || {
        measure_commit_rss_kib(&root, "heads/main", "rfc133-bench: incremental")
    });
    let cache_bytes = cache_file_size(&root).unwrap_or_else(|| {
        panic!("no lifecycle-state.v1 cache file after an incremental commit at N={node_count}")
    });
    if through_measured_commit {
        let _ = std::fs::remove_dir_all(&root);
        return SampleOutcome {
            peak_kib,
            cache_bytes,
            tree_peak_kib: None,
            diff_worktree_kib,
            diff_points_kib: None,
        };
    }
    // RFC 157's `tree` row: the same repository's sealed N-node tip, listed in a fresh process.
    let tree_peak_kib = unit.step(&format!("{label}: tree"), || {
        measure_tree_rss_kib(&root, "heads/main")
    });
    // RFC 153's two-point row: seal the incremental commit, then diff the baseline block against the
    // new tip -- two anchored replays over one read snapshot, in a fresh process.
    unit.step(&format!("{label}: second seal"), || {
        support::ok(&support::seal(&root, "heads/main"), "incremental seal");
    });
    let diff_points_kib = unit.step(&format!("{label}: two-point diff"), || {
        measure_diff_rss_kib(&root, &["--from", &baseline_block, "--to", "heads/main"])
    });
    let _ = std::fs::remove_dir_all(&root);
    SampleOutcome {
        peak_kib,
        cache_bytes,
        tree_peak_kib: Some(tree_peak_kib),
        diff_worktree_kib,
        diff_points_kib: Some(diff_points_kib),
    }
}

/// Which rows a point measures. The release-gate profile measures **only** what release prep gates on; the full sweep keeps
/// the `tree` and `diff` rows and the second seal (measurement-budget handoff §3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Full,
    ReleaseGate,
}

/// One N's baseline, **built once**: a repository holding `node_count` files, committed and sealed (measurement-budget handoff §2).
/// Each sample copies it and makes its own one-file change from its own seed.
struct Baseline {
    root: PathBuf,
    files: Vec<PathBuf>,
}

fn prepare_baseline(unit: &budget::Unit, node_count: usize) -> Baseline {
    let label = format!("N={node_count} baseline (built once)");
    let root = unique_dir(&format!("baseline-{node_count}"));
    std::fs::create_dir_all(&root).unwrap();
    unit.step(&format!("{label}: init"), || support::init(&root));
    let mut rng = SplitMix64::new(
        CONTENT_SEED
            .wrapping_add(0x8000_0000)
            .wrapping_add(node_count as u64),
    );
    let files = unit.step(&format!("{label}: generate the tree"), || {
        generate_tree(&root, node_count, &mut rng)
    });
    unit.step(&format!("{label}: baseline commit"), || {
        support::ok(
            &support::commit(&root, "heads/main", "rfc133-bench: baseline"),
            "baseline commit",
        );
    });
    unit.step(&format!("{label}: baseline seal"), || {
        support::ok(&support::seal(&root, "heads/main"), "baseline seal");
    });
    Baseline { root, files }
}

/// How a sample's copy of the baseline is made (measurement-budget handoff Addendum 1: a copy that gives every file a fresh mtime
/// may make the "incremental" commit re-read files it would otherwise skip, which would be a different operation).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CopyMode {
    /// Every file and directory gets the time of the copy (`std::fs::copy`).
    FreshTimes,
    /// Every file and directory keeps its modification time (`cp -a`).
    PreserveTimes,
}

/// The copy the release-gate profile and the full sweep use. **Set by the Addendum 1 comparison's verdict** (see the arms test).
const GATE_COPY_MODE: CopyMode = CopyMode::FreshTimes;

/// Copy `from` to `to` preserving every timestamp, mode and dotfile: `cp -a from/. to/` (this workspace forbids `unsafe`, and
/// `std` has no portable way to set a directory's time; the harness is Linux-only already).
fn copy_dir_preserving_times(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    let source = format!("{}/.", from.display());
    let output = Command::new("cp")
        .args(["-a", &source])
        .arg(to)
        .output()
        .expect("spawning cp -a");
    assert!(
        output.status.success(),
        "cp -a failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// One sample **from a baseline built once**: copy it (as `copy` says), make this sample's one-file change from its own seed, then
/// measure. The measured commit and the cache-file control are read in both scopes; the `tree`, `diff` and second-seal rows only in
/// [`Scope::Full`].
fn measure_incremental_sample_from(
    unit: &budget::Unit,
    baseline: &Baseline,
    node_count: usize,
    sample_index: usize,
    scope: Scope,
    copy: CopyMode,
) -> SampleOutcome {
    let label = format!("N={node_count} sample {sample_index}");
    let root = unique_dir(&format!("incremental-{node_count}-{sample_index}"));
    unit.step(
        &format!("{label}: copy the baseline ({copy:?})"),
        || match copy {
            CopyMode::FreshTimes => support::copy_dir_recursive(&baseline.root, &root),
            CopyMode::PreserveTimes => copy_dir_preserving_times(&baseline.root, &root),
        },
    );
    let mut rng = SplitMix64::new(
        CONTENT_SEED
            .wrapping_add(0xC0FF_EE00)
            .wrapping_add(node_count as u64)
            .wrapping_add(sample_index as u64),
    );
    mutate_one_file(&root, &baseline.files, &mut rng);
    let full = scope == Scope::Full;
    let baseline_block = full.then(|| tip_block_id(&root));
    let diff_worktree_kib = full.then(|| {
        unit.step(&format!("{label}: worktree diff"), || {
            measure_diff_rss_kib(&root, &[])
        })
    });
    let peak_kib = unit.step(&format!("{label}: measured commit"), || {
        measure_commit_rss_kib(&root, "heads/main", "rfc133-bench: incremental")
    });
    let cache_bytes = cache_file_size(&root).unwrap_or_else(|| {
        panic!("no lifecycle-state.v1 cache file after an incremental commit at N={node_count}")
    });
    let (tree_peak_kib, diff_points_kib) = if let Some(baseline_block) = baseline_block {
        let tree = unit.step(&format!("{label}: tree"), || {
            measure_tree_rss_kib(&root, "heads/main")
        });
        unit.step(&format!("{label}: second seal"), || {
            support::ok(&support::seal(&root, "heads/main"), "incremental seal");
        });
        let diff = unit.step(&format!("{label}: two-point diff"), || {
            measure_diff_rss_kib(&root, &["--from", &baseline_block, "--to", "heads/main"])
        });
        (Some(tree), Some(diff))
    } else {
        (None, None)
    };
    let _ = std::fs::remove_dir_all(&root);
    SampleOutcome {
        peak_kib,
        cache_bytes,
        tree_peak_kib,
        diff_worktree_kib,
        diff_points_kib,
    }
}

fn measure_incremental_point(
    unit: &budget::Unit,
    node_count: usize,
    scope: Scope,
) -> IncrementalPoint {
    let started = std::time::Instant::now();
    let baseline = prepare_baseline(unit, node_count);
    let mut peak_kib = Vec::with_capacity(SAMPLES_PER_POINT);
    let mut cache_bytes = Vec::with_capacity(SAMPLES_PER_POINT);
    let mut tree_peak_kib = Vec::new();
    let mut diff_worktree_kib = Vec::new();
    let mut diff_points_kib = Vec::new();
    for sample_index in 0..SAMPLES_PER_POINT {
        let sample = measure_incremental_sample_from(
            unit,
            &baseline,
            node_count,
            sample_index,
            scope,
            GATE_COPY_MODE,
        );
        peak_kib.push(sample.peak_kib);
        cache_bytes.push(sample.cache_bytes);
        tree_peak_kib.extend(sample.tree_peak_kib);
        diff_worktree_kib.extend(sample.diff_worktree_kib);
        diff_points_kib.extend(sample.diff_points_kib);
    }
    let _ = std::fs::remove_dir_all(&baseline.root);
    let elapsed_secs = started.elapsed().as_secs_f64();
    eprintln!(
        "incremental N={node_count}: RSS {peak_kib:?} KiB, cache {cache_bytes:?} bytes, elapsed \
         {elapsed_secs:.1}s"
    );
    if scope == Scope::Full {
        eprintln!("tree N={node_count}: RSS {tree_peak_kib:?} KiB");
        eprintln!("diff (worktree) N={node_count}: RSS {diff_worktree_kib:?} KiB");
        eprintln!("diff (two points) N={node_count}: RSS {diff_points_kib:?} KiB");
    }
    IncrementalPoint {
        rss: RssSeries {
            node_count,
            peak_kib,
        },
        cache: CacheSeries {
            node_count,
            bytes: cache_bytes,
        },
        tree_rss: RssSeries {
            node_count,
            peak_kib: tree_peak_kib,
        },
        diff_worktree_rss: RssSeries {
            node_count,
            peak_kib: diff_worktree_kib,
        },
        diff_points_rss: RssSeries {
            node_count,
            peak_kib: diff_points_kib,
        },
        elapsed_secs,
    }
}

#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn rfc133_node_count_memory() {
    let revision = RunRevision::capture();
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

    let unit = budget::Unit::begin(
        "rfc133-node-count-memory (full sweep)",
        FULL_SWEEP_BUDGET,
        &measurements_dir().join(revision.file_name("node-count-memory-measurement")),
    );
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
    let mut incremental_elapsed_secs = Vec::new();
    let mut incremental_cache_series = Vec::new();
    let mut tree_rss_series = Vec::new();
    let mut diff_worktree_series = Vec::new();
    let mut diff_points_series = Vec::new();
    for &node_count in &NODE_COUNTS {
        let point = measure_incremental_point(&unit, node_count, Scope::Full);
        incremental_rss_series.push(point.rss);
        incremental_elapsed_secs.push(point.elapsed_secs);
        tree_rss_series.push(point.tree_rss);
        diff_worktree_series.push(point.diff_worktree_rss);
        diff_points_series.push(point.diff_points_rss);
        incremental_cache_series.push(point.cache);
    }

    let report = render_report(
        &revision.stamp(),
        &genesis_series,
        &incremental_rss_series,
        &incremental_elapsed_secs,
        &incremental_cache_series,
        &tree_rss_series,
        &diff_worktree_series,
        &diff_points_series,
    );
    let report = format!("{report}\n{}", unit.finish());
    revision.write_report("node-count-memory-measurement", &report);
}

/// The release-gate profile's own report (the measurement-cost handoff §1): incremental series only,
/// at [`RELEASE_GATE_NODE_COUNTS`], with the same `tree`/`diff` context rows the full driver reports
/// -- they come from the same measured repositories at no extra cost, but are context, never the
/// gate. **States plainly, in its own header, that it is the release-gate profile and not the full
/// sweep** (the measurement-cost handoff §1's own requirement), and names the full driver's command
/// for a reader who wants every point.
fn render_release_gate_report(
    revision: &str,
    incremental_rss: &[RssSeries],
    incremental_elapsed_secs: &[f64],
    incremental_cache: &[CacheSeries],
) -> String {
    assert_eq!(
        incremental_rss.len(),
        incremental_elapsed_secs.len(),
        "the incremental table's rows and elapsed times must be the same length -- `zip` below \
         silently drops a row otherwise, rather than failing"
    );
    let mut out = String::new();
    out.push_str(
        "# RFC 133 §6b.3 step 1 — node-count memory measurement, RELEASE-GATE PROFILE, report v2\n\n",
    );
    out.push_str(&format!(
        "**This is the release-gate profile, not the full sweep.** It measures **only** what release prep gates on: \
         (1) the **incremental commit's peak RSS** -- the gate is its ratio between the two largest N -- and (2) the \
         **persisted cache file size** control (`.prikk/cache/lifecycle-state.v1`). It runs the incremental series only, \
         at N = {RELEASE_GATE_NODE_COUNTS:?} ({} points, {SAMPLES_PER_POINT} samples each). **It does not measure `prikk tree`, \
         `prikk diff` (either row) or the second seal**: those stay in the full sweep, which the release-prep template \
         requires whenever a round changes those commands -- run `cargo test -p prikk --release --locked --test \
         rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory` for them. Each N's baseline \
         repository is built **once** and copied for each sample; every sample makes its own one-file change from its own seed.\n\n",
        RELEASE_GATE_NODE_COUNTS.len(),
    ));
    out.push_str("Generated by `cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_release_gate`.\n");
    out.push_str("Each run writes a new file under `.git-exclude/measurements/rfc133/`, named for the revision it measured.\n\n");
    out.push_str(&format!("Revision measured at: `{}`. Release build, Linux, worktrees under a `tmpfs` temp directory, peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` (via `tests/support/rusage_child.py`, since this workspace forbids unsafe Rust), each measured commit in a fresh process. {SAMPLES_PER_POINT} samples per point.\n\n", revision));

    out.push_str(
        "\n## Incremental series (the gate: the peak-RSS ratio between the two largest N)\n\n",
    );
    out.push_str("Repository already committed and sealed at N nodes, then exactly one file changed and committed. Peak RSS, and wall time for this point (its baseline built once, and every sample):\n\n");
    out.push_str(
        "| N | min (KiB) | median (KiB) | max (KiB) | elapsed (s) |\n|---|---|---|---|---|\n",
    );
    for (series, elapsed_secs) in incremental_rss.iter().zip(incremental_elapsed_secs) {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {elapsed_secs:.1} |\n",
            series.node_count,
            series.min(),
            series.median(),
            series.max()
        ));
    }

    out.push_str("\n## §4's REQUIRED control — persisted incremental-cache file size (`.prikk/cache/lifecycle-state.v1`)\n\n");
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

/// **The before-picture** (measurement-budget handoff §1): **one** sample at N = 64,000 in the release build, in the form the release
/// gate had before this round (repository built from nothing), with **every step timed** -- init, tree generation, baseline commit,
/// baseline seal, worktree diff, measured commit, `tree`, second seal, two-point diff -- and the boot id beside each. It tells which of
/// the setup and the trimming matters most. Writes `step-costs-<revision>.md` under `.git-exclude/measurements/rfc133/`.
///
/// ```text
/// cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_step_costs_one_sample
/// ```
#[test]
#[ignore = "measurement instrument (one N = 64,000 sample, every step timed); run deliberately"]
fn rfc133_step_costs_one_sample() {
    const STEP_COSTS_NODE_COUNT: usize = 64_000;
    let revision = RunRevision::capture();
    let unit = budget::Unit::begin(
        "rfc133 step costs (one N = 64,000 sample)",
        STEP_COSTS_BUDGET,
        &measurements_dir().join(revision.file_name("step-costs")),
    );
    let load_at_start = budget::load_average();
    let sample =
        measure_incremental_sample_rebuilding(&unit, STEP_COSTS_NODE_COUNT, 0, false, true);
    let table = unit.finish();
    let report = format!(
        "# RFC 133 -- what one N = {STEP_COSTS_NODE_COUNT} sample costs, step by step (release)\n\n\
         Revision `{}`. One sample in the form the release gate had before the measurement-budget round (repository built from nothing). Load at start: {load_at_start}.\n\n\
         Peak RSS of the measured commit: {} KiB; cache file {} bytes; `tree` {} KiB; worktree diff {} KiB; two-point diff {} KiB.\n\n{table}",
        revision.stamp(),
        sample.peak_kib,
        sample.cache_bytes,
        sample.tree_peak_kib.unwrap(),
        sample.diff_worktree_kib.unwrap(),
        sample.diff_points_kib.unwrap()
    );
    revision.write_report("step-costs", &report);
}

/// N of the §2 comparison (measurement-budget handoff §2, Addendum 1).
const GATE_COMPARISON_NODE_COUNTS: [usize; 2] = [32_000, 64_000];
/// Samples per arm and N in the Addendum 1 comparison.
const ARMS_SAMPLES: usize = 5;
/// **The decision rule, fixed before the run** (Addendum 1 §3): the chosen form's median must lie within this many percent of arm
/// (i)'s median at each N, **and** its 64,000/32,000 ratio within this many percent of arm (i)'s ratio. Half the smallest
/// release-to-release swing on record (1.0%).
const ARM_TOLERANCE_PERCENT: f64 = 0.5;
/// The four-arm unit's budget (Addendum 1 §2): 75 minutes, stopped at 150. (A rough sum of the round's own measured per-sample times
/// is about 105 minutes: the fresh-build arms at N = 64,000 are the cost. It stops itself at 150.)
const ARMS_BUDGET: Duration = Duration::from_secs(75 * 60);

/// The four forms compared (Addendum 1 §2). (i) is the reference: what the release gate did before this round.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    /// (i) fresh build, with the worktree diff before the measured commit.
    FreshWithDiff,
    /// (ii) fresh build, without it.
    FreshNoDiff,
    /// (iii) a baseline built once, copied with fresh mtimes, without the diff (the form delivered in `8cf2f9f3`).
    CopyFreshTimes,
    /// (iv) a baseline built once, copied preserving every timestamp, without the diff.
    CopyKeepTimes,
}

const ARMS: [Arm; 4] = [
    Arm::FreshWithDiff,
    Arm::FreshNoDiff,
    Arm::CopyFreshTimes,
    Arm::CopyKeepTimes,
];

impl Arm {
    fn label(self) -> &'static str {
        match self {
            Self::FreshWithDiff => "(i) fresh build, with the worktree diff",
            Self::FreshNoDiff => "(ii) fresh build, no diff",
            Self::CopyFreshTimes => "(iii) copy, fresh mtimes, no diff",
            Self::CopyKeepTimes => "(iv) copy, mtimes preserved, no diff",
        }
    }
}

/// What one arm measured: the median peak RSS at each N, and what one sample of it cost (wall seconds, its share of the baseline built
/// once included).
struct ArmResult {
    arm: Arm,
    median_by_n: Vec<(usize, i64)>,
    cost_secs_per_sample: f64,
}

/// One trimmed arm judged against the rule.
#[derive(Debug)]
struct ArmVerdict {
    arm: Arm,
    /// The median's distance from arm (i)'s at each N, in percent (absolute).
    median_deviation_percent: Vec<f64>,
    /// The 64,000/32,000 ratio's distance from arm (i)'s ratio, in percent (absolute).
    ratio_deviation_percent: f64,
    meets: bool,
    cost_secs_per_sample: f64,
}

/// The rule of Addendum 1 §3, as a pure function so it can be controlled without running the instrument: each trimmed arm meets it iff
/// its median is within `tolerance_percent` of arm (i)'s at **every** N and its ratio (largest N over smallest) within
/// `tolerance_percent` of arm (i)'s ratio; the chosen form is the **cheapest** arm that meets it; `None` if none does.
fn evaluate_arms(results: &[ArmResult], tolerance_percent: f64) -> (Vec<ArmVerdict>, Option<Arm>) {
    let reference = results
        .iter()
        .find(|result| result.arm == Arm::FreshWithDiff)
        .expect("arm (i) is the reference");
    let ratio = |result: &ArmResult| {
        let first = result.median_by_n.first().expect("a median").1 as f64;
        let last = result.median_by_n.last().expect("a median").1 as f64;
        last / first
    };
    let percent = |value: f64, reference: f64| 100.0 * (value - reference).abs() / reference;
    let verdicts: Vec<ArmVerdict> = results
        .iter()
        .filter(|result| result.arm != Arm::FreshWithDiff)
        .map(|result| {
            let median_deviation_percent: Vec<f64> = result
                .median_by_n
                .iter()
                .zip(&reference.median_by_n)
                .map(|((_, value), (_, reference))| percent(*value as f64, *reference as f64))
                .collect();
            let ratio_deviation_percent = percent(ratio(result), ratio(reference));
            let meets = median_deviation_percent
                .iter()
                .all(|deviation| *deviation <= tolerance_percent)
                && ratio_deviation_percent <= tolerance_percent;
            ArmVerdict {
                arm: result.arm,
                median_deviation_percent,
                ratio_deviation_percent,
                meets,
                cost_secs_per_sample: result.cost_secs_per_sample,
            }
        })
        .collect();
    let chosen = verdicts
        .iter()
        .filter(|verdict| verdict.meets)
        .min_by(|a, b| a.cost_secs_per_sample.total_cmp(&b.cost_secs_per_sample))
        .map(|verdict| verdict.arm);
    (verdicts, chosen)
}

/// **The four-arm comparison** (measurement-budget handoff Addendum 1): at N = 32,000 and 64,000, in one session, **alternating** with the
/// arm order rotated each round, [`ARMS_SAMPLES`] samples per arm of the four forms in [`Arm`], the measured commit's peak RSS for
/// each. It decides by [`evaluate_arms`] -- fixed before the run -- and writes the arm table, each arm's deviation from arm (i), and the
/// chosen form; **if no arm meets the rule this fails and says stop** (nothing is adjusted to fit).
///
/// ```text
/// cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_gate_form_arms
/// ```
#[test]
#[ignore = "measurement instrument (four arms, alternating); run deliberately"]
fn rfc133_gate_form_arms() {
    let revision = RunRevision::capture();
    let probe = Command::new("python3").arg("--version").output();
    assert!(
        probe.is_ok_and(|probe| probe.status.success()),
        "python3 is needed for getrusage(RUSAGE_CHILDREN) (see module docs)"
    );
    let unit = budget::Unit::begin(
        "rfc133 gate form, four arms (alternating)",
        ARMS_BUDGET,
        &measurements_dir().join(revision.file_name("gate-form-arms")),
    );
    let load_at_start = budget::load_average();
    let mut results: Vec<ArmResult> = ARMS
        .iter()
        .map(|&arm| ArmResult {
            arm,
            median_by_n: Vec::new(),
            cost_secs_per_sample: 0.0,
        })
        .collect();
    let mut raw = String::from(
        "| N | arm | samples (KiB) | min | median | max | mean seconds per sample |\n|---:|---|---|---:|---:|---:|---:|\n",
    );
    for &node_count in &GATE_COMPARISON_NODE_COUNTS {
        // One baseline for both copy arms, built once and timed, its cost shared between them.
        let baseline_started = std::time::Instant::now();
        let baseline = prepare_baseline(&unit, node_count);
        let baseline_secs = baseline_started.elapsed().as_secs_f64();
        let mut peaks: Vec<Vec<i64>> = vec![Vec::new(); ARMS.len()];
        let mut secs: Vec<f64> = vec![0.0; ARMS.len()];
        for sample_index in 0..ARMS_SAMPLES {
            // Alternating, and rotated: no arm is always first (or always last) in a round.
            for offset in 0..ARMS.len() {
                let slot = (offset + sample_index) % ARMS.len();
                let started = std::time::Instant::now();
                let outcome = match ARMS[slot] {
                    Arm::FreshWithDiff => measure_incremental_sample_rebuilding(
                        &unit,
                        node_count,
                        sample_index,
                        true,
                        true,
                    ),
                    Arm::FreshNoDiff => measure_incremental_sample_rebuilding(
                        &unit,
                        node_count,
                        sample_index,
                        true,
                        false,
                    ),
                    Arm::CopyFreshTimes => measure_incremental_sample_from(
                        &unit,
                        &baseline,
                        node_count,
                        sample_index,
                        Scope::ReleaseGate,
                        CopyMode::FreshTimes,
                    ),
                    Arm::CopyKeepTimes => measure_incremental_sample_from(
                        &unit,
                        &baseline,
                        node_count,
                        sample_index,
                        Scope::ReleaseGate,
                        CopyMode::PreserveTimes,
                    ),
                };
                secs[slot] += started.elapsed().as_secs_f64();
                peaks[slot].push(outcome.peak_kib);
            }
        }
        let _ = std::fs::remove_dir_all(&baseline.root);
        for (slot, arm) in ARMS.iter().enumerate() {
            let series = RssSeries {
                node_count,
                peak_kib: peaks[slot].clone(),
            };
            let shared_baseline = match arm {
                Arm::CopyFreshTimes | Arm::CopyKeepTimes => baseline_secs / 2.0,
                _ => 0.0,
            };
            let mean_secs = (secs[slot] + shared_baseline) / ARMS_SAMPLES as f64;
            raw.push_str(&format!(
                "| {node_count} | {} | {:?} | {} | {} | {} | {mean_secs:.1} |\n",
                arm.label(),
                series.peak_kib,
                series.min(),
                series.median(),
                series.max()
            ));
            results[slot]
                .median_by_n
                .push((node_count, series.median()));
            results[slot].cost_secs_per_sample += mean_secs;
        }
        eprintln!("N={node_count}: {peaks:?}");
    }
    let (verdicts, chosen) = evaluate_arms(&results, ARM_TOLERANCE_PERCENT);
    let mut rule_table = String::from(
        "| arm | median vs arm (i) at each N (%) | 64,000/32,000 ratio vs arm (i)'s (%) | meets the rule | cost (s per sample, both N) |\n|---|---|---|---|---:|\n",
    );
    for verdict in &verdicts {
        rule_table.push_str(&format!(
            "| {} | {} | {:.2} | {} | {:.1} |\n",
            verdict.arm.label(),
            verdict
                .median_deviation_percent
                .iter()
                .map(|value| format!("{value:.2}"))
                .collect::<Vec<_>>()
                .join(" / "),
            verdict.ratio_deviation_percent,
            if verdict.meets { "**yes**" } else { "no" },
            verdict.cost_secs_per_sample
        ));
    }
    let reference = &results[0];
    let ratio_line = results
        .iter()
        .map(|result| {
            let first = result.median_by_n.first().unwrap().1 as f64;
            let last = result.median_by_n.last().unwrap().1 as f64;
            format!("{}: {:.4}", result.arm.label(), last / first)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let table = unit.finish();
    let conclusion = chosen.map_or_else(
        || "**NO arm meets the rule. Stop and report again; nothing was adjusted.**".to_string(),
        |arm| {
            format!(
                "**Chosen form: {}** (the cheapest arm that meets the rule).",
                arm.label()
            )
        },
    );
    let report = format!(
        "# RFC 133 -- the gate form, four arms (release)\n\n\
         Revision `{}`. Peak RSS (KiB, `getrusage`) of the measured incremental commit, {ARMS_SAMPLES} samples per arm and N, **alternating** with the \
         arm order rotated each round, in one session (boot `{}`; load at start {load_at_start}). **The rule, fixed before the run:** the chosen form's \
         median within **{ARM_TOLERANCE_PERCENT} %** of arm (i)'s at each N, and its {}/{} ratio within **{ARM_TOLERANCE_PERCENT} %** of arm (i)'s ratio; \
         the chosen form is the cheapest arm that meets it.\n\n{raw}\n{rule_table}\nRatios ({}/{}): {ratio_line}. Arm (i) median at each N: {:?}.\n\n{conclusion}\n\n{table}",
        revision.stamp(),
        budget::boot_id(),
        GATE_COMPARISON_NODE_COUNTS[1],
        GATE_COMPARISON_NODE_COUNTS[0],
        GATE_COMPARISON_NODE_COUNTS[1],
        GATE_COMPARISON_NODE_COUNTS[0],
        reference.median_by_n,
    );
    revision.write_report("gate-form-arms", &report);
    assert!(chosen.is_some(), "no arm meets the rule; see the report");
}

/// **Arm (iv) is what it says**: the preserving copy keeps every file's and directory's modification time (and a dotfile tree), and
/// the fresh-times copy of arm (iii) does not -- so the two arms really differ in the one thing Addendum 1's hypothesis is about.
/// *Perturb: `cp -r` instead of `cp -a` in [`copy_dir_preserving_times`]: this goes red.*
#[test]
fn the_preserving_copy_keeps_modification_times_and_the_fresh_copy_does_not() {
    let base = unique_dir("copy-times-control");
    let source = base.join("src");
    std::fs::create_dir_all(source.join("d/.hidden")).unwrap();
    let file = source.join("d/.hidden/f.txt");
    std::fs::write(&file, b"x").unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(90 * 24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
    let preserved = base.join("preserved");
    copy_dir_preserving_times(&source, &preserved);
    assert_eq!(
        modified(&preserved.join("d/.hidden/f.txt")),
        old,
        "the preserving copy keeps the file's time"
    );
    let fresh = base.join("fresh");
    support::copy_dir_recursive(&source, &fresh);
    assert_ne!(
        modified(&fresh.join("d/.hidden/f.txt")),
        old,
        "the fresh-times copy does not"
    );
    let _ = std::fs::remove_dir_all(base);
}

/// **Control for the rule itself** (Addendum 1 §3, pre-stated): a trimmed arm meets it only if its median is within the tolerance at **every**
/// N **and** its ratio is; the chosen form is the cheapest of those; none meeting it means no form is chosen. Synthetic numbers, never a
/// run. *Perturb: raise `tolerance_percent` far above the deviations, or drop the ratio condition: this goes red.*
#[test]
fn the_arm_rule_chooses_the_cheapest_arm_within_tolerance_and_refuses_when_none_is() {
    let result = |arm, medians: [i64; 2], cost| ArmResult {
        arm,
        median_by_n: vec![(32_000, medians[0]), (64_000, medians[1])],
        cost_secs_per_sample: cost,
    };
    // Reference: 100,000 and 190,000 (ratio 1.9).
    let reference = result(Arm::FreshWithDiff, [100_000, 190_000], 500.0);
    // (ii) meets, costs 300; (iii) meets, costs 100 -> chosen; (iv) misses one N by 0.6 %.
    let arms = [
        reference,
        result(Arm::FreshNoDiff, [100_100, 190_100], 300.0),
        result(Arm::CopyFreshTimes, [100_200, 190_300], 100.0),
        result(Arm::CopyKeepTimes, [100_600, 190_000], 90.0),
    ];
    let (verdicts, chosen) = evaluate_arms(&arms, 0.5);
    assert_eq!(chosen, Some(Arm::CopyFreshTimes), "{verdicts:#?}");
    assert!(
        !verdicts
            .iter()
            .find(|v| v.arm == Arm::CopyKeepTimes)
            .unwrap()
            .meets
    );
    // Both medians within tolerance but the ratio is not: 100,400 and 189,300 -> ratio 1.8853 against 1.9 (0.78 %).
    let ratio_only = [
        result(Arm::FreshWithDiff, [100_000, 190_000], 500.0),
        result(Arm::FreshNoDiff, [100_400, 189_300], 300.0),
    ];
    let (verdicts, chosen) = evaluate_arms(&ratio_only, 0.5);
    assert_eq!(chosen, None, "the ratio alone refuses it: {verdicts:#?}");
    assert!(
        verdicts[0]
            .median_deviation_percent
            .iter()
            .all(|d| *d <= 0.5)
    );
    assert!(verdicts[0].ratio_deviation_percent > 0.5);
    // None meets: no form is chosen.
    let none = [
        result(Arm::FreshWithDiff, [100_000, 190_000], 500.0),
        result(Arm::FreshNoDiff, [101_000, 190_000], 300.0),
    ];
    assert_eq!(evaluate_arms(&none, 0.5).1, None);
}

/// The release-gate profile (the measurement-cost handoff §1, trimmed by the measurement-budget handoff §3): the incremental commit's
/// peak RSS and the cache-file control only, at [`RELEASE_GATE_NODE_COUNTS`] -- the number the release-prep template's own
/// memory-ratio step needs, without the full sweep's cost. Shares its one measurement step ([`measure_incremental_point`]) with
/// [`rfc133_node_count_memory`], which passes [`Scope::Full`]; this passes [`Scope::ReleaseGate`].
///
/// ```text
/// cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_release_gate
/// ```
#[test]
#[ignore = "release-gate measurement profile; run deliberately, see module docs"]
fn rfc133_node_count_memory_release_gate() {
    let revision = RunRevision::capture();
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

    let unit = budget::Unit::begin(
        "rfc133-node-count-memory (release-gate profile)",
        RELEASE_GATE_BUDGET,
        &measurements_dir().join(revision.file_name("node-count-memory-measurement-release-gate")),
    );
    let mut incremental_rss_series = Vec::new();
    let mut incremental_elapsed_secs = Vec::new();
    let mut incremental_cache_series = Vec::new();
    for &node_count in &RELEASE_GATE_NODE_COUNTS {
        let point = measure_incremental_point(&unit, node_count, Scope::ReleaseGate);
        incremental_rss_series.push(point.rss);
        incremental_elapsed_secs.push(point.elapsed_secs);
        incremental_cache_series.push(point.cache);
    }

    let report = render_release_gate_report(
        &revision.stamp(),
        &incremental_rss_series,
        &incremental_elapsed_secs,
        &incremental_cache_series,
    );
    let report = format!("{report}\n{}", unit.finish());
    revision.write_report("node-count-memory-measurement-release-gate", &report);
}

/// §6c.2 — attribution. Does not re-run step 1's genesis series (out of scope here); reuses the
/// incremental series' own repositories and measured commits to also read (b)/(c), and adds the
/// isolated `NodeLifecycleState` probe series (a). See module docs for what each column means and
/// why only (a) is subtracted from RSS growth in the residual.
#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn rfc133_node_count_memory_attribution() {
    let revision = RunRevision::capture();
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
        &revision.stamp(),
        floor_kib,
        isolated_floor_kib,
        &incremental_rss_series,
        &isolated_probe_series,
        &commit_index_series,
        &object_index_series,
        &sweep_smallest,
        &sweep_largest,
    );
    revision.write_report("node-count-memory-attribution", &report);
}

/// One node-count point's resident-index measurement: both probe modes' RSS series, the exact
/// indexed-object count the repository held when they were taken, and whether §6d.3's standing
/// control (resident cost >= the structure's own physical minimum) holds for each mode.
struct ObjectIndexPoint {
    node_count: usize,
    indexed_objects: u64,
    read_snapshot: RssSeries,
    write_session: RssSeries,
    read_holds: bool,
    write_holds: bool,
}

fn render_object_index_report(
    revision: &str,
    binary_identity: &str,
    floor_kib: i64,
    points: &[ObjectIndexPoint],
) -> String {
    let mut out = String::new();
    out.push_str(
        "# RFC 133 §6d.3 — the object index's resident cost, re-measured unmasked, report v1\n\n",
    );
    out.push_str("Generated by (build, then run):\n\n```sh\ncargo build --release -p prikk-benchmarks --locked --bin rusage-object-index-probe\nPRIKK_RUSAGE_PROBE_BIN=target/release/rusage-object-index-probe \\\n  cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_object_index\n```\n");
    out.push_str("Each run writes a new file under `.git-exclude/measurements/rfc133/`, named for the revision it measured. Comparing this series against the attribution round's own residual is done by hand in the narrative report, not baked into this file, so this file does not go stale if the attribution report is ever regenerated on different hardware.\n\n");
    out.push_str(&format!("Revision measured at: `{}`. Probe binary identity: `{binary_identity}`. Release build, Linux, worktrees under a `tmpfs` temp directory, peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` via `tests/support/rusage_child.zsh` (not `rusage_child.py` -- see module docs for why this one probe needs the smaller spawning process). {SAMPLES_PER_POINT} samples per point. Process-startup floor (minimal companion binary, no repository opened), median = {floor_kib} KiB.\n\n", revision));

    out.push_str("## §6d.3's REQUIRED standing control: resident cost >= structure's own physical minimum (88 bytes/entry)\n\n");
    out.push_str("| N | indexed objects | minimum resident (bytes) | read growth (bytes) | read holds? | write growth (bytes) | write holds? |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for point in points {
        let minimum_bytes = point.indexed_objects as i64 * INDEX_ENTRY_MIN_RESIDENT_BYTES;
        let read_growth_bytes = (point.read_snapshot.median() - floor_kib) * 1024;
        let write_growth_bytes = (point.write_session.median() - floor_kib) * 1024;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            point.node_count,
            point.indexed_objects,
            minimum_bytes,
            read_growth_bytes,
            if point.read_holds { "yes" } else { "**NO**" },
            write_growth_bytes,
            if point.write_holds { "yes" } else { "**NO**" },
        ));
    }

    out.push_str("\n## Full series (min/median/max KiB)\n\n");
    out.push_str("| N | indexed objects | (read) min/median/max KiB | (read) growth over floor (KiB) | (write) min/median/max KiB | (write) growth over floor (KiB) |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for point in points {
        out.push_str(&format!(
            "| {} | {} | {}/{}/{} | {} | {}/{}/{} | {} |\n",
            point.node_count,
            point.indexed_objects,
            point.read_snapshot.min(),
            point.read_snapshot.median(),
            point.read_snapshot.max(),
            point.read_snapshot.median() - floor_kib,
            point.write_session.min(),
            point.write_session.median(),
            point.write_session.max(),
            point.write_session.median() - floor_kib,
        ));
    }
    out.push('\n');

    out
}

/// §6d.3 — re-measure the resident-index probe with the masking defeated. Does not re-run the
/// attribution round or §6d.1's own two other series; builds its own repositories (one per N,
/// reused across every read/write sample -- neither `open()` mutates anything) and measures
/// `ObjectReadSnapshot`/`ObjectWriteSession` held live in the minimal companion binary, via
/// `rusage_child.zsh`. See module docs for the floor-masking finding this exists to fix.
///
/// §6d.5 -- the companion binary now lives in `tools/benchmarks` (`prikk-benchmarks`), not this
/// crate, so it must be built first and its path passed via `PRIKK_RUSAGE_PROBE_BIN` (see
/// `rusage_object_index_probe_binary`'s own doc comment for why, and its panic message for the
/// exact build command). No longer feature-gated: run it the same plain way as the other two
/// drivers in this file, with that one variable set.
#[test]
#[ignore = "long-running measurement instrument; run deliberately, see module docs"]
fn rfc133_node_count_memory_object_index() {
    let revision = RunRevision::capture();
    let probe = Command::new("zsh").arg("--version").output();
    if probe.is_err() || !probe.unwrap().status.success() {
        eprintln!(
            "skipping object-index resident-cost measurement: zsh is not on PATH (see \
             rusage_child.zsh's own doc comment for why this probe needs it)"
        );
        return;
    }
    let binary = rusage_object_index_probe_binary();
    let binary_identity = rusage_object_index_probe_identity(&binary);
    eprintln!("probe binary identity: {binary_identity}");

    let mut floor_samples = Vec::with_capacity(SAMPLES_PER_POINT);
    for _ in 0..SAMPLES_PER_POINT {
        floor_samples.push(measure_object_index_probe_floor_rss_kib(&binary));
    }
    let floor_series = RssSeries {
        node_count: 0,
        peak_kib: floor_samples,
    };
    let floor_kib = floor_series.median();
    eprintln!("object-index probe floor: {:?} KiB", floor_series.peak_kib);

    let mut points = Vec::new();
    let mut any_write_failed = false;
    for &node_count in &NODE_COUNTS {
        let root = unique_dir(&format!("object-index-{node_count}"));
        std::fs::create_dir_all(&root).unwrap();
        support::init(&root);
        let seed = CONTENT_SEED
            .wrapping_add(0xC000_0000)
            .wrapping_add(node_count as u64);
        let mut rng = SplitMix64::new(seed);
        let files = generate_tree(&root, node_count, &mut rng);
        support::ok(
            &support::commit(&root, "heads/main", "rfc133-object-index: baseline"),
            "baseline commit",
        );
        support::ok(&support::seal(&root, "heads/main"), "baseline seal");
        mutate_one_file(&root, &files, &mut rng);
        support::ok(
            &support::commit(&root, "heads/main", "rfc133-object-index: incremental"),
            "incremental commit",
        );

        let indexed_objects = indexed_object_count(&root);

        let mut read_kib = Vec::with_capacity(SAMPLES_PER_POINT);
        for _ in 0..SAMPLES_PER_POINT {
            read_kib.push(measure_object_index_probe_rss_kib(&binary, &root, "read"));
        }
        let mut write_kib = Vec::with_capacity(SAMPLES_PER_POINT);
        for _ in 0..SAMPLES_PER_POINT {
            write_kib.push(measure_object_index_probe_rss_kib(&binary, &root, "write"));
        }
        let read_series = RssSeries {
            node_count,
            peak_kib: read_kib,
        };
        let write_series = RssSeries {
            node_count,
            peak_kib: write_kib,
        };
        let read_holds =
            object_index_resident_floor_holds(indexed_objects, read_series.median() - floor_kib);
        let write_holds =
            object_index_resident_floor_holds(indexed_objects, write_series.median() - floor_kib);
        if !write_holds {
            any_write_failed = true;
        }
        eprintln!(
            "object-index N={node_count}: indexed_objects={indexed_objects}, read {:?} KiB (holds={read_holds}), write {:?} KiB (holds={write_holds})",
            read_series.peak_kib, write_series.peak_kib
        );
        points.push(ObjectIndexPoint {
            node_count,
            indexed_objects,
            read_snapshot: read_series,
            write_session: write_series,
            read_holds,
            write_holds,
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    let report =
        render_object_index_report(&revision.stamp(), &binary_identity, floor_kib, &points);
    revision.write_report("object-index-resident-cost", &report);

    assert!(
        !any_write_failed,
        "the standing control failed for at least one point's write-session measurement -- see \
         the report just written for which; a reading below the structure's own physical minimum \
         is a measurement failure, not a finding"
    );
}

/// RFC 133 instrument-output handoff: a report is written under `.git-exclude/measurements/rfc133/`,
/// named for the revision measured, and never under `rfcs/`.
#[test]
fn reports_are_written_under_git_exclude_and_never_under_rfcs() {
    let clean = RunRevision {
        head: "0123456789abcdef0123".to_string(),
        dirty: false,
        rfcs_status: String::new(),
        started_at: 1_700_000_000,
    };
    let path = measurements_dir().join(clean.file_name("node-count-memory-measurement"));
    let text = path.to_string_lossy().replace('\\', "/");
    assert!(
        text.contains("/.git-exclude/measurements/rfc133/"),
        "{text}"
    );
    assert!(!text.contains("/rfcs/"), "{text}");
    assert!(
        text.ends_with("/node-count-memory-measurement-0123456789ab-1700000000.md"),
        "{text}"
    );
    let dirty = RunRevision {
        dirty: true,
        ..clean
    };
    assert!(dirty.file_name("x").ends_with("-dirty-1700000000.md"));
    assert!(
        dirty
            .stamp()
            .ends_with("(with uncommitted tracked changes)")
    );

    // And no writer in this file still names a tracked report path. The needle is assembled so this
    // assertion does not match itself.
    let needle = ["/../../", "rfcs/"].concat();
    assert!(
        !include_str!("rfc133_node_count_memory.rs").contains(&needle),
        "a report path under rfcs/ is back in this instrument"
    );
}

/// Measurement-cost handoff Addendum 2, control 1: two runs of the same revision, at different
/// `started_at`, write to different files -- the defect that actually happened (two clean release-gate
/// runs overwrote each other and one run's evidence was lost). *Perturb: drop `started_at` from
/// `file_name`'s format string -- this goes red, both names collapsing to the same file.*
#[test]
fn two_runs_of_the_same_revision_at_different_times_write_different_files() {
    let first = RunRevision {
        head: "0123456789abcdef0123".to_string(),
        dirty: false,
        rfcs_status: String::new(),
        started_at: 1_700_000_000,
    };
    let first_name = first.file_name("node-count-memory-measurement");
    let second = RunRevision {
        started_at: 1_700_000_600,
        ..first
    };
    let second_name = second.file_name("node-count-memory-measurement");
    assert_ne!(
        first_name, second_name,
        "two runs of the same revision at different times must not write the same file -- one run's \
         evidence silently overwrites the other's otherwise"
    );
    assert!(first_name.ends_with("-1700000000.md"), "{first_name}");
    assert!(second_name.ends_with("-1700000600.md"), "{second_name}");
}

/// A run whose HEAD moved between start and report refuses to write: the stamp would otherwise name a
/// revision the measured binary was not built from.
#[test]
fn a_run_whose_head_moved_refuses_to_write_its_report() {
    let revision = RunRevision {
        head: "aaaa".to_string(),
        dirty: false,
        rfcs_status: String::new(),
        started_at: 1_700_000_000,
    };
    assert!(revision.unchanged_at("aaaa").is_ok());
    let refusal = revision.unchanged_at("bbbb").unwrap_err();
    assert!(
        refusal.contains("HEAD moved during the run, from aaaa to bbbb"),
        "{refusal}"
    );
}

/// Measurement-cost handoff §2, control 1, and measurement-budget handoff §6.3: the release-gate profile's own report states plainly, in
/// its header, that it is the release-gate profile and not the full sweep **and which rows it measures**, and its tables carry exactly
/// [`RELEASE_GATE_NODE_COUNTS`] -- never a point the profile does not name -- and **no `tree`, `diff` or second-seal row**. Asserted on
/// the report text with synthetic series, never by running the instrument ("do not run the instrument to test the instrument").
/// *Perturb: render a `## \`prikk tree\` series` section in the profile's report (or drop the header's "does not measure" sentence):
/// this goes red.*
#[test]
fn release_gate_report_names_itself_and_runs_exactly_its_own_node_counts() {
    let rss_series: Vec<RssSeries> = RELEASE_GATE_NODE_COUNTS
        .iter()
        .map(|&node_count| RssSeries {
            node_count,
            peak_kib: vec![1, 2, 3],
        })
        .collect();
    let cache_series: Vec<CacheSeries> = RELEASE_GATE_NODE_COUNTS
        .iter()
        .map(|&node_count| CacheSeries {
            node_count,
            bytes: vec![1, 2, 3],
        })
        .collect();
    let elapsed_secs = vec![12.5; RELEASE_GATE_NODE_COUNTS.len()];
    let report =
        render_release_gate_report("deadbeefcafe", &rss_series, &elapsed_secs, &cache_series);

    assert!(
        report.contains("RELEASE-GATE PROFILE") && report.contains("not the full sweep"),
        "the header must state plainly that this is the release-gate profile, not the full sweep: \
         {report}"
    );
    assert!(
        report.contains("measures **only** what release prep gates on")
            && report.contains("incremental commit's peak RSS")
            && report.contains("**persisted cache file size**")
            && report.contains("does not measure `prikk tree`"),
        "the header must name the rows the profile measures and the ones it does not: {report}"
    );
    // The row set: the incremental series and the cache control, and nothing else.
    let headings: Vec<&str> = report
        .lines()
        .filter(|line| line.starts_with("## "))
        .collect();
    assert_eq!(
        headings.len(),
        2,
        "exactly two sections, the incremental series and the cache-file control: {headings:?}"
    );
    assert!(headings[0].contains("Incremental series"), "{headings:?}");
    assert!(headings[1].contains("cache file size"), "{headings:?}");
    for forbidden in [
        "## `prikk tree` series",
        "## `prikk diff` series",
        "Two points",
        "Worktree**",
    ] {
        assert!(
            !report.contains(forbidden),
            "the release-gate report must not carry the {forbidden:?} rows: {report}"
        );
    }

    assert!(
        report.contains("elapsed (s)"),
        "the incremental table must carry an elapsed (s) column: {report}"
    );

    for &node_count in &RELEASE_GATE_NODE_COUNTS {
        assert!(
            report.contains(&format!("| {node_count} |")),
            "expected a row for N={node_count}: {report}"
        );
    }
    for &node_count in NODE_COUNTS
        .iter()
        .filter(|n| !RELEASE_GATE_NODE_COUNTS.contains(n))
    {
        assert!(
            !report.contains(&format!("| {node_count} |")),
            "the release-gate report must never carry a point the profile does not name: N={node_count}"
        );
    }

    // Row count, not just presence: the incremental section carries exactly one row per named N.
    let start = report.find("## Incremental series").unwrap();
    let section = &report[start..];
    let end = section[3..].find("\n## ").map_or(section.len(), |i| i + 3);
    let section = &section[..end];
    let data_rows = section
        .lines()
        .filter(|line| {
            line.starts_with("| ") && line.as_bytes().get(2).is_some_and(u8::is_ascii_digit)
        })
        .count();
    assert_eq!(
        data_rows,
        RELEASE_GATE_NODE_COUNTS.len(),
        "exactly {} rows, one per named N, in:\n{section}",
        RELEASE_GATE_NODE_COUNTS.len()
    );
}

/// Measurement-cost handoff §2, control 2: the full driver's defaults are pinned, so a later edit
/// that changes them has to say so here too.
#[test]
fn the_full_drivers_defaults_are_pinned() {
    assert_eq!(
        NODE_COUNTS,
        [100, 1_000, 4_000, 8_000, 16_000, 32_000, 64_000]
    );
    assert_eq!(SAMPLES_PER_POINT, 3);
    assert_eq!(RELEASE_GATE_NODE_COUNTS, [100, 32_000, 64_000]);
}

/// Measurement-cost handoff §2, control 3: the two entry points share one measurement
/// implementation. *Perturb: give the release-gate profile its own copy of the measurement step --
/// this goes red* (the call count below drops to 1, or a forked copy under a different name leaves
/// this exact call shape uncounted either way).
#[test]
fn the_release_gate_profile_shares_the_full_drivers_measurement_step() {
    let source = include_str!("rfc133_node_count_memory.rs");
    // Assembled, not written contiguously, so this assertion's own source does not match itself.
    let call_needle = ["measure_incremental", "_point(&unit, node_count, Scope::"].concat();
    assert_eq!(
        source.matches(&call_needle).count(),
        2,
        "the shared measurement step must be called exactly twice, with this exact call shape: once \
         by rfc133_node_count_memory (Scope::Full), once by rfc133_node_count_memory_release_gate \
         (Scope::ReleaseGate) -- sharing the one step, not a duplicate of it"
    );
}
