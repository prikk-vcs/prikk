//! RFC 139 §6's build-cost curve -- `commit` and `seal` wall clock against sealed depth -- **in the build people
//! run** (release re-measurement handoff, `rfcs/handoffs/139-measurement-corpus/release-remeasurement-handoff-v1.md`
//! §1.1). `#[ignore]`d, an expensive deliberately-invoked instrument.
//!
//! **What changed from the debug-build instrument this replaces.**
//! - The binary is the release build (`support::measurement_binary`), or the debug one only when
//!   `PRIKK_MEASURE_PROFILE=debug` asks for a bridging column; the report says which.
//! - **Three independent samples** (`PRIKK_BCC_SAMPLES`, default 3) where the original had one: the sealing
//!   exponent was "the least-supported figure". Each sample is its own repository grown from the same plan, in
//!   sequence, never in parallel (a parallel run would time itself).
//! - **Every block's commit and seal time is kept** (a TSV per sample), so per-seal cost can be reported against
//!   depth separately from the cumulative cost, and both can be fitted.
//! - **The depth is decided by a rule, not by feel.** The plan is RFC 139 §6's floor (2,048). The first sample builds
//!   to 256 whatever it costs; then, at each checkpoint it has reached, it projects the build **to the next
//!   checkpoint** from the per-block times already measured (a log-log fit of commit + seal against depth, summed
//!   over the blocks still to build) and continues only while that build is projected to finish within
//!   `PRIKK_BCC_MAX_HOURS` (default 2). It stops at the first checkpoint that would not, and says which. The other
//!   samples build to the depth the first reached, so the samples are comparable.
//! - **Peak memory is not measured here.** Timing is this round's point; the original's `VmHWM` samples at 500 us
//!   would have put a polling thread beside the timed command. (RFC 133 owns memory, by `getrusage`.)
//! - The report goes to `.git-exclude/measurements/rfc139/`, not into `rfcs/`.
//!
//! **RFC 159 design round additions.**
//! - **Checkpoint blocks are reported apart from ordinary ones.** A block is a checkpoint when it carries a
//!   snapshot: the first block, then every 64th after (65, 129, ...). The main table's 16-block windows end at a
//!   multiple of 64 and so never contain one; the second table gives each checkpoint block's seal beside the mean of
//!   the eight ordinary blocks before it.
//! - **`PRIKK_BCC_KEEP_DIR` and `PRIKK_BCC_KEEP_AT`**: after the seal at each depth listed (comma separated), the
//!   repository is copied to `<KEEP_DIR>/sealed-d<depth>` (sample 0) or `<KEEP_DIR>/s<sample>-sealed-d<depth>` (the
//!   first `PRIKK_BCC_KEEP_SAMPLES` samples, default 1), outside any timed step, for the memory, catch-up and identity
//!   instruments to run against the same history.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use prikk_corpus::{Profile, execute};

mod support;

/// RFC 139 §6's floor: 32 `REANCHOR_BOUND` (64) intervals.
const FLOOR_DEPTH: u64 = 2048;

/// Depths reported in detail. Doubling, plus the floor.
const CHECKPOINTS: [u64; 7] = [32, 64, 128, 256, 512, 1024, 2048];

/// The depth the first sample always reaches, whatever it costs.
const ALWAYS_BUILD_TO: u64 = 256;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn out_dir() -> PathBuf {
    // `PRIKK_BCC_OUT_SUBDIR` (default `rfc139`): RFC 159's round writes under `rfc159`.
    let subdir = std::env::var("PRIKK_BCC_OUT_SUBDIR").unwrap_or_else(|_| "rfc139".to_string());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.git-exclude/measurements")
        .join(subdir);
    std::fs::create_dir_all(&dir).expect("creating the measurement directory");
    dir
}

/// One sample: every block's `(depth, commit ms, seal ms)`, and where it stopped.
struct Run {
    blocks: Vec<(u64, f64, f64)>,
    /// `(checkpoint not built, projected hours to build to it)` when the 2-hour rule stopped the sample.
    refused: Option<(u64, f64)>,
    total_seconds: f64,
}

