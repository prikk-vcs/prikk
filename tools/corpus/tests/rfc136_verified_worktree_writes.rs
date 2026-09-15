//! RFC 136 increment 2b, handoff §4's cost control: `checkout --patch-materialize` and `branch switch`
//! wall time and peak memory at the §9.3 depths, before and after anchored worktree writes, on RFC 139's
//! corpus (`profiles/prikk-self.toml`).
//!
//! - **After** is the binary this test builds (`support::prikk_binary_path`).
//! - **Before** is a binary from the commit before anchored worktree writes (`2d96196b`), passed as
//!   `PRIKK_RFC136_BEFORE_BIN`. It must be a debug build, as the corpus support is.
//! - One repository, sealed locally by the after binary, so every checkpoint is in the replay-verified
//!   record. `heads/base` is branched at the first block.
//! - At each checkpoint depth `.prikk` is copied per run, never the worktree:
//!   - `--patch-materialize --ref heads/main` is timed into an empty worktree;
//!   - `branch switch heads/main` is timed from `heads/base`; the copy is first materialized as
//!     `heads/main` and switched to `heads/base`, both untimed.
//! - The worktrees the two binaries write must be identical: anchoring changes cost, never output.
//! - The table is written to `.git-exclude/measurements/rfc136/verified-worktree-writes.md`.
//!
//! If the record costs more than it saves, the numbers say so and the increment stops (handoff §4).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use prikk_corpus::{Profile, execute};

mod support;

/// The depths RFC 139 increment 3's instrument measured §9.3's table at.
const CHECKPOINTS: [u64; 5] = [32, 64, 128, 192, 256];
const BASE_REF: &str = "heads/base";

type Tree = BTreeMap<PathBuf, (Vec<u8>, bool)>;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

#[cfg(target_os = "linux")]
fn read_vm_hwm_kb(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
}

/// `(elapsed, peak_kb, output)`, peak RSS sampled from `/proc` on Linux and `None` elsewhere.
fn run_measured(mut command: Command) -> (Duration, Option<u64>, Output) {
    #[cfg(target_os = "linux")]
    {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let start = Instant::now();
        let mut child = command.spawn().expect("spawning prikk");
        let pid = child.id();
        let mut peak_kb: Option<u64> = None;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = stdout {
                std::io::Read::read_to_end(&mut pipe, &mut bytes).ok();
            }
            bytes
        });
        let err_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = stderr {
                std::io::Read::read_to_end(&mut pipe, &mut bytes).ok();
            }
            bytes
        });
        let status = loop {
            if let Some(kb) = read_vm_hwm_kb(pid) {
                peak_kb = Some(peak_kb.map_or(kb, |current: u64| current.max(kb)));
            }
            if let Some(status) = child.try_wait().expect("waiting for prikk") {
                break status;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let elapsed = start.elapsed();
        let output = Output {
            status,
            stdout: out_reader.join().expect("stdout reader"),
            stderr: err_reader.join().expect("stderr reader"),
        };
        (elapsed, peak_kb, output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let start = Instant::now();
        let output = command.output().expect("running prikk");
        (start.elapsed(), None, output)
    }
}

fn copy_metadata(repo_root: &Path, tag: &str) -> PathBuf {
    let dir = support::unique_dir(tag);
    std::fs::create_dir_all(dir.join(".prikk")).expect("creating copy");
    copy_dir(&repo_root.join(".prikk"), &dir.join(".prikk"));
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("reading .prikk").flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            std::fs::create_dir_all(&target).expect("creating directory");
            copy_dir(&source, &target);
        } else {
            std::fs::copy(&source, &target).expect("copying file");
        }
    }
}

/// Every worktree file outside `.prikk`, with its bytes and executable bit.
fn worktree(root: &Path) -> Tree {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("reading worktree").flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".prikk") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                entry.metadata().expect("metadata").permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            let bytes = std::fs::read(&path).expect("reading worktree file");
            files.insert(
                path.strip_prefix(root).expect("under root").to_path_buf(),
                (bytes, executable),
            );
        }
    }
    files
}

fn prikk(binary: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(dir).args(args);
    command
}

struct Timed {
    elapsed: Duration,
    peak_kb: Option<u64>,
    tree: Tree,
    stderr: String,
}

