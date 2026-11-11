//! 0.57.0 U3: a checkout protects the disk, so every materializer asks the collision rule before its first write.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_error::PrikkError;

use super::{refuse_colliding_manifest_paths, refuse_manifest_conflicts};
use crate::RepoPath;
use crate::foundation::layout::RepositoryLayout;
use crate::patch_replay::{ReplayManifest, ReplayManifestEntry};
use crate::test_gates::test_support::unique_temp_dir;

fn manifest(paths: &[&str]) -> ReplayManifest {
    ReplayManifest {
        files: paths
            .iter()
            .map(|path| ReplayManifestEntry {
                node_id: None,
                path: RepoPath::parse(path).unwrap(),
                bytes: b"x".to_vec(),
                mode: 0o100_644,
                kind: None,
                blob_id: None,
            })
            .collect(),
    }
}

#[test]
fn a_manifest_with_two_names_for_one_file_is_refused_before_any_write() {
    for colliding in [
        ["README.md", "readme.md"],
        ["Stra\u{df}e.txt", "STRASSE.txt"],
        ["caf\u{e9}.md", "cafe\u{301}.md"],
        ["\u{212a}.md", "k.md"],
    ] {
        let err = refuse_colliding_manifest_paths(&manifest(&colliding)).expect_err("collision");
        assert!(matches!(err, PrikkError::Precondition(_)), "{err:?}");
        let said = err.to_string();
        assert!(
            said.contains("refusing to materialize")
                && said.contains("case-insensitive path collision involving: ")
                && said.contains("nothing was written"),
            "{said}"
        );
    }
    refuse_colliding_manifest_paths(&manifest(&["a.md", "b.md", "e.md", "\u{e9}.md"]))
        .expect("distinct names pass");
}

#[test]
fn the_conflict_planner_every_checkout_runs_asks_the_collision_rule_first() {
    let root = unique_temp_dir("worktree-collision-planner");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let err = refuse_manifest_conflicts(&layout, &manifest(&["Stra\u{df}e.txt", "strasse.txt"]))
        .expect_err("collision");
    assert!(
        err.to_string().contains("case-insensitive path collision"),
        "{err}"
    );
    refuse_manifest_conflicts(&layout, &manifest(&["a.md", "b.md"]))
        .expect("no collision, no conflict");
    let _ = std::fs::remove_dir_all(root);
}
