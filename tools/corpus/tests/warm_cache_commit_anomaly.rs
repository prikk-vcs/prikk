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
//! **RFC 136 increment 2c's design round** (`baseline-reconstruction-increment-2c-handoff-v1.md`) extends this
//! instrument rather than forking it: the `two_c_*` tests at the end share its corpus, its `grow`, its
//! header/probe pair and its `taken`/`agrees` cross-check, and are `#[ignore]`d for the same reason. They
//! are meant to be run in this order, each from a tree that may carry a prototype (`PRIKK_2C_*` in each
//! test's own doc): `two_c_build_corpus`, then `two_c_chain_walk`, `two_c_warm_cells`,
//! `two_c_cold_and_merge_evidence`, `two_c_rung3_split_and_state_shape`.
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
    baseline_cache_rung_for_test_support, ladder_walk_for_test_support,
    lifecycle_cache_header_for_test_support, lifecycle_cache_state_digest_for_test_support,
    lifecycle_state_shape_for_test_support, replay_time_split_for_test_support,
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

// ---- RFC 136 increment 2c design round ---------------------------------------------------------------

const TWO_C_REF: &str = execute::REF_NAME;

/// The corpus depth (`PRIKK_2C_DEPTH`, default 256; a smaller one is for trying the instrument out).
fn two_c_depth() -> u64 {
    std::env::var("PRIKK_2C_DEPTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(256)
}

fn two_c_depths() -> Vec<u64> {
    DEPTHS
        .iter()
        .copied()
        .filter(|depth| *depth <= two_c_depth())
        .collect()
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("set {name}")))
}

fn label() -> String {
    std::env::var("PRIKK_2C_LABEL").unwrap_or_else(|_| "run".to_string())
}

fn binary_from_env(name: &str) -> PathBuf {
    std::env::var(name).map_or_else(
        |_| support::prikk_binary_path().to_path_buf(),
        PathBuf::from,
    )
}

fn rung_class(rung: &BaselineCacheRung) -> &'static str {
    match rung {
        BaselineCacheRung::Genesis => "genesis",
        BaselineCacheRung::Incremental => "incremental",
        BaselineCacheRung::FullReplayNoUsableCache => "no-cache",
        BaselineCacheRung::FullReplayHorizonMismatch => "horizon-mismatch",
        BaselineCacheRung::FullReplayReanchorDue { .. } => "reanchor-due",
        BaselineCacheRung::FullReplayBlockUnreadable => "block-unreadable",
        BaselineCacheRung::FullReplayParentMismatch { .. } => "parent-mismatch",
        BaselineCacheRung::FullReplayMissingBlobForLifecycleEffect { .. } => "dc65-miss",
        BaselineCacheRung::StepError { .. } => "step-error",
    }
}

const RUNG_CLASSES: [&str; 5] = [
    "incremental",
    "dc65-miss",
    "reanchor-due",
    "no-cache",
    "other",
];

/// The share table: per depth, over tips `1..=depth` (tip *j* is baseline block *j*, resolved by the corpus
/// commit that makes block *j + 1*), how many took each rung.
fn share_table(classes: &[String], depths: &[u64]) -> String {
    let mut text = String::from(
        "| depth | tips | incremental | dc65-miss | reanchor-due | no-cache | other | incremental share | dc65-miss share |\n|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for depth in depths {
        let upto: Vec<&String> = classes.iter().take(*depth as usize).collect();
        let count = |wanted: &str| {
            upto.iter()
                .filter(|class| {
                    if wanted == "other" {
                        !RUNG_CLASSES[..4].contains(&class.as_str())
                    } else {
                        class.as_str() == wanted
                    }
                })
                .count()
        };
        let total = upto.len().max(1);
        text.push_str(&format!(
            "| {depth} | {} | {} | {} | {} | {} | {} | {:.1} % | {:.1} % |\n",
            upto.len(),
            count("incremental"),
            count("dc65-miss"),
            count("reanchor-due"),
            count("no-cache"),
            count("other"),
            100.0 * count("incremental") as f64 / total as f64,
            100.0 * count("dc65-miss") as f64 / total as f64,
        ));
    }
    text
}

