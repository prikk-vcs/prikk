//! RFC 102, the append-length round -- the two measurement units of
//! `rfcs/handoffs/102-container-based-durability/append-length-without-reading-handoff-v1.md` §3. `#[ignore]`d instruments, release
//! build only, each under the measurement watcher (`tools/corpus/tests/support/budget.rs`): a budget declared **in source**, every
//! step's start, end, elapsed time and boot id in the report as it happens, and the unit stops itself at twice its budget.
//!
//! **Two binaries, alternating.** *This build* is the one `cargo test` built (`CARGO_BIN_EXE_prikk`); *the baseline* is 0.47.0's
//! release binary, named by `PRIKK_APPEND_BASELINE_BINARY` (a path, the only thing read from the environment; no budget is). At each point
//! the two arms run one after the other, the order rotated point by point, so neither is always first.
//!
//! - **M1 -- bytes read by a first commit.** `rchar` and `syscr` of the parent's `/proc/self/io` around the reaped `prikk commit` child (the
//!   kernel adds a reaped child's I/O to its parent's), and wall time, for a first commit of 1,000 / 2,000 / 4,000 files of 20 KB and of
//!   8,000 files of 40 B. Acceptance for this build: bytes read linear in N.
//! - **M2 -- peak RSS of a one-small-file commit** (`getrusage`, one fresh process each, through `rusage_child.py`) on a repository whose blob
//!   container is 8 / 64 / 256 MiB (4 MiB random files), three samples per point per arm. Acceptance for this build: the peak at 256 MiB
//!   within 10 % of the 8 MiB case.
//!
//! ```text
//! PRIKK_APPEND_BASELINE_BINARY=/path/to/0.47.0/prikk cargo test -p prikk --release --locked --test append_length_io -- --ignored --nocapture append_length_m1
//! PRIKK_APPEND_BASELINE_BINARY=/path/to/0.47.0/prikk cargo test -p prikk --release --locked --test append_length_io -- --ignored --nocapture append_length_m2
//! ```

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

mod support;

#[path = "../../../tools/corpus/tests/support/budget.rs"]
mod budget;

/// **Unit budgets** (the handoff's §3; stops itself at twice each).
const M1_BUDGET: Duration = Duration::from_secs(5 * 60);
const M2_BUDGET: Duration = Duration::from_secs(5 * 60);

/// M1's points: `(files, bytes per file)`.
const M1_POINTS: [(usize, usize); 4] = [
    (1_000, 20_000),
    (2_000, 20_000),
    (4_000, 20_000),
    (8_000, 40),
];
/// M2's blob-container sizes, MiB (each a run of 4 MiB random files), and samples per point per arm.
const M2_CONTAINER_MIB: [usize; 3] = [8, 64, 256];
const M2_SAMPLES: usize = 3;

const RUSAGE_CHILD_SCRIPT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rusage_child.py");

struct Arm {
    name: &'static str,
    binary: PathBuf,
    sha256: String,
}

fn sha256_of(path: &Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("?")
        .to_string()
}

fn arms() -> [Arm; 2] {
    let this = PathBuf::from(env!("CARGO_BIN_EXE_prikk"));
    let baseline = PathBuf::from(std::env::var("PRIKK_APPEND_BASELINE_BINARY").expect(
        "PRIKK_APPEND_BASELINE_BINARY must name 0.47.0's release binary (a release build of 21895f46, or the release asset)",
    ));
    [
        Arm {
            name: "0.47.0 (baseline)",
            sha256: sha256_of(&baseline),
            binary: baseline,
        },
        Arm {
            name: "this build",
            sha256: sha256_of(&this),
            binary: this,
        },
    ]
}

/// `binary args...` run in `repo` with the harness's key isolation and the fixed author key.
fn command(binary: &Path, repo: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(repo).args(args);
    support::isolate_key_environment_for(&mut cmd, Some(repo));
    cmd.env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID).env(
        "PRIKK_AUTHOR_SEED_FILE",
        support::seed_file(support::AUTHOR_SEED_HEX),
    );
    cmd
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "prikk-append-length-{tag}-{}-{}",
        std::process::id(),
        support::unique_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn proc_io() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/self/io").expect("/proc/self/io");
    let field = |name: &str| -> u64 {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim_start_matches(':').trim().parse().ok())
            .unwrap_or(0)
    };
    (field("rchar"), field("syscr"))
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => total += dir_bytes(&path),
            Ok(_) => total += entry.metadata().map_or(0, |m| m.len()),
            Err(_) => {}
        }
    }
    total
}

