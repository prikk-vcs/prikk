#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use super::{build, production_edge_text_for_tests, reexports_for_tests, walk_root_for_tests};

fn store_src_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .join("crates/prikk-store/src")
}

#[test]
fn production_edge_text_strips_comments_and_strings() {
    let raw = "// crate::commented_out\nfn f() { let s = \"crate::in_a_string\"; crate::real_edge::Thing::new(); }";
    let text = production_edge_text_for_tests(raw);
    assert!(!text.contains("commented_out"));
    assert!(!text.contains("in_a_string"));
    assert!(text.contains("crate::real_edge"));
}

#[test]
fn production_edge_text_excises_inline_test_only_blocks() {
    let raw = "fn f() {}\n#[cfg(test)]\nmod inline_tests {\n    fn g() { crate::should_not_count::X; }\n}\n";
    let text = production_edge_text_for_tests(raw);
    assert!(!text.contains("should_not_count"));
}

#[test]
fn production_edge_text_keeps_inline_non_test_blocks() {
    let raw = "mod ordinary {\n    fn g() { crate::should_count::X; }\n}\n";
    let text = production_edge_text_for_tests(raw);
    assert!(text.contains("crate::should_count"));
}

#[test]
fn production_edge_text_tolerates_an_attribute_between_cfg_and_mod() {
    let raw = "#[cfg(test)]\n#[allow(clippy::foo)]\nmod inline_tests {\n    fn g() { crate::should_not_count::X; }\n}\n";
    let text = production_edge_text_for_tests(raw);
    assert!(!text.contains("should_not_count"));
}

/// A visibility qualifier between `#[cfg(test)]` and its `mod` must not strand the attribute.
/// Regression for a bug found 2026-09-08: the scanner reset `pending_cfg` on any "other code
/// token", and `pub(crate)` is one -- so `#[cfg(test)]\npub(crate) mod tests;` lost its cfg and the
/// module's whole `tests/` subtree was aggregated as production text. `crates/prikk-store/src/
/// patch_replay.rs:570-571` is exactly this shape, so the top hub of the coupling graph was
/// affected. Latent until an implementing round's new test-only import manufactured a spurious
/// `patch_replay -> block_state` production edge.
#[test]
fn production_edge_text_excises_visibility_qualified_test_modules() {
    for vis in ["pub", "pub(crate)", "pub(super)", "pub(in crate::foo)"] {
        let raw = format!(
            "#[cfg(test)]\n{vis} mod inline_tests {{\n    fn g() {{ crate::should_not_count::X; }}\n}}\n"
        );
        let text = production_edge_text_for_tests(&raw);
        assert!(
            !text.contains("should_not_count"),
            "`{vis} mod` under #[cfg(test)] leaked into production text"
        );
    }
}

/// The mirror of the above: a visibility qualifier with **no** pending cfg must still be production.
/// Guards against the fix over-reaching into "skip anything after `pub`".
#[test]
fn production_edge_text_keeps_visibility_qualified_non_test_modules() {
    let raw = "pub(crate) mod ordinary {\n    fn g() { crate::should_count::X; }\n}\n";
    let text = production_edge_text_for_tests(raw);
    assert!(text.contains("crate::should_count"));
}

/// The exact worked example RFC 130 uses to prove a substring check on the word "test" is wrong.
#[test]
fn fsutil_none_module_is_counted_as_production() {
    let modules = walk_root_for_tests(&store_src_root()).expect("walk succeeds");
    // RFC 131 §2.2a grouping: `fsutil` moved under `foundation/fsutil.rs` and is no longer a
    // top-level module in its own right (its text is now collected into the `foundation` node).
    assert!(modules.contains("foundation"));
    let text = std::fs::read_to_string(store_src_root().join("foundation/fsutil/anchored/none.rs"))
        .expect("none.rs exists");
    // If this file were wrongly excluded, the crate::foundation::fsutil self-reference it
    // contains would never reach the edge scan at all -- assert on the file being reachable
    // instead of on the (self-loop, hence invisible) edge itself.
    assert!(text.contains("crate::foundation::fsutil::contract::DurabilityContract"));
}

