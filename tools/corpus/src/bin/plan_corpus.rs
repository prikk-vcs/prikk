//! Thin binary over [`prikk_corpus::plan()`] (RFC 139 increment 2, handoff §1/§7): reads a profile and
//! a target commit count, writes the resulting action manifest as canonical JSON. Also what handoff
//! §6 control 1's determinism test spawns as a **subprocess** (not an in-process call) -- increment
//! 1's own lesson: Rust's default `HashMap` hasher is randomized per *process*, not per call, so two
//! in-process planner calls would agree even under a genuinely nondeterministic implementation.
//!
//! ```text
//! plan-corpus <profile-file> <target-commit-count> [--out <manifest-file>]
//! ```
//!
//! Without `--out`, the manifest is written to stdout.

use std::process::ExitCode;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut out_path = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--out requires a value".to_string())?;
                if out_path.is_some() {
                    return Err("duplicate --out flag".to_string());
                }
                out_path = Some(value);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown argument: {other}"));
            }
            other => positional.push(other.to_string()),
        }
    }
    let [profile_path, target_commit_count] = positional.as_slice() else {
        return Err(format!(
            "usage: plan-corpus <profile-file> <target-commit-count> [--out <manifest-file>], got {} \
             positional argument(s)",
            positional.len()
        ));
    };

    let profile_text = std::fs::read_to_string(profile_path)
        .map_err(|err| format!("reading {profile_path}: {err}"))?;
    let profile: prikk_corpus::Profile =
        toml::from_str(&profile_text).map_err(|err| format!("parsing {profile_path}: {err}"))?;
    let target_commit_count: u64 = target_commit_count
        .parse()
        .map_err(|err| format!("target-commit-count {target_commit_count:?}: {err}"))?;

    let manifest =
        prikk_corpus::plan(&profile, target_commit_count).map_err(|err| err.to_string())?;
    let rendered = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("rendering manifest: {err}"))?;

    match out_path {
        Some(path) => {
            std::fs::write(&path, rendered).map_err(|err| format!("writing {path}: {err}"))?;
        }
        None => println!("{rendered}"),
    }
    Ok(())
}
