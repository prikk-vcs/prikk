//! RFC 130 / v2 handoff: the module coupling invariant over `prikk-store`'s production module
//! graph.
//!
//! **Two things this gate enforces, both as an allowlist-with-reasons, neither as a bare bound**
//! (RFC 130 §4.1, amended by §4b to cover cycles the same way §4.1 always covered hubs — "the gate
//! forces a recorded decision; it does not adjudicate one"):
//!
//! 1. **Every directed edge inside a strongly-connected component must be declared** in
//!    [`DECLARED_CYCLES`], with a reason *and* a statement of what would remove it (§4b.3 —
//!    stricter than the hub treatment, because a cycle is the thing itself, not a proxy for it:
//!    "what breaks if I change this?" is unanswerable inside one). **Checked per edge, not per
//!    elementary cycle** — a strongly-connected component of N modules can admit combinatorially
//!    many distinct elementary cycles built from the same handful of underlying edges, and
//!    blessing every recombination separately would both multiply entries without adding
//!    information and miss a genuinely new one hiding among old edges. An edge is the unit an
//!    entry can actually explain and a diff can actually change.
//! 2. **A module newly crossing the hub threshold fails until declared** in [`DECLARED_HUBS`], with
//!    a reason.
//!
//! **What this gate does not do** (§5 / v2 handoff §6): it does not gate line count or module
//! count, it does not repair the graph, and declaring an entry is not licence to act on it.
//!
//! ## History: a round found more than RFC 130's own table named, and a later one shrank it back
//!
//! §4b.4 named four cycles across six modules. The coupling-gate round's own re-derivation (v2
//! handoff §3.2's instruction to re-measure, not copy) found **fifteen distinct directed edges
//! across seven modules** — five edges and one module (`recognition_claim`) beyond that table,
//! each confirmed at source and, via `git log -S`, predating `04e9391` by weeks to months, except
//! one: `recognition_claim` had joined the component only because that same session's RFC 138
//! round added `trust -> recognition_claim` (`trust.rs`'s `load_maintainer_trust_policy_or_empty`,
//! calling an existing helper by its current address rather than relocating it).
//!
//! **RFC 138 carried-defects C then relocated that helper** into `trust.rs` itself, removing the
//! edge it had added. `recognition_claim -> trust` (the old, legitimate leg) survived — but with
//! the return leg gone, `recognition_claim` is no longer part of any cycle, and needs no
//! `DECLARED_CYCLES` entry at all. The component is back to six modules and thirteen edges; the
//! `trust` hub crossing that same edge caused is gone with it. [`DECLARED_CYCLES`] and
//! [`DECLARED_HUBS`] below reflect the post-relocation graph; `graph::tests::the_scc_has_exactly_
//! this_edge_set` and `graph::tests::recognition_claim_to_trust_survives_but_is_no_longer_cyclic`
//! pin both facts against the real repository.

mod cfg_expr;
mod graph;

use std::collections::BTreeSet;
use std::path::Path;

use super::{BoundaryError, push};

/// A module pair is a hub if it has at least this much fan-in *and* fan-out (`min(fan_in,
/// fan_out) >= HUB_THRESHOLD`). Derived from the measured distribution, not asserted -- sorted by
/// `min(fan_in, fan_out)` under RFC 131 §6c's qualified-node graph, today's ranking is 7
/// (`merge_evidence`), 6 (`patch_replay`, `active`, `wal`, `author::author_key_index`), then a
/// clean drop to 5 (`trust`, `lifecycle_cache::replay`, `patch_set_digest`). The break sits
/// between 6 and 5.
///
/// **History, resolved as of RFC 131 §6c.7: `active` and `wal` dropped out of the declared set at
/// RFC 131 §2.2a's `foundation` grouping (2026-09-08)**, a real consolidation effect rather than a
/// code change to either module -- several of each one's distinct fan-out edges into
/// `layout`/`fsutil`/`byte_cursor`/etc. collapsed into one edge into the merged `foundation` node,
/// the same way RFC 122's consolidation concentrated edges rather than adding unrelated reach
/// (`wal` 6 -> 2, `active` 6 -> 5 at the time). **The qualified-node graph (`42bcab15` onward)
/// undoes exactly that collapse** -- `foundation`'s own submodules (`foundation::layout`,
/// `foundation::fsutil`, ...) are separate nodes again, so `active`/`wal`'s fan-out is no longer
/// concentrated into one shared target, and both are declared again below at their *original*
/// reasons (§6c.7 ruled: recovered from git history, not reworded). `refs` (28/4) and
/// `lifecycle_cache` (3/2) were removed from the declared set the same round: `min()` is
/// deliberately about *bidirectional* traffic, and neither meets it any more once its own
/// submodules (`refs::evidence`, `lifecycle_cache::replay`, ...) carry their own fan
/// independently -- `refs` in particular is a sink (high fan-in, low fan-out), not a middle hub.
const HUB_THRESHOLD: usize = 6;