#[test]
fn walk_finds_the_confirmed_production_module_count() {
    let modules = walk_root_for_tests(&store_src_root()).expect("walk succeeds");
    // RFC 131 §2.2a grouping (2026-09-08): 61 production modules at the coupling-gate-graph-
    // contradiction round's own count shrank to 51 -- `foundation` (byte_cursor, container,
    // file_codec, frame_resync, fsutil, generation, index, layout: 8 -> 1), `author`
    // (author_key_index, author_signing: 2 -> 1), `node` (node_id_gen, node_lifecycle: 2 -> 1),
    // and `received_index` folding into the already-top-level `received` (-1). None of §2's seven
    // constrained modules moved; `patch`, `rollback`, `worktree`, `merge` name-family grouping was
    // measured to create new coupling-graph cycles and was not done -- see the increment's report.
    // RFC 142 (2026-09-08) then added one new top-level module, `show`: 51 -> 52.
    // RFC 144 §4o.2 (2026-09-09) added one new top-level module, `rename_declaration`: 52 -> 53.
    assert_eq!(modules.len(), 53, "modules: {modules:?}");
    assert!(modules.contains("foundation"));
    assert!(modules.contains("show"));
    assert!(!modules.contains("fsutil"));
    assert!(!modules.contains("layout"));
    assert!(!modules.contains("dc55_identity_evidence"));
    assert!(!modules.contains("test_support"));
}

#[test]
fn reexports_resolve_active_ref_metadata_to_active() {
    let owners = reexports_for_tests(&store_src_root()).expect("reexports parse");
    assert_eq!(
        owners.get("read_active_ref_metadata").map(String::as_str),
        Some("active")
    );
    assert_eq!(
        owners.get("ActiveRefMetadata").map(String::as_str),
        Some("active")
    );
}

/// The concrete re-export-only edge RFC 130 §1 warned a naive extractor would miss:
/// `patch_replay.rs` never writes `crate::active::` anywhere, only the re-exported names.
#[test]
fn patch_replay_never_writes_crate_active_directly() {
    let text = std::fs::read_to_string(store_src_root().join("patch_replay.rs")).unwrap();
    assert!(!text.contains("crate::active::"));
}

#[test]
fn graph_finds_the_re_export_only_edge_from_patch_replay_to_active() {
    let graph = build(&store_src_root()).expect("graph builds");
    assert!(
        graph
            .edges
            .contains(&("patch_replay".to_owned(), "active".to_owned())),
        "patch_replay -> active must be found through the read_active_ref_metadata/\
         ActiveRefMetadata re-exports even though the module never writes crate::active:: \
         directly"
    );
}

#[test]
fn graph_matches_every_cited_cycle_leg() {
    let graph = build(&store_src_root()).expect("graph builds");
    for (from, to) in [
        ("active", "refs"),
        ("refs", "active"),
        ("refs", "trust"),
        ("trust", "refs"),
        ("lifecycle_cache", "patch_replay"),
        ("patch_replay", "lifecycle_cache"),
        ("active", "worktree_patch"),
        ("worktree_patch", "patch_replay"),
        ("patch_replay", "active"),
    ] {
        assert!(
            graph.edges.contains(&(from.to_owned(), to.to_owned())),
            "expected edge {from} -> {to}"
        );
    }
}

#[test]
fn no_self_loops_in_the_graph() {
    let graph = build(&store_src_root()).expect("graph builds");
    for (from, to) in &graph.edges {
        assert_ne!(from, to, "self-loop is not a real coupling edge");
    }
}

#[test]
fn fsutil_has_zero_production_out_edges() {
    // RFC 131 §2.2a grouping folded `fsutil` into the `foundation` node alongside layout,
    // byte_cursor, file_codec, frame_resync, container, index, generation -- `fsutil`'s own
    // edges are no longer separately visible to this gate (a real cost, not free: RFC 130 §6's
    // "fsutil is the crate's one genuinely clean seam" argument leaned on this exact number in
    // isolation). The aggregate property this test can still check -- the whole foundation group
    // has no *production* edge leaving it, matching every one of its eight members' own
    // wide-fan-in-and-mostly-upward shape (RFC 130 §2.3) -- still holds and is checked here.
    let graph = build(&store_src_root()).expect("graph builds");
    assert_eq!(graph.fan_out("foundation"), 0);
    assert!(graph.fan_in("foundation") > 0);
}

