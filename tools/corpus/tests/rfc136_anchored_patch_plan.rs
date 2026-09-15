//! RFC 136 increment 2a, handoff §3's cost control: `checkout --patch-plan` wall time and peak memory at
//! the §9.3 depths, before and after anchored read-only reports, on RFC 139's corpus
//! (`profiles/prikk-self.toml`).
//!
//! - **After** is the binary this test builds (`support::prikk_binary_path`).
//! - **Before** is a binary from the commit before anchoring, passed as `PRIKK_RFC136_BEFORE_BIN`. Both
//!   write the same checkpoints (RFC 136 increment 1b), so one history serves both.
//! - At each checkpoint depth `.prikk` is copied twice, never the worktree, and each binary's
//!   `checkout --patch-plan` is timed in its own copy. The two stdouts must be byte-identical: anchoring
//!   changes cost, never output (§10.3a ruling 1).
//! - The table is written to `.git-exclude/measurements/rfc136/anchored-patch-plan.md` (RFC 133's rule).
//!
//! If decoding the whole chain eats the saving, the numbers say so and the increment stops (handoff §3).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use prikk_corpus::{Profile, execute};

mod support;

/// The depths RFC 139 increment 3's instrument measured §9.3's table at.
const CHECKPOINTS: [u64; 5] = [32, 64, 128, 192, 256];

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

struct Sample {
    nominal_depth: u64,
    before_ms: f64,
    before_peak_kb: Option<u64>,
    after_ms: f64,
    after_peak_kb: Option<u64>,
}

fn time_patch_plan(binary: &Path, repo_root: &Path, tag: &str) -> (Duration, Option<u64>, Output) {
    let copy = copy_metadata(repo_root, tag);
    let measured = run_measured({
        let mut command = Command::new(binary);
        command
            .current_dir(&copy)
            .args(["checkout", "--patch-plan", "--ref", execute::REF_NAME]);
        command
    });
    let _ = std::fs::remove_dir_all(&copy);
    measured
}

fn kb(peak: Option<u64>) -> String {
    peak.map_or_else(|| "not measured".to_owned(), |kb| kb.to_string())
}

#[test]
#[ignore = "RFC 136 increment 2a's cost measurement; expensive, run deliberately with PRIKK_RFC136_BEFORE_BIN"]
fn anchored_patch_plan_cost() {
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
    let repo_root = support::unique_dir("rfc136-anchored-patch-plan");
    execute::init_repository(&after, &repo_root).expect("init");

    let mut trusted = false;
    let mut samples = Vec::new();
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

        let nominal_depth = (index + 1) as u64;
        if !CHECKPOINTS.contains(&nominal_depth) {
            continue;
        }
        let (before_elapsed, before_peak_kb, before_output) =
            time_patch_plan(&before, &repo_root, &format!("before-{nominal_depth}"));
        let (after_elapsed, after_peak_kb, after_output) =
            time_patch_plan(&after, &repo_root, &format!("after-{nominal_depth}"));
        assert!(
            before_output.status.success() && after_output.status.success(),
            "patch-plan failed at depth {nominal_depth}\nbefore: {}\nafter: {}",
            String::from_utf8_lossy(&before_output.stderr),
            String::from_utf8_lossy(&after_output.stderr)
        );
        let normalize = |output: &Output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.starts_with("patch replay plan repository:"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            normalize(&before_output),
            normalize(&after_output),
            "anchoring changed --patch-plan's output at depth {nominal_depth}"
        );
        assert!(
            after_output.stderr.is_empty(),
            "no snapshot in the corpus fails validation: {}",
            String::from_utf8_lossy(&after_output.stderr)
        );
        eprintln!(
            "depth {nominal_depth}: before {:.1} ms (peak {}), after {:.1} ms (peak {})",
            before_elapsed.as_secs_f64() * 1000.0,
            kb(before_peak_kb),
            after_elapsed.as_secs_f64() * 1000.0,
            kb(after_peak_kb)
        );
        samples.push(Sample {
            nominal_depth,
            before_ms: before_elapsed.as_secs_f64() * 1000.0,
            before_peak_kb,
            after_ms: after_elapsed.as_secs_f64() * 1000.0,
            after_peak_kb,
        });
    }
    let _ = std::fs::remove_dir_all(&repo_root);

    let mut report = String::new();
    report.push_str(
        "# RFC 136 increment 2a -- `checkout --patch-plan` before and after anchoring\n\n",
    );
    report.push_str(&format!(
        "Generated by `PRIKK_RFC136_BEFORE_BIN=<path> cargo test -p prikk-corpus --locked --test \
         rfc136_anchored_patch_plan -- --ignored --nocapture`. Profile: `profiles/prikk-self.toml`, \
         nominal depths {CHECKPOINTS:?}. Stdout was byte-identical between the two binaries at every \
         depth (the repository line excepted, which names the copy's path).\n\n\
         - before: `{}` (`{}`), sha256 `{}`\n- after: `{}` (`{}`), sha256 `{}`\n\n",
        before_identity.path,
        before_identity.version_output,
        before_identity.sha256,
        after_identity.path,
        after_identity.version_output,
        after_identity.sha256,
    ));
    report.push_str(
        "| Nominal depth | before (ms) | before peak (KB) | after (ms) | after peak (KB) | after / before |\n",
    );
    report.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for sample in &samples {
        report.push_str(&format!(
            "| {} | {:.1} | {} | {:.1} | {} | {:.2} |\n",
            sample.nominal_depth,
            sample.before_ms,
            kb(sample.before_peak_kb),
            sample.after_ms,
            kb(sample.after_peak_kb),
            sample.after_ms / sample.before_ms
        ));
    }
    let out_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc136");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("anchored-patch-plan.md"), &report).expect("writing the table");
    eprintln!("{report}");
}
