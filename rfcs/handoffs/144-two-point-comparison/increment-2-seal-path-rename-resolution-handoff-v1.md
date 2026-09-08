# RFC 144 increment 2 — the same rename resolution on the seal path

**RFC:** `rfcs/accepted/144-two-point-comparison.md` — **§4j rules this increment**, and §4j.1 makes it
a prerequisite to rename authoring rather than a follow-up.
**Base:** `main` at the tip carrying §4j. **Check `git log`.**
**Prior round:** `.git-exclude/reviewed/rfc144-increment1-review-v1.md`. **Increment 1 is accepted and
nothing in it is being undone** — this increment does for the write path what it did for the read path.

**The deliverable is convergence, not a fix.** Two paths currently disagree about which rename runs are
valid, in *both directions*. When you are done they must agree exactly.

---

## 1. What is wrong

**Increment 1 taught `patch_replay` to materialize a rename cycle. The write path still cannot produce
one.**

`apply_state_effect` applies operations one at a time, and `rename_node`
(`prikk-replay/src/node_lifecycle/mutation.rs:93-101`) rejects a target occupied by another live node.
**So a two-node swap fails**, and because `derive_next_state_root_for_candidate` folds the same way, **a
rename cycle cannot be sealed into history at all.**

**And increment 1 introduced the opposite divergence.** Its Phase 0 validates every `old_path` against
the **pre-run** state, so a *chained* rename — `A→B` then `B→C` for one node in one patch — is
rejected, while the sequential fold accepts it. **Today that shape would seal successfully and then
fail to check out.**

## 2. The fact that makes this safe, and you should confirm it before relying on it

**No sealed history anywhere contains a `RenamePath`.** `node_authoring.rs` never constructs one — its
only mention (`:146`) is a canonical *sort-order* arm, not a constructor — and `commit` authors moves
as delete+create.

**Therefore this change cannot alter any existing derived state root.** §4j.1's constraint — *every
patch set that derives a root today must still derive the same root* — is satisfied trivially for real
history, and the only things affected are raw-patch test fixtures.

**Confirm it yourself before leaning on it.** If you find a path that can author a `RenamePath` today,
stop and report: the whole risk profile of this increment changes.

## 3. RULED — one shared resolution, not three

**There are three fold sites, all in `lifecycle_cache/replay.rs`:**

- `apply_queued_patch_envelopes` (~:438) — unsealed WAL records
- `replay_chain_with_appended_patches` (~:467)
- `apply_patch_ids` (~:542) — patches read by id, used by `apply_one_block`

**Implement the resolution once and have all three use it.** Three copies of a rule is three chances to
drift, and this project's own precedent is that a single shared implementation makes correctness *a
property rather than a decision*.

**`lifecycle_cache`'s job is simpler than `patch_replay`'s was.** It is node-primary — `live_by_id`
plus a `path_to_id` secondary index — so resolving a swap means **clearing every source's `path_to_id`
entry before inserting any destination.** No bytes move. Increment 1 had to work around a path-keyed
`files` map; you do not.

## 4. RULED — align to `patch_replay`, not the reverse

**§4j.2: the chained rename is the malformed one.** A patch describes one set of changes; renaming a
node twice within it means the intermediate path existed in no sealed state, and its only honest
meaning is the net move.

**So this increment makes the seal path reject what `patch_replay` rejects** — validate every
operation's `old_path` against the pre-run state. **That is a success→error flip**, and it is
admissible only because §2 shows it is unreachable in authored history.

**Match increment 1's run scoping too:** maximal runs of consecutive `RenamePath` operations within one
patch, not across patch boundaries and not across interleaved non-renames. **Read
`patch_replay/apply.rs::apply_rename_batch` first** — this increment's validation must agree with it
operation for operation, and the cheapest way to guarantee that is to follow it deliberately rather
than re-derive it.

## 5. Controls

Each seen to fail before it passes.

1. **A rename cycle seals.** Build a two-node swap through `derive_next_state_root` and show it failing
   with `Integrity("rename target path … is occupied by another live node")` before the fix, then
   deriving a root after it. **This is the control the increment exists for.**
2. **CONVERGENCE — the two paths agree, and this is the real deliverable.** For each of: a plain rename,
   a two-node swap, a chained rename, a genuine collision, and a rename-free patch set — **assert that
   `derive_next_state_root` and `patch_replay` reach the same verdict** (both accept, or both reject).
   **A table of five shapes × two paths, with no disagreement anywhere.**
3. **No existing state root changes.** Re-derive roots across the repository's own sealed history and
   show them identical to before. **§2 predicts this trivially; prove it rather than citing it.**
4. **`verify` still passes** over existing history — it re-derives and compares, so it is the
   independent check on control 3.
5. **A genuine collision still fails** at seal time, and for the same reason it did before: there is no
   valid post-state to derive a root from.
6. **Nothing regressed** on rename-free history — the existing suite passes unchanged.

## 6. Explicitly NOT in this round

- **Any authoring change.** `commit` still must not emit a `RenamePath`. That is the next increment and
  it is gated on this one.
- **The thirteenth conflict witness.** Also the next increment.
- **`patch_replay`.** Increment 1 is accepted; do not adjust it to meet this path. **If you find that
  convergence requires changing `patch_replay` rather than the seal path, stop and report** — that
  would mean §4j.2's ruling is wrong, which is mine to correct, not yours to work around.

## 7. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

**This touches `lifecycle_cache`, part of the six-module SCC RFC 130 declares.** Expect
`boundary-check` to have an opinion if you add a cross-module reference; if it does, that is the gate
working.

## 8. `CHANGELOG.md`

**None.** Nothing user-visible changes — no history can yet contain a rename. **Ruled here rather than
left unsaid.**

## 9. Reporting

`.git-exclude/review-request/`. Include:

- **control 2's five-by-two convergence table.** This is the report.
- **control 1's before/after**, the cycle failing then sealing.
- **control 3's evidence** that no existing root moved, and control 4's `verify` result.
- **whether the shared resolution genuinely served all three fold sites**, or whether one needed
  special handling — and if so, why.
- **anything suggesting §4j.2's ruling is wrong.** You will be the second person to hold both paths'
  validation side by side. **Increment 1's round corrected my premise by testing it; if this one is
  also wrong, the same courtesy applies.**
