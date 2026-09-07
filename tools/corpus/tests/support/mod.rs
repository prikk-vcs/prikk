//! Shared support for this crate's own `#[ignore]`d integration tests, which drive a real `prikk`
//! binary through [`prikk_corpus::execute`].
//!
//! ## Why this exists instead of `env!("CARGO_BIN_EXE_prikk")`
//!
//! That macro does not work here. It resolves only for the crate that declares a binary target (or a
//! normal dependency on one that also has a `[lib]` to link against); `crates/prikk-cli` (package
//! name `prikk`) declares no `[lib]` at all -- confirmed empirically, not assumed: adding `prikk` as
//! a dependency of this crate produced `warning: ... ignoring invalid dependency 'prikk' which is
//! missing a lib target`, and the macro never resolved.
//!
//! Two ways around that were available. Widening `crates/prikk-cli`'s own manifest with a
//! dev-dependency on this crate (so *it* could use the macro and hand this crate a path some other
//! way) is a change under `crates/`, which RFC 139 increment 2's own handoff (§5) puts out of scope.
//! The other is to ask Cargo itself, authoritatively, where `cargo build -p prikk` places the
//! resulting binary -- via `--message-format=json`, parsing the `compiler-artifact` record for the
//! `prikk` binary target. That is what [`prikk_binary_path`] does. This is not the "guess
//! `target/<profile>/prikk`" the handoff's §2.1 forbids for the *executor* -- it is Cargo's own
//! machine-readable report of the exact path it just built, the same mechanism crates like
//! `escargot` exist to wrap for precisely this multi-crate-workspace scenario. The executor itself
//! still only ever receives an explicit `&Path`; this module is just how *these tests* obtain one.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Build `crates/prikk-cli` (package `prikk`) and return the resulting binary's path. Cached for the
/// life of the test process.
pub fn prikk_binary_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        locate_prikk_binary().unwrap_or_else(|err| panic!("locating the `prikk` binary: {err}"))
    })
}

/// `env!("CARGO")` -- the exact cargo binary that built this test, per rustc/cargo's own
/// documented mechanism for a crate to reliably re-invoke cargo without assuming it is on `PATH`.
const CARGO: &str = env!("CARGO");

fn locate_prikk_binary() -> Result<PathBuf, String> {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = std::process::Command::new(CARGO)
        .args([
            "build",
            "--locked",
            "--message-format=json",
            "--manifest-path",
            manifest_path,
            "-p",
            "prikk",
        ])
        .output()
        .map_err(|err| format!("spawning {CARGO} build: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "{CARGO} build -p prikk failed (status {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let Some(target) = value.get("target") else {
            continue;
        };
        let is_prikk_bin = target.get("name").and_then(serde_json::Value::as_str) == Some("prikk")
            && target
                .get("kind")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")));
        if !is_prikk_bin {
            continue;
        }
        if let Some(executable) = value.get("executable").and_then(serde_json::Value::as_str) {
            return Ok(PathBuf::from(executable));
        }
    }
    Err(format!(
        "{CARGO} build -p prikk produced no compiler-artifact message naming the prikk binary"
    ))
}

/// A fresh, uniquely named directory under the system temp dir. Not created -- callers that need it
/// to exist (e.g. before `prikk init`) create it themselves.
pub fn unique_dir(tag: &str) -> PathBuf {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "prikk-corpus-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    dir
}

/// Run `prikk log --limit <limit> --ref <ref_name>` and return the `block <id>` ids it printed, most
/// recent first (matching `log`'s own output order).
pub fn block_ids(binary: &Path, repo_root: &Path, ref_name: &str, limit: usize) -> Vec<String> {
    let output = std::process::Command::new(binary)
        .current_dir(repo_root)
        .args(["log", "--ref", ref_name, "--limit", &limit.to_string()])
        .output()
        .expect("running prikk log");
    assert!(
        output.status.success(),
        "prikk log failed (status {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("block ").map(str::to_owned))
        .collect()
}

/// Copy only `src`'s `.prikk` directory into a fresh `dst` (created if needed). Used to measure
/// `checkout` cost against a repository's sealed history alone, the way a fresh clone would --
/// never copies worktree files, since a real clone would not have any either.
pub fn copy_prikk_only(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst.join(".prikk")).expect("creating dst/.prikk");
    copy_dir_recursive(&src.join(".prikk"), &dst.join(".prikk"));
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("creating directory");
    for entry in std::fs::read_dir(src).expect("reading directory") {
        let entry = entry.expect("reading directory entry");
        let file_type = entry.file_type().expect("reading file type");
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path);
        } else {
            std::fs::copy(entry.path(), &dst_path).expect("copying file");
        }
    }
}

/// Every real file under `root`, excluding `.prikk` -- a materialized worktree's file count.
pub fn count_tree_files(root: &Path) -> u64 {
    fn walk(dir: &Path, count: &mut u64) {
        for entry in std::fs::read_dir(dir).expect("reading directory") {
            let entry = entry.expect("reading directory entry");
            if entry.file_name() == ".prikk" {
                continue;
            }
            if entry.file_type().expect("reading file type").is_dir() {
                walk(&entry.path(), count);
            } else {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(root, &mut count);
    count
}
