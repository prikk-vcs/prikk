//! The store's two layers, as the list of top-level modules on each side (RFC 149 path B). Moved out of
//! `coupling.rs` in 0.57.0 step 2 (L1), when that file reached its 1200-line limit: the lists are data
//! with one rule over them (`coupling::check_layer`), and a new top-level module is a one-line edit here.

/// RFC 149 path B: the store's **upper layer** -- the operations built on it.
///
/// The owner ruled (2026-09-13) against cutting these into their own crate: the census showed the
/// layering is real as a *direction* -- nothing below reaches them -- but not narrow as an
/// *interface*, since they need roughly half the store's internals. A direction is enforced by a
/// rule, so this is one: **no production edge from a [`LOWER_LAYER`] module into one of these.**
///
/// These are the twenty families RFC 149's move order would have moved, plus `branch_switch` (RFC 151
/// increment 2), which is built on `patch_replay`, `worktree_status` and `refs` and reached by nothing
/// below them.
pub(super) const UPPER_LAYER: [&str; 33] = [
    // RFC 155 implementation Part E: `prikk archive export`, built on `object_store`, `refs`,
    // `received`, `trust_index`, `lock` and `foundation` (all lower). Reached only by the CLI;
    // nothing lower reaches it, and it reaches nothing that reaches back into it.
    "archive",
    "branch_switch",
    "bundle",
    // 0.56.0 step 3e: the claim chain walk (D1-D4) -- walks held `RecognitionClaim`s back to a
    // reachable Block, used by `extract` (this round) and, from 0.56.0 step 3f, `show`. Depends on
    // `recognition_claim` (upper, for the signature classification its own doc explains it cannot
    // import from `patch_exchange` to avoid), `refs`, `received`, `merge::evidence` and
    // `object_store` (all lower); reached only by `extract` and `show`, both upper.
    "claim_chain",
    "compact",
    // RFC 153: `diff`, comparing two resolved points through the anchored replay. Built on `patch_replay`,
    // `point_reading` and `line_diff`; reached only by the CLI.
    "diff",
    "doctor",
    // 0.54.0 step 3 Part 2: `prikk extract`, writing a resolved point's files into a new directory
    // that does not exist yet, through `patch_replay::extract`'s own streamed replay (lower). Built on `patch_replay`,
    // `point`, `anchor_trust` and `path` (all lower); reached only by the CLI.
    "extract",
    // RFC 156 §5b: runs `verify` and takes `ActiveLock` before changing the format marker.
    "format_upgrade",
    "history",
    "merge",
    "patch_algebra",
    "patch_checkout",
    "patch_exchange",
    "patch_inverse",
    "patch_set_digest",
    // RFC 157: `tree` and `cat`, reading a resolved point through the anchored replay. Built on
    // `patch_replay` and `point`; reached only by the CLI.
    "point_reading",
    // RFC 165 R5: `prikk doctor --rebuild-pointer-index` -- built on `refs`, `trust`, `lock` and
    // `foundation` (all lower), plus `ref_completion`'s own `evaluate_known_lead` (upper), the same
    // R4 rule a completable lead must be evaluated against. Reached only by the CLI; nothing lower
    // reaches it, and it reaches nothing that reaches back into it.
    "pointer_rebuild",
    "received",
    "recognition_claim",
    "way_out", // 0.57.0 step 1e: doctor and verify print each finding's way out from here
    // RFC 165 R4: `prikk ref complete`'s own rule (`plan_ref_completion`/`complete_ref_publication`)
    // -- built on `refs`, `trust`, `wal`, `object_store`, `maintainer_signing` and
    // `commit_boundary::active`, the same "operation over the lower layer" shape as
    // `seal_from_accepted`. Reached by `verify` (completable-vs-divergent reclassification, "the
    // same table" R4's own handoff requires) and by the CLI; nothing lower reaches it.
    "ref_completion",
    // RFC 132 refusal sweep: the one resolver for a consumed ref reads both `refs` and `received`.
    "ref_resolution",
    "rollback",
    "seal_from_accepted",
    "show",
    "sync_negotiation",
    "tag_travel",
    // 0.57.0 step 3, S5b: `prikk uncommit`, taking back the newest queued patch. Built on `worktree_status` (upper, for what
    // the next commit does about each operation), `wal`, `recovery_log`, `rename_declaration`, `worktree_baseline` and
    // `commit_boundary` (all lower); reached only by the CLI.
    "uncommit",
    "unlock",
    "verify",
    // 0.56.0 step W3b: the shared "clean against the record, mark dirty, materialize through the
    // verified path" sequence R3's ruling gives (`branch_switch`'s own ordinary materialization,
    // and every producer that moves the current branch). Depends on `worktree_status` (upper, for
    // its own clean-check report), `worktree_baseline`, `worktree_marker` and `patch_replay` (all
    // lower); reached only by `branch_switch` and `merge`, both upper.
    "worktree_materialize",
    "worktree_status",
];

