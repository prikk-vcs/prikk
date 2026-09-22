# An unpublished current branch is not an absent ref — handoff v1

**Live 2026-09-22.** The specification is **RFC 147 §2i**, which this handoff implements. It is **the first round
of 0.47.0**, ahead of the measurement tooling: this is a behaviour regression we shipped in 0.45.0 and widened in
0.46.0, and a consumer's released feature runs into it.

**From stikk's letter 015**, reproduced by the architect on the released 0.46.0 binary before any of this was
written.

## 1. What is wrong

One fresh repository, one untracked file, nothing sealed, `heads/main` the current branch:

```text
$ prikk worktree-status                      # the full report, exit 0, naming ref: heads/main
$ prikk worktree-status --ref heads/main     # error: precondition not met:
                                             #   ref heads/main does not exist in this repository   (exit 1)
```

The same for `log --ref`, `tree --ref`, `diff --from` and `checkout`'s read-only modes, while the bare forms of
`log`, `tree` and `diff` all answer for that very ref — and `status` prints `heads/main RefState: <not
published>` beside `current branch: heads/main`.

**The refusal is false**, and it is the architect's defect: the refusal sweep's handoff said "an explicitly named
absent ref refuses" without asking what *absent* means for a branch that has never been sealed, and RFC 157 §3
and RFC 153 §7.3 then inherited the same unexamined word.

## 2. The fix

**One change at the resolver, not five at the call sites.** `require_existing_ref` (and `resolve_point`, which
extends it) must treat **the repository's current branch as existing, published or not**, and let the reader
answer what it answers for the implicit path.

