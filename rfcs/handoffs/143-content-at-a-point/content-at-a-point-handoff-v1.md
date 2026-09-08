# RFC 143 — content at a point: expose the bytes the plan already computes

**RFC:** `rfcs/accepted/143-content-at-a-point.md` — **accepted 2026-09-08**, the day it was opened.
§5's surface, §6's coverage requirement and §7's refusals are settled input. **The schema name and
field shape are yours** (§7 lists them open).
**Base:** `main` at the tip carrying the acceptance. **Check `git log`.**
**Origin:** the stikk project asked for this (`005-content-at-a-point.md`). **They are shipping their
narrowed view regardless and have said plainly this is not a blocker.** Build it well, not fast.

---

## 1. What this is, in one line

**`checkout --patch-plan [path] --format json` returns the replayed content at that point.** Nothing
else changes.

## 2. The finding this rests on — verify it before you build on it

`crates/prikk-store/src/patch_replay.rs`:

```rust
pub(crate) struct ReplayManifestEntry {
    pub(crate) path: RepoPath,
    pub(crate) bytes: Vec<u8>,
    pub(crate) mode: u32,
}
fn total_content_bytes(&self) -> u64 {
    self.files.iter().map(|entry| entry.bytes.len() as u64).sum()
}
```

**`prepare_patch_replay_plan` materializes every file's bytes, sums their lengths, and drops the
content.** This round returns it instead of dropping it. **Read those two definitions yourself before
starting** — the whole RFC rests on them and an architect's summary is not a substitute.

## 3. RULED — the surface

- **A format on the existing read, not a new command.** `prepare_patch_replay_plan` is already
  read-only; `patch_replay.rs:158-161` records why and asks a future *writing* caller to re-check that.
  **If your change makes a writing caller reach it, stop and report** rather than deciding it is still
  fine.
- **Path-scoped. No whole-tree default.** `checkout --patch-plan` already accepts `[path]`; a document
  carrying every file's content at a point is unbounded. **The requesting view knows which paths
  differ before it asks.** Decide and state what a bare `--format json` with no path does — reporting
  metadata without content is the obvious answer, but it is yours to choose and justify.
- **Binary is never rendered.** Id and declared size only, exactly as `show` does (RFC 142 §7).
  **One project, one answer.**
- **`ReplayManifestEntry` and `ReplayManifest` are `pub(crate)`.** Exposing content means a boundary
  decision. **Widen the minimum** — RFC 131 applies, and the `show` round's own restraint
  (`lifecycle_state_at`, one narrow function, nothing else widened) is the standard to match.

## 4. RULED — §6, and it is the part that outlives the feature

**The replay covers a subset and the document must say so in a field a machine can branch on.**

`ensure_apply_supported` (`patch_replay/decode.rs:128`) admits `CreateFile`, `DeleteNode(file)`,
`EditText`, `ReplaceBinary`, `ChangePerm` and **errors** on `RenamePath`, `CreateSymlink`,
`DeleteNode(symlink)`. `single_parent_chain` walks one parent, so **merge blocks are outside it**.

**A consumer must be able to tell "complete state" from "what the supported subset reconstructs"
without parsing prose.** The requesting project intends to name our gaps in their own UI — **they can
only do that if we hand them the fact as data.**

**This is RFC 140 §7b's third instance after RFC 142 §6b.** Read §6b before you design the field: the
rule there is **degrade absence, propagate error**, and the mistake it corrects was folding an error
into a degraded state. **The unsupported-operation errors here must stay errors.**

## 5. Cost

**Measure it and file it into RFC 133**, as RFC 139 and RFC 142 both did. This path shares
`replay_supported_patch_chain` with the cost RFC 133 §5c and §5d already record — **say whether your
numbers agree with those or diverge, and if they diverge, say why before assuming either is wrong.**

## 6. Controls

Each seen to fail before it passes.

1. **The content returned is byte-identical to what `--patch-materialize` writes** for the same ref and
   path. **This is the control that matters most**: two paths through the same replay must not
   disagree, and if they do, that is a finding far bigger than this feature.
2. **`--patch-plan` still writes nothing.** Repository and worktree byte-identical before and after,
   including with `--format json`. Assert it, do not reason it.
3. **An unsupported operation still errors** and does not appear as a degraded field. Construct a
   repository reaching `RenamePath` or `CreateSymlink` — the RFC says they are unauthored today, so
   this needs the raw-patch-then-seal technique the `show` rounds established.
4. **The coverage field is branchable** — a consumer can read it without string-matching prose, and it
   distinguishes the single-parent walk from a complete one.
5. **Binary content is never emitted as bytes.**
6. **Exit codes hold** (RFC 121): `2` for a malformed path or argument, `1` for an operational failure,
   `0` when the plan resolves.

## 7. Documentation and CHANGELOG

- **`docs/src/reference/commands.md`** gains the flag.
- **A guide page or an added section** — the `show` round found a bare line in `commands.md` does not
  satisfy the documentation gate's own scanner. **State what the coverage field means and that it is
  not a completeness promise.**
- **`CHANGELOG.md`**: a new `## Unreleased` entry. **Say what it is not** — there is still no
  comparison surface — the way the `show` entry did, and mind rule (A)'s scanner, which matches
  `` `prikk <word>` `` spans against the real command registry even inside a negation.

## 8. Gates

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

## 9. Reporting

`.git-exclude/review-request/`. Include:

- **the schema name and field shape you chose**, and the reasoning — §7 left these to you deliberately;
- **the boundary you widened**, and what you refused to widen;
- **control 1's result** — the byte-identity check against `--patch-materialize`;
- **the cost numbers**, and how they sit against RFC 133 §5c/§5d;
- **what a bare `--format json` with no path does**, and why;
- **anything that makes you think exposing this is a mistake.** You will be the first to see the
  document a consumer actually gets. **The RFC's own §9 names the risk that this raises pressure to
  complete the replay — if building it sharpens that, say so.**
