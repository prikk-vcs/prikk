# One point resolver: a ref or a bare block id — handoff v1

**Order of work for 0.46.0 (the three handoffs of 2026-09-17):**
1. `153-content-diff/point-resolver-handoff-v1.md` (this one);
2. `157-reading-a-point/tree-and-cat-handoff-v1.md`;
3. `153-content-diff/diff-handoff-v1.md`.

Take them one at a time, each reported and reviewed before the next.

**Live 2026-09-17.** RFC 153 (`rfcs/accepted/153-content-diff.md`, §2 and §7.1) and RFC 157
(`rfcs/accepted/157-reading-a-point.md`, §2) both name a point as a ref or a bare block id, through **one** function.
This round builds that function and wires it into `checkout`, which closes RFC 144 §4t, stikk's named dependency.
Nothing of `diff`, `tree` or `cat` is built here.

## 1. What exists (checked at source 2026-09-17)

- `prikk_store::require_existing_ref(layout, name, ReceivedRefs)` (`crates/prikk-store/src/ref_resolution.rs:49`):
  0.45.0's shared absent-ref and received-ref check, with modes `Read`, `Refused` and `LeftToNameValidation`.
- `replay_supported_patch_chain(layout, ref_name)` (`patch_replay.rs:392`) replays **by ref**. The block-addressed walk,
  `single_parent_chain(object_store, target)` (`patch_replay/read.rs:23`), is crate-internal.
- **Measured on the 0.45.0 asset:** `checkout --patch-plan --ref <block-id>` answers `integrity error: ref <id> is not
  published`, exit 1. That is the pre-sweep wording, because `require_existing_ref` leaves a name that is neither a
  branch, a tag nor a received ref to its caller. `--ref 00` answers the same. `--ref tags/<name>` resolves to its
  block and works. RFC 144 §4t records stikk's need: two blocks **on one ref** cannot be compared today.
- Replay is anchored at checkpoints (RFC 136).

## 2. Ruling

1. **One resolver.** It extends `require_existing_ref` into a function that returns a **point**:
   - how the point was named (the string given);
   - its kind: local ref, tag, received ref, or block;
   - the resolved block id.

   Every point-taking read uses it, and `require_existing_ref` itself becomes a call into it. **Do not leave two
   functions deciding existence.**
2. **A bare block id** is 64 lowercase hex, and cannot collide with a ref name (those start `heads/`, `tags/` or
   `remotes/`). How each case refuses or resolves:

   | Input | Result |
   |---|---|
   | malformed id | usage, exit 2 |
   | well-formed id the store does not hold | `Precondition`: "block <id> is not in this repository" |
   | id the store holds as another type | `Precondition` naming the type, e.g. "object <id> is a patch, not a block" |
   | a block the store holds | resolves, **whether or not any ref reaches it**: received history's blocks and older blocks on a ref included |

   Reading is not adopting.
3. **One replay.** Add a **block-addressed** replay beside the ref-addressed one, and make the ref-addressed entry a
   thin wrapper: resolve, then replay the block. **Both go through the same anchored path** (RFC 136), so a block id at
   depth replays O(tree + at most 63 patches), exactly as its ref would.
4. **`checkout` accepts a block id in its read-only modes only:** `--plan-only`, `--patch-plan` (with and without
   `--content-path`, prose and JSON), `--snapshot-plan` and `--patch-delete-plan`.
   - **Every mode that writes the worktree refuses a bare block id**: `--patch-materialize`, `--snapshot-materialize`,
     `--patch-materialize-delete`. They refuse with `Precondition`, saying that writing the worktree needs a branch
     (the next `commit` authors against one), and **name no route** unless your control runs it.
   - Received refs keep 0.45.0's per-command rule: `checkout` refuses them.
5. **JSON:** `ref` keeps the string as named, now possibly a block id. `target_block_id` is unchanged. **No new
   schema version:** the value space of `ref` widens, and the CHANGELOG says so.
6. **The prose header** names the point as given, e.g. `ref: <id>` becomes `block: <id>` for a bare id. Say which,
   and keep ref output byte-identical.

## 3. Controls (each shown failing under its perturbation, then restored identical)

1. **The §4t case.** One ref edits a path, reverts it, and edits it again. `checkout --patch-plan --format json
   --content-path <p> --ref <each block id>` returns each block's own content.
   *Perturb: resolve every block id to the ref tip.*