fn require(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn finish(copy: PathBuf, measured: (Duration, Option<u64>, Output), what: &str) -> Timed {
    let (elapsed, peak_kb, output) = measured;
    require(&output, what);
    let timed = Timed {
        elapsed,
        peak_kb,
        tree: worktree(&copy),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    let _ = std::fs::remove_dir_all(&copy);
    timed
}

fn time_patch_materialize(binary: &Path, repo_root: &Path, tag: &str) -> Timed {
    let copy = copy_metadata(repo_root, tag);
    let measured = run_measured(prikk(
        binary,
        &copy,
        &[
            "checkout",
            "--patch-materialize",
            "--ref",
            execute::REF_NAME,
        ],
    ));
    finish(copy, measured, "checkout --patch-materialize")
}

fn time_branch_switch(binary: &Path, repo_root: &Path, tag: &str) -> Timed {
    let copy = copy_metadata(repo_root, tag);
    // `branch switch` refuses unless the worktree is clean against the current branch, so the copy is
    // first materialized as heads/main, untimed.
    require(
        &prikk(
            binary,
            &copy,
            &[
                "checkout",
                "--patch-materialize",
                "--ref",
                execute::REF_NAME,
            ],
        )
        .output()
        .expect("running prikk"),
        "untimed checkout --patch-materialize",
    );
    require(
        &prikk(binary, &copy, &["branch", "switch", BASE_REF])
            .output()
            .expect("running prikk"),
        "untimed branch switch to heads/base",
    );
    let measured = run_measured(prikk(
        binary,
        &copy,
        &["branch", "switch", execute::REF_NAME],
    ));
    finish(copy, measured, "branch switch heads/main")
}

fn kb(peak: Option<u64>) -> String {
    peak.map_or_else(|| "not measured".to_owned(), |kb| kb.to_string())
}

fn ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

type Timer = fn(&Path, &Path, &str) -> Timed;

#[test]
#[ignore = "RFC 136 increment 2b's cost measurement; expensive, run deliberately with PRIKK_RFC136_BEFORE_BIN"]
fn verified_worktree_writes_cost() {
    let before = PathBuf::from(
        std::env::var("PRIKK_RFC136_BEFORE_BIN")
            .expect("PRIKK_RFC136_BEFORE_BIN names the pre-anchoring prikk binary"),
    );
    let after = support::prikk_binary_path().to_path_buf();
    let before_identity = execute::binary_identity(&before).expect("before binary identity");
    let after_identity = execute::binary_identity(&after).expect("after binary identity");
    let profile = self_profile();
    let depth = *CHECKPOINTS.last().expect("checkpoints");
    let manifest = prikk_corpus::plan(&profile, depth).expect("planning the corpus");
    let repo_root = support::unique_dir("rfc136-verified-worktree-writes");
    execute::init_repository(&after, &repo_root).expect("init");

    let timers: [(&str, Timer); 2] = [
        ("checkout --patch-materialize", time_patch_materialize),
        ("branch switch", time_branch_switch),
    ];
    let mut trusted = false;
    let mut rows = Vec::new();
    for (index, commit) in manifest.commits.iter().enumerate() {
        execute::materialize_commit(&repo_root, commit).expect("materializing commit");
        execute::run_commit(
            &after,
            &repo_root,
            &profile,
            execute::REF_NAME,
            &format!("corpus commit {index}"),
        )
        .expect("commit");
        if !trusted {
            execute::trust_maintainer(&after, &repo_root, &profile).expect("trust");
            trusted = true;
        }
        execute::run_seal(&after, &repo_root, &profile, execute::REF_NAME).expect("seal");
        if index == 0 {
            execute::branch_create(&after, &repo_root, &profile, BASE_REF, execute::REF_NAME)
                .expect("branch create heads/base");
        }

        let nominal_depth = (index + 1) as u64;
        if !CHECKPOINTS.contains(&nominal_depth) {
            continue;
        }
        for (command, time) in timers {
            let tag = command.replace([' ', '-'], "");
            let before_run = time(
                &before,
                &repo_root,
                &format!("before-{tag}-{nominal_depth}"),
            );
            let after_run = time(&after, &repo_root, &format!("after-{tag}-{nominal_depth}"));
            assert!(
                before_run.tree == after_run.tree,
                "anchoring changed the worktree `{command}` writes at depth {nominal_depth}"
            );
            assert!(
                after_run.stderr.is_empty() && before_run.stderr.is_empty(),
                "`{command}` at depth {nominal_depth} warned: before {:?}, after {:?}",
                before_run.stderr,
                after_run.stderr
            );
            eprintln!(
                "{command} depth {nominal_depth}: before {:.1} ms (peak {}), after {:.1} ms (peak {}), {} files",
                ms(before_run.elapsed),
                kb(before_run.peak_kb),
                ms(after_run.elapsed),
                kb(after_run.peak_kb),
                after_run.tree.len()
            );
            rows.push((command, nominal_depth, before_run, after_run));
        }
    }
    let _ = std::fs::remove_dir_all(&repo_root);

    let mut report = String::new();
    report.push_str(
        "# RFC 136 increment 2b -- `checkout --patch-materialize` and `branch switch` before and after \
         anchored worktree writes\n\n",
    );
    report.push_str(&format!(
        "Generated by `PRIKK_RFC136_BEFORE_BIN=<path> cargo test -p prikk-corpus --locked --test \
         rfc136_verified_worktree_writes -- --ignored --nocapture`. Profile: `profiles/prikk-self.toml`, \
         nominal depths {CHECKPOINTS:?}, one repository sealed locally by the after binary. The worktree \
         each binary wrote (every file's bytes and executable bit) was identical at every depth, and \
         stderr was empty.\n\n\
         - before: `{}` (`{}`), sha256 `{}`\n- after: `{}` (`{}`), sha256 `{}`\n\n",
        before_identity.path,
        before_identity.version_output,
        before_identity.sha256,
        after_identity.path,
        after_identity.version_output,
        after_identity.sha256,
    ));
    report.push_str(
        "| Command | Nominal depth | files | before (ms) | before peak (KB) | after (ms) | after peak (KB) | after / before |\n",
    );
    report.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for (command, nominal_depth, before_run, after_run) in &rows {
        report.push_str(&format!(
            "| `{command}` | {nominal_depth} | {} | {:.1} | {} | {:.1} | {} | {:.2} |\n",
            after_run.tree.len(),
            ms(before_run.elapsed),
            kb(before_run.peak_kb),
            ms(after_run.elapsed),
            kb(after_run.peak_kb),
            ms(after_run.elapsed) / ms(before_run.elapsed)
        ));
    }
    let out_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc136");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("verified-worktree-writes.md"), &report)
        .expect("writing the table");
    eprintln!("{report}");
}
