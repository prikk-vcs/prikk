#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeSet;

use super::{
    DECLARED_CYCLES, DECLARED_HUBS, LOWER_LAYER, UPPER_LAYER, check,
    check_allowlists_are_well_formed, check_declared_entries_still_exist, check_layer,
    check_with_layers, graph_report, layer_report, top_level,
};
use crate::boundary::BoundaryError;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .to_path_buf()
}

#[test]
fn the_real_repository_passes_with_no_undeclared_cycle_or_hub() {
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(&repo_root(), &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

/// Self-guard on the escape hatch itself (`DECLARED_UNDOCUMENTED`'s own tests are the model): a
/// stale or misspelled entry must not silently exempt nothing.
#[test]
fn declared_cycles_have_real_reasons_and_removal_statements() {
    for entry in DECLARED_CYCLES {
        assert!(!entry.edges.is_empty(), "{:?}", entry.edges);
        assert!(entry.reason.trim().len() >= 20, "{:?}", entry.edges);
        assert!(
            entry.what_would_remove_it.trim().len() >= 20,
            "{:?}",
            entry.edges
        );
    }
}

#[test]
fn declared_hubs_have_real_reasons() {
    for entry in DECLARED_HUBS {
        assert!(entry.reason.trim().len() >= 20, "{}", entry.module);
    }
}

/// Control 4: an entry with an empty or placeholder reason is refused, same as
/// `DECLARED_UNDOCUMENTED`'s own guard.
#[test]
fn a_placeholder_reason_is_refused() {
    struct FakeCycle {
        reason: &'static str,
        what_would_remove_it: &'static str,
    }
    // Mirrors `check_allowlists_are_well_formed`'s own logic against a deliberately bad entry,
    // since the real constant cannot be mutated for a test.
    let fake = FakeCycle {
        reason: "todo",
        what_would_remove_it: "n/a",
    };
    assert!(super::is_placeholder(fake.reason));
    assert!(super::is_placeholder(fake.what_would_remove_it));
    let mut errors = Vec::new();
    check_allowlists_are_well_formed(&mut errors);
    assert!(
        errors.is_empty(),
        "the real DECLARED_CYCLES/DECLARED_HUBS must already be well-formed"
    );
}

/// Review v1 §5's required follow-up: the allowlist binds in both directions. A synthetic graph
/// containing none of `DECLARED_CYCLES`'s edges and none of `DECLARED_HUBS`'s modules at
/// hub-level fan must report every single declared entry as stale, by name.
#[test]
fn stale_declared_entries_are_rejected() {
    let graph = super::graph::ModuleGraph {
        modules: DECLARED_HUBS
            .iter()
            .map(|entry| entry.module.to_owned())
            .collect(),
        edges: BTreeSet::new(),
    };
    let declared_edges: BTreeSet<(String, String)> = DECLARED_CYCLES
        .iter()
        .flat_map(|entry| {
            entry
                .edges
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        })
        .collect();
    // No subtree cycles at all in this synthetic graph (empty `edges`) -- every declared edge
    // must be reported stale, the same way an empty raw `graph.edges` did before RFC 131 §6c.4
    // moved staleness from "is this a raw edge" to "is this a subtree cycle".
    let subtree_cycle_edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut errors = Vec::new();
    check_declared_entries_still_exist(&graph, &declared_edges, &subtree_cycle_edges, &mut errors);

    for (from, to) in &declared_edges {
        assert!(
            errors
                .iter()
                .any(|error| error.detail.contains(&format!("{from} -> {to}"))),
            "expected a stale-entry error for {from} -> {to}: {errors:?}"
        );
    }
    for entry in DECLARED_HUBS {
        assert!(
            errors
                .iter()
                .any(|error| error.detail.contains(entry.module) && error.detail.contains("hub")),
            "expected a stale-entry error for hub `{}`: {errors:?}",
            entry.module
        );
    }
}

/// The inverse: a graph containing exactly the declared edges and hub-level fan must report
/// nothing stale -- proves the check does not simply always fire.
#[test]
fn non_stale_declared_entries_are_accepted() {
    let mut modules: BTreeSet<String> = DECLARED_HUBS
        .iter()
        .map(|entry| entry.module.to_owned())
        .collect();
    let declared_edges: BTreeSet<(String, String)> = DECLARED_CYCLES
        .iter()
        .flat_map(|entry| {
            entry
                .edges
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        })
        .collect();
    for (from, to) in &declared_edges {
        modules.insert(from.clone());
        modules.insert(to.clone());
    }
    // Give every declared hub exactly `HUB_THRESHOLD` synthetic fan-in and fan-out neighbours,
    // distinct per hub so they do not interfere with each other's counts.
    let mut edges = declared_edges.clone();
    for entry in DECLARED_HUBS {
        for i in 0..super::HUB_THRESHOLD {
            let inbound = format!("__synthetic_in_{}_{i}", entry.module);
            let outbound = format!("__synthetic_out_{}_{i}", entry.module);
            modules.insert(inbound.clone());
            modules.insert(outbound.clone());
            edges.insert((inbound, entry.module.to_owned()));
            edges.insert((entry.module.to_owned(), outbound));
        }
    }
    let graph = super::graph::ModuleGraph { modules, edges };
    // Synthesize that every declared edge is still a current subtree cycle -- this test exercises
    // `check_declared_entries_still_exist`'s own comparison logic, not `subtree_cycles`'s
    // computation (covered separately in `graph::tests`), so the "current truth" it compares
    // against is handed in directly rather than derived from `graph`.
    let subtree_cycle_edges = declared_edges.clone();
    let mut errors = Vec::new();
    check_declared_entries_still_exist(&graph, &declared_edges, &subtree_cycle_edges, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

/// RFC 131 §6e step 0: `--graph`'s hub list and the gate's own hub check must agree, on the real
/// repository.
///
/// **The census this emission exists for is only worth taking if it says what the gate says.** Both
/// sides are recomputed here from the same source tree — the emission through `graph_report`, the
/// gate's view by applying `HUB_THRESHOLD` to `graph::build` exactly as `check` does — and compared
/// as sets. A future change that alters one path and not the other fails here rather than producing
/// a census that quietly disagrees with the gate it was meant to match.
#[test]
fn the_graph_emission_and_the_gate_agree_on_the_hub_list() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root");
    let src_root = root.join("crates/prikk-store/src");

    let emitted = super::graph_report(root).expect("graph report");
    let graph = super::graph::build(&src_root).expect("graph build");

    let gate_hubs: BTreeSet<String> = graph
        .modules
        .iter()
        .filter(|module| graph.fan_in(module).min(graph.fan_out(module)) >= super::HUB_THRESHOLD)
        .cloned()
        .collect();
    let emitted_hubs: BTreeSet<String> = emitted.hubs.iter().cloned().collect();
    assert_eq!(
        emitted_hubs, gate_hubs,
        "`--graph`'s hub list must be the gate's own"
    );

    // And the per-node flags must agree with the list, so a reader can filter on `is_hub` instead of
    // recomputing `hub_score >= hub_threshold` themselves.
    for node in &emitted.modules {
        assert_eq!(
            node.is_hub,
            gate_hubs.contains(&node.module),
            "{}: is_hub disagrees with the gate",
            node.module
        );
        assert_eq!(
            node.hub_score,
            node.fan_in.min(node.fan_out),
            "{}: hub_score must be min(fan_in, fan_out)",
            node.module
        );
    }

    // The emission is not vacuous: this repository has modules, edges and at least one hub.
    assert!(
        emitted.modules.len() > 40,
        "{} modules",
        emitted.modules.len()
    );
    assert!(!emitted.edges.is_empty());
    assert!(!emitted.hubs.is_empty());
    assert_eq!(emitted.hub_threshold, super::HUB_THRESHOLD);
}

// RFC 149 path B: the layer rule.

fn real_graph() -> super::graph::ModuleGraph {
    super::graph::build(&repo_root().join("crates/prikk-store/src")).expect("graph builds")
}

fn layer_errors(upper: &[&str], lower: &[&str]) -> Vec<BoundaryError> {
    let mut errors = Vec::new();
    check_layer(&real_graph(), upper, lower, &mut errors);
    errors
}

/// The real tree: zero crossing edges, every top-level module on exactly one side.
#[test]
fn the_real_tree_has_no_layer_violation_and_every_module_is_classified() {
    let report = layer_report(&real_graph(), &UPPER_LAYER, &LOWER_LAYER);
    assert_eq!(report.violations.len(), 0, "{:?}", report.violations);
    assert!(report.unclassified.is_empty(), "{:?}", report.unclassified);
    assert!(report.stale.is_empty(), "{:?}", report.stale);
    assert!(report.listed_twice.is_empty(), "{:?}", report.listed_twice);
}

/// Perturb: `checkout` moved up. `worktree.rs` names it in production, so the move crosses the layer
/// and the gate must name that edge.
#[test]
fn moving_checkout_up_names_the_edges_that_cross() {
    let upper: Vec<&str> = UPPER_LAYER.iter().copied().chain(["checkout"]).collect();
    let lower: Vec<&str> = LOWER_LAYER
        .iter()
        .copied()
        .filter(|module| *module != "checkout")
        .collect();
    let errors = layer_errors(&upper, &lower);
    assert!(
        errors
            .iter()
            .any(|error| error.category == "layer" && error.detail.contains("worktree -> checkout")),
        "{errors:?}"
    );
}

/// Perturb the other way: `merge` moved down. Its own edges into the upper layer now cross, and each
/// is named -- computed from the graph rather than written down, and required to be non-empty so the
/// control cannot pass on an empty set.
#[test]
fn moving_merge_down_names_the_edges_that_cross() {
    let graph = real_graph();
    let expected: Vec<String> = graph
        .edges
        .iter()
        .filter(|(from, to)| {
            top_level(from) == "merge"
                && top_level(to) != "merge"
                && UPPER_LAYER.contains(&top_level(to))
        })
        .map(|(from, to)| format!("{from} -> {to}"))
        .collect();
    assert!(
        !expected.is_empty(),
        "merge reaches nothing upper -- the control would be vacuous"
    );
    let upper: Vec<&str> = UPPER_LAYER
        .iter()
        .copied()
        .filter(|module| *module != "merge")
        .collect();
    let lower: Vec<&str> = LOWER_LAYER.iter().copied().chain(["merge"]).collect();
    let errors = layer_errors(&upper, &lower);
    for edge in &expected {
        assert!(
            errors
                .iter()
                .any(|error| error.detail.contains(edge.as_str())),
            "{edge} not named: {errors:?}"
        );
    }
}

/// Perturb: a fake name in a constant is listed but absent.
#[test]
fn a_fake_listed_module_is_reported_stale() {
    let upper: Vec<&str> = UPPER_LAYER
        .iter()
        .copied()
        .chain(["no_such_module"])
        .collect();
    let errors = layer_errors(&upper, &LOWER_LAYER);
    assert!(
        errors
            .iter()
            .any(|error| error.detail.contains("`no_such_module`")
                && error.detail.contains("not a top-level module")),
        "{errors:?}"
    );
}

/// Perturb: a real module dropped from the lists is unclassified -- the growth-direction control.
#[test]
fn a_real_module_on_neither_list_is_unclassified() {
    let lower: Vec<&str> = LOWER_LAYER
        .iter()
        .copied()
        .filter(|module| *module != "wal")
        .collect();
    let errors = layer_errors(&UPPER_LAYER, &lower);
    assert!(
        errors
            .iter()
            .any(|error| error.detail.contains("unclassified top-level module `wal`")),
        "{errors:?}"
    );
}

/// A module on both lists is refused rather than resolved in favour of either side.
#[test]
fn a_module_on_both_lists_is_refused() {
    let upper: Vec<&str> = UPPER_LAYER.iter().copied().chain(["wal"]).collect();
    let errors = layer_errors(&upper, &LOWER_LAYER);
    assert!(
        errors
            .iter()
            .any(|error| error.detail.contains("`wal` is listed in both")),
        "{errors:?}"
    );
}

/// `--graph` emits the same layer view the gate enforces.
#[test]
fn the_graph_emission_and_the_gate_agree_on_the_layer() {
    let emitted = graph_report(&repo_root()).expect("graph report").layer;
    assert_eq!(
        emitted,
        layer_report(&real_graph(), &UPPER_LAYER, &LOWER_LAYER)
    );
}

/// The whole gate, not only `check_layer`: with `checkout` moved up, `check` itself reports the
/// crossing. This is what fails if the gate's call to `check_layer` is ever deleted -- every other
/// layer control calls the checker directly and would stay green.
#[test]
fn the_gate_itself_applies_the_layer_rule() {
    let upper: Vec<&str> = UPPER_LAYER.iter().copied().chain(["checkout"]).collect();
    let lower: Vec<&str> = LOWER_LAYER
        .iter()
        .copied()
        .filter(|module| *module != "checkout")
        .collect();
    let mut errors = Vec::new();
    check_with_layers(&repo_root(), &upper, &lower, &mut errors);
    assert!(
        errors
            .iter()
            .any(|error| error.category == "layer" && error.detail.contains("worktree -> checkout")),
        "{errors:?}"
    );
}
