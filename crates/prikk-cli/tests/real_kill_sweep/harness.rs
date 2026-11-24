// SPDX-License-Identifier: Apache-2.0
// adapted from the external review kit for prikk (reviews 053 and 055): `probe25.sh`'s loop, in Rust.
//! The sweep's loop. For each mutating-syscall ordinal of a command: a fresh copy of the prepared repository, the kill, then `verify` and `doctor`
//! (exit codes kept), every lock cleared, the command run again, `verify`, and the shape's own check.
//!
//! **The recovery rules (K3 ruling 4).** `unlock --yes` first. `--force` **only** where `--yes` refused because the lock's body is incomplete (the
//! creator died between creating the file and writing it: no other reading is true of it); any other refusal is unsound, because a blanket `--force`
//! could hide a lock written wrong. A command the refusal names in backticks is run, and **it must exit 0**; then the command again.
//!
//! **Classes.** `A` sound at once (verify 0, no lock left). `L` sound after `unlock --yes`. `F` sound after `--force` of an incomplete lock body.
//! `N` `verify` was not 0 after the kill (or a refusal named a command) and the rerun or the named command made it 0. `X` unsound.
//!
//! **Three limits, every place that names the sweep says them:** it kills a process at a syscall boundary; it is not power loss (the failpoint rows hold
//! fsync order); and it does not tear a write inside one syscall (the direct restore tests hold that).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic
)]

use prikk_test_support::ptrace_sweep::{self, When};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The one isolated config home and scratch root of a test: keys live in `config`, repositories under `root`.
pub struct Ctx {
    pub root: PathBuf,
    pub config: PathBuf,
}

impl Ctx {
    pub fn new(tag: &str) -> Self {
        let base = std::env::var_os("PRIKK_SWEEP_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let shm = PathBuf::from("/dev/shm");
                (shm.is_dir()).then_some(shm)
            })
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!("prikk-k3-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        Self { root, config }
    }

    /// A `prikk` command in `dir`, with the config home isolated and every `PRIKK_*` seed variable removed.
    pub fn prikk(&self, dir: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(prikk_test_support::prikk_bin_path());
        cmd.current_dir(dir).args(args);
        for var in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "APPDATA"] {
            cmd.env(var, &self.config);
        }
        for var in [
            "PRIKK_AUTHOR_SEED",
            "PRIKK_MAINTAINER_SEED",
            "PRIKK_AUTHOR_KEY_ID",
            "PRIKK_MAINTAINER_KEY_ID",
            "PRIKK_AUTHOR_SEED_FILE",
            "PRIKK_MAINTAINER_SEED_FILE",
        ] {
            cmd.env_remove(var);
        }
        cmd
    }

    pub fn out(&self, dir: &Path, args: &[&str]) -> Output {
        self.prikk(dir, args).output().expect("run prikk")
    }

    pub fn ok(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.out(dir, args);
        let text = text(&output);
        assert!(output.status.success(), "prikk {args:?} in {dir:?}: {text}");
        text
    }

    pub fn exit(&self, dir: &Path, args: &[&str]) -> i32 {
        self.out(dir, args).status.code().unwrap_or(-1)
    }

    /// A fresh repository: `prikk setup .` in an empty directory.
    pub fn repo(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _ = self.out(&dir, &["setup", "."]);
        dir
    }

    /// `n` generations from `start`: a file written, committed and sealed each time (`edit`: the same file; else one new file each).
    pub fn gens(&self, dir: &Path, n: u32, edit: bool, start: u32) {
        for i in start..start + n {
            if edit {
                std::fs::write(dir.join("s.txt"), format!("step {i}\n")).unwrap();
            } else {
                std::fs::write(dir.join(format!("g{i}.txt")), format!("gen {i}\n")).unwrap();
            }
            self.ok(dir, &["commit", "--from-worktree", "-m", &format!("s{i}")]);
            self.ok(dir, &["seal", "--allow-no-audit"]);
        }
    }

    /// The number of file entries in the sealed tree.
    pub fn tree_files(&self, dir: &Path) -> usize {
        text(&self.out(dir, &["tree"]))
            .lines()
            .filter(|line| {
                let t = line.trim_start();
                t.len() > 6 && t[..6].bytes().all(|b| b.is_ascii_digit()) && t.as_bytes()[6] == b' '
            })
            .count()
    }

    pub fn clean(&self, dir: &Path) -> bool {
        self.exit(dir, &["verify"]) == 0 && self.exit(dir, &["doctor"]) == 0
    }
}

pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// Every regular file under `dir/.prikk` (not `recovery/`, not `cache/`, not a lock) with its bytes: the repository's state, for a check that
/// compares bytes.
pub fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            let first = rel.components().next().map(|c| c.as_os_str().to_owned());
            if first.is_some_and(|f| f == "recovery" || f == "cache") {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                walk(base, &path, out);
            } else if path.extension().is_none_or(|e| e != "lock") {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let base = dir.join(".prikk");
    let mut out = BTreeMap::new();
    walk(&base, &base, &mut out);
    out
}

pub fn lock_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "lock") {
                    out.push(path);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&dir.join(".prikk"), &mut out);
    out.sort();
    out
}

fn temp_files(dir: &Path) -> usize {
    fn walk(dir: &Path, n: &mut usize) {
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(&path, n);
                } else if path
                    .file_name()
                    .is_some_and(|f| f.to_string_lossy().contains(".tmp."))
                {
                    *n += 1;
                }
            }
        }
    }
    let mut n = 0;
    walk(&dir.join(".prikk"), &mut n);
    n
}

/// A shape's own check of what "sound" means for it, run inside the repository copy.
pub type Check = Box<dyn Fn(&Ctx, &Path) -> bool>;

/// A prepared shape: the repository to copy, the command, and what "sound" means for it.
pub struct Shape {
    pub name: &'static str,
    pub base: PathBuf,
    pub argv: Vec<String>,
    /// The shape's own check, run inside the repository copy after the recovery and the rerun.
    pub check: Check,
    /// A restore ends in a repository `verify` fails by design.
    pub verify_may_fail: bool,
    /// A refusal's named command is not followed (a refusal that is the expected outcome).
    pub follow_named: bool,
    /// A program other than `prikk` (the control, which is not a prikk command); `argv` are then its arguments.
    pub program: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub ordinals: u64,
    pub a: u64,
    pub l: u64,
    pub f: u64,
    pub n: u64,
    pub x: Vec<String>,
    pub verify_after_kill_failed: u64,
    pub leftover_tmp_ordinals: u64,
    pub refused_as_done: u64,
}

fn real_command(ctx: &Ctx, shape: &Shape, dir: &Path) -> Command {
    let refs: Vec<&str> = shape.argv.iter().map(String::as_str).collect();
    match &shape.program {
        None => ctx.prikk(dir, &refs),
        Some(program) => {
            let mut cmd = Command::new(program);
            cmd.current_dir(dir).args(&refs);
            cmd
        }
    }
}

/// The number of mutating syscalls the command issues, without killing it.
pub fn dry_count(ctx: &Ctx, shape: &Shape) -> u64 {
    let work = ctx.root.join(format!("dry-{}", shape.name));
    let sink = ctx.root.join(format!("dry-out-{}.txt", shape.name));
    let _ = std::fs::remove_dir_all(&work);
    copy_tree(&shape.base, &work);
    let dry = ptrace_sweep::trace(&real_command(ctx, shape, &work), &sink, None, When::Entry)
        .unwrap_or_else(|error| panic!("{}: {error}", shape.name));
    let _ = std::fs::remove_dir_all(&work);
    dry.mutating.len() as u64
}

