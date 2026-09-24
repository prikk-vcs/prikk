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

/// Which build of `prikk` a measurement wants (RFC 136 increment 2c, item 0).
///
/// **Timing is measured in release.** Every corpus-driven timing before 2c came through a `cargo build` without
/// `--release`: the shapes of those curves (the exponents, the sealing projection, the cold-`commit` growth) are
/// debug-build artefacts, and a release build is ~30x faster with different shapes. So the default build is
/// release, **fixed in source** -- not an environment knob, for the reason RFC 133's memory instrument runs its
/// release profile -- and a debug build is a separate, explicitly named request for the bridging columns a
/// re-measurement needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    /// What every timing uses: `cargo build --release`.
    Release,
    /// Only when a debug column is wanted beside the release one: `cargo build`.
    DebugForBridgingColumns,
}

/// The arguments of the `cargo build` that produces the binary for `profile`.
pub fn build_args(profile: BuildProfile, manifest_path: &str) -> Vec<String> {
    let mut args: Vec<String> = ["build", "--locked", "--message-format=json"]
        .into_iter()
        .map(str::to_string)
        .collect();
    if profile == BuildProfile::Release {
        args.push("--release".to_string());
    }
    args.extend(
        ["--manifest-path", manifest_path, "-p", "prikk"]
            .into_iter()
            .map(str::to_string),
    );
    args
}

/// The `opt_level` Cargo's own `compiler-artifact` record names for a build, e.g. `"3"` or `"0"`.
pub fn artifact_opt_level(artifact: &serde_json::Value) -> Option<String> {
    artifact
        .get("profile")?
        .get("opt_level")?
        .as_str()
        .map(str::to_string)
}

/// Refuse a default (release) binary whose artifact record says it is not optimized: read from Cargo's JSON, not
/// inferred from a path or an argument list, so a change of profile settings that quietly produced an
/// unoptimized build is caught where the number would have been taken.
pub fn require_optimized(opt_level: Option<&str>) -> Result<(), String> {
    match opt_level {
        Some("0") => Err(
            "the `prikk` binary is unoptimized (opt_level 0): timings from it are debug-build \
                          timings"
                .to_string(),
        ),
        Some(_) => Ok(()),
        None => Err("Cargo's compiler-artifact record for `prikk` names no opt_level".to_string()),
    }
}

/// The release binary of `crates/prikk-cli` (package `prikk`), built once per test process and checked
/// optimized. **The one every timing uses.**
pub fn prikk_binary_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let built = locate_prikk_binary(BuildProfile::Release)
            .unwrap_or_else(|err| panic!("locating the `prikk` binary: {err}"));
        require_optimized(built.opt_level.as_deref()).unwrap_or_else(|err| panic!("{err}"));
        built.path
    })
}

/// The debug binary, for a bridging column beside a release one. Named so that asking for it is deliberate.
pub fn prikk_debug_binary_path_for_bridging_columns() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        locate_prikk_binary(BuildProfile::DebugForBridgingColumns)
            .unwrap_or_else(|err| panic!("locating the debug `prikk` binary: {err}"))
            .path
    })
}

/// The binary a **measurement** uses, and the label its report must state: the release build, unless
/// `PRIKK_MEASURE_PROFILE=debug` asks for the debug column of a bridging table (RFC 139 release re-measurement
/// handoff, §1.4). Never the other way round: release is the default, in source.
pub fn measurement_binary() -> (&'static Path, &'static str) {
    if std::env::var("PRIKK_MEASURE_PROFILE").is_ok_and(|value| value == "debug") {
        (prikk_debug_binary_path_for_bridging_columns(), "debug")
    } else {
        (prikk_binary_path(), "release")
    }
}

/// `env!("CARGO")` -- the exact cargo binary that built this test, per rustc/cargo's own
/// documented mechanism for a crate to reliably re-invoke cargo without assuming it is on `PATH`.
const CARGO: &str = env!("CARGO");

/// Where a build put the binary, and the optimization level Cargo recorded for it.
pub struct BuiltBinary {
    pub path: PathBuf,
    pub opt_level: Option<String>,
}