/// `(elapsed, peak RSS in KiB, output)` of one command, with the peak read exactly from
/// `getrusage(RUSAGE_CHILDREN)` by `rusage_child.py` (never a sampled `VmHWM`). The elapsed time includes the
/// wrapper's own start-up (Python, about 30 ms), the same for every sample of every binary.
fn run_rusage(command: &Command) -> (Duration, Option<u64>, std::process::Output) {
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

/// `(elapsed, peak KiB)` per sample.
type Samples = Vec<(Duration, Option<u64>)>;

fn median<T: Copy + Ord>(values: &[T]) -> T {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// **Builds the corpus once and keeps it** (`PRIKK_2C_CORPUS_OUT`, which must not exist yet): the sealed
/// repository at `<out>/corpus`, one `.prikk` copy per measurement cell under `<out>/cells/<depth>`, and per
/// tip the rung the binary took (cache header around the corpus `commit`) against the rung the probe named
/// before it (an instrument failure panics, at every tip, not only at the sampled depths).
///
/// `PRIKK_2C_BINARY` names the `prikk` to build with (default: this tree's). `PRIKK_2C_CELLS` is `miss`
/// (default: for each depth, the first tip at or after it whose baseline step misses on DC-65, so the
/// cell is a tip that edits previously edited text), `depth` (tip exactly at the depth) or `none`.
/// `PRIKK_2C_LABEL` names the report. The build time per block is recorded as the secondary signal
/// (review F1). One plan, to 256; depth *D* is block *D* of it, as in increment 3 (`plan(profile, N)`
/// rescales, so a plan to 64 is not the first 64 blocks of a plan to 256).
#[test]
#[ignore = "RFC 136 2c: builds the corpus to depth 256 and keeps it"]
fn two_c_build_corpus() {
    let binary = binary_from_env("PRIKK_2C_BINARY");
    let identity = execute::binary_identity(&binary).expect("identity");
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, two_c_depth()).expect("planning");
    let out = env_path("PRIKK_2C_CORPUS_OUT");
    assert!(!out.exists(), "{} already exists", out.display());
    std::fs::create_dir_all(&out).unwrap();
    let repo = out.join("corpus");
    execute::init_repository(&binary, &repo).expect("init");
    let cells_mode = std::env::var("PRIKK_2C_CELLS").unwrap_or_else(|_| "miss".to_string());
    let mut pending: Vec<u64> = two_c_depths();
    let mut cells: Vec<(u64, u64, String, u32)> = Vec::new();
    let mut trusted = false;
    let mut rows = String::from("baseline_block\tclass\ttaken\tsteps_after\tcommit_and_seal_ms\n");
    let mut classes: Vec<String> = Vec::new();
    let mut build_ms: Vec<u128> = Vec::new();
    let started = std::time::Instant::now();
    for index in 0..=manifest.commits.len() {
        // Tip `index` is sealed now; the cache (if any) is over block `index - 1`. The next corpus commit
        // resolves this tip as its baseline.
        let baseline_block = index as u64;
        let predicted = (index > 0).then(|| probe(&repo));
        if let Some(rung) = &predicted {
            let class = rung_class(rung);
            if cells_mode != "none" {
                let due: Vec<u64> = pending
                    .iter()
                    .copied()
                    .filter(|depth| {
                        baseline_block >= *depth && (cells_mode == "depth" || class == "dc65-miss")
                    })
                    .collect();
                for depth in due {
                    let cell = out.join(format!("cells/{depth}"));
                    support::copy_prikk_only(&repo, &cell);
                    let steps = header(&repo).map_or(0, |h| h.steps_since_reanchor);
                    cells.push((depth, baseline_block, class.to_string(), steps));
                    pending.retain(|pending_depth| *pending_depth != depth);
                }
            }
        }
        let Some(commit) = manifest.commits.get(index) else {
            break;
        };
        let before = header(&repo);
        let step = std::time::Instant::now();
        grow(&binary, &repo, &profile, index, commit, &mut trusted);
        let elapsed = step.elapsed();
        if let Some(rung) = predicted {
            let after = header(&repo);
            let took = taken(before, after);
            assert!(
                agrees(took, &rung),
                "INSTRUMENT FAILURE at tip {baseline_block}: the binary took `{took}` but the probe says {rung:?}"
            );
            let class = rung_class(&rung).to_string();
            rows.push_str(&format!(
                "{baseline_block}\t{class}\t{took}\t{}\t{}\n",
                after.map_or(0, |h| h.steps_since_reanchor),
                elapsed.as_millis()
            ));
            classes.push(class);
        }
        build_ms.push(elapsed.as_millis());
        eprintln!(
            "block {} sealed in {} ms (tip {baseline_block}: {})",
            index + 1,
            elapsed.as_millis(),
            classes.last().map_or("-", String::as_str)
        );
    }
    let total = started.elapsed();
    std::fs::write(out.join("build.tsv"), &rows).unwrap();
    let mut cells_text = String::new();
    for (depth, block, class, steps) in &cells {
        cells_text.push_str(&format!("{depth}\t{block}\t{class}\t{steps}\n"));
    }
    std::fs::write(out.join("cells.tsv"), &cells_text).unwrap();

    let mut text = format!(
        "# 2c corpus build: `{}`\n\nBinary `{}` (`{}`), sha256 `{}`. Cells: `{cells_mode}`. Build (commit + seal, every block): {} s in all.\n\n",
        label(),
        identity.path,
        identity.version_output,
        identity.sha256,
        total.as_secs()
    );
    text.push_str("Rung taken by the binary at every tip, cross-checked against the probe at every tip (an instrument failure panics).\n\n");
    text.push_str(&share_table(&classes, &two_c_depths()));
    text.push_str("\n| depth | build seconds up to it | median ms per block (commit + seal) |\n|---:|---:|---:|\n");
    for depth in two_c_depths() {
        let upto = &build_ms[..depth as usize];
        text.push_str(&format!(
            "| {depth} | {} | {} |\n",
            upto.iter().sum::<u128>() / 1000,
            median(upto)
        ));
    }
    text.push_str(
        "\n| cell depth | tip block | probe class at it | cached steps |\n|---:|---:|---|---:|\n",
    );
    for (depth, block, class, steps) in &cells {
        text.push_str(&format!("| {depth} | {block} | {class} | {steps} |\n"));
    }
    text.push_str(&format!("\nDepths with no cell: {pending:?}\n"));
    std::fs::write(out_dir().join(format!("2c-build-{}.md", label())), &text).expect("writing");
    eprintln!("{text}");
}

/// **The whole ladder over the kept corpus, tip by tip, in one process** (`PRIKK_2C_CORPUS_OUT`): a cache
/// that starts absent and is refreshed after each step, exactly as the corpus build's successive `commit`s
/// left it, on a copy of `<out>/corpus/.prikk`. At **every** tip it compares the state the ladder returned
/// with an independent full replay's, every field of it (`PartialEq` on the whole `NodeLifecycleState`:
/// live nodes, path index, `latest_tombstone_by_id`, `seen_ids`), and records the SHA-256 of each rendering.
/// A difference panics naming the tip. `PRIKK_2C_ASSERT_BUILD=1` also requires the rung the walk names at
/// each tip to equal the one `two_c_build_corpus` recorded, which holds when the corpus was built by this
/// tree's own binary (it validates the walk as an instrument). `PRIKK_2C_DROP_VERIFIED=1` deletes the copy's
/// replay-verified record first: with a prototype that anchors only at a verified block, the walk must then
/// name the rungs a build without the prototype took (`PRIKK_2C_ASSERT_BUILD=1` against a corpus that
/// binary built), and a prototype that trusts an unrecorded snapshot goes red.
#[test]
#[ignore = "RFC 136 2c: the ladder over the kept corpus, whole-state identity against full replay at every tip"]
fn two_c_chain_walk() {
    let out = env_path("PRIKK_2C_CORPUS_OUT");
    let scratch = support::unique_dir("two-c-walk");
    support::copy_prikk_only(&out.join("corpus"), &scratch);
    if std::env::var("PRIKK_2C_DROP_VERIFIED").is_ok_and(|value| value == "1") {
        // The replay-verified record is rebuildable; without it no snapshot may anchor a baseline.
        let record = scratch.join(".prikk/cache/replay-verified-blocks.v1");
        assert!(record.exists(), "the corpus has a replay-verified record");
        std::fs::remove_file(record).unwrap();
    }
    let layout = RepositoryLayout::open(scratch.clone()).unwrap();
    let compare = std::env::var("PRIKK_2C_COMPARE").map_or(true, |value| value != "0");
    let started = std::time::Instant::now();
    let tips = ladder_walk_for_test_support(&layout, TWO_C_REF, compare).expect("ladder walk");
    let took = started.elapsed();
    assert_eq!(tips.len() as u64, two_c_depth(), "the corpus depth");

    let mut rows = String::from(
        "tip\tclass\tsteps_after\tladder_ms\tidentical\tladder_digest\treplay_digest\tlive\ttombstones\n",
    );
    let mut classes = Vec::new();
    let mut first_difference = None;
    for (number, tip) in tips.iter().enumerate() {
        let class = rung_class(&tip.rung).to_string();
        let (ladder, replay) = tip.digests.clone().unwrap_or_default();
        rows.push_str(&format!(
            "{}\t{class}\t{}\t{}\t{:?}\t{ladder}\t{replay}\t{}\t{}\n",
            number + 1,
            tip.steps_after
                .map_or("-".to_string(), |steps| steps.to_string()),
            tip.elapsed.as_millis(),
            tip.identical,
            tip.live_nodes,
            tip.tombstones,
        ));
        if tip.identical == Some(false) && first_difference.is_none() {
            first_difference = Some(number + 1);
        }
        classes.push(class);
    }
    std::fs::write(out.join(format!("walk-{}.tsv", label())), &rows).unwrap();
    if std::env::var("PRIKK_2C_ASSERT_BUILD").is_ok_and(|value| value == "1") {
        let build = std::fs::read_to_string(out.join("build.tsv")).expect("build.tsv");
        let recorded: Vec<(String, String)> = build
            .lines()
            .skip(1)
            .map(|line| {
                let mut fields = line.split('\t');
                fields.next();
                (
                    fields.next().unwrap().to_string(),
                    fields.next().unwrap().to_string(),
                )
            })
            .collect();
        for (number, (class, took)) in recorded.iter().enumerate() {
            // The build records tips 1..=255 (the last block is sealed, never a baseline); the walk's
            // tip *j* is the same baseline.
            assert_eq!(
                &classes[number],
                class,
                "the walk and the build disagree at tip {} (build took `{took}`)",
                number + 1
            );
        }
    }
    let identical = tips
        .iter()
        .filter(|tip| tip.identical == Some(true))
        .count();
    let mut text = format!(
        "# 2c chain walk: `{}`\n\nCorpus `{}`. {} tips walked in {} s; whole-state comparison against full replay {}.\n\n",
        label(),
        out.display(),
        tips.len(),
        took.as_secs(),
        if compare { "on" } else { "off" }
    );
    if compare {
        text.push_str(&format!(
            "**Identical at {identical} of {} tips** (`NodeLifecycleState` equality: live nodes, path index, `latest_tombstone_by_id`, `seen_ids`).\n\n",
            tips.len()
        ));
    } else {
        text.push_str("**Identity not measured in this run** (`PRIKK_2C_COMPARE=0`).\n\n");
    }
    text.push_str(&share_table(&classes, &two_c_depths()));
    let ladder_ms: Vec<u128> = tips.iter().map(|tip| tip.elapsed.as_millis()).collect();
    text.push_str("\n| depth | ladder seconds over tips up to it |\n|---:|---:|\n");
    for depth in two_c_depths() {
        text.push_str(&format!(
            "| {depth} | {} |\n",
            ladder_ms[..depth as usize].iter().sum::<u128>() / 1000
        ));
    }
    std::fs::write(out_dir().join(format!("2c-walk-{}.md", label())), &text).expect("writing");
    eprintln!("{text}");
    let _ = std::fs::remove_dir_all(&scratch);
    if let Some(number) = first_difference {
        panic!(
            "the ladder's state differs from full replay's at tip {number}; see walk-{}.tsv",
            label()
        );
    }
    if compare {
        assert_eq!(identical, tips.len());
    }
}

fn read_cells(out: &Path) -> Vec<(u64, u64, String, u32)> {
    std::fs::read_to_string(out.join("cells.tsv"))
        .expect("cells.tsv")
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            (
                fields.next().unwrap().parse().unwrap(),
                fields.next().unwrap().parse().unwrap(),
                fields.next().unwrap().to_string(),
                fields.next().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

/// One sample of the warm cell: a copy of the cell, an untimed materialize, one new file, and the timed
/// `commit`, measured by `rusage_child.py`. Returns `(elapsed, peak KiB, taken, cache state digest)`.
fn warm_sample(
    binary: &Path,
    cell: &Path,
    profile: &Profile,
    baseline_cache_sha: &Option<String>,
    edit_path: Option<&str>,
    tag: &str,
) -> (Duration, Option<u64>, &'static str, Option<String>) {
    let dir = support::unique_dir(tag);
    support::copy_prikk_only(cell, &dir);
    assert_eq!(&cache_sha(&dir), baseline_cache_sha, "the copy's cache");
    require(
        &prikk(
            binary,
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
    assert_eq!(
        &cache_sha(&dir),
        baseline_cache_sha,
        "the untimed materialize must leave the cache alone"
    );
    match edit_path {
        // The file the tip block itself edited: its baseline content is an `EditText` result no one
        // stored, so authoring an edit of it materializes it (`current_text_for_node`).
        Some(path) => {
            let file = dir.join(path);
            let mut bytes = std::fs::read(&file).expect("the edited file is in the worktree");
            bytes.extend_from_slice(b"\nan edit made by the instrument\n");
            std::fs::write(file, bytes).unwrap();
        }
        None => {
            std::fs::create_dir_all(dir.join("bench")).unwrap();
            std::fs::write(dir.join("bench/sample.txt"), b"a sample commit\n").unwrap();
        }
    }
    let before = header(&dir);
    let command = execute::commit_command(binary, &dir, profile, execute::REF_NAME, "sample")
        .expect("commit command");
    let (elapsed, peak, output) = run_rusage(&command);
    require(&output, "timed commit");
    let after = header(&dir);
    let took = taken(before, after);
    assert_eq!(
        after.map(|h| h.baseline_block_id.to_string()),
        Some(tip_block(binary, &dir)),
        "the persisted cache must be over the tip"
    );
    let digest = lifecycle_cache_state_digest_for_test_support(
        &RepositoryLayout::open(dir.clone()).unwrap(),
    );
    let _ = std::fs::remove_dir_all(&dir);
    (elapsed, peak, took, digest)
}

/// **The warm `commit` at a tip that edits previously edited text, before vs with**, on the cells
/// `two_c_build_corpus` kept. `PRIKK_2C_BINARY_BEFORE` and `PRIKK_2C_BINARY_WITH` name the two `prikk`
/// binaries (the one without the prototype, and the one with it); `PRIKK_2C_PROBE_WORLD` says which of them
/// this test binary's own `prikk-store` describes (`before` or `with`), and for that one the taken rung
/// must equal the probe's or the run panics. Each cell gets `SAMPLES` rounds; in a round, both binaries run
/// on fresh copies of the same cell, in an order that alternates by round (interleaved, so a drifting load
/// lands on both). Peak RSS is `getrusage(RUSAGE_CHILDREN)`, exact. After every pair the two persisted
/// caches' states must be equal (the header aside): the binary's own output, end to end.
#[test]
#[ignore = "RFC 136 2c: warm commit cost, memory and rung at cells, before vs with"]
fn two_c_warm_cells() {
    let out = env_path("PRIKK_2C_CORPUS_OUT");
    let before_binary = binary_from_env("PRIKK_2C_BINARY_BEFORE");
    let with_binary = binary_from_env("PRIKK_2C_BINARY_WITH");
    let probe_world = std::env::var("PRIKK_2C_PROBE_WORLD").unwrap_or_else(|_| "before".into());
    assert!(probe_world == "before" || probe_world == "with");
    let profile = self_profile();
    let manifest = prikk_corpus::plan(&profile, two_c_depth()).expect("planning");
    let sample_kind = std::env::var("PRIKK_2C_SAMPLE").unwrap_or_else(|_| "new".to_string());
    assert!(sample_kind == "new" || sample_kind == "edit");
    let mut text = format!(
        "# 2c warm cells: `{}` (the timed commit {})\n\nbefore `{}` (sha256 `{}`), with `{}` (sha256 `{}`). Probe describes `{probe_world}`. {SAMPLES} interleaved rounds per cell; peak RSS by `getrusage(RUSAGE_CHILDREN)`; elapsed includes ~30 ms of wrapper start-up.\n\n",
        label(),
        if sample_kind == "edit" {
            "appends a line to the file the tip block edited"
        } else {
            "adds one new file"
        },
        before_binary.display(),
        execute::binary_identity(&before_binary).unwrap().sha256,
        with_binary.display(),
        execute::binary_identity(&with_binary).unwrap().sha256,
    );
    text.push_str("| cell depth | tip block | probe (this world) | before: ms (min–max) | with: ms (min–max) | ratio before/with | before: peak KiB | with: peak KiB | before took | with took | caches equal |\n|---:|---:|---|---|---|---:|---:|---:|---|---|---|\n");
    for (depth, block, class, _steps) in read_cells(&out) {
        let cell = out.join(format!("cells/{depth}"));
        let layout = RepositoryLayout::open(cell.clone()).unwrap();
        let cell_probe = baseline_cache_rung_for_test_support(&layout, execute::REF_NAME)
            .expect("probe at the cell");
        let baseline_cache_sha = cache_sha(&cell);
        let edit_path: Option<String> = (sample_kind == "edit").then(|| {
            manifest.commits[block as usize - 1]
                .actions
                .iter()
                .find_map(|action| match action {
                    PlannedAction::EditText { path, .. } => Some(path.clone()),
                    _ => None,
                })
                .expect("the tip block of a dc65-miss cell edits a file")
        });
        let mut before_runs = Vec::new();
        let mut with_runs = Vec::new();
        let mut caches_equal = true;
        for round in 0..SAMPLES {
            let order = if round % 2 == 0 {
                [("before", &before_binary), ("with", &with_binary)]
            } else {
                [("with", &with_binary), ("before", &before_binary)]
            };
            let mut digests = Vec::new();
            for (name, binary) in order {
                let sample = warm_sample(
                    binary,
                    &cell,
                    &profile,
                    &baseline_cache_sha,
                    edit_path.as_deref(),
                    &format!("two-c-warm-{depth}-{round}-{name}"),
                );
                eprintln!(
                    "cell {depth} (tip {block}) round {round} {name}: {} ms, peak {:?} KiB, took {}",
                    sample.0.as_millis(),
                    sample.1,
                    sample.2
                );
                if name == probe_world {
                    assert!(
                        agrees(sample.2, &cell_probe),
                        "INSTRUMENT FAILURE at cell {depth}: `{name}` took `{}` but the probe says {cell_probe:?}",
                        sample.2
                    );
                }
                digests.push(sample.3.clone());
                if name == "before" {
                    before_runs.push(sample);
                } else {
                    with_runs.push(sample);
                }
            }
            caches_equal &= digests[0].is_some() && digests[0] == digests[1];
        }
        let ms = |runs: &[(Duration, Option<u64>, &'static str, Option<String>)]| {
            runs.iter().map(|run| run.0.as_millis()).collect::<Vec<_>>()
        };
        let range = |values: &[u128]| {
            let mut sorted = values.to_vec();
            sorted.sort_unstable();
            format!(
                "{} ({}–{})",
                sorted[sorted.len() / 2],
                sorted[0],
                sorted[sorted.len() - 1]
            )
        };
        let peak = |runs: &[(Duration, Option<u64>, &'static str, Option<String>)]| {
            runs.iter()
                .map(|run| run.1.map_or("-".to_string(), |kib| kib.to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let took = |runs: &[(Duration, Option<u64>, &'static str, Option<String>)]| {
            runs.iter().map(|run| run.2).collect::<Vec<_>>().join(", ")
        };
        let (before_ms, with_ms) = (ms(&before_runs), ms(&with_runs));
        text.push_str(&format!(
            "| {depth} | {block} | {} | {} | {} | {:.2} | {} | {} | {} | {} | {caches_equal} |\n",
            reason(&cell_probe),
            range(&before_ms),
            range(&with_ms),
            median(&before_ms) as f64 / median(&with_ms).max(1) as f64,
            peak(&before_runs),
            peak(&with_runs),
            took(&before_runs),
            took(&with_runs),
        ));
        assert!(
            caches_equal,
            "the two binaries left different lifecycle states at cell {depth}"
        );
        assert!(!class.is_empty());
    }
    std::fs::write(
        out_dir().join(format!("2c-warm-{}-{sample_kind}.md", label())),
        &text,
    )
    .expect("writing");
    eprintln!("{text}");
}

/// **What the cold half and `merge-evidence` cost today** (`PRIKK_2C_CORPUS_OUT`, both binaries as in
/// `two_c_warm_cells`). Cold `commit`: each cell with its lifecycle cache deleted, so the ladder's rung 3
/// (full replay from the horizon) runs, at the cell's own depth. `merge-evidence`: on the kept corpus in
/// place, baseline block *D − 2*, left *D − 1*, right *D* (a linear stretch, so the two sides are one and two
/// blocks; the cost is `replay_derived_state` at the baseline, which no cache helps), labelled by *D*.
#[test]
#[ignore = "RFC 136 2c: cold commit and merge-evidence cost at five depths"]
fn two_c_cold_and_merge_evidence() {
    let out = env_path("PRIKK_2C_CORPUS_OUT");
    let before_binary = binary_from_env("PRIKK_2C_BINARY_BEFORE");
    let with_binary = binary_from_env("PRIKK_2C_BINARY_WITH");
    let profile = self_profile();
    let mut text = format!(
        "# 2c cold commit and merge-evidence: `{}`\n\n{SAMPLES} interleaved rounds; peak RSS by `getrusage(RUSAGE_CHILDREN)`.\n\n## Cold `commit` (cache deleted; rung 3)\n\n| cell depth | tip block | before: ms (min–max) | with: ms (min–max) | before: peak KiB | with: peak KiB |\n|---:|---:|---|---|---:|---:|\n",
        label()
    );
    for (depth, block, _class, _steps) in read_cells(&out) {
        let cell = out.join(format!("cells/{depth}"));
        let mut runs: [Samples; 2] = [Vec::new(), Vec::new()];
        for round in 0..SAMPLES {
            let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
            for which in order {
                let binary = if which == 0 {
                    &before_binary
                } else {
                    &with_binary
                };
                let dir = support::unique_dir(&format!("two-c-cold-{depth}-{round}-{which}"));
                support::copy_prikk_only(&cell, &dir);
                std::fs::remove_file(dir.join(".prikk/cache/lifecycle-state.v1")).ok();
                require(
                    &prikk(
                        binary,
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
                std::fs::remove_file(dir.join(".prikk/cache/lifecycle-state.v1")).ok();
                std::fs::create_dir_all(dir.join("bench")).unwrap();
                std::fs::write(dir.join("bench/sample.txt"), b"a sample commit\n").unwrap();
                let before = header(&dir);
                assert!(before.is_none(), "the cold sample must start with no cache");
                let command =
                    execute::commit_command(binary, &dir, &profile, execute::REF_NAME, "sample")
                        .expect("commit command");
                let (elapsed, peak, output) = run_rusage(&command);
                require(&output, "cold commit");
                assert_eq!(
                    header(&dir).map(|h| h.steps_since_reanchor),
                    Some(0),
                    "a cold commit is a full replay"
                );
                eprintln!(
                    "cold {depth} round {round} binary {which}: {} ms, {peak:?} KiB",
                    elapsed.as_millis()
                );
                runs[which].push((elapsed, peak));
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        let fmt_ms = |runs: &[(Duration, Option<u64>)]| {
            let mut ms: Vec<u128> = runs.iter().map(|run| run.0.as_millis()).collect();
            ms.sort_unstable();
            format!("{} ({}–{})", ms[ms.len() / 2], ms[0], ms[ms.len() - 1])
        };
        let fmt_peak = |runs: &[(Duration, Option<u64>)]| {
            runs.iter()
                .map(|run| run.1.map_or("-".to_string(), |kib| kib.to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        text.push_str(&format!(
            "| {depth} | {block} | {} | {} | {} | {} |\n",
            fmt_ms(&runs[0]),
            fmt_ms(&runs[1]),
            fmt_peak(&runs[0]),
            fmt_peak(&runs[1])
        ));
    }

    let corpus = out.join("corpus");
    let mut ids = support::block_ids(&before_binary, &corpus, execute::REF_NAME, 300);
    ids.reverse();
    assert_eq!(ids.len() as u64, two_c_depth());
    text.push_str("\n## `merge-evidence` (kept corpus, in place)\n\n| depth D (baseline D − 2) | before: ms (min–max) | with: ms (min–max) | before: peak KiB | with: peak KiB |\n|---:|---|---|---:|---:|\n");
    let merge_depths = two_c_depths();
    let mut merge: Vec<[Samples; 2]> = merge_depths
        .iter()
        .map(|_| [Vec::new(), Vec::new()])
        .collect();
    for round in 0..SAMPLES {
        for (slot, depth) in merge_depths.iter().enumerate() {
            let d = *depth as usize;
            let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
            for which in order {
                let binary = if which == 0 {
                    &before_binary
                } else {
                    &with_binary
                };
                let mut command = Command::new(binary);
                command.current_dir(&corpus).args([
                    "merge-evidence",
                    "--baseline-block",
                    &ids[d - 3],
                    "--left-block",
                    &ids[d - 2],
                    "--right-block",
                    &ids[d - 1],
                ]);
                let (elapsed, peak, output) = run_rusage(&command);
                require(&output, "merge-evidence");
                eprintln!(
                    "merge-evidence depth {depth} round {round} binary {which}: {} ms, {peak:?} KiB",
                    elapsed.as_millis()
                );
                merge[slot][which].push((elapsed, peak));
            }
        }
    }
    for (slot, depth) in merge_depths.iter().enumerate() {
        let fmt_ms = |runs: &[(Duration, Option<u64>)]| {
            let mut ms: Vec<u128> = runs.iter().map(|run| run.0.as_millis()).collect();
            ms.sort_unstable();
            format!("{} ({}–{})", ms[ms.len() / 2], ms[0], ms[ms.len() - 1])
        };
        let fmt_peak = |runs: &[(Duration, Option<u64>)]| {
            runs.iter()
                .map(|run| run.1.map_or("-".to_string(), |kib| kib.to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        text.push_str(&format!(
            "| {depth} | {} | {} | {} | {} |\n",
            fmt_ms(&merge[slot][0]),
            fmt_ms(&merge[slot][1]),
            fmt_peak(&merge[slot][0]),
            fmt_peak(&merge[slot][1])
        ));
    }
    std::fs::write(out_dir().join(format!("2c-cold-{}.md", label())), &text).expect("writing");
    eprintln!("{text}");
}

/// **Where a full replay's time goes, and what the history fields weigh** (`PRIKK_2C_CORPUS_OUT`): for
/// each depth, the split of a full replay of that block into block/patch reads, `EditText` effects,
/// `CreateFile` effects and the rest (option (i): if bookkeeping dominates, splitting the walk saves
/// little); and at each checkpoint block, the live nodes, tombstones and the bytes the DC-64 codec spends
/// on each (option (ii)). The split refuses to run if its timed fold builds a state different from the
/// product's.
#[test]
#[ignore = "RFC 136 2c: rung 3 time split and the size of the history fields per checkpoint"]
fn two_c_rung3_split_and_state_shape() {
    let out = env_path("PRIKK_2C_CORPUS_OUT");
    let scratch = support::unique_dir("two-c-split");
    support::copy_prikk_only(&out.join("corpus"), &scratch);
    let layout = RepositoryLayout::open(scratch.clone()).unwrap();
    let mut text = format!(
        "# 2c rung-3 time split and state shape: `{}`\n\nThree runs per depth; the median total is reported with that run's split.\n\n## Time split of a full replay\n\n| depth | total ms | block walk | patch read+decode | EditText effects | CreateFile effects | other effects | consistency check | blocks | patches | operations | EditText ops | CreateFile ops | text held (nodes / bytes) |\n|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
        label()
    );
    for depth in two_c_depths() {
        let mut splits: Vec<prikk_store::ReplayTimeSplit> = (0..SAMPLES)
            .map(|_| {
                replay_time_split_for_test_support(&layout, TWO_C_REF, depth as usize)
                    .expect("time split")
            })
            .collect();
        splits.sort_by_key(|split| split.total);
        let split = &splits[splits.len() / 2];
        let ms = |duration: Duration| format!("{:.0}", duration.as_secs_f64() * 1000.0);
        text.push_str(&format!(
            "| {depth} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} |\n",
            ms(split.total),
            ms(split.walk),
            ms(split.patch_read_decode),
            ms(split.edit_text_effect),
            ms(split.create_file_effect),
            ms(split.other_effect),
            ms(split.consistency_check),
            split.blocks,
            split.patches,
            split.operations,
            split.edit_text_operations,
            split.create_file_operations,
            split.text_cache_nodes,
            split.text_cache_bytes
        ));
    }
    text.push_str("\n## The history fields at each checkpoint block\n\n| checkpoint block | live nodes | tombstones | live records bytes | tombstone records bytes | whole cache file bytes |\n|---:|---:|---:|---:|---:|---:|\n");
    for block in CHECKPOINT_BLOCKS
        .iter()
        .copied()
        .filter(|block| *block <= two_c_depth())
        .chain([two_c_depth()])
    {
        let shape = lifecycle_state_shape_for_test_support(&layout, TWO_C_REF, block as usize)
            .expect("state shape");
        text.push_str(&format!(
            "| {block} | {} | {} | {} | {} | {} |\n",
            shape.live_nodes,
            shape.tombstones,
            shape.live_record_bytes,
            shape.tombstone_record_bytes,
            shape.cache_file_bytes
        ));
    }
    std::fs::write(out_dir().join(format!("2c-split-{}.md", label())), &text).expect("writing");
    eprintln!("{text}");
    let _ = std::fs::remove_dir_all(&scratch);
}