/// Sweep every mutating-syscall ordinal of `shape.argv` over copies of `shape.base`.
pub fn sweep(ctx: &Ctx, shape: &Shape) -> Outcome {
    let work = ctx.root.join(format!("work-{}", shape.name));
    let sink = ctx.root.join(format!("out-{}.txt", shape.name));
    // The dry run: the whole list of mutating syscalls.
    let _ = std::fs::remove_dir_all(&work);
    copy_tree(&shape.base, &work);
    let dry = ptrace_sweep::trace(&real_command(ctx, shape, &work), &sink, None, When::Entry)
        .unwrap_or_else(|error| panic!("{}: {error}", shape.name));
    let total = dry.mutating.len() as u64;
    assert!(
        total > 0,
        "{}: the dry run issued no mutating syscall",
        shape.name
    );
    let mut outcome = Outcome {
        ordinals: total,
        ..Outcome::default()
    };
    for n in 1..=total {
        let _ = std::fs::remove_dir_all(&work);
        copy_tree(&shape.base, &work);
        let run = ptrace_sweep::trace(
            &real_command(ctx, shape, &work),
            &sink,
            Some(n),
            When::Entry,
        )
        .unwrap_or_else(|error| panic!("{} at ordinal {n}: {error}", shape.name));
        assert!(
            run.killed_at.is_some(),
            "{}: ordinal {n} of {total} was not reached (the run is not deterministic)",
            shape.name
        );
        let v1 = ctx.exit(&work, &["verify"]);
        let d1 = ctx.exit(&work, &["doctor"]);
        if v1 != 0 {
            outcome.verify_after_kill_failed += 1;
        }
        let locks = lock_files(&work);
        let mut forced = false;
        let mut bad: Option<String> = None;
        for lock in &locks {
            let path = lock.to_string_lossy().into_owned();
            let yes = ctx.out(&work, &["unlock", "--lock", &path, "--yes"]);
            if !yes.status.success() {
                if text(&yes).contains("the lock body is incomplete") {
                    let force = ctx.out(&work, &["unlock", "--lock", &path, "--force"]);
                    if force.status.success() {
                        forced = true;
                    } else {
                        bad = Some(format!("--force of {path} failed: {}", text(&force)));
                    }
                } else {
                    bad = Some(format!(
                        "`unlock --yes` of {path} refused for another reason: {}",
                        text(&yes)
                    ));
                }
            }
        }
        let mut named = String::new();
        let mut rerun = real_command(ctx, shape, &work).output().expect("rerun");
        if !rerun.status.success() && shape.follow_named {
            if let Some(command) = backticked_command(&text(&rerun), &shape.argv) {
                named = command.clone();
                let words: Vec<&str> = command.split_whitespace().skip(1).collect();
                let named_run = ctx.out(&work, &words);
                if !named_run.status.success() {
                    bad = Some(format!(
                        "the named command `{command}` did not exit 0: {}",
                        text(&named_run)
                    ));
                }
                rerun = real_command(ctx, shape, &work).output().expect("rerun");
            }
        }
        let v2 = ctx.exit(&work, &["verify"]);
        let check_ok = (shape.check)(ctx, &work);
        if temp_files(&work) > 0 {
            outcome.leftover_tmp_ordinals += 1;
        }
        let verified = v2 == 0 || shape.verify_may_fail;
        if bad.is_none() && verified && check_ok {
            if !rerun.status.success() {
                outcome.refused_as_done += 1;
            }
            if !named.is_empty() {
                outcome.n += 1;
            } else if forced {
                outcome.f += 1;
            } else if !locks.is_empty() {
                outcome.l += 1;
            } else if v1 != 0 {
                outcome.n += 1;
            } else {
                outcome.a += 1;
            }
        } else {
            outcome.x.push(format!(
                "ordinal {n} ({} at {}): v1={v1} d1={d1} locks={} forced={forced} named={named:?} rerun={:?} v2={v2} check={check_ok}: {}",
                run.killed_at.as_ref().map_or("?", |k| k.name),
                run.killed_at.as_ref().map_or(0, |k| k.ordinal),
                locks.len(),
                rerun.status.code(),
                bad.unwrap_or_else(|| text(&rerun).lines().find(|l| l.contains("rror")).unwrap_or("").to_string())
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

/// The first `prikk …` command in backticks in a refusal that is **not the shape's own command** (a prefix of it, or it) and not a `--plan-only` query: a refusal that says "run that same command again" names
/// the command being swept (`prikk uncommit`, with no argument), and the sweep runs the command again itself.
pub fn backticked_command(text: &str, own: &[String]) -> Option<String> {
    let mut parts = text.split('`');
    parts.next();
    while let Some(inside) = parts.next() {
        let words: Vec<&str> = inside.split_whitespace().skip(1).collect();
        // The command being swept (or its bare verb: "run that same command again") is not a step to follow.
        let is_own = words.iter().zip(own.iter()).all(|(w, o)| *w == o.as_str());
        // A `--plan-only` command is a question ("says whether it can be put back"), not a step: its exit code is its answer.
        if inside.starts_with("prikk ") && !is_own && !inside.contains("--plan-only") {
            return Some(inside.to_string());
        }
        parts.next();
    }
    None
}

/// A one-line summary of a shape's sweep, printed by every test (`--nocapture`) and read for the report.
pub fn summary(shape: &Shape, o: &Outcome) -> String {
    format!(
        "SWEEP {:<42} mutating {:>4}  A {:>3} L {:>3} F {:>3} N {:>3} X {:>2}  (verify not 0 after the kill: {}; rerun refused as already done: {}; temp files left: {})",
        shape.name,
        o.ordinals,
        o.a,
        o.l,
        o.f,
        o.n,
        o.x.len(),
        o.verify_after_kill_failed,
        o.refused_as_done,
        o.leftover_tmp_ordinals
    )
}

/// Run one shape's sweep, print its summary, and fail on any unsound ordinal.
pub fn run_shape(ctx: &Ctx, shape: &Shape) {
    let outcome = sweep(ctx, shape);
    eprintln!("{}", summary(shape, &outcome));
    assert!(
        outcome.x.is_empty(),
        "{}: {} unsound ordinal(s):\n{}",
        shape.name,
        outcome.x.len(),
        outcome.x.join("\n")
    );
}

pub fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

#[allow(dead_code)]
pub fn os(s: &str) -> OsString {
    OsString::from(s)
}