pub fn locate_prikk_binary(profile: BuildProfile) -> Result<BuiltBinary, String> {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = std::process::Command::new(CARGO)
        .args(build_args(profile, manifest_path))
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
            return Ok(BuiltBinary {
                path: PathBuf::from(executable),
                opt_level: artifact_opt_level(&value),
            });
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

/// A whole-directory copy (worktree and `.prikk`), for a throwaway repository a measurement may perturb.
pub fn copy_dir_all(src: &Path, dst: &Path) {
    copy_dir_recursive(src, dst);
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

// ---- RFC 136 increment 3: shared measurement helpers -------------------------------------------------
// Added here so the increment 3 instrument does not become a third copy of `run_measured`
// (`two_measurements.rs`, `rfc136_*` carry their own from earlier rounds and are left as they were).

/// `(elapsed, peak_kb, output)`: peak RSS sampled from `/proc/<pid>/status` (`VmHWM`) on Linux, `None`
/// elsewhere. A missed sample is *not measured*, never zero.
pub fn run_measured(
    mut command: std::process::Command,
) -> (std::time::Duration, Option<u64>, std::process::Output) {
    #[cfg(target_os = "linux")]
    {
        use std::process::Stdio;
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let start = std::time::Instant::now();
        let mut child = command.spawn().expect("spawning prikk");
        let pid = child.id();
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
        let mut peak_kb: Option<u64> = None;
        let status = loop {
            let hwm = std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|text| {
                    text.lines()
                        .find_map(|line| line.strip_prefix("VmHWM:"))
                        .and_then(|rest| rest.split_whitespace().next())
                        .and_then(|kb| kb.parse::<u64>().ok())
                });
            if let Some(kb) = hwm {
                peak_kb = Some(peak_kb.map_or(kb, |current| current.max(kb)));
            }
            if let Some(status) = child.try_wait().expect("waiting for prikk") {
                break status;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let elapsed = start.elapsed();
        let output = std::process::Output {
            status,
            stdout: out_reader.join().expect("stdout reader"),
            stderr: err_reader.join().expect("stderr reader"),
        };
        (elapsed, peak_kb, output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let start = std::time::Instant::now();
        let output = command.output().expect("running prikk");
        (start.elapsed(), None, output)
    }
}

/// Every worktree file outside `.prikk`: its relative path, bytes and executable bit.
pub fn worktree_digest(root: &Path) -> std::collections::BTreeMap<PathBuf, (Vec<u8>, bool)> {
    let mut files = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("reading worktree").flatten() {
            let path = entry.path();
            if entry.file_name() == ".prikk" {
                continue;
            }
            if entry.file_type().expect("file type").is_dir() {
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

/// Total bytes of every file under `dir`.
pub fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)
            .expect("reading directory")
            .flatten()
        {
            let file_type = entry.file_type().expect("file type");
            if file_type.is_dir() {
                stack.push(entry.path());
            } else {
                total += entry.metadata().map_or(0, |metadata| metadata.len());
            }
        }
    }
    total
}

/// The object count `prikk verify` reports (`object items: N scanned`).
pub fn verified_object_count(binary: &Path, repo_root: &Path) -> Option<u64> {
    let output = std::process::Command::new(binary)
        .current_dir(repo_root)
        .arg("verify")
        .output()
        .expect("running prikk verify");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("object items: "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
}

/// `(median, min, max)` of `samples`; `None` when any sample is `None` or there are none.
pub fn median_range<T: Copy + PartialOrd>(samples: &[Option<T>]) -> Option<(T, T, T)> {
    let mut values: Vec<T> = samples.iter().copied().collect::<Option<Vec<T>>>()?;
    values.sort_by(|a, b| a.partial_cmp(b).expect("comparable samples"));
    // `get`/`first`/`last` rather than indexing: this crate's tests are linted with
    // `-D clippy::indexing-slicing`, and an empty sample set is a `None`, not a panic.
    let median = *values.get(values.len() / 2)?;
    Some((median, *values.first()?, *values.last()?))
}