/// One declared cycle-forming edge: the reason it exists, and — the property that makes a cycle
/// entry stricter than a hub entry (§4b.3) — a statement of what would have to change to remove
/// it. Neither string may be empty or a placeholder
/// (`declared_cycles_have_real_reasons_and_removal_statements` below). Grouped as `edges` rather
/// than one entry per edge purely so a genuinely mutual pair (`a <-> b`) reads as one relationship
/// with one explanation instead of two identical-looking half-entries — the gate itself checks
/// each edge independently regardless of how entries group them.
struct DeclaredCycle {
    edges: &'static [(&'static str, &'static str)],
    reason: &'static str,
    what_would_remove_it: &'static str,
}

/// Every edge found inside `prikk-store`'s one strongly-connected component today. Eight entries,
/// covering all thirteen directed edges among `active`, `refs`, `trust`, `worktree_patch`,
/// `patch_replay`, `lifecycle_cache::replay` — four mutual pairs (eight edges) plus four one-way
/// relationships (one grouped pair, three singles; five edges) that close longer cycles through
/// them. RFC 131 §6c.7: `lifecycle_cache` renamed to `lifecycle_cache::replay` here (the only
/// submodule that actually writes this component's edges; `lifecycle_cache.rs` itself writes
/// none) -- the coupling is the same coupling this table has named since RFC 130, just at the
/// qualified node it always actually lived at. Checked exhaustively against the real graph by
/// `check()`'s own comparison against `ModuleGraph::subtree_cycles()` (RFC 131 §6c.4/§6c.5) --
/// `graph::tests::every_scc_edge_is_covered_by_a_declared_cycle` no longer checks this (it studies
/// the *raw* graph, which has had zero cycles since `74e6edc2`; see that test's own doc).
const DECLARED_CYCLES: &[DeclaredCycle] = &[
    DeclaredCycle {
        edges: &[
            ("commit_boundary::active", "refs"),
            ("refs", "commit_boundary::active"),
        ],
        reason: "the only cycle anyone had evaluated before this gate: active-session/WAL state \
                  and ref publication are two views of the same commit boundary, and each \
                  legitimately needs to ask the other about it (RFC 130 §2.1's own grandfather)",
        what_would_remove_it: "extracting the shared commit-boundary question both sides ask into \
                                a third module neither depends on -- not attempted here, since \
                                this pair predates this gate and no defect motivates touching it",
    },
    DeclaredCycle {
        edges: &[("refs", "trust"), ("trust", "refs")],
        reason: "refs enforces no-incomplete-publication before a trust-policy write \
                  (`ensure_no_incomplete_publication`), and trust supplies the adopted-key policy \
                  refs' own publication-trust verification reads back -- present since before \
                  `04e9391` and missed by two independent extractions at that time (RFC 130 §4a), \
                  not new, and not yet evaluated by anyone until this gate's own derivation",
        what_would_remove_it: "moving the incomplete-publication guard trust currently calls into \
                                a module neither refs nor trust depends on, or accepting the \
                                policy snapshot as a plain argument at the one call site instead \
                                of reading it back from refs -- a real option, not attempted here \
                                because no defect motivates it and RFC 130 §7 rules out module \
                                moves in this increment",
    },
    DeclaredCycle {
        edges: &[
            ("lifecycle_cache::replay", "patch_replay"),
            ("patch_replay", "lifecycle_cache::replay"),
        ],
        reason: "created by RFC 122 (`7a01168`), consolidating two duplicate baseline \
                  derivations into `lifecycle_cache::incremental::resolve_baseline_state` -- \
                  exactly the consolidation RFC 130 §4's own counter-example describes: sharing a \
                  derivation necessarily concentrates edges at the shared site, and a bare \
                  absolute-acyclicity rule would have rejected this correct, audit-required fix",
        what_would_remove_it: "giving `patch_replay` its own copy of the baseline derivation \
                                again, reintroducing the duplication RFC 122 removed -- worse, not \
                                better, and not a change this gate should ever encourage",
    },
    DeclaredCycle {
        edges: &[
            ("commit_boundary::active", "commit_boundary::worktree_patch"),
            ("commit_boundary::worktree_patch", "commit_boundary::active"),
        ],
        reason: "found by this round's own re-derivation, present since 2026-07-03/2026-08-02 \
                  (git log -S on each leg) and missed by every prior pass: `active` asks \
                  `worktree_patch::active_patch_limit_exceeded` whether the queue is full, and \
                  `worktree_patch`'s node authoring asks `active` to prepare/validate the active \
                  ref metadata it needs before authoring -- two co-designed layers each checking \
                  the half of the operation the other owns, the same shape as `active <-> refs`",
        what_would_remove_it: "extracting the patch-limit check into a module neither depends on, \
                                and having callers pass already-prepared active-ref metadata into \
                                worktree_patch's authoring functions instead of worktree_patch \
                                asking active for it mid-call -- a real, scoped option, not \
                                attempted here for the same §7 reason as the pair above",
    },
    DeclaredCycle {
        edges: &[
            ("commit_boundary::worktree_patch", "patch_replay"),
            ("patch_replay", "commit_boundary::active"),
        ],
        reason: "the original RFC 130 §4a.2/§4b.4 3-cycle's own two edges (its third leg, `active \
                  -> worktree_patch`, now has its own entry above since this round also found the \
                  return leg `worktree_patch -> active`): worktree_patch's authoring resolves a \
                  folded baseline via `patch_replay::resolve_folded_worktree_baseline`, and \
                  `patch_replay` reads active-ref metadata back through `active`'s own crate-root \
                  re-exports (`read_active_ref_metadata`/`ActiveRefMetadata`) -- the concrete case \
                  RFC 130 §1 warned an edge extractor could miss: `patch_replay.rs` never writes \
                  `crate::active::` anywhere, only the re-exported names",
        what_would_remove_it: "having `worktree_patch` pass the active-ref metadata it already \
                                holds into `patch_replay`'s call directly, so `patch_replay` never \
                                needs to ask `active` for it a second time -- a real, scoped fix, \
                                not attempted here for the same §7 reason as the others",
    },
    DeclaredCycle {
        edges: &[("commit_boundary::worktree_patch", "lifecycle_cache::replay")],
        reason: "found by this round's own re-derivation (2026-08-02, DC-66): worktree_patch's \
                  node authoring uses `lifecycle_cache`'s `TextCache`/`materialize_edited_text` \
                  to author text-edit patches against the cached derived state -- an ordinary \
                  consumer-of-a-cache relationship, closing a longer cycle only because \
                  `lifecycle_cache` already reaches back to `active`'s own layer through \
                  `patch_replay`",
        what_would_remove_it: "giving worktree_patch's text-edit authoring its own text \
                                materialization instead of reusing lifecycle_cache's, which would \
                                reintroduce duplication this crate has otherwise been removing \
                                (RFC 122's own consolidation) -- not a change worth making",
    },
    DeclaredCycle {
        edges: &[("commit_boundary::worktree_patch", "refs")],
        reason: "found by this round's own re-derivation (2026-07-15, DC-38): worktree_patch's \
                  node authoring calls `refs::ensure_no_incomplete_publication` before authoring, \
                  the identical pre-mutation guard `trust.rs` calls at two of its own call sites -- \
                  the same crash-recovery precondition enforced at every mutation entry point, not \
                  something specific to worktree_patch",
        what_would_remove_it: "moving the incomplete-publication guard into a module every mutator \
                                (worktree_patch, trust, and refs' own callers) depends on instead \
                                of refs itself -- the same option named for the refs/trust pair \
                                above, and it would remove this edge along with that one",
    },
    DeclaredCycle {
        edges: &[("patch_replay", "refs")],
        reason: "found by this round's own re-derivation (2026-06-27/2026-07-29): patch_replay \
                  needs `RefStore` to resolve which ref/block it is replaying from -- an ordinary \
                  \"replay engine reads the ref it targets\" dependency, closing a longer cycle \
                  only because refs already reaches back into the active/worktree_patch layer",
        what_would_remove_it: "passing the already-resolved block id into patch_replay's replay \
                                functions instead of having them resolve a ref via RefStore \
                                themselves -- callers already have this information in every \
                                current call site, so this is a real, scoped option, not attempted \
                                here for the same §7 reason as the others",
    },
];

