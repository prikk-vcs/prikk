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
