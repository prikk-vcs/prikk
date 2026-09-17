//! The warm-cache `commit` anomaly (`rfcs/handoffs/136-block-aggregation-payoff/warm-cache-commit-anomaly-handoff-v1.md`):
//! **which rung `commit` takes to its baseline at each depth, and why.** Measure-only, `#[ignore]`d.
//!
//! The corpus, depths and warm-cache sample are RFC 136 increment 3's (`rfc136_increment3.rs`): one
//! repository sealed by this tree's binary, and per sample a copy of `.prikk`, an untimed
//! `checkout --patch-materialize`, one new file, and a timed `commit`.
//!
//! **The rung is named twice, independently.**
//! - **Taken, by the binary:** the DC-64 cache header before and after the timed `commit`. An incremental
//!   step persists `steps + 1` over the tip, and a full replay persists `0`
//!   (`lifecycle_cache/incremental.rs::resolve_baseline_state`).
//! - **Why, by the probe:** `baseline_cache_rung_for_test_support` (feature `test-support`, never in the
//!   binary) reproduces that function's decision through the same loader and block step, and names the
//!   check that failed.
//!
//! A disagreement between the two is an instrument failure and panics.
//!
//! **Separating the instrument from the subject (§3.3):** after the depths, two further corpora are built
//! to one fast depth and one slow depth, and `commit` runs once **in place** in each: in the corpus's own
//! worktree, with no copy and no materialize.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use prikk_corpus::{PlannedAction, Profile, execute};
use prikk_store::{
    BaselineCacheRung, LifecycleCacheHeader, RepositoryLayout,
    baseline_cache_rung_for_test_support, lifecycle_cache_header_for_test_support,
};

mod support;

const DEPTHS: [u64; 5] = [32, 64, 128, 192, 256];
const SAMPLES: usize = 3;
/// Checkpoint blocks: a ref's first block and every 64th after it (RFC 136 §10.2).
const CHECKPOINT_BLOCKS: [u64; 4] = [1, 65, 129, 193];

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

fn in_place_depths() -> Vec<u64> {
    std::env::var("PRIKK_WARM_CACHE_IN_PLACE_DEPTHS")
        .unwrap_or_else(|_| "32,64".to_string())
        .split(',')
        .map(|depth| depth.trim().parse().expect("a depth"))
        .collect()
}

fn max_depth() -> u64 {
    std::env::var("PRIKK_WARM_CACHE_MAX_DEPTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(u64::MAX)
}

fn require(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn prikk(binary: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(dir).args(args);
    command
}

fn grow(
    binary: &Path,
    repo: &Path,
    profile: &Profile,
    index: usize,
    commit: &prikk_corpus::PlannedCommit,
    trusted: &mut bool,
) {
    execute::materialize_commit(repo, commit).expect("materializing commit");
    execute::run_commit(
        binary,
        repo,
        profile,
        execute::REF_NAME,
        &format!("corpus commit {index}"),
    )
    .expect("commit");
    if !*trusted {
        execute::trust_maintainer(binary, repo, profile).expect("trust");
        *trusted = true;
    }
    execute::run_seal(binary, repo, profile, execute::REF_NAME).expect("seal");
    let block = (index + 1) as u64;
    if CHECKPOINT_BLOCKS.contains(&block) {
        execute::branch_create(
            binary,
            repo,
            profile,
            &format!("heads/cp-{block}"),
            execute::REF_NAME,
        )
        .expect("checkpoint branch");
    }
}

fn header(dir: &Path) -> Option<LifecycleCacheHeader> {
    lifecycle_cache_header_for_test_support(&RepositoryLayout::open(dir.to_path_buf()).unwrap())
}

fn probe(dir: &Path) -> BaselineCacheRung {
    baseline_cache_rung_for_test_support(
        &RepositoryLayout::open(dir.to_path_buf()).unwrap(),
        execute::REF_NAME,
    )
    .expect("probe")
}

fn cache_sha(dir: &Path) -> Option<String> {
    std::fs::read(dir.join(".prikk/cache/lifecycle-state.v1"))
        .ok()
        .map(|bytes| prikk_hash::to_hex(&prikk_hash::sha256(&bytes)))
}

/// The rung the binary took, from the headers around the command.
fn taken(
    before: Option<LifecycleCacheHeader>,
    after: Option<LifecycleCacheHeader>,
) -> &'static str {
    match (before, after) {
        (_, None) => "no cache persisted",
        (Some(b), Some(a)) if a.steps_since_reanchor == b.steps_since_reanchor + 1 => "incremental",
        (_, Some(a)) if a.steps_since_reanchor == 0 => "full replay",
        _ => "unexplained header change",
    }
}

fn agrees(taken: &str, rung: &BaselineCacheRung) -> bool {
    match rung {
        BaselineCacheRung::Incremental => taken == "incremental",
        BaselineCacheRung::Genesis | BaselineCacheRung::StepError { .. } => false,
        _ => taken == "full replay",
    }
}

fn reason(rung: &BaselineCacheRung) -> String {
    match rung {
        BaselineCacheRung::Incremental => "(hit)".to_string(),
        BaselineCacheRung::FullReplayMissingBlobForLifecycleEffect { detail } => {
            format!("MissingBlobForLifecycleEffect: {detail}")
        }
        other => format!("{other:?}"),
    }
}

#[derive(Default)]
struct Row {
    depth: u64,
    elapsed: Vec<Duration>,
    taken: Vec<&'static str>,
    rung: Option<BaselineCacheRung>,
    corpus_header: Option<LifecycleCacheHeader>,
    tip_parent_is_cached: bool,
    tip_actions: (usize, usize, usize),
    copy_cache_identical: bool,
    materialize_left_cache_unchanged: bool,
}

fn tip_block(binary: &Path, dir: &Path) -> String {
    support::block_ids(binary, dir, execute::REF_NAME, 1).remove(0)
}

fn actions(commit: &prikk_corpus::PlannedCommit) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for action in &commit.actions {
        match action {
            PlannedAction::CreateFile { .. } => counts.0 += 1,
            PlannedAction::EditText { .. } => counts.1 += 1,
            PlannedAction::DeleteNode { .. } => counts.2 += 1,
        }
    }
    counts
}

