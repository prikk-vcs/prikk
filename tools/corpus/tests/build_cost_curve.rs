//! RFC 139 §6's own deliverable: the build-cost curve -- wall clock and peak RSS against sealed
//! depth, out to the depth the curve itself says is reachable -- and the depth target this increment
//! states against it. `#[ignore]`d, matching `dc59_commit_benchmark.rs`/`dc92_lineage_replay_
//! benchmark.rs`'s own precedent: an expensive, deliberately-invoked measurement instrument, not a
//! correctness test.
//!
//! **One repository, grown generation by generation to the target depth** (dc92's own technique):
//! rebuilding independently at every checkpoint would redo every smaller depth's work each time.
//! Every commit and every seal is timed individually; `CHECKPOINTS` reports a subset for
//! readability.
//!
//! **Peak RSS is folded into the same invocations, not a separate growth pass.** dc59/dc92's own
//! memory passes rebuild a second time because their timing pass uses `.output()` and the memory
//! pass needs `.spawn()` + `/proc` polling. Here, since this is already a single continuously-growing
//! repository, the commit at each checkpoint depth is simply spawned-and-polled instead of run via
//! `.output()` -- same invocation, same cost, both figures. Non-checkpoint depths stay on the cheaper
//! `.output()` path. **Peak RSS is Linux-only** (`/proc/<pid>/status`); a missed or unavailable sample
//! is reported as *not measured*, never as zero (DC-62's own discipline).
//!
//! **One sample run, not several.** Growing to RFC 139 §6's own 2,048-block floor is itself the
//! expensive part this measurement exists to price; running it several times for a median would
//! multiply that cost answering a shape question (does cost grow, and how) a single run already
//! answers. dc92's own "reduced sample count, stated rather than hidden" precedent, restated rather
//! than silently reused.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use prikk_corpus::{Profile, execute};

mod support;

/// RFC 139 §6's floor: 32 `REANCHOR_BOUND` (64) intervals.
const FLOOR_DEPTH: u64 = 2048;

/// Depths this run reports in detail. Doubling, plus the floor itself.
const CHECKPOINTS: [u64; 7] = [32, 64, 128, 256, 512, 1024, 2048];

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