/// The full strongly-connected component's own edge set, pinned exactly -- **six** modules and
/// thirteen edges, down from the seven modules/fifteen edges the coupling-gate round measured.
/// `recognition_claim` left the component when carried-defect C relocated
/// `load_maintainer_trust_policy_or_empty` from `recognition_claim.rs` into `trust.rs`: the
/// `trust -> recognition_claim` leg it added is gone, and `recognition_claim -> trust` (still
/// real -- `recognition_claim.rs` still uses `MaintainerTrustPolicy`) is no longer part of any
/// cycle, so it needs no `DECLARED_CYCLES` entry at all. This test exists so a future change to
/// any of these edges is caught here first, not discovered again by surprise.
#[test]
fn the_scc_has_exactly_this_edge_set() {
    let graph = build(&store_src_root()).expect("graph builds");
    let scc_nodes = [
        "active",
        "refs",
        "trust",
        "worktree_patch",
        "patch_replay",
        "lifecycle_cache",
    ];
    let mut edges: Vec<(String, String)> = graph
        .edges
        .iter()
        .filter(|(a, b)| scc_nodes.contains(&a.as_str()) && scc_nodes.contains(&b.as_str()))
        .cloned()
        .collect();
    edges.sort();
    let mut expected: Vec<(String, String)> = [
        ("active", "refs"),
        ("active", "worktree_patch"),
        ("lifecycle_cache", "patch_replay"),
        ("patch_replay", "active"),
        ("patch_replay", "lifecycle_cache"),
        ("patch_replay", "refs"),
        ("refs", "active"),
        ("refs", "trust"),
        ("trust", "refs"),
        ("worktree_patch", "active"),
        ("worktree_patch", "lifecycle_cache"),
        ("worktree_patch", "patch_replay"),
        ("worktree_patch", "refs"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_owned(), b.to_owned()))
    .collect();
    expected.sort();
    assert_eq!(
        edges, expected,
        "SCC edge set changed -- update this test and DECLARED_CYCLES together"
    );
}

/// `recognition_claim -> trust` is real (checked directly, not merely absent from the SCC list
/// above by omission) but no longer cyclic -- confirms carried-defect C's relocation broke the
/// cycle rather than merely hiding the edge from this test's own filter.
#[test]
fn recognition_claim_to_trust_survives_but_is_no_longer_cyclic() {
    let graph = build(&store_src_root()).expect("graph builds");
    assert!(
        graph
            .edges
            .contains(&("recognition_claim".to_owned(), "trust".to_owned())),
        "recognition_claim still legitimately depends on trust (MaintainerTrustPolicy)"
    );
    assert!(
        !graph
            .edges
            .contains(&("trust".to_owned(), "recognition_claim".to_owned())),
        "the relocation must have removed the return leg"
    );
    let in_an_scc = super::strongly_connected_components(&graph)
        .into_iter()
        .any(|component| {
            component.len() > 1 && component.contains(&"recognition_claim".to_owned())
        });
    assert!(
        !in_an_scc,
        "recognition_claim must no longer be in any cycle"
    );
}

/// Every SCC-internal edge must be accounted for by some [`super::super::DECLARED_CYCLES`] entry
/// -- the property the gate itself checks, proven directly against the real graph rather than only
/// trusted because `check()` is green (which could also be green from a bug that finds no edges at
/// all).
#[test]
fn every_scc_edge_is_covered_by_a_declared_cycle() {
    let graph = build(&store_src_root()).expect("graph builds");
    let declared: std::collections::BTreeSet<(String, String)> = super::super::DECLARED_CYCLES
        .iter()
        .flat_map(|entry| {
            entry
                .edges
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        })
        .collect();
    for component in super::strongly_connected_components(&graph) {
        if component.len() < 2 {
            continue;
        }
        let members: std::collections::BTreeSet<&str> =
            component.iter().map(String::as_str).collect();
        for (from, to) in &graph.edges {
            if members.contains(from.as_str()) && members.contains(to.as_str()) {
                assert!(
                    declared.contains(&(from.clone(), to.clone())),
                    "undeclared SCC edge: {from} -> {to}"
                );
            }
        }
    }
}