/// RFC 149 path B: everything else at the top level, **listed rather than derived as "not upper"**.
///
/// A new top-level module appears on neither list and fails the gate until someone decides which
/// side it belongs on. That decision is the growth-direction control: without it, a new module lands
/// on whichever side a default puts it and the rule silently stops describing the store.
pub(super) const LOWER_LAYER: [&str; 41] = [
    // The core: the five modules of the declared cycles.
    "commit_boundary",
    "lifecycle_cache",
    "patch_replay",
    "refs",
    "trust",
    // Infrastructure the core reaches.
    // 0.57.0 U4: what an artifact's declared repository format means to a receiver. A leaf over `foundation::layout` and
    // `prikk-error`; `archive` and `bundle` (both upper) are its only callers, and it reaches nothing above it.
    "artifact_format",
    "author",
    "blob_access",
    "commit_index",
    "format",
    "foundation",
    "ignore",
    // RFC 153: the shortest edit script over lines and its unified hunks. Pure text in, text out; it
    // depends on no store module, and `diff` (upper) is its only caller.
    "line_diff",
    "lock",
    "lock_holder", // 0.57.0 step 2 (L1): the holder record `lock` writes and `unlock` reads; a leaf over std, rustix and prikk-ffi
    // RFC 168 §3.1: the recovery log. Its own dependencies are the foundation and the locks; the repairs (`wal`, `refs`,
    // `doctor`) and the object index reach it, and it reaches nothing above them.
    "recovery_log",
    "maintainer_signing",
    "node",
    "object_store",
    "path",
    "rename_declaration",
    "signature_diagnostics",
    "snapshot",
    "text_span",
    "trust_index",
    "wal",
    "worktree_marker",
    // 0.56.0 step W2: the recorded worktree baseline (R1/R2), read and written by `commit`
    // (`commit_boundary::worktree_patch`) itself, not only by the CLI -- so, unlike
    // `worktree_status`/`branch_switch` above, it sits here: a lower module depends on it.
    "worktree_baseline",
    // RFC 136 increment 2c: a snapshot anchor that could not be used, and the per-thread list the CLI drains to
    // name it on stderr. Depends on no store module (only `prikk_object`); `lifecycle_cache::anchored_text` and
    // `patch_replay::anchor` (both lower) record and construct it, and `lib.rs` exports the drain.
    "anchor_fallback",
    // RFC 159 §8.2: the one anchor-trust function -- record, adopted maintainer signature, manifest, distance -- that
    // stack A (`block_state`) and RFC 136 2b's worktree writes (`patch_replay`, `worktree`) ask before a snapshot's
    // state may reach a signature. Depends on `trust`, `snapshot` and `verified_blocks`; every caller is lower, and
    // `merge`/`seal_from_accepted` (upper) only name its `AnchorSite`.
    "anchor_trust",
    // RFC 136 increment 2b: the replay-verified block record, a rebuildable cache under `.prikk/cache/`
    // that `block_state` (seal) and `verify` write and the anchored worktree writes read. It depends
    // only on `foundation`, so it sits with the lower-layer state it records.
    "verified_blocks",
    // RFC 147 §2f: what `commit` will do with each live rename declaration, shared with
    // `worktree_status` so a report cannot contradict the commit it describes. It depends only on
    // `foundation`, `path`, `blob_access` and `rename_declaration`, and both callers sit above it.
    "declaration_resolution",
    // 0.54.0 step 3 Part 5d: `extract`'s own refusal classification (`ExtractError`,
    // `ExtractRefusalKind`, `NoCheckpointReason`), carried out of `PrikkError`'s message text so the
    // CLI's `refusal-v1` document does not have to recover it from words. Depends only on
    // `prikk_error`. Sits here, not in `extract` (upper), because `patch_replay::extract` (lower)
    // must construct it too (the pre-walk's own `no-checkpoint-in-reach` classification happens
    // inside the replay itself, not only at `extract`'s own upper-layer call site) -- the identical
    // reason `anchor_trust` sits here rather than beside `merge`/`seal_from_accepted`, which only
    // name its `AnchorSite`.
    "extract_refusal",
    // RFC 156 §4: which carried signatures may join a stored object. Depends only on `author` and
    // `trust`; its callers, `bundle` and `patch_exchange`, sit above it.
    "signature_admission",
    // RFC 153 §7.1: a point's shape -- a ref or a bare block id -- and the checks its spelling alone
    // decides. Depends on no store module; `checkout`, `patch_replay` and `worktree` take a resolved
    // point, and `ref_resolution` (upper) resolves one.
    "point",
    // Surfaces by census, store by nature (RFC 149 §6b, §6d). Each is reached from lower-layer code
    // or lower-layer tests -- `worktree.rs` names `checkout` in production -- so putting any of them
    // upper would create exactly the crossing this rule forbids.
    "block_state",
    "checkout",
    "memory_store",
    "rfc111_seal_simulation",
    "state_root",
    "worktree",
];