/// One declared hub: a module with high fan-in *and* high fan-out, and why that is consolidation
/// rather than sprawl (RFC 130 §4.1 — the idiom this constant already existed for, before §4b
/// extended it to cycles too).
struct DeclaredHub {
    module: &'static str,
    reason: &'static str,
}

/// Today's five hubs by `min(fan_in, fan_out)`, reconciled at RFC 131 §6c.7 against the
/// qualified-node graph (`cc15e616`): `refs` (28/4, min 4) and `lifecycle_cache` (3/2, min 2) are
/// **removed** -- neither meets the threshold any more once their own submodules
/// (`refs::evidence`, `lifecycle_cache::replay`, ...) carry their own fan independently; `refs`
/// with fan-in 28 is a sink, not a middle hub, which is exactly what `min()`'s bidirectional-only
/// shape is for. `active` (7/6) and `wal` (13/6) are **declared again**: both were removed at RFC
/// 131 §2.2a's `foundation` grouping (2026-09-08) purely because several of each module's distinct
/// fan-out edges into `layout`/`fsutil`/`byte_cursor`/etc. collapsed into one edge into the merged
/// `foundation` node (`wal` 6 -> 2, `active` 6 -> 5 at the time) -- a recorded artifact of
/// aggregation, not a code change to either module, and the qualified-node graph undoes it
/// precisely. Their reasons below are recovered from git history (`4acd7e8a`, the commit whose own
/// diff shows the text immediately before removal), not reworded. `author::author_key_index`
/// (6/6) is **genuinely newly visible** -- a submodule no mechanism before `42bcab15` could name at
/// all. `trust` briefly joined this list too, for exactly as long as RFC 138's own `trust ->
/// recognition_claim` edge existed; carried-defects C removed that edge along with the cycle it
/// caused, and `trust` dropped back below the threshold with it (see the module doc).
/// `merge_evidence` crossed it at RFC 142 (2026-09-08): `show` reuses `merge_evidence::
/// lifecycle_state_at` rather than duplicating its private `lineage_horizon`/`replay_derived_
/// state` call sequence, the same "add one narrow function instead of widening internals" shape
/// RFC 131 §3 argued for.
const DECLARED_HUBS: &[DeclaredHub] = &[
    DeclaredHub {
        module: "merge_evidence",
        reason: "RFC 142's `show` reuses this module's own lineage-horizon-to-replay sequence \
                  through one new narrow function (`lifecycle_state_at`) rather than duplicating \
                  it or widening `lifecycle_cache`'s already-`pub(crate)` internals further -- a \
                  second real consumer sharing one derivation, not two derivations existing \
                  because reach crept outward",
    },
    DeclaredHub {
        module: "patch_replay",
        reason: "the shared replay engine every checkout, verification, and merge path drives, \
                  and RFC 122's consolidation concentrated a second baseline derivation into it on \
                  top of that -- fan-out grew for the same reason the lifecycle_cache cycle above \
                  exists: a correct consolidation, not sprawl",
    },
    DeclaredHub {
        module: "commit_boundary::active",
        reason: "the active-session/ref-metadata layer every commit-boundary operation touches on \
                  both sides -- readers asking whether an active ref exists and is valid, writers \
                  preparing or clearing it. Newly crossing the threshold as a trend already flagged \
                  by the coupling-gate-graph-contradiction round, driven by the same cyclic \
                  relationships (with refs, worktree_patch, patch_replay) declared above, not by \
                  unrelated scope creep",
    },
    DeclaredHub {
        module: "wal",
        reason: "the active write-ahead-log container every active-session operation (commit, \
                  seal, doctor, unlock, compact) reads or appends through -- a single shared queue \
                  necessarily has wide fan-in from everything that queues work and wide fan-out to \
                  everything that shapes a queued record",
    },
    DeclaredHub {
        module: "author::author_key_index",
        reason: "the durable store for recorded AUTHOR public-key material (DC-53): every path \
                  that verifies or attributes a Patch's authorship reads it -- bundle export, \
                  patch-exchange accept and artifact verification, rollback-draft construction, \
                  `verify`'s own object-level check, and worktree_patch's node authoring, six \
                  distinct real consumers, not one caller reached six ways. Its own fan-out is \
                  entirely into this crate's shared durable-file toolkit (`foundation::byte_cursor`, \
                  `foundation::file_codec`, `foundation::frame_resync`, `foundation::fsutil`, \
                  `foundation::layout`, plus `lock`) -- the same primitives every other durable \
                  on-disk container in this crate already depends on to persist an index, not \
                  reach that crept outward on its own",
    },
];

