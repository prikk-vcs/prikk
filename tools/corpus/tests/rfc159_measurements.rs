//! RFC 159 design round, handoff §3: the measurements the build-cost curve does not make. `#[ignore]`d,
//! release only (`support::measurement_binary`, whose build Cargo records as optimized), one report per test under
//! `.git-exclude/measurements/rfc159/`, every report naming its binary by path, version output and sha256.
//!
//! Both need **kept repositories** from the build-cost curve (`PRIKK_BCC_KEEP_DIR`, `PRIKK_BCC_KEEP_AT`), so the
//! two binaries are measured against the same history:
//! `<PRIKK_159_KEPT>/sealed-d<depth>` is a repository sealed at exactly `<depth>` blocks.
//!
//! - [`seal_time_and_memory_at_depth`] -- one `seal` at each kept depth, wall time and **peak RSS by
//!   `getrusage(RUSAGE_CHILDREN)`** (`rusage_child.py`, never a sampled `VmHWM`), for the checkpoint block right
//!   after the kept depth and for the ordinary block after that. Each sample is a fresh copy.
//! - [`sync_catchup_at_depth`] -- a receiver at the kept depth accepts an artifact carrying `PRIKK_159_CATCHUP`
//!   (default 64) further blocks from a sender, then seals them all (`sync seal --claims`). `accept` and `seal` are
//!   timed apart, each with its peak RSS. **The artifact is built once** (by whichever binary runs first) into
//!   `PRIKK_159_ARTIFACT_DIR` and reused, so both binaries accept the same bytes.
//!
//! The elapsed times include `rusage_child.py`'s own start-up (Python, about 30 ms), the same for every sample of
//! every binary. Each step records the machine's load average at its start.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use prikk_corpus::{Profile, execute};

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

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|text| {
            text.split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|_| "unknown".to_string())
}

fn require_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `(elapsed, peak RSS in KiB, output)` of one command; the peak is read exactly from `getrusage` by
/// `rusage_child.py`.
fn run_rusage(command: &Command) -> (Duration, Option<u64>, Output) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/prikk-cli/tests/support/rusage_child.py");
    let cwd = command.get_current_dir().expect("a current dir");
    let mut wrapped = Command::new("python3");
    wrapped
        .arg(&script)
        .arg(cwd)
        .arg(command.get_program())
        .args(command.get_args())
        .current_dir(cwd);
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            wrapped.env(key, value);
        }
    }
    let start = std::time::Instant::now();
    let output = wrapped.output().expect("running rusage_child.py");
    let elapsed = start.elapsed();
    let peak = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|line| line.trim().parse().ok());
    (elapsed, peak, output)
}

fn median<T: Copy + PartialOrd>(values: &[T]) -> T {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted[sorted.len() / 2]
}

fn kept_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("PRIKK_159_KEPT")
            .expect("PRIKK_159_KEPT names the build-cost curve's keep directory"),
    )
}

fn binary_header(build: &str, identity: &execute::BinaryIdentity) -> String {
    format!(
        "**{build} build**: `{}` (`{}`), sha256 `{}`.",
        identity.path, identity.version_output, identity.sha256
    )
}

/// `PRIKK_159_TRACE=1` asks the prototype to print where a seal's parent-state derivation spent its time (a
/// binary without the trace ignores the variable): the line goes to stderr, never into a report.
fn traced(mut command: Command) -> Command {
    if std::env::var("PRIKK_159_TRACE").is_ok_and(|value| value == "1") {
        command.env("PRIKK_RFC159_TRACE", "1");
    }
    command
}

fn show_trace(label: &str, block: usize, output: &Output) {
    for line in String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|line| line.starts_with("rfc159:"))
    {
        eprintln!("[{label}] block {block}: {line}");
    }
}