2. **Byte identity.** For the tip, `--ref heads/main` and `--ref <tip block id>` give identical `--patch-plan` JSON,
   except the `ref` value, and identical content. *Perturb: replay the block path without anchors.*
3. **Anchored at depth.** A block id at depth ≥ 65 replays from its checkpoint, not from genesis. Count blocks
   replayed through a test-support seam, like the warm-cache probe's. *Perturb: give the block-addressed entry its
   own un-anchored walk (perturb the sharing).*
4. **Refusals.** An unknown block id, a patch id, and a malformed id each get their answer and exit code. A write mode
   given a block id refuses and writes nothing (compare the worktree before and after).
   *Perturb: let a write mode accept a block id.*
5. **Unchanged.** Absent refs, received refs, the implicit fresh branch, and every 0.45.0 control in
   `rfc132_absent_and_received_refs.rs` still pass. So does every path-shaped assertion through
   `support::assert_same_path`. **Read the Windows and macOS CI jobs after the architect pushes.**

## 4. Docs and CHANGELOG

- The `checkout` synopsis in `commands.rs`: `--ref <ref|block-id>`, for the modes that take one.
- `commands.md`, and `guide/checkout` pages where `--ref` is described.
- RFC 144 §4t's row: the architect updates it; the team names the commit.
- CHANGELOG `### Added — a point can be a bare block id in checkout's read-only modes`, including the widened `ref`
  value and the write-mode refusal.

## 5. Report

`.git-exclude/review-request/point-resolver-report-v1.md`. **Stop and report before implementing** if one replay
cannot serve both entries without changing ref-addressed output, or if anchoring is not reachable from a bare block.

## Addendum 1 2026-09-17 — accepted; three follow-ups in this round

**Accepted** (review `point-resolver-review-v1`): `aaa5ed43`, `ae903a51`, `6ac5b570`.

**Readings confirmed:**
- a block id's existence is decided only in `resolve_point`, and `require_existing_ref`'s other callers keep 0.45.0's
  answers;
- a `--ref` value that is neither a ref name nor a bare block id is a usage error, with exit 2, in every checkout mode
  (CHANGELOG'd);
- `ref-state: <none>` for a bare block in `--plan-only`;
- the lower-layer `point` module, and moving `run_checkout` out of `main.rs`;
- P2 cannot fail control 2, by RFC 136's contract; control 3 holds anchoring.

**Follow-ups:**
1. **A damaged ref (report §6.1): option (a), keep the new answers.** A missing pointer with history is damage, and
   `Integrity` is its class (RFC 132). The old `--plan-only` exit 0 reported damage as a clean "not published" plan,
   and the old `--snapshot-plan` named a route that fails. Required:
   - a `### Changed` CHANGELOG line naming `--plan-only`'s exit 0 → 1 and `--snapshot-plan`'s new message for a
     damaged ref;
   - a control pinning all four read-only modes on that damage state (the report's probe, committed).
2. **`merge-evidence` and `merge-plan` move to `resolve_point`** (report §6.2). RFC 153 §7.1 says one resolver, and
   `merge/evidence.rs::resolve_target` is a second one deciding block existence. Route both its `Block` and `Ref`
   arms through `resolve_point`, with `ReceivedRefs::Read`, keeping DC-85's received-ref reading. Required:
   - a block id not held answers "block <id> is not in this repository" (was `integrity error`), and a patch id
     names its type; both in the CHANGELOG under `### Changed`;
   - every existing merge, merge-evidence and merge-plan control passes unchanged;
   - **perturb the sharing:** give `resolve_target` its own block check back, and a control fails.
3. **Control 4 must be able to catch a snapshot write.** The architect removed the refusal from
   `materialize_snapshot_checkout` alone. Control 4 failed only on the answer, because its block id is not a
   checkpoint: the snapshot path then refuses "not a checkpoint" and could never write. Use a **checkpoint** block id
   for `--snapshot-materialize` (the genesis block is one), so an accepted write would change the tree and the tree
   comparison can fail. Show it failing under that same perturbation.

**Report:** `.git-exclude/review-request/point-resolver-follow-up-report-v1.md`. **The `tree` and `cat` round starts
after it is reviewed.**
