//! RFC 136 increment 2a: who may anchor.

use std::path::{Path, PathBuf};

/// Only read-only reports may name `Anchoring::ReadOnlyReport` (§10.3c ruling 1). Every worktree write
/// and rollback preview goes through `replay_supported_patch_chain`, which passes `Anchoring::Never`. A
/// new file naming the anchored purpose fails here, naming the file, until it is judged a read-only
/// report and added to the list.
#[test]
fn every_read_only_anchor_caller_is_a_read_only_report() {
    const ALLOWED: &[&str] = &[
        "patch_replay.rs",
        "bundle/preview.rs",
        "patch_replay/anchor.rs",
    ];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src.clone()];
    let mut callers: Vec<PathBuf> = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.extension().and_then(|e| e.to_str()) != Some("rs")
                || name == "tests.rs"
                || path.components().any(|c| c.as_os_str() == "tests")
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let names_it = text.lines().any(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && line.contains("Anchoring::ReadOnlyReport")
            });
            if names_it {
                callers.push(path.strip_prefix(&src).unwrap_or(&path).to_path_buf());
            }
        }
    }
    let unexpected: Vec<String> = callers
        .iter()
        .filter(|path| {
            !ALLOWED
                .iter()
                .any(|allowed| Path::new(allowed) == path.as_path())
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        unexpected.is_empty(),
        "these files name Anchoring::ReadOnlyReport but are not read-only reports: {unexpected:?}"
    );
    for required in ["patch_replay.rs", "bundle/preview.rs"] {
        assert!(
            callers.iter().any(|path| path == Path::new(required)),
            "fixture sanity: {required} anchors its read-only report"
        );
    }
}

/// RFC 136 increment 2b §2: only `patch_replay.rs` names `Anchoring::VerifiedWorktreeWrite`, and only
/// the worktree writes (`patch_checkout.rs`, `branch_switch.rs`) call its entry point. A new file using
/// either fails here, naming the file.
#[test]
fn only_worktree_writes_use_the_verified_anchor() {
    fn files_naming(needle: &str) -> Vec<String> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src.clone()];
        let mut found = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if path.extension().and_then(|e| e.to_str()) != Some("rs")
                    || name == "tests.rs"
                    || path.components().any(|c| c.as_os_str() == "tests")
                {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if text
                    .lines()
                    .any(|line| !line.trim_start().starts_with("//") && line.contains(needle))
                {
                    found.push(
                        path.strip_prefix(&src)
                            .unwrap_or(&path)
                            .display()
                            .to_string(),
                    );
                }
            }
        }
        found.sort();
        found
    }
    assert_eq!(
        files_naming("Anchoring::VerifiedWorktreeWrite"),
        ["patch_replay.rs", "patch_replay/anchor.rs"],
        "files naming the verified worktree-write anchor"
    );
    assert_eq!(
        files_naming("replay_for_verified_worktree_write("),
        ["branch_switch.rs", "patch_checkout.rs", "patch_replay.rs"],
        "callers of the verified worktree-write replay"
    );
}
