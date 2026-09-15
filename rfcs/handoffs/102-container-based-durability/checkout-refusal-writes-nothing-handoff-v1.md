# A refused checkout writes files first, leaves the dirty marker set, and blocks `commit`

**Live 2026-09-15**, from stikk's letter 012
(`.git-exclude/upstream/stikk/receive/012-a-refused-patch-materialize-leaves-files-behind.md`), with the
architect's own measurement, which found more than the letter did.

**Order:** start after RFC 136 increment 2b's report is reviewed. 2b changes
`materialize_patch_checkout_inner`, so this lands on top of it. **0.43.0 does not cut without it.**

## 0. Carried from RFC 136 increment 2b's review (first commit of this round)

These come from `.git-exclude/reviewed/rfc136-increment-2b-review-v1.md`. They come first because this round
touches the same materializers.

1. **Gate the lying-snapshot fixture.**
   - **The defect:** `pub enum SnapshotFixture` and `pub fn publish_snapshot_fixture_for_test_support`
     (`rfc111_seal_simulation.rs`) carry no `#[cfg(feature = "test-support")]`. Only their re-export in
     `lib.rs` is gated, so the code is compiled into every `prikk-store` build. Its doc says otherwise.
   - **The fix:** put the gate on each item, and on every helper only they use.
   - **Add a guard:** a source scan requiring every `pub` item whose name ends in `_for_test_support`, and
     every type only such items use, to carry the gate. Perturb by removing one, and the guard must name it.
2. **`#[non_exhaustive]` on the report structs 2a/2b extended.** `SnapshotMaterializationReport` gained
   `provisional` without it.
   - List every `pub` report struct 2a and 2b added a field to, and mark each.
   - `CHANGELOG.md`: `### Changed — breaking once for Rust callers`, naming them.

## 1. Measured by the architect

A binary of `origin/main` (`26e8c528`) and an isolated repository. `heads/main` holds `shared.txt` =
`main`. `heads/dev` changes it to `dev` and adds `dev-only.txt`. Back on `heads/main`, with a clean tree:

| step | result |
|---|---|
| `checkout --patch-materialize --ref heads/dev` | exit 1, `integrity error: refusing to overwrite existing file with different content: …/shared.txt` |
| the worktree after it | **`dev-only.txt` written** with `heads/dev`'s bytes; `shared.txt` still `main` |
| `.prikk/worktree.marker` (RFC 102) | **set**, 21 bytes; 42 after a second attempt |
| `status`, `doctor` | byte-identical to before, exit 0 |
| the same checkout again | refuses the same way |
| **`commit` on the user's own branch** | **exit 1, `integrity error: worktree materialization was interrupted; … (re-run checkout materialization to complete it)`**. The route it names loops: re-running *that* checkout refuses again |
| `checkout --patch-materialize --ref heads/main` (own branch), or `branch switch heads/main` | exit 0, marker cleared, and `commit` works again. The stray file stays in the worktree |

**The cause, at source.** `materialize_patch_checkout_inner` (`patch_checkout.rs`):
1. analyses deletions up front;
2. calls `mark_worktree_dirty`;
3. calls `materialize_replay_manifest_entries` (`worktree.rs`), which writes entries **one at a time** and
   returns `Integrity` at the **first** conflict (`materialize_replay_entry`, `worktree.rs:136-141`);
4. returns before `clear_worktree_dirty`.

`materialize_snapshot_checkout` uses the same materializer. `branch switch` already does it right: refusal 4
checks every path before the first write (`branch_switch.rs:148`).

## 2. Required

1. **Plan before the first write.** Make one pure pass over every manifest entry, using exactly the checks
   `materialize_replay_entry` applies:
   - existing file with different bytes;
   - a non-file at the path;
   - a symlinked parent or target;
   - escape from the root.

   **Any conflict refuses before `mark_worktree_dirty`, and before 2a's provisional marker for
   `--snapshot-materialize`**, naming **every** conflicting path, not the first. Deletion analysis is already
   up front; keep it.
   - **Callers:** `--patch-materialize`, `--patch-materialize-delete`, `--snapshot-materialize`.
   - **One planner** shared by all three, not three copies. Say whether `branch switch`'s refusal 4 can use
     it too.
2. **The write-time check stays** as the second guard. After a clean plan it fires only when the worktree
   changed during the checkout. In that case the dirty marker stays set, correctly (RFC 102). The message
   says the worktree changed during the checkout and names the route in 4.
3. **Classes, per site (RFC 132 mould):**

   | site | ruling |
   |---|---|
   | the overwrite conflict (`worktree.rs:138`) | `Precondition`, naming the paths and the way out |
   | `refusing checkout deletion because N candidate(s) are unsafe` (`patch_checkout.rs:177`) | `Precondition`, naming the paths |
   | `commit`'s dirty-marker refusal (`node_authoring.rs:319`) | `Precondition`, and its route must be **true**: `prikk checkout --patch-materialize --ref <the current branch>` or `prikk branch switch <the current branch>`, both measured above to clear the marker |
   | `snapshot checkout plan … names no block` (`worktree.rs:66`) | report whether it is reachable, then classify it in the same mould |
   | `materialization target escaped repository root` (`worktree.rs:180`) | **stays `Integrity`**: a path that escapes the root is not a user state |

4. **Report the dirty marker.**
   - **`status` prose:** a line naming the route in 3.
   - **`status --format json`:** additive within `status-report-v1`.
   - **`doctor`:** a finding with a code, naming the same route.
   - **Reachability after the fix:** only a crash or a concurrent change sets it, and a repository already
     stuck by this defect finds its way out here. No migration.
5. **Out of scope, and why.** After recovery, `commit` authors whatever is in the worktree, including a
   file an older refused checkout left behind. That is `commit`'s documented whole-worktree behaviour. The
   fix is that a refusal writes nothing, not a change to `commit`.

## 3. Controls (each must be able to fail)

- **stikk's scenario on the binary.**
  - Assert the refusal writes nothing: the worktree is byte-identical, `worktree.marker` is 0 bytes, and
    `worktree.provisional` is 0 bytes.
  - Assert the class is `precondition not met:`, naming `shared.txt`, and that `commit` still works.
  - **Perturb:** move the plan after `mark_worktree_dirty`, or drop it, and `dev-only.txt` appears.
- **Two conflicting paths,** both named.
- **`--snapshot-materialize`** at a checkpoint tip over a conflicting file: nothing written, and neither
  marker set.
- **The concurrent-change guard.** Change a file between plan and write, through a store failpoint or a
  test seam that production cannot reach, and check that:
  - the marker is set;
  - `status` and `doctor` report it;
  - `commit` refuses naming the route;
  - each of the two routes clears it.
- **The deletion refusal** names its paths, as `Precondition`.

## 4. Docs, CHANGELOG, report

**Docs:**
- `checkout.md`: a refused checkout writes nothing;
- `troubleshooting.md`: the new messages and the dirty-marker route;
- `status.md`: the new line and JSON field.

**CHANGELOG, naming stikk's letter 012:**
- `### Fixed`: a refused checkout writes nothing and no longer blocks `commit`;
- `### Changed`: the classes;
- `### Added`: the `status` and `doctor` report.

**Discipline:** fmt, clippy with 0 errors, workspace tests with 0 failed, and the staged list before every
commit; the full gates on the final commit.

**Report:** `.git-exclude/review-request/checkout-refusal-writes-nothing-report-v1.md`.