#[test]
#[ignore = "RFC 159 seal time and peak RSS at kept depths; run deliberately"]
fn seal_time_and_memory_at_depth() {
    let profile = self_profile();
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let label = std::env::var("PRIKK_159_LABEL").unwrap_or_else(|_| build.to_string());
    let samples = env_usize("PRIKK_159_SAMPLES", 3);
    let depths: Vec<usize> = std::env::var("PRIKK_159_DEPTHS")
        .unwrap_or_else(|_| "256,1024".to_string())
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    let mut manifest = prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning");
    manifest.commits.truncate(FLOOR_DEPTH as usize);

    let mut report = format!(
        "# RFC 159 -- one `seal` at depth: wall time and peak RSS ({label})\n\n{} {samples} samples per cell, each a fresh copy of \
         the kept repository. Peak RSS is `getrusage(RUSAGE_CHILDREN)` by `rusage_child.py`; elapsed includes ~30 ms of \
         wrapper start-up. For each kept depth the next two blocks are sealed in turn, each timed. **Offset** is the block's \
         distance from the checkpoint at or before it: 0 is a checkpoint (it carries a snapshot and writes one); an anchored seal \
         folds `offset - 1` blocks, so its cost depends on the offset and a full-walk seal's does not.\n\n\
         | kept depth | block | offset | seal, ms per sample | peak RSS, KiB per sample | load at start |\n|---:|---:|---:|---|---|---|\n",
        binary_header(&label, &identity)
    );
    let fmt_f = |values: &[f64]| {
        values
            .iter()
            .map(|v| format!("{v:.0}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fmt_u = |values: &[u64]| {
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    for depth in depths {
        let kept = kept_dir().join(format!("sealed-d{depth}"));
        assert!(kept.is_dir(), "no kept repository at {}", kept.display());
        // Per measured block (depth+1, depth+2): (ms, KiB) per sample.
        let mut cells: [(Vec<f64>, Vec<u64>); 2] = Default::default();
        let mut loads = Vec::new();
        for sample in 0..samples {
            let copy = support::unique_dir(&format!("m159-seal-{depth}"));
            support::copy_dir_all(&kept, &copy);
            for (slot, cell) in cells.iter_mut().enumerate() {
                let block = depth + 1 + slot;
                execute::materialize_commit(&copy, &manifest.commits[block - 1])
                    .expect("materializing");
                execute::run_commit(binary, &copy, &profile, execute::REF_NAME, "next")
                    .expect("commit");
                if slot == 0 {
                    loads.push(load_average());
                }
                let mut command = traced(
                    execute::seal_command(binary, &copy, &profile, execute::REF_NAME).unwrap(),
                );
                // Tracing needs the child's stderr, which the rusage wrapper drops on success: a traced run is
                // for finding where the time goes, never for a reported figure (peak RSS reads 0).
                let (elapsed, peak, output) =
                    if std::env::var("PRIKK_159_TRACE").is_ok_and(|value| value == "1") {
                        let start = std::time::Instant::now();
                        let output = command.output().expect("running prikk");
                        (start.elapsed(), Some(0), output)
                    } else {
                        run_rusage(&command)
                    };
                require_success(&output, &format!("seal of block {block}"));
                show_trace(&label, block, &output);
                cell.0.push(elapsed.as_secs_f64() * 1000.0);
                cell.1.push(peak.expect("a peak RSS"));
                eprintln!(
                    "[{label}] kept depth {depth} sample {sample}: block {block}: {:.0} ms {} KiB",
                    cell.0.last().unwrap(),
                    cell.1.last().unwrap()
                );
            }
            let _ = std::fs::remove_dir_all(&copy);
        }
        for (slot, (ms, kib)) in cells.iter().enumerate() {
            let block = depth + 1 + slot;
            report.push_str(&format!(
                "| {depth} | {block} | {} | {} (median {:.0}) | {} (median {}) | {} |\n",
                (block - 1) % 64,
                fmt_f(ms),
                median(ms),
                fmt_u(kib),
                median(kib),
                if slot == 0 {
                    loads.join(" ; ")
                } else {
                    String::new()
                }
            ));
        }
    }
    std::fs::write(
        out_dir().join(format!("seal-time-memory-{label}.md")),
        &report,
    )
    .expect("writing the report");
    eprintln!("{report}");
}

#[test]
#[ignore = "RFC 159 seal-from-accepted catch-up cost; run deliberately"]
fn sync_catchup_at_depth() {
    let profile = self_profile();
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let label = std::env::var("PRIKK_159_LABEL").unwrap_or_else(|_| build.to_string());
    let samples = env_usize("PRIKK_159_SAMPLES", 3);
    let depth = env_usize("PRIKK_159_CATCHUP_DEPTH", 1024);
    let catchup = env_usize("PRIKK_159_CATCHUP", 64);
    let artifact_dir =
        PathBuf::from(std::env::var("PRIKK_159_ARTIFACT_DIR").expect("PRIKK_159_ARTIFACT_DIR"));
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let artifact = artifact_dir.join(format!("catchup-d{depth}-n{catchup}.bin"));
    let have_file = artifact_dir.join(format!("have-d{depth}.bin"));
    let kept = kept_dir().join(format!("sealed-d{depth}"));
    assert!(kept.is_dir(), "no kept repository at {}", kept.display());
    let mut manifest = prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning");
    manifest.commits.truncate(FLOOR_DEPTH as usize);

    let mut built_here = false;
    if !artifact.exists() {
        built_here = true;
        // The sender: a copy of the kept repository grown by `catchup` blocks; its `sync build` against the
        // receiver's have-list (the receiver is the kept repository itself).
        let receiver = support::unique_dir("m159-recv-have");
        support::copy_dir_all(&kept, &receiver);
        let output = execute::keyed_command(binary, &receiver, &profile)
            .unwrap()
            .args(["sync", "have", execute::REF_NAME, "--output"])
            .arg(&have_file)
            .output()
            .unwrap();
        require_success(&output, "sync have");
        let _ = std::fs::remove_dir_all(&receiver);
        let sender = support::unique_dir("m159-sender");
        support::copy_dir_all(&kept, &sender);
        for commit in manifest.commits.iter().skip(depth).take(catchup) {
            execute::materialize_commit(&sender, commit).expect("materializing");
            execute::run_commit(binary, &sender, &profile, execute::REF_NAME, "catch-up")
                .expect("commit");
            execute::run_seal(binary, &sender, &profile, execute::REF_NAME).expect("seal");
        }
        let output = execute::keyed_command(binary, &sender, &profile)
            .unwrap()
            .args(["sync", "build", execute::REF_NAME, "--have"])
            .arg(&have_file)
            .arg("--output")
            .arg(&artifact)
            .output()
            .unwrap();
        require_success(&output, "sync build");
        let _ = std::fs::remove_dir_all(&sender);
    }

    let mut accept_ms = Vec::new();
    let mut accept_kib = Vec::new();
    let mut seal_ms = Vec::new();
    let mut seal_kib = Vec::new();
    let mut loads = Vec::new();
    for sample in 0..samples {
        let copy = support::unique_dir("m159-catchup");
        support::copy_dir_all(&kept, &copy);
        let claims = copy.join("claims.txt");
        loads.push(load_average());
        let mut command = execute::keyed_command(binary, &copy, &profile).unwrap();
        command
            .args(["sync", "accept"])
            .arg(&artifact)
            .arg("--claims-out")
            .arg(&claims);
        let (elapsed, peak, output) = run_rusage(&command);
        require_success(&output, "sync accept");
        accept_ms.push(elapsed.as_secs_f64() * 1000.0);
        accept_kib.push(peak.expect("a peak RSS"));
        let mut command = execute::keyed_command(binary, &copy, &profile).unwrap();
        command
            .args(["sync", "seal", execute::REF_NAME, "--claims"])
            .arg(&claims);
        let (elapsed, peak, output) = run_rusage(&command);
        require_success(&output, "sync seal --claims");
        seal_ms.push(elapsed.as_secs_f64() * 1000.0);
        seal_kib.push(peak.expect("a peak RSS"));
        eprintln!(
            "[{label}] sample {sample}: accept {:.0} ms {} KiB; seal of {catchup} blocks {:.0} ms {} KiB",
            accept_ms.last().unwrap(),
            accept_kib.last().unwrap(),
            seal_ms.last().unwrap(),
            seal_kib.last().unwrap()
        );
        let _ = std::fs::remove_dir_all(&copy);
    }
    let fmt_f = |values: &[f64]| {
        values
            .iter()
            .map(|v| format!("{v:.0}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fmt_u = |values: &[u64]| {
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let report = format!(
        "# RFC 159 -- seal-from-accepted: a {catchup}-block catch-up at depth {depth} ({label})\n\n{} A receiver at depth {depth} \
         accepts one artifact of {catchup} further blocks (built {}; the same bytes for every binary), then `sync seal \
         --claims` seals them all. {samples} samples, each a fresh copy. Peak RSS by `getrusage`; elapsed includes ~30 ms of \
         wrapper start-up.\n\n\
         | step | ms per sample | peak RSS, KiB per sample | load at start of the sample |\n|---|---|---|---|\n\
         | `sync accept` | {} (median {:.0}) | {} (median {}) | {} |\n\
         | `sync seal --claims` ({catchup} blocks) | {} (median {:.0}) | {} (median {}) | |\n\
         \nPer sealed block, median: **{:.0} ms**.\n",
        binary_header(&label, &identity),
        if built_here {
            "by this binary"
        } else {
            "earlier, by another run"
        },
        fmt_f(&accept_ms),
        median(&accept_ms),
        fmt_u(&accept_kib),
        median(&accept_kib),
        loads.join(" ; "),
        fmt_f(&seal_ms),
        median(&seal_ms),
        fmt_u(&seal_kib),
        median(&seal_kib),
        median(&seal_ms) / catchup as f64
    );
    std::fs::write(out_dir().join(format!("sync-catchup-{label}.md")), &report)
        .expect("writing the report");
    eprintln!("{report}");
}
