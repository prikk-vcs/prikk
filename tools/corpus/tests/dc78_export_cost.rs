//! DC-78 v2 §3.3: what deriving a deletion's preimage costs `bundle export`, on RFC 139's corpus.
//!
//! **There is no before/after ratio to report, and the reason is the defect itself.** The corpus history
//! deletes text files it edited earlier, so a binary without the fix *refuses* to export it at every
//! depth (`missing blob object`). This instrument therefore records:
//!
//! - the fixed exporter's wall time and peak memory at three depths, three samples each;
//! - the unfixed binary's refusal at the same depths, timed, so the comparison is stated rather than
//!   implied (pass it as `PRIKK_DC78_BEFORE_BIN`);
//! - `checkout --patch-plan` at the same depth as a yardstick: it is one anchored replay of the same
//!   history, which is what the derivation adds at most.
//!
//! Run with `PRIKK_DC78_BEFORE_BIN=<path> cargo test -p prikk-corpus --locked --test dc78_export_cost --
//! --ignored --nocapture`. Numbers go to `.git-exclude/measurements/dc78/`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;

use prikk_corpus::{Profile, execute};

mod support;

const DEPTHS: [u64; 3] = [64, 128, 256];
const SAMPLES: usize = 3;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

fn prikk(binary: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(dir).args(args);
    command
}

/// `(median ms, min, max, peak KB median, every run's exit code)` for one command.
struct Cell {
    ms: Vec<Option<f64>>,
    kb: Vec<Option<u64>>,
    codes: Vec<Option<i32>>,
}

impl Cell {
    fn new() -> Self {
        Self {
            ms: Vec::new(),
            kb: Vec::new(),
            codes: Vec::new(),
        }
    }
    fn ms(&self) -> String {
        support::median_range(&self.ms).map_or_else(
            || "n/a".into(),
            |(m, lo, hi)| format!("{m:.0} ({lo:.0}–{hi:.0})"),
        )
    }
    fn kb(&self) -> String {
        support::median_range(&self.kb)
            .map_or_else(|| "not measured".into(), |(m, _, _)| format!("{m}"))
    }
    fn median_ms(&self) -> f64 {
        support::median_range(&self.ms).map_or(f64::NAN, |(m, _, _)| m)
    }
    fn outcome(&self) -> String {
        if self.codes.iter().all(|code| *code == Some(0)) {
            "exit 0".into()
        } else {
            format!("exit {:?}", self.codes.first().copied().flatten())
        }
    }
}

