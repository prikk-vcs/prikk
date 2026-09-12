# RFC 132 — six more preconditions, and the arm that must be narrowed first

**RFC:** `rfcs/done/132-error-taxonomy-structure.md` — **§5's deferral of increment 2 stands and is
not reopened.** This is the same separable treatment the first two sites received
(`precondition-variant-handoff-v1.md`), applied to six more.
**Base:** `main` at `0e4e9af`.
**Origin:** stikk letter 004
(`.git-exclude/upstream/stikk/receive/004-four-more-preconditions-under-lock-conflict.md`),
verified independently at `e2e4811` — see `.git-exclude/reviewed/stikk-letters-003-004-review-v1.md`.
**Both `.git-exclude/` paths above are architect-internal and are not in the repository** (`.gitignore:30`)
— cited for provenance, not as reading you need. **This handoff is self-contained**: §3 lists every site
and its message, and §4 lists every site that stays.

**Part 1 lands before part 2. That ordering is the point of this handoff, not a preference** — see §2.

---

## 1. What this is

`PrikkError::LockConflict` has ten construction sites. **Six of them are not lock conflicts.** They
are caller preconditions: nothing is held, no other writer is involved, and waiting does not help.
The variant's own doc comment (`crates/prikk-error/src/lib.rs:43-45`) is the test they fail.

The clearest case is on the **commit** path — `worktree_patch/node_authoring.rs:238` prints
`lock conflict: active WAL has N queued patches ... run \`prikk seal\` before committing again`.
**The class word contradicts the sentence it prefixes.**

## 2. Part 1 — narrow `branch.rs:290` FIRST

**Do this before touching any error site, and do not combine the two parts in one commit.**

`crates/prikk-cli/src/branch.rs:290` is:

```rust
Err(PrikkError::Precondition(_)) => {}
```

It means *"this branch's active WAL is not implicated, so closing may proceed."* That is true of
exactly **one** condition — `require_active_ref_for_non_empty_wal`'s ownership mismatch
(`active.rs:231`). It is written as a match on the **whole variant**.

**It is safe today. I verified that rather than assuming it:** the only other errors that function can
produce are `InvalidName` (via `validate_local_branch_ref`, `refs.rs:529-550`) and `Integrity`
(`active.rs:234`, `:237`). Neither is `Precondition`.

**It stops being safe the moment `Precondition` gets more populated — which is what part 2 does.** The
failure mode is `prikk branch close` silently closing a branch it should have refused to close. That
is why this is first.

**The fix, and the reason it is shaped this way.** *"The active WAL is owned by a different ref"* is a
legitimate **answer** to an ownership question, not a failure. Returning it as `Err` forces
`branch.rs` to un-file it, and un-filing by variant is what makes the arm broad. This is the same
question RFC 138 §7.2 ruled for `trust maintainer check`: **a negative answer is an answer.**

Add a sibling that answers the question **as a value** — the ownership state, not a `Result` whose
`Err` encodes an answer — and have `branch.rs` consume that. **Keep
`require_active_ref_for_non_empty_wal` as it is**: its two other callers (`active.rs:96`,
`node_authoring.rs:258`) both use a plain `?` and genuinely want the error. One derivation, two
presentations — the arrangement RFC 138 §7.1 used for the trust policy. **Do not duplicate the
comparison logic**; the assertion form should be written in terms of the question form.

`patch_replay.rs:331-337` deliberately reuses this function's classification and wording — read that
comment before you move anything, and leave that decision intact.

**After part 1, `branch.rs` must not match on `PrikkError::Precondition` at all.** That is the check
that part 1 actually worked.

## 3. Part 2 — the six sites

All six move from `LockConflict` to `Precondition`. **Messages keep their wording**; only the variant
and therefore the `precondition not met:` / `lock conflict:` prefix change.

| Site | Message (unchanged) |
|---|---|
| `worktree_patch/node_authoring.rs:238` | `active WAL has N queued patches, at or above the configured limit (L); run \`prikk seal\` before committing again` |
| `active.rs:85` | `active WAL has N queued patches, at or above the configured limit (L); run doctor or seal before appending again` |
| `refs.rs:133` | `repository mutation is blocked by incomplete ref publication; run verify/doctor and use signer-backed seal retry` |
| `rollback_draft.rs:158` | `rollback-draft requires an empty active WAL` |
| `rollback_verify.rs:178` | `rollback-draft-verify requires an active WAL containing only the rollback draft` |
| `seal_from_accepted.rs:188` | `sealing from an accepted claim requires an empty active WAL -- seal or discard local work first` |

**`node_authoring.rs:238` is wrapped in `AuthorError::Store(...)`. Keep that wrapper** — only the
inner variant changes. `node_authoring.rs:103` unwraps it (`AuthorError::Store(inner) => inner`) and
inspects nothing, so no consumer is affected.

## 4. The four that stay, and why they are listed