fn measurements_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc102");
    std::fs::create_dir_all(&dir).expect("creating the measurement directory");
    dir
}

fn stamp() -> String {
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    )
}

fn head() -> String {
    Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// A tree of `count` files of `size` bytes under `d0..d39`, content unique per file.
fn write_tree(root: &Path, count: usize, size: usize) {
    for index in 0..count {
        let dir = root.join(format!("d{}", index % 40));
        std::fs::create_dir_all(&dir).unwrap();
        let mut content = format!("line {index} ").repeat(size / 6 + 4);
        content.truncate(size);
        content.push('\n');
        std::fs::write(dir.join(format!("f{index}.txt")), content).unwrap();
    }
}

/// **M1.**
#[test]
#[ignore = "measurement unit M1 (bytes read by a first commit); run deliberately, release build"]
fn append_length_m1_bytes_read() {
    let arms = arms();
    let report = measurements_dir().join(format!("append-length-m1-{}-{}.md", head(), stamp()));
    let unit = budget::Unit::begin(
        "append-length M1: bytes read by a first commit",
        M1_BUDGET,
        &report,
    );
    let load = budget::load_average();
    let mut rows = String::from(
        "| point | arm | wall (s) | rchar (MB) | read syscalls | worktree (MB) | containers at the end (MB) | rchar / (worktree + containers) |\n|---|---|---:|---:|---:|---:|---:|---:|\n",
    );
    for (point_index, (count, size)) in M1_POINTS.iter().copied().enumerate() {
        for offset in 0..arms.len() {
            let arm = &arms[(offset + point_index) % arms.len()];
            let label = format!("{count} x {size} B, {}", arm.name);
            let root = scratch("m1");
            unit.step(&format!("{label}: init"), || {
                support::ok(
                    &command(&arm.binary, &root, &["init"]).output().unwrap(),
                    "init",
                );
            });
            unit.step(&format!("{label}: write the tree"), || {
                write_tree(&root, count, size)
            });
            let (before, syscr_before) = proc_io();
            let started = Instant::now();
            let output = unit.step(&format!("{label}: first commit"), || {
                command(
                    &arm.binary,
                    &root,
                    &["commit", "--ref", "heads/main", "-m", "base"],
                )
                .output()
                .unwrap()
            });
            let wall = started.elapsed().as_secs_f64();
            let (after, syscr_after) = proc_io();
            support::ok(&output, "commit");
            let worktree = (count * (size + 1)) as f64;
            let containers = dir_bytes(&root.join(".prikk/containers")) as f64;
            let rchar = (after - before) as f64;
            rows.push_str(&format!(
                "| {count} x {size} B | {} | {wall:.1} | {:.1} | {} | {:.1} | {:.1} | {:.2} |\n",
                arm.name,
                rchar / 1e6,
                syscr_after - syscr_before,
                worktree / 1e6,
                containers / 1e6,
                rchar / (worktree + containers)
            ));
            let _ = std::fs::remove_dir_all(&root);
        }
    }
    let steps = unit.finish();
    let binaries: String = arms
        .iter()
        .map(|arm| {
            format!(
                "- {}: `{}`, sha256 `{}`\n",
                arm.name,
                arm.binary.display(),
                arm.sha256
            )
        })
        .collect();
    let text = format!(
        "# RFC 102 append-length round -- M1: bytes read by a first commit (release)\n\nBoot `{}`, load at start {load}. Binaries (release):\n{binaries}\n{rows}\n{steps}",
        budget::boot_id()
    );
    std::fs::write(&report, &text).unwrap();
    eprintln!("{text}");
}

fn random_file(path: &Path, bytes: usize, state: &mut u64) {
    let mut out = Vec::with_capacity(bytes);
    while out.len() < bytes {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out.truncate(bytes);
    std::fs::write(path, out).unwrap();
}

/// Peak `RUSAGE_CHILDREN` RSS (KiB) of one `binary args...` in `repo`, in a fresh process (see `rfc133_node_count_memory.rs`).
fn rusage_peak_kib(binary: &Path, repo: &Path, args: &[&str]) -> i64 {
    let mut cmd = Command::new("python3");
    cmd.arg(RUSAGE_CHILD_SCRIPT)
        .arg(repo)
        .arg(binary)
        .args(args);
    support::isolate_key_environment_for(&mut cmd, Some(repo));
    cmd.env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID).env(
        "PRIKK_AUTHOR_SEED_FILE",
        support::seed_file(support::AUTHOR_SEED_HEX),
    );
    let output = cmd.output().expect("python3 rusage_child.py");
    assert!(
        output.status.success(),
        "measured child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("an integer")
}

/// **M2.**
#[test]
#[ignore = "measurement unit M2 (peak RSS of a one-small-file commit); run deliberately, release build"]
fn append_length_m2_peak_rss() {
    let arms = arms();
    let report = measurements_dir().join(format!("append-length-m2-{}-{}.md", head(), stamp()));
    let unit = budget::Unit::begin(
        "append-length M2: peak RSS of a one-small-file commit",
        M2_BUDGET,
        &report,
    );
    let load = budget::load_average();
    let mut rows = String::from(
        "| blob container | arm | peak RSS per sample (KiB) | median (KiB) |\n|---:|---|---|---:|\n",
    );
    let mut this_medians: Vec<(usize, i64)> = Vec::new();
    let mut state = 0x1020_3040_5060_7080_u64;
    for (point_index, mib) in M2_CONTAINER_MIB.iter().copied().enumerate() {
        // One repository per arm and point, built once (the big commit), then copied for each sample.
        let mut repositories = Vec::new();
        for offset in 0..arms.len() {
            let arm = &arms[(offset + point_index) % arms.len()];
            let label = format!("{mib} MiB, {}", arm.name);
            let root = scratch("m2");
            unit.step(&format!("{label}: init and write the files"), || {
                support::ok(
                    &command(&arm.binary, &root, &["init"]).output().unwrap(),
                    "init",
                );
                for index in 0..mib / 4 {
                    random_file(
                        &root.join(format!("b{index}.bin")),
                        4 * 1024 * 1024,
                        &mut state,
                    );
                }
            });
            unit.step(&format!("{label}: the big commit"), || {
                support::ok(
                    &command(
                        &arm.binary,
                        &root,
                        &["commit", "--ref", "heads/main", "-m", "big"],
                    )
                    .output()
                    .unwrap(),
                    "big commit",
                );
            });
            repositories.push((arm, root));
        }
        let mut peaks: Vec<Vec<i64>> = vec![Vec::new(); repositories.len()];
        for sample in 0..M2_SAMPLES {
            for (slot, (arm, root)) in repositories.iter().enumerate() {
                let label = format!("{mib} MiB, {}, sample {sample}", arm.name);
                let copy = scratch("m2-sample");
                unit.step(&format!("{label}: copy"), || {
                    support::copy_dir_recursive(root, &copy)
                });
                std::fs::write(copy.join("small.txt"), "one small file\n").unwrap();
                let kib = unit.step(&format!("{label}: the one-small-file commit"), || {
                    rusage_peak_kib(
                        &arm.binary,
                        &copy,
                        &["commit", "--ref", "heads/main", "-m", "small"],
                    )
                });
                peaks[slot].push(kib);
                let _ = std::fs::remove_dir_all(&copy);
            }
        }
        for (slot, (arm, root)) in repositories.iter().enumerate() {
            let mut sorted = peaks[slot].clone();
            sorted.sort_unstable();
            let median = sorted[sorted.len() / 2];
            rows.push_str(&format!(
                "| {mib} MiB | {} | {:?} | {median} |\n",
                arm.name, peaks[slot]
            ));
            if arm.name == "this build" {
                this_medians.push((mib, median));
            }
            let _ = std::fs::remove_dir_all(root);
        }
    }
    let steps = unit.finish();
    let small = this_medians.first().map_or(1, |m| m.1) as f64;
    let large = this_medians.last().map_or(1, |m| m.1) as f64;
    let verdict = format!(
        "**Acceptance (this build):** the one-small-file commit's peak RSS at {} MiB is {:.0} KiB against {:.0} KiB at {} MiB: **{:+.1} %** (the rule: within 10 %). **{}**",
        M2_CONTAINER_MIB[2],
        large,
        small,
        M2_CONTAINER_MIB[0],
        100.0 * (large - small) / small,
        if large <= small * 1.10 {
            "MET"
        } else {
            "NOT MET"
        }
    );
    let binaries: String = arms
        .iter()
        .map(|arm| {
            format!(
                "- {}: `{}`, sha256 `{}`\n",
                arm.name,
                arm.binary.display(),
                arm.sha256
            )
        })
        .collect();
    let text = format!(
        "# RFC 102 append-length round -- M2: peak RSS of a one-small-file commit (release)\n\nBoot `{}`, load at start {load}. Binaries (release):\n{binaries}\n{rows}\n{verdict}\n\n{steps}",
        budget::boot_id()
    );
    std::fs::write(&report, &text).unwrap();
    eprintln!("{text}");
    assert!(
        large <= small * 1.10,
        "M2's acceptance is not met; see the report"
    );
}