/// One module in the emitted graph.
#[derive(Debug, serde::Serialize)]
pub struct GraphNode {
    pub module: String,
    pub fan_in: usize,
    pub fan_out: usize,
    /// `min(fan_in, fan_out)` — the value `HUB_THRESHOLD` is compared against, emitted so a reader
    /// never has to recompute the gate's own arithmetic and risk getting it wrong.
    pub hub_score: usize,
    pub is_hub: bool,
    pub is_declared_hub: bool,
}

/// `boundary-check --graph`: the coupling graph the gate already builds, emitted.
///
/// RFC 131 §6e step 0. The gate printed only `valid` and `errors`, so every census of this graph had
/// to be assembled by reading imports by hand — which is exactly how a census and the gate come to
/// disagree. **This adds no check and changes no verdict**; it emits what `check` below already
/// computes, from the same `graph::build` and the same `HUB_THRESHOLD`, so the two cannot diverge.
#[derive(Debug, serde::Serialize)]
pub struct GraphReport {
    pub schema_version: &'static str,
    pub hub_threshold: usize,
    pub modules: Vec<GraphNode>,
    pub edges: Vec<[String; 2]>,
    /// Every module the gate would call a hub, declared or not — the list a census must match.
    pub hubs: Vec<String>,
    /// Subtree-cycle edges, as `check` detects them.
    pub subtree_cycles: Vec<[String; 2]>,
}