struct DepthSample {
    depth: u64,
    commit_ms: f64,
    seal_ms: f64,
    /// Total wall-clock elapsed for every commit+seal so far, from the first commit through this
    /// checkpoint's own seal -- the practically useful figure ("how long to build a corpus of depth
    /// N"), and far more robust to this profile's own high per-commit variance (1 to 66 files
    /// changed, tens of bytes to megabytes) than any single checkpoint's own commit/seal time, which
    /// can be an outlier in either direction depending on what that one commit happened to draw.
    cumulative_ms: f64,
    tree_files: u64,
    peak_kb: Option<u64>,
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

/// Spawn `command`, poll for `VmHWM` while it runs, and return `(elapsed, peak_kb)`. Asserts the
/// command succeeded. Polling interval matches DC-62's own harness.
#[cfg(target_os = "linux")]
fn spawn_and_measure(mut command: Command, what: &str) -> (Duration, Option<u64>) {
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
    assert!(
        output.status.success(),
        "{what} failed (status {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    (elapsed, peak_kb)
}

fn count_tree_files(repo_root: &Path) -> u64 {
    fn walk(dir: &Path, count: &mut u64) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".prikk" {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), count);
            } else {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(repo_root, &mut count);
    count
}

#[test]
#[ignore = "RFC 139 §6's own measurement instrument; expensive, run deliberately"]
fn build_cost_curve() {
    let profile = self_profile();
    let manifest =
        prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning to the RFC 139 §6 floor");
    let binary = support::prikk_binary_path();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let repo_root = support::unique_dir("build-cost-curve");

    eprintln!("binary: {} ({})", identity.path, identity.version_output);
    eprintln!("binary sha256: {}", identity.sha256);
    eprintln!("planned commits: {}", manifest.commits.len());

    execute::init_repository(binary, &repo_root).expect("init");

    let mut trusted = false;
    let mut samples = Vec::new();
    let run_start = Instant::now();

    for (index, commit) in manifest.commits.iter().enumerate() {
        execute::materialize_commit(&repo_root, commit).expect("materializing commit");
        let depth = (index + 1) as u64;
        let message = format!("corpus commit {index}");
        let is_checkpoint = CHECKPOINTS.contains(&depth);

        #[cfg(target_os = "linux")]
        let (commit_elapsed, peak_kb) = if is_checkpoint {
            spawn_and_measure(
                execute::commit_command(binary, &repo_root, &profile, &message),
                "commit",
            )
        } else {
            let start = Instant::now();
            execute::run_commit(binary, &repo_root, &profile, &message).expect("commit");
            (start.elapsed(), None)
        };
        #[cfg(not(target_os = "linux"))]
        let (commit_elapsed, peak_kb) = {
            let start = Instant::now();
            execute::run_commit(binary, &repo_root, &profile, &message).expect("commit");
            (start.elapsed(), None)
        };

        if !trusted {
            execute::trust_maintainer(binary, &repo_root, &profile).expect("trust");
            trusted = true;
        }

        let seal_start = Instant::now();
        execute::run_seal(binary, &repo_root, &profile).expect("seal");
        let seal_elapsed = seal_start.elapsed();

        if is_checkpoint {
            let tree_files = count_tree_files(&repo_root);
            let cumulative_ms = run_start.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "depth {depth}: commit {} ms, seal {} ms, cumulative {:.0} ms, tree {tree_files} \
                 files, peak {}",
                fmt_ms(commit_elapsed),
                fmt_ms(seal_elapsed),
                cumulative_ms,
                peak_kb.map_or_else(|| "not measured".to_owned(), |kb| format!("{kb} KB")),
            );
            samples.push(DepthSample {
                depth,
                commit_ms: commit_elapsed.as_secs_f64() * 1000.0,
                seal_ms: seal_elapsed.as_secs_f64() * 1000.0,
                cumulative_ms,
                tree_files,
                peak_kb,
            });
        }
    }

    let mut report = String::new();
    report.push_str("# RFC 139 §6 -- corpus build-cost curve\n\n");
    report.push_str(&format!(
        "Generated by `cargo test -p prikk-corpus --locked --test build_cost_curve -- --ignored \
         --nocapture build_cost_curve`. Profile: `profiles/prikk-self.toml`. One growing repository, \
         one sample run (see module doc for why). Binary: `{}` (`{}`), sha256 `{}`.\n\n",
        identity.path, identity.version_output, identity.sha256,
    ));
    report.push_str(
        "| Sealed blocks (depth) | Tree files | Commit (ms) | Seal (ms) | Cumulative (s) | Peak VmHWM (KB) |\n",
    );
    report.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for sample in &samples {
        report.push_str(&format!(
            "| {} | {} | {:.2} | {:.2} | {:.1} | {} |\n",
            sample.depth,
            sample.tree_files,
            sample.commit_ms,
            sample.seal_ms,
            sample.cumulative_ms / 1000.0,
            sample
                .peak_kb
                .map_or_else(|| "not measured".to_owned(), |kb| kb.to_string()),
        ));
    }
    // Matches dc59/dc92's own established path for this kind of harness: a raw data artifact next
    // to the RFC's own handoff material, regenerated by re-running the instrument -- not narrative
    // RFC content, so distinct from this round's own prose report (which goes to
    // `.git-exclude/review-request/`, per this increment's handoff §9).
    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rfcs/handoffs/139-measurement-corpus/build-cost-curve-report-v1.md"
    );
    std::fs::write(report_path, &report).expect("writing report");
    eprintln!("report written to {report_path}");

    let _ = std::fs::remove_dir_all(&repo_root);
}