/// Time `args` in a copy of the repository's `.prikk`, `SAMPLES` times.
fn measure(binary: &Path, repo: &Path, tag: &str, args: &[&str]) -> (Cell, String) {
    let mut cell = Cell::new();
    let mut first_stderr = String::new();
    for round in 0..SAMPLES {
        let dir = support::unique_dir(&format!("{tag}-{round}"));
        support::copy_prikk_only(repo, &dir);
        let output_path = dir.join("out.bundle");
        let mut full: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        if args.first().is_some_and(|arg| *arg == "bundle") {
            full.push("--output".into());
            full.push(output_path.display().to_string());
        }
        let borrowed: Vec<&str> = full.iter().map(String::as_str).collect();
        let (elapsed, peak, output) = support::run_measured(prikk(binary, &dir, &borrowed));
        cell.ms.push(Some(elapsed.as_secs_f64() * 1000.0));
        cell.kb.push(peak);
        cell.codes.push(output.status.code());
        if round == 0 {
            first_stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    (cell, first_stderr)
}

#[test]
#[ignore = "DC-78 v2 §3.3: builds the RFC 139 corpus and times bundle export; run with PRIKK_DC78_BEFORE_BIN"]
fn export_cost_with_the_derivation() {
    let before = PathBuf::from(
        std::env::var("PRIKK_DC78_BEFORE_BIN")
            .expect("PRIKK_DC78_BEFORE_BIN names a debug prikk binary without the fix"),
    );
    let after = support::prikk_binary_path().to_path_buf();
    let before_identity = execute::binary_identity(&before).expect("before identity");
    let after_identity = execute::binary_identity(&after).expect("after identity");
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, *DEPTHS.last().unwrap()).expect("planning");
    let repo = support::unique_dir("dc78-export-cost");
    execute::init_repository(&after, &repo).expect("init");

    let mut rows = Vec::new();
    let mut trusted = false;
    for (index, commit) in manifest.commits.iter().enumerate() {
        execute::materialize_commit(&repo, commit).expect("materializing commit");
        execute::run_commit(
            &after,
            &repo,
            &profile,
            execute::REF_NAME,
            &format!("corpus commit {index}"),
        )
        .expect("commit");
        if !trusted {
            execute::trust_maintainer(&after, &repo, &profile).expect("trust");
            trusted = true;
        }
        execute::run_seal(&after, &repo, &profile, execute::REF_NAME).expect("seal");

        let depth = (index + 1) as u64;
        if !DEPTHS.contains(&depth) {
            continue;
        }
        let export = ["bundle", "export", "--ref", execute::REF_NAME];
        let (after_cell, _) = measure(&after, &repo, &format!("after-{depth}"), &export);
        let (before_cell, before_stderr) =
            measure(&before, &repo, &format!("before-{depth}"), &export);
        let (plan_cell, _) = measure(
            &after,
            &repo,
            &format!("plan-{depth}"),
            &["checkout", "--patch-plan", "--ref", execute::REF_NAME],
        );
        eprintln!(
            "depth {depth}: export after {} ms ({}), before {} ({}), patch-plan {} ms",
            after_cell.ms(),
            after_cell.outcome(),
            before_cell.ms(),
            before_cell.outcome(),
            plan_cell.ms()
        );
        rows.push((depth, after_cell, before_cell, before_stderr, plan_cell));
    }
    let _ = std::fs::remove_dir_all(&repo);

    let mut report = String::new();
    report.push_str("# DC-78 v2 -- what deriving a deletion's preimage costs `bundle export`\n\n");
    report.push_str(&format!(
        "Generated by `PRIKK_DC78_BEFORE_BIN=<path> cargo test -p prikk-corpus --locked --test \
         dc78_export_cost -- --ignored --nocapture`. Profile `profiles/prikk-self.toml`; one repository \
         sealed by the after binary; {SAMPLES} samples per cell, median (min-max).\n\n\
         - after (with the derivation): `{}` (`{}`), sha256 `{}`\n\
         - before (without it): `{}` (`{}`), sha256 `{}`\n\n\
         **There is no before/after ratio**: the corpus deletes text files it edited earlier, so the \
         unfixed exporter refuses it at every depth. The before column records that refusal and its \
         timing. `checkout --patch-plan` is one anchored replay of the same history -- the yardstick for \
         what a derivation replay costs.\n\n",
        after_identity.path,
        after_identity.version_output,
        after_identity.sha256,
        before_identity.path,
        before_identity.version_output,
        before_identity.sha256,
    ));
    report.push_str(
        "| depth | export, with derivation (ms) | peak KB | export, without (ms) | outcome | `--patch-plan` (ms) | export / patch-plan |\n|---:|---:|---:|---:|---|---:|---:|\n",
    );
    for (depth, after_cell, before_cell, _, plan_cell) in &rows {
        report.push_str(&format!(
            "| {depth} | {} | {} | {} | {} | {} | {:.2} |\n",
            after_cell.ms(),
            after_cell.kb(),
            before_cell.ms(),
            before_cell.outcome(),
            plan_cell.ms(),
            after_cell.median_ms() / plan_cell.median_ms(),
        ));
    }
    if let Some((_, _, _, stderr, _)) = rows.first() {
        report.push_str(&format!(
            "\nThe unfixed exporter's refusal:\n\n```\n{stderr}\n```\n"
        ));
    }
    let out_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/dc78");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("export-cost.md"), &report).expect("writing the table");
    eprintln!("{report}");

    for (depth, after_cell, _, _, _) in &rows {
        assert!(
            after_cell.outcome() == "exit 0",
            "the fixed exporter must succeed at depth {depth}"
        );
    }
}
