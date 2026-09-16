//! DC-78 follow-ups §3: what the `has_object` index probe saves on the **seal** path.
//!
//! The deletion-preimage derivation (v2) added a check on every seal: for each `CreateFile` a candidate
//! block carries, does the store already hold its content Blob? That check was written as
//! `read_typed(id, Blob)?.is_some()`, which reads the whole object; `ObjectReader::has_object` answers
//! from the index. Seal happens on every ordinary commit-and-seal, so the review asked for the number.
//!
//! - **before** is a binary from `219de57a` (the v2 fix, with the `read_typed` probe), passed as
//!   `PRIKK_DC78_SEAL_BEFORE_BIN`;
//! - **after** is the binary this test builds.
//! - One corpus repository grown to [`DEPTH`], then per binary three samples: a fresh copy, one planned
//!   commit (untimed), and `seal` timed.
//!
//! Run with `PRIKK_DC78_SEAL_BEFORE_BIN=<path> cargo test -p prikk-corpus --locked --test dc78_seal_cost
//! -- --ignored --nocapture`. The table goes to `.git-exclude/measurements/dc78/seal-cost.md`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use prikk_corpus::{Profile, execute};
use std::path::{Path, PathBuf};

mod support;

/// One depth, as the review asked. Deep enough that a seal's own derivation walk is not trivial.
const DEPTH: u64 = 128;
const SAMPLES: usize = 3;

/// One binary's seal samples: its label, wall times, and peak memory.
type SealRow = (&'static str, Vec<Option<f64>>, Vec<Option<u64>>);

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

/// Copy the whole repository, worktree included: `commit` reads the worktree, unlike the read-only
/// measurements elsewhere in this crate.
fn copy_repository(repo_root: &Path, tag: &str) -> PathBuf {
    let dir = support::unique_dir(tag);
    std::fs::create_dir_all(&dir).expect("creating the copy");
    let mut stack = vec![repo_root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)
            .expect("reading the repository")
            .flatten()
        {
            let source = entry.path();
            let target = dir.join(source.strip_prefix(repo_root).expect("under the root"));
            if source.is_dir() {
                std::fs::create_dir_all(&target).expect("creating a directory");
                stack.push(source);
            } else {
                std::fs::copy(&source, &target).expect("copying a file");
            }
        }
    }
    dir
}

#[test]
#[ignore = "DC-78 follow-ups §3: builds the RFC 139 corpus and times seal; run with PRIKK_DC78_SEAL_BEFORE_BIN"]
fn seal_cost_before_and_after_the_index_probe() {
    let before = PathBuf::from(
        std::env::var("PRIKK_DC78_SEAL_BEFORE_BIN")
            .expect("PRIKK_DC78_SEAL_BEFORE_BIN names a debug prikk binary from before the change"),
    );
    let after = support::prikk_binary_path().to_path_buf();
    let before_identity = execute::binary_identity(&before).expect("before identity");
    let after_identity = execute::binary_identity(&after).expect("after identity");
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, DEPTH + 1).expect("planning");
    let repo = support::unique_dir("dc78-seal-cost");
    execute::init_repository(&after, &repo).expect("init");

    let mut trusted = false;
    for (index, commit) in manifest.commits.iter().enumerate().take(DEPTH as usize) {
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
    }

    let next = manifest.commits.last().expect("one more planned commit");
    let mut rows: Vec<SealRow> = Vec::new();
    for (label, binary) in [("before", &before), ("after", &after)] {
        let mut ms = Vec::new();
        let mut kb = Vec::new();
        for round in 0..SAMPLES {
            let copy = copy_repository(&repo, &format!("seal-{label}-{round}"));
            execute::materialize_commit(&copy, next).expect("materializing the extra commit");
            execute::run_commit(binary, &copy, &profile, execute::REF_NAME, "timed commit")
                .expect("the extra commit");
            let command = execute::seal_command(binary, &copy, &profile, execute::REF_NAME)
                .expect("seal command");
            let (elapsed, peak, output) = support::run_measured(command);
            assert!(
                output.status.success(),
                "{label} seal failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            ms.push(Some(elapsed.as_secs_f64() * 1000.0));
            kb.push(peak);
            let _ = std::fs::remove_dir_all(&copy);
        }
        eprintln!(
            "{label}: {:?} ms, peak {:?}",
            support::median_range(&ms),
            support::median_range(&kb)
        );
        rows.push((label, ms, kb));
    }
    let _ = std::fs::remove_dir_all(&repo);

    let median =
        |values: &[Option<f64>]| support::median_range(values).map_or(f64::NAN, |(m, _, _)| m);
    let mut report = format!(
        "# DC-78 follow-ups §3 -- `seal` before and after the index probe\n\n\
         Profile `profiles/prikk-self.toml`, depth {DEPTH}, {SAMPLES} samples: a fresh copy of the \
         repository, one planned commit (untimed), then `seal` timed.\n\n\
         - before (`read_typed` probe): `{}` (`{}`), sha256 `{}`\n\
         - after (`has_object` probe): `{}` (`{}`), sha256 `{}`\n\n\
         | binary | seal (ms), median (min-max) | peak KB |\n|---|---:|---:|\n",
        before_identity.path,
        before_identity.version_output,
        before_identity.sha256,
        after_identity.path,
        after_identity.version_output,
        after_identity.sha256,
    );
    for (label, ms, kb) in &rows {
        report.push_str(&format!(
            "| {label} | {} | {} |\n",
            support::median_range(ms).map_or_else(
                || "n/a".to_string(),
                |(m, lo, hi)| format!("{m:.0} ({lo:.0}-{hi:.0})")
            ),
            support::median_range(kb)
                .map_or_else(|| "not measured".to_string(), |(m, _, _)| m.to_string()),
        ));
    }
    let ratio = median(&rows[1].1) / median(&rows[0].1);
    report.push_str(&format!("\nafter / before: **{ratio:.2}**\n"));
    let out_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/dc78");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("seal-cost.md"), &report).expect("writing the table");
    eprintln!("{report}");
}
