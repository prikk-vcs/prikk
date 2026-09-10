#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{
    build, crate_idents_for_tests, production_edge_text_for_tests, reexports_for_tests,
    resolve_target_for_tests, walk_root_for_tests,
};

fn store_src_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .join("crates/prikk-store/src")
}

/// Write `content` to `relative` (a `.rs` path) under `root`, creating parent directories as
/// needed -- the same helper shape `placement/tests.rs` already uses for its own synthetic trees.
fn write_module(root: &Path, relative: &str, content: &str) {
    let full = root.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
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
    // RFC 131 §6c (2026-09-10): this count's own MEANING changed, not just its number. Before this
    // amendment `walk` returned one entry per *top-level* module only, with every descendant
    // file's text concatenated into it -- that count was 53. Now `walk` returns one entry per
    // production module at *every depth*, keyed by its qualified path from the crate root
    // (`foundation::layout` distinct from `foundation::fsutil`), each holding only its own file's
    // text -- so this is a count of files, not of top-level names, and grew accordingly: 53 -> 127.
    // (History preserved for the top-level figure this superseded: 61 production modules at the
    // coupling-gate-graph-contradiction round's own count shrank to 51 at RFC 131 §2.2a's grouping
    // 2026-09-08 -- `foundation` 8 -> 1, `author` 2 -> 1, `node` 2 -> 1, `received_index` folding
    // into `received` (-1); RFC 142 added `show` (51 -> 52); RFC 144 §4o.2 added
    // `rename_declaration` (52 -> 53).)
    assert_eq!(modules.len(), 127, "modules: {modules:?}");
    assert!(modules.contains("foundation"));
    assert!(modules.contains("show"));
    // The bare, pre-amendment names no longer exist as keys at all -- only their qualified forms
    // do, restoring the separate visibility RFC 131 §6b.4 disclosed as a cost of grouping.
    assert!(!modules.contains("fsutil"));
    assert!(!modules.contains("layout"));
    assert!(modules.contains("foundation::fsutil"));
    assert!(modules.contains("foundation::layout"));
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

/// RFC 131 §6c (2026-09-10): every leg this test used to pin as a top-level-to-top-level edge is
/// still a real, verified textual reference (none was invented, none was lost) -- but nine of the
/// nine no longer name a `DECLARED_CYCLES`-shaped pair, because the qualified-name amendment
/// re-attributes each to the *specific* file that actually writes it, and in every case that file
/// turns out to be a submodule on at least one end. The measured consequence (§4 of the round's own
/// handoff, restated in the round's report): the six-module SCC these nine legs used to close does
/// not survive at this precision -- `the_scc_has_exactly_this_edge_set` below now pins zero edges
/// among the six top-level names, and `graph::tests::probe`-style direct queries during that round
/// confirmed zero elementary cycles anywhere in the 127-node graph. This test's own renamed
/// assertion: the underlying edges are still found, each at the qualified node that actually writes
/// it, not silently dropped by the more precise resolution (control 5's own concern, restated here
/// against nine concrete, previously-relied-upon cases rather than one).
#[test]
fn every_previously_cited_cycle_leg_survives_at_its_correct_qualified_node() {
    let graph = build(&store_src_root()).expect("graph builds");
    for (from, to) in [
        // Unchanged: the referencing file already was the top-level file itself.
        ("active", "refs"),
        ("trust", "refs"),
        ("active", "worktree_patch"),
        ("patch_replay", "active"),
        // Re-attributed: the old top-level pair named the *group*; the real writer is one of its
        // submodules, on one or both ends.
        ("refs::evidence", "active"),
        ("refs::evidence", "trust"),
        ("lifecycle_cache::replay", "patch_replay::decode"),
        ("patch_replay", "lifecycle_cache::incremental"),
        ("patch_replay", "lifecycle_cache::replay"),
        ("worktree_patch::node_authoring", "patch_replay"),
    ] {
        assert!(
            graph.edges.contains(&(from.to_owned(), to.to_owned())),
            "expected edge {from} -> {to}"
        );
    }
    // The four legs this list replaces, each confirmed genuinely absent (not merely renamed) --
    // `refs -> active`/`refs -> trust`/`lifecycle_cache -> patch_replay`/`worktree_patch ->
    // patch_replay` were always the *group's* aggregate attribution of a submodule's own
    // reference, never something refs.rs/lifecycle_cache.rs/worktree_patch.rs wrote itself.
    for (from, to) in [
        ("refs", "active"),
        ("refs", "trust"),
        ("lifecycle_cache", "patch_replay"),
        ("worktree_patch", "patch_replay"),
    ] {
        assert!(
            !graph.edges.contains(&(from.to_owned(), to.to_owned())),
            "{from} -> {to} was always an aggregate attribution, not a direct reference -- it \
             should not reappear now that node text is per-file"
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
    // RFC 131 §2.2a's grouping folded `fsutil` into the `foundation` node, and this test's own
    // prior form could only check the *aggregate* group's fan-out -- `fsutil`'s own figure was no
    // longer separately checkable (a disclosed cost: RFC 130 §6's "fsutil is the crate's one
    // genuinely clean seam" argument leaned on this exact number in isolation, and RFC 131 §6b.4
    // recorded losing it).
    //
    // RFC 131 §6c's qualified-name amendment (2026-09-10) restores that separate visibility as a
    // side effect, not a goal of this round: `foundation::fsutil` is its own node again, and its
    // fan-out is checkable directly, at the exact granularity RFC 130 §6 originally measured.
    let graph = build(&store_src_root()).expect("graph builds");
    assert_eq!(graph.fan_out("foundation::fsutil"), 0);
    assert!(graph.fan_in("foundation::fsutil") > 0);
    // `foundation.rs` itself, distinct from `foundation::fsutil`, is now just the thin `pub mod`
    // declarations file every grouped module has -- no production code of its own to produce an
    // edge either way.
    assert_eq!(graph.fan_out("foundation"), 0);
    assert_eq!(graph.fan_in("foundation"), 0);
}

/// RFC 131 §6c (2026-09-10): before this amendment, these six top-level names closed a real
/// strongly-connected component -- six modules, thirteen edges, pinned here exactly. **That SCC
/// does not survive qualified naming.** Every one of the thirteen edges was contributed by
/// concatenating a submodule's own text into its top-level ancestor; measured directly against the
/// per-file graph, none of the six top-level names has any edge back into the group once each
/// submodule's own reference is attributed to itself rather than its parent (verified: zero
/// elementary cycles anywhere in the full 127-node graph, not merely among these six -- see the
/// round's own report). This test is renamed and repurposed from pinning the SCC's edge set to
/// pinning its **dissolution**: the exact top-level-to-top-level edge set that remains among the
/// six (five edges, all of them one-directional -- `graph_matches_every_cited_cycle_leg`'s
/// successor above shows where each of the old thirteen actually went), plus a direct assertion
/// that none of the six sits in any multi-member component any more. `DECLARED_CYCLES` is
/// deliberately left untouched by this round (RFC 131 §6c.2: "populate no allowlist, fix no
/// cycle") -- its eight entries are now stale against this graph, reported as such rather than
/// removed; `boundary::coupling::tests::the_real_repository_passes_with_no_undeclared_cycle_or_hub`
/// documents that failure as the round's own expected, ruled-after-the-number outcome.
#[test]
fn the_former_scc_has_dissolved_into_this_five_edge_dag_fragment() {
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
        ("patch_replay", "active"),
        ("patch_replay", "refs"),
        ("trust", "refs"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_owned(), b.to_owned()))
    .collect();
    expected.sort();
    assert_eq!(
        edges, expected,
        "the former SCC's remaining top-level-to-top-level edge set changed -- update this test \
         (and this round's own report numbers, if this is read later) together"
    );
    for node in scc_nodes {
        let in_a_multi_member_component = super::strongly_connected_components(&graph)
            .into_iter()
            .any(|component| component.len() > 1 && component.contains(&node.to_owned()));
        assert!(
            !in_a_multi_member_component,
            "{node} must no longer be part of any cycle"
        );
    }
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

/// RFC 131 §6c.4: **no longer the property the gate itself checks** -- `check()` compares
/// `subtree_cycles()` against `DECLARED_CYCLES` now, not raw-node SCC edges (the raw graph has
/// zero cycles at all, confirmed at `42bcab15`, so this loop's body never executes -- vacuously
/// true, not a defect). Kept as a historical fact about the *raw* graph rather than deleted, the
/// same reasoning `recognition_claim_to_trust_survives_but_is_no_longer_cyclic` and
/// `the_former_scc_has_dissolved_into_this_five_edge_dag_fragment` already apply.
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

// RFC 131 §6c -- qualified module names. Controls 1-5, each named after the handoff's own
// numbering (`qualified-module-names-handoff-v1.md` §6).

/// Control 1 (the whole point of the round): a cycle wholly inside one former top-level module is
/// now visible. Two modules under one parent, `crate::` path references in both directions -- under
/// the pre-amendment model both files' text concatenated into one `parent` node and each reference
/// resolved to `parent` itself, a self-loop, excluded; now each is its own node and the cycle is a
/// real, reported one.
#[test]
fn control1_a_cycle_inside_one_former_top_level_module_is_now_visible() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_module(root, "lib.rs", "mod parent;\n");
    write_module(root, "parent.rs", "mod child_a;\nmod child_b;\n");
    write_module(
        root,
        "parent/child_a.rs",
        "pub fn a() { crate::parent::child_b::b(); }\n",
    );
    write_module(
        root,
        "parent/child_b.rs",
        "pub fn b() { crate::parent::child_a::a(); }\n",
    );
    let graph = build(root).expect("graph builds");
    assert!(
        graph
            .edges
            .contains(&("parent::child_a".to_owned(), "parent::child_b".to_owned())),
        "expected parent::child_a -> parent::child_b, got {:?}",
        graph.edges
    );
    assert!(
        graph
            .edges
            .contains(&("parent::child_b".to_owned(), "parent::child_a".to_owned())),
        "expected parent::child_b -> parent::child_a, got {:?}",
        graph.edges
    );
    let cycles = graph.elementary_cycles();
    assert!(
        cycles.iter().any(|cycle| {
            let members: BTreeSet<&str> = cycle.iter().map(String::as_str).collect();
            members.contains("parent::child_a") && members.contains("parent::child_b")
        }),
        "expected a reported cycle between parent::child_a and parent::child_b, got {cycles:?}"
    );
    // The self-loop this cycle used to collapse into, pre-amendment, must not appear now that the
    // two children are distinct nodes.
    assert!(
        !graph
            .edges
            .contains(&("parent".to_owned(), "parent".to_owned()))
    );
}

/// Control 2: resolution picks the deepest *existing* module -- `crate::a::b::C` reaches `a::b`,
/// not `a`, when `a::b` is itself a real module.
#[test]
fn control2_resolution_picks_the_deepest_existing_module() {
    let modules: BTreeSet<String> = ["a", "a::b"].into_iter().map(str::to_owned).collect();
    let owners: BTreeMap<String, String> = BTreeMap::new();
    assert_eq!(
        resolve_target_for_tests("a::b::C", &modules, &owners),
        Some("a::b".to_owned())
    );
}

/// Control 3: a path whose deeper segments are not modules still reaches its module --
/// `crate::a::Item` reaches `a` when `a::Item` is not itself a module (an item name is not a node).
#[test]
fn control3_a_path_whose_deeper_segments_are_not_modules_still_reaches_its_module() {
    let modules: BTreeSet<String> = ["a"].into_iter().map(str::to_owned).collect();
    let owners: BTreeMap<String, String> = BTreeMap::new();
    assert_eq!(
        resolve_target_for_tests("a::Item", &modules, &owners),
        Some("a".to_owned())
    );
}

/// Control 4: grouped imports resolve per element, each to its own deepest node -- `use
/// crate::{a, b::c};` must record two distinct edges, `-> a` and `-> b::c`, never a group-wide
/// first-segment collapse (`-> a` and `-> b` alone, losing `c`'s own depth).
#[test]
fn control4_grouped_imports_resolve_each_element_to_its_own_deepest_node() {
    let idents = crate_idents_for_tests("use crate::{a, b::c, b::c::D};");
    assert_eq!(
        idents,
        vec!["a".to_owned(), "b::c".to_owned(), "b::c::D".to_owned()]
    );

    // End to end: a synthetic tree where `b::c` is a real module, so the group must resolve to it
    // specifically, not to `b`.
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_module(root, "lib.rs", "mod importer;\nmod a;\nmod b;\n");
    write_module(root, "a.rs", "");
    write_module(root, "b.rs", "mod c;\n");
    write_module(root, "b/c.rs", "");
    write_module(root, "importer.rs", "use crate::{a, b::c};\n");
    let graph = build(root).expect("graph builds");
    assert!(
        graph
            .edges
            .contains(&("importer".to_owned(), "a".to_owned()))
    );
    assert!(
        graph
            .edges
            .contains(&("importer".to_owned(), "b::c".to_owned()))
    );
    assert!(
        !graph
            .edges
            .contains(&("importer".to_owned(), "b".to_owned())),
        "the b::c element must not collapse to its parent b when b::c is itself a real module"
    );
}

/// Control 5 (the regression guard most easily vacuous to pass): a `crate::` reference the
/// pre-amendment scanner already found must not be silently lost now that resolution walks a
/// longer path. The concrete, already-real case: `patch_replay.rs` writes
/// `crate::ActiveRefMetadata::Valid(...)` (and its sibling variants) -- a re-exported *item*
/// accessed with a further segment (an enum variant, not a module). A resolver that only consulted
/// the re-export table for a bare single-segment path would drop this edge entirely, since
/// `"ActiveRefMetadata::Valid"` is two segments and neither it nor `"ActiveRefMetadata"` is a
/// module.
#[test]
fn control5_a_reexported_item_accessed_with_a_further_segment_still_resolves() {
    let modules: BTreeSet<String> = BTreeSet::new();
    let owners: BTreeMap<String, String> = [("ActiveRefMetadata".to_owned(), "active".to_owned())]
        .into_iter()
        .collect();
    assert_eq!(
        resolve_target_for_tests("ActiveRefMetadata::Valid", &modules, &owners),
        Some("active".to_owned()),
        "a reexported item accessed with a further segment must still resolve through the \
         re-export table, exactly as a bare reference to it already does"
    );
}

/// Control 5's own real-repository corroboration -- checked honestly, not assumed: `patch_replay
/// -> active` is real (see `patch_replay_never_writes_crate_active_directly` above: it never
/// writes `crate::active::` directly). **Perturbing `resolve_target` to drop the multi-segment
/// re-export fallback did not fail this test** -- `patch_replay.rs` also writes the bare,
/// single-segment `crate::read_active_ref_metadata(...)`, which alone keeps this edge alive
/// regardless of whether `crate::ActiveRefMetadata::Valid` resolves. This test is therefore *not*
/// the sharp regression guard for control 5's specific failure mode; `control5_a_reexported_item_
/// accessed_with_a_further_segment_still_resolves` above is the one that actually caught the
/// perturbation (see the round's own report for the full account) -- kept here anyway as a
/// real-repository sanity check that the edge itself is intact, not as proof of the multi-segment
/// case specifically.
#[test]
fn control5_patch_replay_to_active_survives_through_the_real_repository() {
    let graph = build(&store_src_root()).expect("graph builds");
    assert!(
        graph
            .edges
            .contains(&("patch_replay".to_owned(), "active".to_owned())),
        "patch_replay -> active must still be found through crate::ActiveRefMetadata::Valid/\
         Missing/Invalid -- a multi-segment reference to a re-exported item, not a bare one"
    );
}

// RFC 131 §6c.4 -- subtree-aware cycle detection. Controls 1-5, named after
// `subtree-aware-cycles-handoff-v1.md` §4. `42bcab15`'s own controls above are untouched (§5:
// "do not reopen 42bcab15's node identity, resolution rule, or grouped-import handling").

/// Subtree control 1 (the round's whole point): both pairs traced by hand at review are now
/// reported. `42bcab15` reported neither (0 cycles anywhere).
///
/// **`refs <-> active` is reported exactly as traced** -- `refs.rs` itself never writes
/// `crate::active::` (only `refs::evidence` does), and `active.rs` writes `crate::refs::`
/// directly, so `refs` is the smallest subtree on that side (§6c.4 rule 3's own worked example).
///
/// **`lifecycle_cache <-> patch_replay` is reported at a node one level deeper than the handoff's
/// own informal wording** -- checked directly against source, not assumed: `lifecycle_cache.rs`
/// itself never writes `crate::patch_replay` (confirmed by grep, zero hits); only
/// `lifecycle_cache/replay.rs` does. `patch_replay.rs` in turn writes `crate::lifecycle_cache::
/// replay::TextCache` and `crate::lifecycle_cache::replay::apply_queued_patch_envelopes`
/// (`lifecycle_cache::replay` is a real module, so these resolve there, not to the coarser
/// `lifecycle_cache`). So the smallest pair the rule actually derives is `lifecycle_cache::replay
/// <-> patch_replay`, not `lifecycle_cache <-> patch_replay` -- the handoff's own table names the
/// pair the way the *old*, now-superseded `DECLARED_CYCLES` entry did, not independently re-traced
/// the way it explicitly did for `refs`/`active`. Reported as a finding, not silently matched to
/// the handoff's literal wording; see the round's own report.
#[test]
fn subtree_control1_both_traced_pairs_are_reported() {
    let graph = build(&store_src_root()).expect("graph builds");
    let cycles = graph.subtree_cycles();
    assert!(
        cycles.contains(&("active".to_owned(), "refs".to_owned())),
        "expected active -> refs among {cycles:?}"
    );
    assert!(
        cycles.contains(&("refs".to_owned(), "active".to_owned())),
        "expected refs -> active among {cycles:?}"
    );
    assert!(
        cycles.contains(&(
            "lifecycle_cache::replay".to_owned(),
            "patch_replay".to_owned()
        )),
        "expected lifecycle_cache::replay -> patch_replay among {cycles:?}"
    );
    assert!(
        cycles.contains(&(
            "patch_replay".to_owned(),
            "lifecycle_cache::replay".to_owned()
        )),
        "expected patch_replay -> lifecycle_cache::replay among {cycles:?}"
    );
}

/// Subtree control 2: the reported pair is neither the top-level pair by default nor the deepest
/// node pair -- a synthetic tree where the true minimal pair sits one level below the top on one
/// side, with a deeper leaf on that same side excluded because the *return* edge does not reach
/// it. `x::mid::leaf -> y` (forward, from the deepest node on the x side) and `y -> x::mid`
/// (return, landing at `x::mid`, not at `x::mid::leaf` and not at bare `x`). The deepest node
/// (`x::mid::leaf`) cannot be the reported pair (nothing reaches specifically into it), and the
/// top-level node (`x`) is not minimal either (`x::mid` still works and is deeper) -- `x::mid` is
/// the unique node satisfying both.
#[test]
fn subtree_control2_reports_the_smallest_pair_not_top_level_not_deepest() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_module(root, "lib.rs", "mod x;\nmod y;\n");
    write_module(root, "x.rs", "mod mid;\n");
    write_module(root, "x/mid.rs", "mod leaf;\n");
    write_module(
        root,
        "x/mid/leaf.rs",
        "pub fn f() { crate::y::something(); }\n",
    );
    write_module(root, "y.rs", "pub fn g() { crate::x::mid::something(); }\n");
    let graph = build(root).expect("graph builds");
    let cycles = graph.subtree_cycles();
    assert_eq!(
        cycles,
        vec![
            ("x::mid".to_owned(), "y".to_owned()),
            ("y".to_owned(), "x::mid".to_owned()),
        ],
        "expected exactly the (x::mid, y) pair -- neither the top-level (x, y) pair nor the \
         deepest (x::mid::leaf, y) pair"
    );
}

/// Subtree control 3 (perturbed below): a child referencing its own parent absolutely is not a
/// cycle. Synthetic (a lone child->parent reference, nothing else) plus a direct structural
/// invariant against the real repository: no reported pair is ever an ancestor/descendant of the
/// other -- the general form of "none of the 35 files that reference their own parent absolutely
/// become a reported cycle" the handoff names, checked without needing to enumerate all 35 by
/// name.
#[test]
fn subtree_control3_a_child_referencing_its_own_parent_is_not_a_cycle() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_module(root, "lib.rs", "mod parent;\n");
    write_module(root, "parent.rs", "mod child;\n");
    write_module(
        root,
        "parent/child.rs",
        "pub fn f() { crate::parent::something(); }\n",
    );
    let graph = build(root).expect("graph builds");
    assert!(
        graph.subtree_cycles().is_empty(),
        "a child referencing its own parent must not report a cycle: {:?}",
        graph.subtree_cycles()
    );
    assert!(
        !graph.subtree_depends("parent::child", "parent"),
        "rule 4: an ancestor/descendant edge contributes nothing, not even one-directional \
         dependency"
    );
}

