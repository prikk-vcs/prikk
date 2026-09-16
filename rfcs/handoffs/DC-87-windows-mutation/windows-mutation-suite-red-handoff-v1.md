# The Windows mutation suite has been red since 2026-09-14, and it holds 0.43.0

**Live 2026-09-16.** Found by the owner in GitHub Actions while the release commit was under review.
Review: `.git-exclude/reviewed/release-0-43-0-prep-review-v1.md`.

**Order: this round, before anything else.** 0.43.0 does not cut until the `Windows mutation test suite`
job is green. The release commit `dee02867` is dropped (it was never pushed) and redone after.

## 1. What is red, and since when

`ci.yml`'s `Windows mutation test suite` (`cargo test --workspace --locked --no-fail-fast` on
`windows-latest`). Last green run: `95c74055`, 2026-09-14 21:11. Twelve consecutive failures since; every
other job green in every one of them.

| test | failing since | observed |
|---|---|---|
| `snapshot::tests::snapshot_materialization_is_idempotent_for_same_bytes` | `35fc207d` (RFC 136 increment 1a) | `assert_eq!(second.written_files, 0)` fails with `left: 1` |
| `patch_replay::anchor::tests::only_worktree_writes_use_the_verified_anchor` | `ec433d21` (increment 2b) | `left: ["patch_replay.rs", "patch_replay\\anchor.rs"]` |

Latest run: `930 passed; 2 failed`. `--no-fail-fast` is already on, so that is the whole inventory.

## 2. Item 1 — the anchor test compares path strings

`crates/prikk-store/src/patch_replay/anchor/tests.rs:108-128` builds
`path.strip_prefix(&src).display().to_string()` and compares against `"patch_replay/anchor.rs"`. Windows
renders `\`.

- Fix the comparison so it is separator-independent (compare `Path` components, or normalize before
  comparing — your call, one of them, not both).
- **Then find every other test doing this.** A repository-wide check for tests that compare a rendered
  path against a literal containing `/`; fix each. Say in the report how you searched and what it found,
  including "nothing else" if that is the answer.
- This test cannot fail on Linux either way, so the control is the CI run itself (§5).

## 3. Item 2 — a re-materialization on Windows is reported as a write

`crates/prikk-store/src/worktree.rs:161-172`:

```rust
let current_mode = stat_file_state_if_exists(...)?.and_then(|stat| stat.mode).map(|m| m & 0o7777);
sync_directory_required(...)?;
if current_mode == Some(entry.mode & 0o7777) { return Ok(EntryWriteOutcome::Unchanged); }
set_regular_file_mode_required(...)?;
Ok(EntryWriteOutcome::Written)
```

`stat.mode` is `None` on a platform with no observable POSIX mode — the comment says so and cites DC-87
§3.3/§4.3 — so on Windows the skip never fires: the mode is set again and the entry counts as `Written`,
every time, for a file whose bytes already match.

**Ruled: identical bytes on a platform with no observable mode are `Unchanged`.** The directory re-sync
(1a's durability repair) still happens; what stops is reporting a write that changed nothing, and doing
work whose result is not observable on that platform.

**Two things to get right, not to assume:**

1. **`None` has two causes here.** `stat_file_state_if_exists` returns `None` when the file is absent, and
   `stat.mode` is `None` when the platform has no mode. The file being gone mid-checkout is the
   "changed during the checkout" case and must stay an error — distinguish the outer `Option` from the
   inner one rather than collapsing both.
2. **Decide what `set_regular_file_mode_required` does on Windows and say it.** If it is a no-op, skipping
   it changes nothing but the count; if it sets something real (a read-only attribute), skipping it is a
   behaviour change that belongs in the CHANGELOG. Read it before choosing.

## 4. Docs and CHANGELOG

- `docs/src/reference/platform-support.md` (and any page stating what a re-checkout costs): say that on a
  platform with no observable POSIX mode, an unchanged file is recognized by its bytes.
- CHANGELOG: `### Fixed` only if a user-visible count or behaviour changes on Windows — decide from §3.2
  and say which you chose and why.

## 5. Controls

1. **A Linux-runnable control for §3**, if a seam exists or can be added cheaply: make the mode
   unobservable (a test seam on the stat, not a `cfg`) and assert the second materialization reports
   `Unchanged`. **If that costs more than it buys, say so and rely on CI** — but then say it plainly in the
   report rather than leaving the reader to infer it.
2. **The existing idempotency test must pass unchanged.** Do not gate it by platform: the fix is meant to
   make it true everywhere. A platform gate here would be the defect hiding from its own test.
3. **The anchor test** passes on Linux before and after; CI is its only real check.
4. Perturb each fix and show the failing control, as always — for §2 that means showing the string
   comparison failing against a backslash path in a unit test you can run locally.

## 6. Verification, which is the architect's

**You cannot push, so you cannot run CI.** Report with the fixes committed locally; the architect pushes,
watches the `Windows mutation test suite` job, and reports the result back into the round. Do not mark this
closed in the report — say "awaiting the CI run", and the architect closes it.

**Discipline:** the full gate set on the final commit, both cross-target clippies included; fmt, clippy
with `-D warnings` gated on exit code, `cargo +1.85.0 check` in the pre-commit run; the staged list before
every commit. Never push, tag or publish.

**Report:** `.git-exclude/review-request/windows-mutation-suite-red-report-v1.md`.