fn out_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc136");
    std::fs::create_dir_all(&dir).expect("creating the measurement directory");
    dir
}

#[test]
#[ignore = "warm-cache commit anomaly: builds the corpus to depth 256 and names the rung commit takes"]
fn warm_cache_commit_rung_per_depth() {
    let binary = support::prikk_binary_path().to_path_buf();
    let identity = execute::binary_identity(&binary).expect("identity");
    let profile = self_profile();
    let depths: Vec<u64> = DEPTHS
        .iter()
        .copied()
        .filter(|d| *d <= max_depth())
        .collect();
    let manifest = prikk_corpus::plan(&profile, *depths.last().unwrap()).expect("planning");
    let repo = support::unique_dir("warm-cache-anomaly");
    execute::init_repository(&binary, &repo).expect("init");
    let mut trusted = false;
    let mut rows = Vec::new();

    for (index, commit) in manifest.commits.iter().enumerate() {
        grow(&binary, &repo, &profile, index, commit, &mut trusted);
        let depth = (index + 1) as u64;
        if !depths.contains(&depth) {
            continue;
        }
        let mut row = Row {
            depth,
            corpus_header: header(&repo),
            tip_actions: actions(commit),
            copy_cache_identical: true,
            materialize_left_cache_unchanged: true,
            ..Row::default()
        };
        // The tip's parent is the block the cache was written over (the corpus commit that made the tip
        // authored against it).
        let parents = support::block_ids(&binary, &repo, execute::REF_NAME, 2);
        row.tip_parent_is_cached = row.corpus_header.is_some_and(|h| {
            parents
                .get(1)
                .is_some_and(|p| *p == h.baseline_block_id.to_string())
        });
        let corpus_rung = probe(&repo);
        for round in 0..SAMPLES {
            let dir = support::unique_dir(&format!("warm-cache-{depth}-{round}"));
            support::copy_prikk_only(&repo, &dir);
            row.copy_cache_identical &= cache_sha(&dir) == cache_sha(&repo);
            require(
                &prikk(
                    &binary,
                    &dir,
                    &[
                        "checkout",
                        "--patch-materialize",
                        "--ref",
                        execute::REF_NAME,
                    ],
                )
                .output()
                .unwrap(),
                "untimed materialize",
            );
            row.materialize_left_cache_unchanged &= cache_sha(&dir) == cache_sha(&repo);
            std::fs::create_dir_all(dir.join("bench")).unwrap();
            std::fs::write(dir.join("bench/sample.txt"), b"a sample commit\n").unwrap();
            let rung = probe(&dir);
            assert_eq!(
                rung, corpus_rung,
                "the copy must decide as the corpus does at depth {depth}"
            );
            let before = header(&dir);
            let measured = support::run_measured(
                execute::commit_command(&binary, &dir, &profile, execute::REF_NAME, "sample")
                    .expect("commit command"),
            );
            require(&measured.2, "timed commit");
            let after = header(&dir);
            let took = taken(before, after);
            assert!(
                agrees(took, &rung),
                "INSTRUMENT FAILURE at depth {depth}: the binary took `{took}` but the probe says {rung:?}"
            );
            assert_eq!(
                after.map(|h| h.baseline_block_id.to_string()),
                Some(tip_block(&binary, &dir)),
                "the persisted cache must be over the tip"
            );
            eprintln!(
                "depth {depth} round {round}: {} ms, taken {took}, {}",
                measured.0.as_millis(),
                reason(&rung)
            );
            row.elapsed.push(measured.0);
            row.taken.push(took);
            let _ = std::fs::remove_dir_all(&dir);
        }
        row.rung = Some(corpus_rung);
        rows.push(row);
    }
    let _ = std::fs::remove_dir_all(&repo);

    // §3.3: in place, no copy and no materialize, in corpora of their own.
    let mut in_place = Vec::new();
    for depth in in_place_depths() {
        let manifest = prikk_corpus::plan(&profile, depth).expect("planning");
        let repo = support::unique_dir(&format!("warm-cache-in-place-{depth}"));
        execute::init_repository(&binary, &repo).expect("init");
        let mut trusted = false;
        for (index, commit) in manifest.commits.iter().enumerate() {
            grow(&binary, &repo, &profile, index, commit, &mut trusted);
        }
        std::fs::create_dir_all(repo.join("bench")).unwrap();
        std::fs::write(repo.join("bench/sample.txt"), b"a sample commit\n").unwrap();
        let rung = probe(&repo);
        let before = header(&repo);
        let measured = support::run_measured(
            execute::commit_command(&binary, &repo, &profile, execute::REF_NAME, "sample")
                .expect("commit command"),
        );
        require(&measured.2, "in-place commit");
        let took = taken(before, header(&repo));
        assert!(
            agrees(took, &rung),
            "INSTRUMENT FAILURE in place at depth {depth}"
        );
        eprintln!(
            "in place depth {depth}: {} ms, taken {took}, {}",
            measured.0.as_millis(),
            reason(&rung)
        );
        in_place.push((depth, measured.0, took, rung));
        let _ = std::fs::remove_dir_all(&repo);
    }

    let mut text = String::new();
    text.push_str("# Warm-cache `commit`: the rung taken per depth\n\n");
    text.push_str(&format!(
        "Generated by `cargo test -p prikk-corpus --locked --test warm_cache_commit_anomaly -- --ignored --nocapture`. \
         Profile `profiles/prikk-self.toml`; {SAMPLES} samples per depth. Binary `{}` (`{}`), sha256 `{}`.\n\n",
        identity.path, identity.version_output, identity.sha256
    ));
    text.push_str("| depth | median ms (min–max) | taken by the binary | probe: rung / miss reason | tip commit actions (create/edit/delete) | cache over tip's parent | cached steps | copy's cache identical | materialize left cache unchanged |\n|---:|---|---|---|---|---|---:|---|---|\n");
    for row in &rows {
        let mut ms: Vec<u128> = row.elapsed.iter().map(Duration::as_millis).collect();
        ms.sort_unstable();
        text.push_str(&format!(
            "| {} | {} ({}–{}) | {} | {} | {}/{}/{} | {} | {} | {} | {} |\n",
            row.depth,
            ms[ms.len() / 2],
            ms[0],
            ms[ms.len() - 1],
            row.taken.join(", "),
            reason(row.rung.as_ref().unwrap()),
            row.tip_actions.0,
            row.tip_actions.1,
            row.tip_actions.2,
            row.tip_parent_is_cached,
            row.corpus_header
                .map_or("-".into(), |h| h.steps_since_reanchor.to_string()),
            row.copy_cache_identical,
            row.materialize_left_cache_unchanged,
        ));
    }
    text.push_str("\n## In place (no copy, no materialize)\n\n| depth | ms | taken | probe |\n|---:|---:|---|---|\n");
    for (depth, elapsed, took, rung) in &in_place {
        text.push_str(&format!(
            "| {depth} | {} | {took} | {} |\n",
            elapsed.as_millis(),
            reason(rung)
        ));
    }
    std::fs::write(out_dir().join("warm-cache-commit-anomaly.md"), &text).expect("writing");
    eprintln!("{text}");
}