/// Build the emission. Same source root, same builder, same threshold as [`check`].
pub(super) fn graph_report(root: &Path) -> Result<GraphReport, String> {
    let src_root = root.join("crates/prikk-store/src");
    let graph = graph::build(&src_root)?;
    let declared: BTreeSet<&str> = DECLARED_HUBS.iter().map(|entry| entry.module).collect();

    let mut modules = Vec::with_capacity(graph.modules.len());
    let mut hubs = Vec::new();
    for module in &graph.modules {
        let fan_in = graph.fan_in(module);
        let fan_out = graph.fan_out(module);
        let hub_score = fan_in.min(fan_out);
        let is_hub = hub_score >= HUB_THRESHOLD;
        if is_hub {
            hubs.push(module.clone());
        }
        modules.push(GraphNode {
            module: module.clone(),
            fan_in,
            fan_out,
            hub_score,
            is_hub,
            is_declared_hub: declared.contains(module.as_str()),
        });
    }

    Ok(GraphReport {
        schema_version: "release-policy-module-graph-v1",
        hub_threshold: HUB_THRESHOLD,
        modules,
        edges: graph
            .edges
            .iter()
            .map(|(from, to)| [from.clone(), to.clone()])
            .collect(),
        hubs,
        subtree_cycles: graph
            .subtree_cycles()
            .into_iter()
            .map(|(from, to)| [from, to])
            .collect(),
    })
}

