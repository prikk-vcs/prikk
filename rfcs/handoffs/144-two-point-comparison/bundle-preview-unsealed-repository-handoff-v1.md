# RFC 144 — `bundle preview` on a repository that has not sealed yet

**Ruled:** `rfcs/accepted/144-two-point-comparison.md` **§4n.1**. The preview round (`b814716a`) is
**accepted**; this does not reopen it. **This must land before any release ships `bundle preview`.**

Small round. One state, one variant, its tests, and the docs line.

## 1. The gap

`preview_bundle` (`crates/prikk-store/src/bundle.rs:445-447`) reads the local ref state and `ok_or_else`s
into `Integrity("ref {ref_name} is not published")` when it is absent. There is no branch, and no fixture
covers it — `genesis_repo` and `unrelated_genesis_repo` both seal first.

Verified empirically at review against a bare `RepositoryLayout::init`:

```
PROBE: Err -> integrity error: ref heads/main is not published
```

**Why it matters:** the tutorial's own sequence is `prikk init` → `commit` → `seal`, so nothing is
published until the first seal. **A user who just initialized a repository and was handed a bundle is
exactly the person who most needs to ask what is in it — and today they get exit 1.**

## 2. RULED — a fifth connectivity state, not an overloaded `FastForward`

Add a fifth `BundlePreviewConnectivity` variant for *the local ref has no published history*. Name it for
what is true of the **local** side.

**Do not reuse `FastForward`.** With no local history the bundle's whole history is new, which makes
`FastForward` look right — but that variant asserts *your history is a prefix of the bundle's*, and a
repository with no history has no prefix. `connectivity` exists to be branched on by machines; a value
that is convenient and false is worse than one more variant.

**Effects in this state:** report the bundle's whole content as `Created`, the same way `FastForward`
already reports new nodes — the honest answer to *what would this do to me* is "all of it arrives."
**Conflict:** there is nothing to conflict with, so report the same "no conflict" shape `FastForward`
uses; do not compute an LCA against an absent ref.

**Exit code stays `0`** (RFC 121). This is an answer, not a failure.

## 3. What to build

- The variant, in both the Rust enum and the `bundle-preview-v1` JSON `connectivity` field. **The JSON
  string value must be new and distinct** — a caller branching on it must be able to tell this state from
  the other four.
- Human-readable output for it, in the same voice as the existing four.
- **Tests**: an end-to-end preview against a bare `RepositoryLayout::init` (no seal) asserting the new
  state, `Created` effects covering the bundle's content, no conflict, and **`Ok`, not `Err`**; plus a CLI
  test asserting **exit 0** and the JSON field's value.
- **Check the neighbours while you are there**: does `--ref` naming a ref that exists in the repo but has
  never been published behave the same way, and does a ref name that has never existed at all? Report what
  you find. If they are already distinguishable and correct, say so — a negative result is a result.
- `CHANGELOG.md`: this changes user-visible behaviour from an error to an answer. Say so.

## 4. What this round must NOT do

- **Do not change the other four states**, the conflict machinery, the LCA search, or the reader.
- **Do not touch the write-nothing guarantee or its tests** — beyond confirming the new path also writes
  nothing, which the existing digest test will cover if you route the new state through it.
- No new `pub(crate)` widenings. §4n.2 already records 39 from the previous round; if this needs another,
  stop and report rather than adding it.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit.

Report to `.git-exclude/review-request/`. State the new state's JSON value verbatim, and answer §3's
neighbour question explicitly — including if the answer is "already correct, nothing to change."