- **Accept:** the named ref equals the current branch (RFC 151's pointer), whether or not it has a `RefState`.
  The answer must be **byte-identical** to the same command run with no `--ref`, on the same repository, in the
  same state. That is the property to test, and it is cheap to test exactly.
- **Keep refusing:** every other ref with no `RefState` — a mistyped name above all, which is what the sweep was
  for — with today's wording, unchanged.
- **Do not disturb:** a received ref (`remotes/…`, read where reading is allowed), and a **damaged** ref, which
  0.46.0 answers `integrity error: ref <ref> is not published`. Note the collision in *words*: damage says "is
  not published" of a ref whose pointer is gone, while an unpublished branch is simply new. **Those two must not
  end up answering alike** — assert both in the same test so a later edit cannot merge them.

**Surfaces to cover** (the seam is the resolver, so this list is what must be *asserted*, not what must be
edited): `worktree-status`, `log`, `tree`, `cat`, `diff --from`, `checkout --plan-only`, `--snapshot-plan`,
`--patch-plan`, `--patch-delete-plan`.

**`cat --path <p> --ref heads/main` on a fresh repository** has no file to read, so it refuses — but it must
refuse with *`path <p> does not exist at heads/main`*, the absence of the **path**, not the absence of the ref.
Say in the report which message it gives before and after.

## 3. Controls

1. **Implicit equals explicit, per command, on a fresh repository.** For each surface above, run it bare and with
   `--ref <the current branch>` and assert the outputs are **identical** (`--format json` where it exists, prose
   otherwise), both exit 0. *Perturb: make the resolver refuse the current branch again — every one of these must
   go red.*
2. **A mistyped ref still refuses**, with the exact 0.45.0 wording, exit 1.
3. **A damaged ref still answers `integrity error: … is not published`**, distinct from control 1's empty report —
   asserted in the same test, so the two cannot be merged by a later edit.
4. **After the first `seal`, nothing changes**: the explicit form keeps answering as it does today.
5. **A received ref is unaffected** wherever it is read today.

## 4. Docs

- `troubleshooting.md` and any guide page that quotes the absent-ref refusal: say which refs it applies to, now
  that the current branch is not one of them;
- **`tree-and-cat.md` and `diff.md`**: their "an explicit absent `--ref` refuses" sentences are true only of a
  genuinely absent ref. Correct both, and say plainly that naming the current branch is the same as not naming
  it;
- **CHANGELOG `### Fixed`**: name the regression, the releases it affected (0.45.0 and 0.46.0), and the commands.
  A consumer reading the entry must be able to tell whether their own invocation was affected.

## 5. Not in this round

- **`branch list` prints `no branches` while `status` prints `current branch: heads/main`**, and `branch create`
  refuses before the first seal. Same family, different question, and it changes a report consumers parse — the
  architect has asked stikk what they consume before ruling it. **Do not touch `branch list`.**
- The measurement-cost round (`133-…/measurement-cost-handoff-v1.md`) follows this one.

**Report:** `.git-exclude/review-request/unpublished-current-branch-report-v1.md`.

## Addendum 1 2026-09-22 — the round is accepted; two follow-ups, both from my wording

**Accepted** (review `unpublished-current-branch-review-v1`): `6dee6c4f`. Gates 14/14 re-run by the architect —
**including the two cross-target clippy runs, both green** — 2,252 / 0 / 30 per toolchain; every surface probed
bare against explicit on the built binary; and the choke point perturbed independently, which turns
`control1_…` and `cat_names_the_absent_path_not_the_ref` red.

**§1's instruction was wrong and you were right to refuse it.** "One change at the resolver, not five at the
call sites" would have leaked into `checkout`'s materialize modes and would have read the current-branch pointer
from `prikk-store`, which RFC 151 §2.2 forbids. Two existing tests caught both. A store predicate that never
reads the pointer, plus one CLI function that does through the single allowed reader, is the better shape, and
leaving `decide_ref_existence` byte-for-byte unchanged is what keeps every unlisted caller still.

Two follow-ups, in **one** round. Both are consequences of how I wrote the handoff, not of how you read it.

### 1. `cat`'s parenthetical states what this round ruled false

```text
error: precondition not met: path a.txt does not exist at heads/main
       (ref heads/main does not exist in this repository)
```

Both forms agree — and both now carry a reason RFC 147 §2i declares false. The wording is mine (RFC 157
Addendum 3 item 2 pinned it, arguing it names "the reason"); the reason changed underneath it.

**For the unpublished current branch, the parenthetical says the branch has no published history yet.** The
outer clause is unchanged, and every other case — a genuinely absent ref above all — keeps today's pinned
string. Update the pinned test with it.

### 2. `--from <the unpublished current branch>` must not fold the queue

Measured: unpublished `heads/main`, **one queued unsealed commit** holding `a.txt`, a new worktree file `b.txt`.

```text
tree --ref heads/main              → entries: 0
cat --path a.txt --ref heads/main  → path a.txt does not exist at heads/main
diff --from heads/main             → from: … the empty state, plus 1 queued commit not yet sealed
                                     entries: 1 — added b.txt
```

`tree` and `cat` say the point holds nothing; `diff` says it already holds `a.txt`. And `guide/diff.md`
contradicts itself two bullets apart — *"`--from <point>` compares against that point exactly, queue or no
queue"*, then *"naming that same branch explicitly … answers exactly the same way"* as the folding bare form.

**I caused this** by making "byte-identical to the bare form" the property without noticing it is right only
when the queue is empty — the state stikk reported and the only one I probed.

**Ruled: `--from <the unpublished current branch>` compares against that point exactly — the empty state — and
does not fold the queue.** `--from` then means one thing on both sides of the first seal, the three read verbs
agree about what a point holds, and the documented rule stays true. **The bare form does not change.**

- the left side reads `from: heads/main (not published: the empty state)`, with **no** queued clause, and
  `queued_patches` absent from its JSON side as for any other explicit point;
- **control 1 stays as it is** — its fixture has no queue, so identity still holds there;
- **a new control asserts the difference**: with one queued commit, bare folds and `--from <branch>` does not,
  and the queued file appears as `added` in the explicit form;
- `diff.md`'s two bullets are reconciled — the exception goes away rather than being written down.

### 3. Then

The measurement round (`133-…/measurement-cost-handoff-v1.md`) follows, then RFC 158 Stage A.

**Report:** `.git-exclude/review-request/unpublished-current-branch-follow-up-report-v1.md`.
