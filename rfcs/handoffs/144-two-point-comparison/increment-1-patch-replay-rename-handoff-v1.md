# RFC 144 increment 1 — teach `patch_replay` to apply `RenamePath`

**RFC:** `rfcs/accepted/144-two-point-comparison.md` — **accepted 2026-09-09** after three rounds of
external review. **§4h.7 rules this increment.**
**Base:** `main` at the tip carrying the acceptance. **Check `git log`.**

**This is a gate, not a feature.** Nothing user-visible changes. **Everything else in RFC 144 that
touches renames is blocked until it lands**, and landing it wrongly is the one way to seal history
that cannot be read back.

---

## 1. Why this exists

**Two replay paths disagree about `RenamePath` today:**

- **`lifecycle_cache/replay/effect.rs:67` applies it correctly** — `rename_node_checked` looks the node
  up by `node_id`, validates the stated old path, moves the path, keeps the id. **This is what `show`
  and lifecycle state use.**
- **`patch_replay/decode.rs:145` refuses it** —
  `unsupported_operation("RenamePath (node-addressed apply pending a rename authoring path)")`.
  **This is the path behind `checkout --patch-plan`, `--patch-materialize`, and RFC 143's
  content-at-a-point.**

**So the moment `commit` authors a rename, every checkout and every content read over that history
fails.** The refusal is loud rather than silent, so nothing would be quietly wrong — but the feature is
unusable until this is fixed, and **history sealed before the fix would be history `checkout` cannot
materialize.**

## 2. RULED — resolve node before path, do not apply renames in sequence

**The external review's Q11 answer, adopted.** There is a materialization analogue of the merge
ordering hazard, and both cases dissolve under the same change:

- **A rename cycle** (`a→b`, `b→a` — a swap). Applied *sequentially by path*, the first rename
  collides or clobbers and the second is wrong. **Applied by node — resolve the final `node → path`
  projection and write that — a swap is two nodes exchanging a path attribute**, with no sequence to
  get wrong.
- **A rename onto an occupied path.** Two live nodes cannot share a path; **resolving by node makes the
  collision a structural, detectable event** (the `path_to_id` secondary index collides) rather than a
  silent overwrite.

**So the instruction is not "apply renames in the right order." It is: teach `patch_replay` what
`lifecycle_cache` already knows — resolve node before path, and materialize the final projection.**

**There is a reference implementation in this repository.** `lifecycle_cache` does it correctly today.
**Read it before writing anything**, and say in your report where you followed it and where you had to
diverge, because the two paths work on different things — one on state, one on files.

## 3. Scope

**In:** `patch_replay`'s apply path for `RenamePath`, and whatever `ensure_apply_supported` must become.

**Out, explicitly:**

- **Any authoring change.** `commit` still must not emit a `RenamePath`. **That is increment 3 and it
  is gated on this one.**
- **`DeleteNode(symlink)` and `CreateSymlink`**, which `ensure_apply_supported` also refuses. **They
  are refused for a different reason — no authoring path — and are not this increment's business.**
  Do not sweep them in because you are already in the file.
- **The thirteenth conflict witness.** Increment 3.

## 4. Controls

Each seen to fail before it passes.

1. **A rename replays through `patch_replay` and materializes correctly.** Construct history containing
   a `RenamePath` — `commit` will not author one, so this needs the raw-patch-then-seal technique the
   RFC 142/143 rounds established.
2. **A rename cycle (`a→b`, `b→a`) materializes correctly.** **This is the control that would fail
   under a sequential implementation**, so build it first and watch it fail before the fix.
3. **A rename onto a path occupied by another live node is detected, not silently applied.** State what
   it produces — an error is acceptable here; a silent overwrite is not.
4. **`checkout --patch-materialize` and `--patch-plan --format json --content-path` both work over
   history containing a rename** — the two surfaces this unblocks. **Content at a point must report the
   node's content at its *new* path.**
5. **Byte-identity holds** — content from `--patch-plan --format json` matches what
   `--patch-materialize` writes, over rename-containing history. **RFC 143's own control 1, extended to
   the case that broke it.**
6. **Nothing regressed** on rename-free history: the existing replay tests pass unchanged.

## 5. Gates

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

**`boundary-check` is worth watching** — this touches `patch_replay`, the coupling graph's top hub.

## 6. `CHANGELOG.md`

**None.** Nothing user-visible changes: no surface gains a capability, and no history can yet contain a
rename. **Ruled here rather than left unsaid.**

## 7. Reporting

`.git-exclude/review-request/`. Include:

- **control 2's before/after** — the cycle case failing under a sequential approach, then passing;
- **where you followed `lifecycle_cache` and where you had to diverge**, and why;
- **what control 3 produces** on a collision, and whether you think that is the right answer;
- **anything that suggests the node-before-path approach does not fit the materialization path.** You
  will be the first to hold both implementations side by side. **The external review reasoned this from
  our model rather than our code — if the code disagrees, say so.**
