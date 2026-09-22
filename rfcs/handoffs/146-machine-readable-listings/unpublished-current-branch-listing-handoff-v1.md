# The unpublished current branch is named — handoff v1

**Live 2026-09-22.** Two rulings, one round: **RFC 146 §8f** (the listing) and **RFC 147 §2j** (`branch
switch`'s dead route). Both come from stikk's letter 017, which answered the question our letter 018 asked
them, and both were measured on the released 0.46.0 binary and on `main` before either was written.

**Scheduled in 0.47.0 after the measurement round and RFC 158 Stage A**, with the queued-patch-paths round
already delivered. It is small; it is not urgent; it must not be rushed past the two items ahead of it.

## 1. What is true today

| state | `branch list` | `--all --format json` |
|---|---|---|
| fresh repository, nothing sealed | `no branches` | `{"schema_version": "branch-list-v1", "branches": [], "received": []}` |
| after the first `seal` | `* heads/main <64 hex>` | one row: `{"ref_name", "ref_state_id", "closed", "current"}` |

`branch create` **refuses entirely** before the first seal and **publishes immediately** after one. So **at most
one unpublished branch can exist, it is always the current branch, and only before the first seal** — the
invariant the design rests on. If a round finds that false, stop and report rather than generalising the shape.

## 2. The listing (RFC 146 §8f)

- **`branches[]` and `received[]` do not change.** Membership keeps meaning *has a published `RefState`*: a
  shipped consumer decides "this branch has no history" from absence there, and a new row would silently invert
  a claim it makes to its users while breaking no count.
- **One nullable top-level field**, additive within `branch-list-v1`:
  `"unpublished_current_branch": "heads/main"`, or `null` when there is none. Not an array, not a row.
- **Prose gains one line with no id column**, exactly:

  ```text
  current branch heads/main has no published history yet; the first `prikk seal` publishes it
  ```

  It appears **below** `no branches` (or below the table, if a listing ever coexists with it — see the
  invariant), and **must not be parseable as a row**: a consumer's reader refuses a row whose id column is not
  an id, and would fail every ref read rather than merely misreport. The spelling above has been given to that
  consumer in advance; **if the round wants to change a character of it, stop and say so** rather than shipping
  a different string.
- `--format json` and prose both answer with exit `0`, as they do today. `status` is untouched.

## 3. `branch switch` (RFC 147 §2j)

`branch switch <the current branch>` currently says *"heads/main does not exist; run `prikk branch create
heads/main` first"*, and that route refuses. It must instead say that it **is** the current branch and — when
unpublished — that the first `seal` publishes it. **Do not name `branch create`.** Every other absent target
keeps today's refusal unchanged, `branch create`'s own included.

## 4. Controls

1. **The field is present and correct in both states**: `"unpublished_current_branch": "heads/main"` on a fresh
   repository, `null` after the first `seal`. *Perturb: emit the name unconditionally — the second half must go
   red.*
2. **`branches[]` is byte-identical to today in both states** — assert the array, not just its length, so a row
   cannot be added under the field's cover.
3. **The prose line is the exact string above**, and **no line of `branch list` output on a fresh repository
   parses as `[*] <ref-name> <64 hex>`** — assert that directly, since that is the consumer's failure mode and
   not merely a cosmetic question.
4. **`branch switch <the current branch>`** names neither `branch create` nor "does not exist", and says the
   branch is already current; **`branch switch <a genuinely absent branch>`** keeps today's message. *Perturb
   each.*
5. **`branch create heads/side` on a fresh repository still refuses**, and after a seal still publishes — the
   invariant §1 rests on, asserted so a later change cannot move it silently.

## 5. Docs and CHANGELOG

- `guide/faq.md`'s branch section and any page quoting `branch list`'s shape: the new field and the new line;
- **CHANGELOG `### Added`** for the field and the prose line (a consumer must learn the field exists), and
  **`### Fixed`** for `branch switch`'s dead route, naming both messages.

**Report:** `.git-exclude/review-request/unpublished-current-branch-listing-report-v1.md`.