pub(super) fn check(root: &Path, errors: &mut Vec<BoundaryError>) {
    check_allowlists_are_well_formed(errors);
    let src_root = root.join("crates/prikk-store/src");
    let graph = match graph::build(&src_root) {
        Ok(graph) => graph,
        Err(message) => {
            push(
                errors,
                "module-coupling",
                format!("graph build failed: {message}"),
            );
            return;
        }
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
    // RFC 131 §6c.4: cycles are computed over subtrees, not raw nodes -- `subtree_cycles` already
    // reports each mutually-dependent pair at its own smallest exhibiting nodes (rule 3), as both
    // directed edges, so no separate "find an example cycle" step is needed the way the raw-SCC
    // check above this once required: the pair itself is the whole explanation. Computed once,
    // shared with `check_declared_entries_still_exist` below rather than recomputed.
    let subtree_cycle_edges: BTreeSet<(String, String)> =
        graph.subtree_cycles().into_iter().collect();
    for (from, to) in &subtree_cycle_edges {
        if !declared_edges.contains(&(from.clone(), to.clone())) {
            push(
                errors,
                "module-coupling",
                format!(
                    "undeclared subtree-cycle edge: {from} -> {to} -- add a DECLARED_CYCLES entry \
                     with a reason and a statement of what would remove it, or this is real \
                     accidental coupling to fix instead"
                ),
            );
        }
    }

    let declared_hub_names: BTreeSet<&str> =
        DECLARED_HUBS.iter().map(|entry| entry.module).collect();
    for module in &graph.modules {
        let fan_in = graph.fan_in(module);
        let fan_out = graph.fan_out(module);
        if fan_in.min(fan_out) >= HUB_THRESHOLD && !declared_hub_names.contains(module.as_str()) {
            push(
                errors,
                "module-coupling",
                format!(
                    "undeclared hub: {module} (fan-in {fan_in}, fan-out {fan_out}) -- add a \
                     DECLARED_HUBS entry saying why this is consolidation and not sprawl"
                ),
            );
        }
    }

    check_declared_entries_still_exist(&graph, &declared_edges, &subtree_cycle_edges, errors);
}

/// Reverse binding (review v1 §5, required follow-up): the allowlist is a ledger of structural
/// debt (§4b.3), and a ledger with uncollectable entries stops being read (RFC 120 §6 Q3's own
/// argument, applied here). A declared edge or hub that no longer exists in the graph must fail
/// the gate by name, the same way `every_declared_document_exists` binds both directions rather
/// than only forward. A standalone function (not inlined into [`check`]) so
/// `coupling::tests::stale_declared_entries_are_rejected` can exercise it directly against a
/// synthetic graph, without needing a real edge or hub to first go stale in `prikk-store` itself.
fn check_declared_entries_still_exist(
    graph: &graph::ModuleGraph,
    declared_edges: &BTreeSet<(String, String)>,
    subtree_cycle_edges: &BTreeSet<(String, String)>,
    errors: &mut Vec<BoundaryError>,
) {
    // RFC 131 §6c.4: "still exists" means "still a subtree cycle", not "still a raw edge" -- a
    // declared pair like `refs -> active` was never a raw edge even before this amendment (the
    // raw writer is `refs::evidence`), so checking raw `graph.edges` here would flag it stale for
    // a reason that has nothing to do with whether the coupling it names is still real.
    for (from, to) in declared_edges {
        if !subtree_cycle_edges.contains(&(from.clone(), to.clone())) {
            push(
                errors,
                "module-coupling",
                format!(
                    "stale DECLARED_CYCLES entry: {from} -> {to} no longer exists in the graph -- \
                     remove the entry (or the edges it groups with, if some still exist)"
                ),
            );
        }
    }
    for entry in DECLARED_HUBS {
        let fan_in = graph.fan_in(entry.module);
        let fan_out = graph.fan_out(entry.module);
        if !graph.modules.contains(entry.module) {
            push(
                errors,
                "module-coupling",
                format!(
                    "stale DECLARED_HUBS entry: `{}` is not a real module",
                    entry.module
                ),
            );
        } else if fan_in.min(fan_out) < HUB_THRESHOLD {
            push(
                errors,
                "module-coupling",
                format!(
                    "stale DECLARED_HUBS entry: `{}` no longer meets the hub threshold (fan-in \
                     {fan_in}, fan-out {fan_out}) -- remove the entry",
                    entry.module
                ),
            );
        }
    }
}

/// Self-guard on both allowlists, the same idiom `DECLARED_UNDOCUMENTED`'s own tests use
/// (`crates/prikk-cli/src/commands/tests.rs`): an entry with an empty or placeholder reason
/// documents nothing, and for a cycle specifically, one that cannot say what would remove it is an
/// entry nobody understands (§4b.3) rather than a real evaluation.
fn check_allowlists_are_well_formed(errors: &mut Vec<BoundaryError>) {
    for entry in DECLARED_CYCLES {
        if entry.edges.is_empty() {
            push(
                errors,
                "module-coupling",
                "DECLARED_CYCLES entry has no edges".to_owned(),
            );
        }
        if is_placeholder(entry.reason) {
            push(
                errors,
                "module-coupling",
                format!("DECLARED_CYCLES entry {:?} has no real reason", entry.edges),
            );
        }
        if is_placeholder(entry.what_would_remove_it) {
            push(
                errors,
                "module-coupling",
                format!(
                    "DECLARED_CYCLES entry {:?} does not state what would remove it",
                    entry.edges
                ),
            );
        }
    }
    for entry in DECLARED_HUBS {
        if is_placeholder(entry.reason) {
            push(
                errors,
                "module-coupling",
                format!("DECLARED_HUBS entry `{}` has no real reason", entry.module),
            );
        }
    }
}

fn is_placeholder(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed.len() < 20
}

#[cfg(test)]
#[path = "coupling/tests.rs"]
mod tests;
