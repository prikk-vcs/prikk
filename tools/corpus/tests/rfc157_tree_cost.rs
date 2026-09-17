//! RFC 157 tree-and-cat handoff, Stage 1 step 2: what `prikk tree` costs against `checkout --patch-plan` at
//! nominal depths 64 and 256 on RFC 139's corpus (`profiles/prikk-self.toml`).
//!
//! - One binary, the one this test builds (`support::prikk_binary_path`), times both commands, so both are
//!   built the same way.
//! - At each depth `.prikk` is copied once, and the two commands alternate in that copy, `SAMPLES` times
//!   each, so neither always runs first against a cold page cache.
//! - `tree` is timed in prose and in `--format json`; the stop compares each with `--patch-plan`'s prose.
//! - **The stop (handoff Stage 1 step 2):** `tree` above 1.25x `--patch-plan`'s median wall time at either
//!   depth. This instrument reports; the ratio is read by a person, and the test asserts only that every
//!   run succeeded and that `tree` lists exactly the files `--patch-plan` does.
//! - The table is written to `.git-exclude/measurements/rfc157/tree-cost.md` (RFC 133's rule).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use prikk_corpus::{Profile, execute};

mod support;

const DEPTHS: [u64; 2] = [64, 256];
const SAMPLES: usize = 9;
const STOP_RATIO: f64 = 1.25;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

fn timed(binary: &Path, cwd: &Path, args: &[&str]) -> (f64, Option<u64>, Output) {
    let mut command = Command::new(binary);
    command.current_dir(cwd).args(args);
    let (elapsed, peak_kb, output) = support::run_measured(command);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (elapsed.as_secs_f64() * 1000.0, peak_kb, output)
}

fn median(values: &[f64]) -> (f64, f64, f64) {
    support::median_range(&values.iter().copied().map(Some).collect::<Vec<_>>()).expect("samples")
}

struct Row {
    depth: u64,
    files: usize,
    plan: (f64, f64, f64),
    tree: (f64, f64, f64),
    tree_json: (f64, f64, f64),
    plan_peak_kb: Option<u64>,
    tree_peak_kb: Option<u64>,
}