/// Fit `y = exp(a) * depth^b` by least squares on `(ln depth, ln y)`, over blocks at or above `from_depth`.
/// Returns `(a, b)`.
fn power_fit(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    let (sx, sy): (f64, f64) = points
        .iter()
        .fold((0.0, 0.0), |acc, (x, y)| (acc.0 + x.ln(), acc.1 + y.ln()));
    let (mx, my) = (sx / n, sy / n);
    let (mut num, mut den) = (0.0, 0.0);
    for (x, y) in points {
        num += (x.ln() - mx) * (y.ln() - my);
        den += (x.ln() - mx).powi(2);
    }
    let b = num / den;
    (my - b * mx, b)
}

/// Grow one repository. `stop_at` fixes the depth (samples after the first); `None` applies the projection rule.
/// `keep` = `(directory, depths)`: copy the repository to `directory/sealed-d<depth>` after the seal at each depth.
fn grow(
    binary: &Path,
    profile: &Profile,
    manifest: &prikk_corpus::ActionManifest,
    stop_at: Option<u64>,
    max_hours: f64,
    label: &str,
    keep: Option<&(PathBuf, Vec<u64>)>,
    sample: usize,
) -> Run {
    let repo_root = support::unique_dir(label);
    execute::init_repository(binary, &repo_root).expect("init");
    let mut trusted = false;
    let mut blocks = Vec::new();
    let mut refused = None;
    let run_start = Instant::now();
    for (index, commit) in manifest.commits.iter().enumerate() {
        let depth = (index + 1) as u64;
        execute::materialize_commit(&repo_root, commit).expect("materializing commit");
        let start = Instant::now();
        execute::run_commit(
            binary,
            &repo_root,
            profile,
            execute::REF_NAME,
            &format!("corpus commit {index}"),
        )
        .expect("commit");
        let commit_ms = start.elapsed().as_secs_f64() * 1000.0;
        if !trusted {
            execute::trust_maintainer(binary, &repo_root, profile).expect("trust");
            trusted = true;
        }
        let start = Instant::now();
        execute::run_seal(binary, &repo_root, profile, execute::REF_NAME).expect("seal");
        let seal_ms = start.elapsed().as_secs_f64() * 1000.0;
        blocks.push((depth, commit_ms, seal_ms));
        if let Some((directory, depths)) = keep {
            if depths.contains(&depth) {
                let destination = if sample == 0 {
                    directory.join(format!("sealed-d{depth}"))
                } else {
                    directory.join(format!("s{sample}-sealed-d{depth}"))
                };
                let _ = std::fs::remove_dir_all(&destination);
                support::copy_dir_all(&repo_root, &destination);
                eprintln!(
                    "[{label}] kept the repository at depth {depth}: {}",
                    destination.display()
                );
            }
        }

        if CHECKPOINTS.contains(&depth) {
            eprintln!(
                "[{label}] depth {depth}: commit {commit_ms:.1} ms, seal {seal_ms:.1} ms, cumulative {:.1} s",
                run_start.elapsed().as_secs_f64()
            );
            if let Some(stop) = stop_at {
                if depth >= stop {
                    break;
                }
            } else if depth >= ALWAYS_BUILD_TO {
                let Some(next) = CHECKPOINTS.iter().copied().find(|c| *c > depth) else {
                    break;
                };
                // The projection: commit + seal per block, fitted over the blocks measured so far (from
                // depth 16, past the first blocks' fixed costs), summed over the blocks still to build.
                let points: Vec<(f64, f64)> = blocks
                    .iter()
                    .filter(|(d, _, _)| *d >= 16)
                    .map(|(d, c, s)| (*d as f64, c + s))
                    .collect();
                let (a, b) = power_fit(&points);
                let remaining_s: f64 = ((depth + 1)..=next)
                    .map(|d| a.exp() * (d as f64).powf(b) / 1000.0)
                    .sum();
                let projected_hours = (run_start.elapsed().as_secs_f64() + remaining_s) / 3600.0;
                eprintln!(
                    "[{label}] projection to {next}: fit exponent {b:.2}, {projected_hours:.2} h (limit {max_hours} h)"
                );
                if projected_hours > max_hours {
                    refused = Some((next, projected_hours));
                    break;
                }
            }
        }
    }
    let total_seconds = run_start.elapsed().as_secs_f64();
    let _ = std::fs::remove_dir_all(&repo_root);
    Run {
        blocks,
        refused,
        total_seconds,
    }
}