/// Subtree control 3's real-repository corroboration: no pair `subtree_cycles` reports is ever an
/// ancestor/descendant of the other, checked against the full 127-node graph rather than trusted
/// from the synthetic case alone.
#[test]
fn subtree_control3_no_reported_pair_is_ancestor_or_descendant_in_the_real_repository() {
    let graph = build(&store_src_root()).expect("graph builds");
    for (from, to) in graph.subtree_cycles() {
        assert!(
            !from.starts_with(&format!("{to}::")) && !to.starts_with(&format!("{from}::")),
            "{from} and {to} are ancestor/descendant -- must never be reported as a cycle"
        );
    }
}

/// Subtree control 4: `42bcab15`'s own control 1 (a cycle wholly inside one former top-level
/// module, `parent::child_a <-> parent::child_b`) must still be *visible* -- subtree-awareness
/// must not re-hide what qualified naming exposed. `elementary_cycles` (the raw-node check that
/// test itself uses) is untouched and still passes on its own; this control additionally checks
/// that `subtree_cycles` -- the new, now-production mechanism -- reports the same pair, since two
/// leaf-level siblings with no descendants of their own is the degenerate case where subtree
/// cycles and raw-node cycles coincide.
#[test]
fn subtree_control4_a_cycle_inside_one_former_top_level_module_is_still_visible() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_module(root, "lib.rs", "mod parent;\n");
    write_module(root, "parent.rs", "mod child_a;\nmod child_b;\n");
    write_module(
        root,
        "parent/child_a.rs",
        "pub fn a() { crate::parent::child_b::b(); }\n",
    );
    write_module(
        root,
        "parent/child_b.rs",
        "pub fn b() { crate::parent::child_a::a(); }\n",
    );
    let graph = build(root).expect("graph builds");
    let cycles = graph.subtree_cycles();
    assert!(
        cycles.contains(&("parent::child_a".to_owned(), "parent::child_b".to_owned())),
        "{cycles:?}"
    );
    assert!(
        cycles.contains(&("parent::child_b".to_owned(), "parent::child_a".to_owned())),
        "{cycles:?}"
    );
}

/// Subtree control 5: a genuinely acyclic pair -- one-directional dependency between two
/// subtrees, at depth -- reports nothing, at any depth on either side.
#[test]
fn subtree_control5_a_one_directional_dependency_reports_nothing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write_module(root, "lib.rs", "mod x;\nmod y;\n");
    write_module(root, "x.rs", "mod inner;\n");
    write_module(
        root,
        "x/inner.rs",
        "pub fn f() { crate::y::something(); }\n",
    );
    write_module(root, "y.rs", "mod inner;\n");
    write_module(root, "y/inner.rs", "");
    let graph = build(root).expect("graph builds");
    assert!(
        graph.subtree_cycles().is_empty(),
        "a one-directional dependency must never be reported: {:?}",
        graph.subtree_cycles()
    );
}