#[test]
#[ignore = "RFC 157 Stage 1 cost measurement; expensive, run deliberately"]
fn tree_cost_against_patch_plan() {
    let binary = support::prikk_binary_path().to_path_buf();
    let identity = execute::binary_identity(&binary).expect("binary identity");
    let profile = self_profile();
    let depth = *DEPTHS.last().expect("depths");
    let manifest = prikk_corpus::plan(&profile, depth).expect("planning the corpus");
    let repo_root = support::unique_dir("rfc157-tree-cost");
    execute::init_repository(&binary, &repo_root).expect("init");

    let mut trusted = false;
    let mut rows = Vec::new();
    for (index, commit) in manifest.commits.iter().enumerate() {
        execute::materialize_commit(&repo_root, commit).expect("materializing commit");
        execute::run_commit(
            &binary,
            &repo_root,
            &profile,
            execute::REF_NAME,
            &format!("corpus commit {index}"),
        )
        .expect("commit");
        if !trusted {
            execute::trust_maintainer(&binary, &repo_root, &profile).expect("trust");
            trusted = true;
        }
        execute::run_seal(&binary, &repo_root, &profile, execute::REF_NAME).expect("seal");
        let nominal_depth = (index + 1) as u64;
        if !DEPTHS.contains(&nominal_depth) {
            continue;
        }

        let copy = support::unique_dir(&format!("rfc157-tree-cost-{nominal_depth}"));
        std::fs::create_dir_all(&copy).expect("creating copy");
        support::copy_prikk_only(&repo_root, &copy);
        let plan_args = ["checkout", "--patch-plan", "--ref", execute::REF_NAME];
        let tree_args = ["tree", "--ref", execute::REF_NAME];
        let tree_json_args = ["tree", "--ref", execute::REF_NAME, "--format", "json"];
        let (mut plan_ms, mut tree_ms, mut tree_json_ms) = (Vec::new(), Vec::new(), Vec::new());
        let (mut plan_peak, mut tree_peak) = (None::<u64>, None::<u64>);
        let mut plan_files = Vec::new();
        let mut tree_files = Vec::new();
        for sample in 0..SAMPLES {
            let order: [&[&str]; 3] = if sample % 2 == 0 {
                [&plan_args, &tree_args, &tree_json_args]
            } else {
                [&tree_json_args, &tree_args, &plan_args]
            };
            for args in order {
                let (ms, peak, output) = timed(&binary, &copy, args);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                if args == plan_args.as_slice() {
                    plan_ms.push(ms);
                    plan_peak = plan_peak.max(peak);
                    plan_files = stdout
                        .lines()
                        .filter_map(|line| line.strip_prefix("  file: "))
                        .map(str::to_string)
                        .collect();
                } else if args == tree_args.as_slice() {
                    tree_ms.push(ms);
                    tree_peak = tree_peak.max(peak);
                    tree_files = stdout
                        .lines()
                        .filter(|line| line.starts_with("  "))
                        .filter_map(|line| line.trim().splitn(4, ' ').nth(3))
                        .map(str::to_string)
                        .collect();
                } else {
                    tree_json_ms.push(ms);
                }
            }
        }
        assert_eq!(
            plan_files, tree_files,
            "tree lists exactly the files --patch-plan replays, at depth {nominal_depth}"
        );
        let _ = std::fs::remove_dir_all(&copy);
        let row = Row {
            depth: nominal_depth,
            files: tree_files.len(),
            plan: median(&plan_ms),
            tree: median(&tree_ms),
            tree_json: median(&tree_json_ms),
            plan_peak_kb: plan_peak,
            tree_peak_kb: tree_peak,
        };
        eprintln!(
            "depth {}: {} files; patch-plan {:.1} ms, tree {:.1} ms ({:.2}x), tree json {:.1} ms ({:.2}x)",
            row.depth,
            row.files,
            row.plan.0,
            row.tree.0,
            row.tree.0 / row.plan.0,
            row.tree_json.0,
            row.tree_json.0 / row.plan.0
        );
        rows.push(row);
    }
    let _ = std::fs::remove_dir_all(&repo_root);

    let kb =
        |peak: Option<u64>| peak.map_or_else(|| "not measured".to_string(), |kb| kb.to_string());
    let mut report = String::new();
    report.push_str("# RFC 157 Stage 1 -- `prikk tree` against `checkout --patch-plan`\n\n");
    report.push_str(&format!(
        "Generated by `cargo test -p prikk-corpus --locked --test rfc157_tree_cost -- --ignored --nocapture`. \
         Profile `profiles/prikk-self.toml`, nominal depths {DEPTHS:?}, {SAMPLES} alternating samples per \
         command in one copy of `.prikk`. Median (min-max) wall time, ms. Stop: `tree` above {STOP_RATIO}x \
         `--patch-plan`.\n\n- binary: `{}` (`{}`), sha256 `{}`\n\n",
        identity.path, identity.version_output, identity.sha256
    ));
    report.push_str(
        "| depth | files | --patch-plan | tree | tree / plan | tree --format json | json / plan | plan peak KB | tree peak KB |\n",
    );
    report.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for row in &rows {
        report.push_str(&format!(
            "| {} | {} | {:.1} ({:.1}-{:.1}) | {:.1} ({:.1}-{:.1}) | {:.2} | {:.1} ({:.1}-{:.1}) | {:.2} | {} | {} |\n",
            row.depth,
            row.files,
            row.plan.0,
            row.plan.1,
            row.plan.2,
            row.tree.0,
            row.tree.1,
            row.tree.2,
            row.tree.0 / row.plan.0,
            row.tree_json.0,
            row.tree_json.1,
            row.tree_json.2,
            row.tree_json.0 / row.plan.0,
            kb(row.plan_peak_kb),
            kb(row.tree_peak_kb),
        ));
    }
    let out_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc157");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("tree-cost.md"), &report).expect("writing the report");
    eprintln!("{report}");
}