#[test]
#[ignore = "RFC 139 §6's build-cost curve, in release; expensive, run deliberately"]
fn build_cost_curve() {
    let profile = self_profile();
    let manifest =
        prikk_corpus::plan(&profile, FLOOR_DEPTH).expect("planning to the RFC 139 §6 floor");
    let (binary, build) = support::measurement_binary();
    let identity = execute::binary_identity(binary).expect("binary identity");
    let samples = env_f64("PRIKK_BCC_SAMPLES", 3.0) as usize;
    let max_hours = env_f64("PRIKK_BCC_MAX_HOURS", 2.0);
    let max_depth = env_f64("PRIKK_BCC_MAX_DEPTH", FLOOR_DEPTH as f64) as u64;
    let label = std::env::var("PRIKK_BCC_LABEL").unwrap_or_else(|_| build.to_string());
    let keep_samples = env_f64("PRIKK_BCC_KEEP_SAMPLES", 1.0) as usize;
    let keep: Option<(PathBuf, Vec<u64>)> = std::env::var("PRIKK_BCC_KEEP_DIR").ok().map(|dir| {
        let depths = std::env::var("PRIKK_BCC_KEEP_AT")
            .unwrap_or_default()
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .collect();
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("creating the keep directory");
        (dir, depths)
    });
    eprintln!(
        "build: {build}; binary {} ({}), sha256 {}",
        identity.path, identity.version_output, identity.sha256
    );

    let mut runs: Vec<Run> = Vec::new();
    let mut manifest = manifest;
    manifest.commits.truncate(max_depth as usize);
    for sample in 0..samples {
        let stop_at = runs
            .first()
            .map(|first| first.blocks.last().map_or(0, |(depth, _, _)| *depth));
        let run = grow(
            binary,
            &profile,
            &manifest,
            stop_at,
            max_hours,
            &format!("bcc-{label}-{sample}"),
            if sample < keep_samples {
                keep.as_ref()
            } else {
                None
            },
            sample,
        );
        let mut tsv = String::from("depth\tcommit_ms\tseal_ms\n");
        for (depth, commit_ms, seal_ms) in &run.blocks {
            tsv.push_str(&format!("{depth}\t{commit_ms:.3}\t{seal_ms:.3}\n"));
        }
        std::fs::write(
            out_dir().join(format!("build-curve-{label}-run{sample}.tsv")),
            tsv,
        )
        .expect("writing the per-block times");
        eprintln!(
            "[{label}] sample {sample}: {} blocks in {:.0} s",
            run.blocks.len(),
            run.total_seconds
        );
        runs.push(run);
    }

    // Report: per checkpoint, the median over samples of the block at the checkpoint, and of the mean over the
    // 16 blocks ending at it (a block's own time is noisy: this profile changes 1 to 66 files per commit).
    let deepest = runs[0].blocks.last().map_or(0, |(depth, _, _)| *depth);
    let mut report = format!(
        "# RFC 139 §6 -- corpus build-cost curve ({build})\n\nProfile `profiles/prikk-self.toml`, planned to {FLOOR_DEPTH}. \
         **{build} build**: `{}` (`{}`), sha256 `{}`. {samples} independent samples, each a repository grown in sequence. \
         Deepest depth reached: **{deepest}**.\n\n",
        identity.path, identity.version_output, identity.sha256,
    );
    if let Some((next, hours)) = runs[0].refused {
        report.push_str(&format!(
            "**Stopped by the {max_hours}-hour rule**: the build to checkpoint {next} was projected at {hours:.1} h.\n\n"
        ));
    }
    report.push_str("| depth | seal at the block (ms), per sample | seal, mean of the last 16 blocks (ms), median | commit, mean of the last 16 (ms), median | cumulative commit+seal (s), per sample |\n|---:|---|---:|---:|---|\n");
    for checkpoint in CHECKPOINTS.iter().copied().filter(|c| *c <= deepest) {
        let window = |run: &Run, pick: fn(&(u64, f64, f64)) -> f64| -> f64 {
            let lo = checkpoint.saturating_sub(15);
            let values: Vec<f64> = run
                .blocks
                .iter()
                .filter(|(d, _, _)| *d >= lo && *d <= checkpoint)
                .map(pick)
                .collect();
            values.iter().sum::<f64>() / values.len() as f64
        };
        let median = |mut values: Vec<f64>| {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            values[values.len() / 2]
        };
        let at_block: Vec<String> = runs
            .iter()
            .map(|run| {
                run.blocks
                    .iter()
                    .find(|(d, _, _)| *d == checkpoint)
                    .map_or("-".to_string(), |(_, _, s)| format!("{s:.1}"))
            })
            .collect();
        let cumulative: Vec<String> = runs
            .iter()
            .map(|run| {
                let total: f64 = run
                    .blocks
                    .iter()
                    .filter(|(d, _, _)| *d <= checkpoint)
                    .map(|(_, c, s)| c + s)
                    .sum();
                format!("{:.1}", total / 1000.0)
            })
            .collect();
        report.push_str(&format!(
            "| {checkpoint} | {} | {:.1} | {:.1} | {} |\n",
            at_block.join(", "),
            median(runs.iter().map(|r| window(r, |b| b.2)).collect()),
            median(runs.iter().map(|r| window(r, |b| b.1)).collect()),
            cumulative.join(", "),
        ));
    }
    // Checkpoint blocks (65, 129, ...) beside the eight ordinary blocks before each.
    report.push_str(
        "\n**Checkpoint blocks** (the block carries a snapshot: 65, 129, 193, ...), per sample, beside the mean seal of the eight ordinary blocks before it. Median over samples.\n\n\
         | checkpoint block | seal at it (ms), per sample | ordinary, mean of the 8 before (ms), median | ratio, median |\n|---:|---|---:|---:|\n",
    );
    let mut ratios: Vec<f64> = Vec::new();
    let mut checkpoint = 65_u64;
    while checkpoint <= deepest {
        let at: Vec<Option<f64>> = runs
            .iter()
            .map(|run| {
                run.blocks
                    .iter()
                    .find(|(d, _, _)| *d == checkpoint)
                    .map(|(_, _, s)| *s)
            })
            .collect();
        let before: Vec<f64> = runs
            .iter()
            .map(|run| {
                let values: Vec<f64> = run
                    .blocks
                    .iter()
                    .filter(|(d, _, _)| *d + 8 > checkpoint && *d < checkpoint)
                    .map(|(_, _, s)| *s)
                    .collect();
                values.iter().sum::<f64>() / values.len().max(1) as f64
            })
            .collect();
        let per_sample_ratio: Vec<f64> = at
            .iter()
            .zip(&before)
            .filter_map(|(a, b)| a.map(|a| a / b))
            .collect();
        if !per_sample_ratio.is_empty() {
            let mut sorted = per_sample_ratio.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median_ratio = sorted[sorted.len() / 2];
            ratios.push(median_ratio);
            let mut before_sorted = before.clone();
            before_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            report.push_str(&format!(
                "| {checkpoint} | {} | {:.1} | {:.2} |\n",
                at.iter()
                    .map(|v| v.map_or("-".to_string(), |v| format!("{v:.1}")))
                    .collect::<Vec<_>>()
                    .join(", "),
                before_sorted[before_sorted.len() / 2],
                median_ratio
            ));
        }
        checkpoint += 64;
    }
    if !ratios.is_empty() {
        let mut sorted = ratios.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        report.push_str(&format!(
            "\nCheckpoint / ordinary seal, median over the {} checkpoint blocks reached: **{:.2}x** (range {:.2}-{:.2}).\n",
            sorted.len(),
            sorted[sorted.len() / 2],
            sorted[0],
            sorted[sorted.len() - 1]
        ));
    }
    report.push_str("\nPer-block times: `build-curve-*-run*.tsv` beside this file; exponents are fitted from them (`fit_build_curve.py`).\n");
    std::fs::write(
        out_dir().join(format!("build-cost-curve-{label}.md")),
        &report,
    )
    .expect("writing the report");
    eprintln!("{report}");
}