**Do not touch these.** They are correctly classified, and listing them is how you know the set was
enumerated rather than sampled:

- `lock.rs:51` — `active lock belongs to a different repository authority` — a lock exists.
- `lock.rs:179` — `{kind} lock already exists: {path}` — a lock exists.
- `refs.rs:450` — `ref CAS mismatch for {ref}: expected …, got …` — genuine optimistic concurrency;
  re-read and retry is the correct response.
- `rollback_draft.rs:164` — `rollback-draft target ref changed during planning; retry rollback-draft`
  — transient by its own wording.

**If you find an eleventh site, stop and report it.** The set was enumerated at `e2e4811`; a
disagreement in the count means one of us is reading a different tree.

## 5. The breakage risk, already measured — but re-measure it

The reporting consumer could not see this from outside and said so. **From inside it is one match, not
six**, and I measured it at `e2e4811`:

- **Nothing in this workspace pattern-matches `PrikkError::LockConflict`.** Outside the construction
  sites there are only the enum definition, the `Display` arm, and four comments.
- **Nothing matches the `"lock conflict:"` message text** either.
- The whole `Precondition` consumer surface is the single `branch.rs:290` arm that part 1 removes.

**Re-derive both greps against your own final diff rather than trusting this paragraph.** The 0.33.0
round is the precedent for why: reclassifying one site would have silently broken `prikk branch close`,
and that was caught by a test, not by reasoning.

## 6. Controls

1. **The `branch close` behaviour is unchanged.** `crates/prikk-cli/tests/dc61_branch_closure.rs:379`
   already covers the "owned by a different ref → proceed" case that part 1 rewires. It must still
   pass **without modification**. If it needs editing, part 1 changed behaviour and that is a finding,
   not a test to update.
2. **Part 1 bites.** Before part 1, construct the ownership-mismatch case and confirm `close`
   proceeds; after, confirm it still proceeds *through the new value path*. Then confirm a
   **different** `Precondition` reaching that call site is no longer silently swallowed — this is the
   defect part 1 exists to close, so it must be demonstrated, not asserted.
3. **Each of the six changes its prefix**, asserted against the compiled binary, not the source. Six
   assertions on `precondition not met:` where `lock conflict:` was.
4. **The four are untouched**, asserted the same way. A test that only checks the six would pass if
   you moved all ten.
5. **The full-queue message is reachable and correct on the commit path.** This is the site that
   matters most; drive it through a real repository at the configured limit rather than unit-testing
   the constructor.
6. **`grep` proves the absence.** After part 2, no `LockConflict` construction site remains outside
   the four in §4, and `branch.rs` matches no `Precondition`. State both greps and their output in
   the report.

## 7. Out of scope

- **RFC 132 increment 2.** Still deferred. Its 29 sites are the hand-built `Io` sites — a **disjoint**
  set from these ten. Nothing here opens it, and the report should not argue for it.
- **A machine-readable error surface.** Named by the reporter as their highest-value ask and recorded
  as a live design question the owner has not opened. Not this round.
- **`refs.rs:450`'s wording**, and the other three of §4.
- **Queued-patch enumeration**, which the same reporter asked for separately. It is a larger piece
  than it looks and is being designed on its own; do not anticipate it here.

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

**`boundary-check` includes the RFC 130 coupling gate now.** Part 1 adds a function to `active.rs` and
changes a `prikk-cli` call site; if that moves an edge or a hub, the gate will say so and the entry
needs a reason **and** a `what_would_remove_it`. Run it early.

**Cross-target clippy only if your own diff introduces `#[cfg(target_os)]`/`#[cfg(unix)]`/
`#[cfg(windows)]`.**

## 9. This changes user-visible messages, so it needs a `CHANGELOG.md` entry

**Six error messages change their prefix.** That is a user-facing change and a compatibility event for
any consumer that classifies on message text — 0.33.0's two prefix changes were exactly that for the
reporting front-end.

**Write the entry under `## Unreleased`.** Name the six conditions in prose (not a table of file
paths — the changelog is for users, not for us), say plainly that the prefix moves from
`lock conflict:` to `precondition not met:`, and say why: none of them involves a lock, and waiting
never helped.

**This instruction is here because its absence has shipped undocumented features twice** —
`.prikkignore` in 0.29.0 and `prikk key`/`prikk setup` in 0.33.0. Part 1 needs no entry (no
user-visible change); part 2 does.

## 10. Reporting

`.git-exclude/review-request/`, per the standing convention. Include:

- **the two greps of §5 re-derived against your final diff**, with their output;
- how you demonstrated control 2's second half — a different `Precondition` no longer being swallowed;
- whether `dc61_branch_closure.rs:379` passed unmodified;
- whether `boundary-check`'s coupling gate moved, and if so what you declared;
- **anything in the four of §4 you disagree with.** The reporter drew that line and I agreed with it
  independently; a third reading that disagrees is worth more than a third that concurs.
