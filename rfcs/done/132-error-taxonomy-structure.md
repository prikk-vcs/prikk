# RFC 132 — `PrikkError` carries less than it knows, and cannot grow

**Status.** **CLOSED 2026-09-04 and moved to `rfcs/done/`**, on the owner's authorization of the
2026-09-04 schedule (`ROADMAP.md`, "Proposed ordering", item 0). Increment 1 is delivered
(`264ba73`); increment 2 is ruled deferred with three named re-open triggers (§5).

**Closed does not mean the triggers lapse.** If any of §5's three fire, this RFC is the design record
to reopen against — that is what `done/` holding a deferred increment means here, and it is why the
triggers are named rather than described.

Everything below is the record as it stood. Previously: **ACCEPTED by the project owner 2026-09-03**, at the scope written here — including
§5's two-increment split, §5's refusal of `source()` in increment 1, and §6's requirement that
`Display` output not change.

**Increment 1 is COMPLETE** (`264ba73`, CI 15/15) **and its evidence accepted by the owner the same
day**, re-accepting this RFC after §5's evidence section was added. That section is therefore
settled input, not a proposal: increment 2 may be scoped against it without re-deriving the
classification or the derive answer.

**Increment 2 is RULED 2026-09-03: deferred, with three named triggers** (§5). **RFC 132 therefore
has no outstanding work**, and the `PrikkError` change it landed is ready to ship.

Raised by the external architecture audit of 2026-08-31 as **`ROADMAP.md`'s `AUD-04`**, the last of
that program's four remaining rows and the only one carrying a sequencing constraint: **before any
stability promise.**

**Tracks.** The shape of one public type. **No user-visible message changes in increment 1** — see §6.

---

## 1. What the audit found, and the part it did not

`AUD-04`'s row reads: *"`PrikkError` discards `io::ErrorKind`, implements no `source()`, and is not
`#[non_exhaustive]`."* All three are true at `663ccf6`.

**The row understates the first one.** `From<std::io::Error>` does discard the kind:

```rust
impl From<std::io::Error> for PrikkError {
    fn from(value: std::io::Error) -> Self { Self::Io(value.to_string()) }
}
```

**But that is not the only thing wrong with `Io`.** There are **45 explicit `PrikkError::Io(...)`
construction sites across 20 files, and none of them is an operating-system I/O failure.** A sample:

| Site | Message |
|---|---|
| `fsutil.rs:108` | `"temporary path destination has no file name"` |
| `fsutil/anchored.rs:201` | `"repository mutation requires Linux, macOS, or Windows root-scoped filesystem capabilities"` |
| `fsutil/anchored/directory.rs:372` | `"path must be relative to its authority root"` |

These are a caller-precondition violation, a platform-capability refusal, and a validation failure.
**`Io` is this codebase's catch-all**, and the only place a real `ErrorKind` ever exists is the `From`
impl above. A mandatory `kind` field would therefore be `None`, or a lie, at 45 of 46 sites.

**That is the actual design defect, and it is bigger than the row.** Fixing it means re-classifying 45
sites into variants that describe what they are — which changes user-facing messages, and so cannot
ride along with the cheap structural work.

## 2. `source()` is in tension with the type's existing derives

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrikkError { ... }
```

**`std::io::Error` is none of `Clone`, `PartialEq`, or `Eq`.** A `source()` that returns anything real
must store the underlying error, which costs at least `Eq` and probably `PartialEq`, or forces an
`Arc` and a hand-written `PartialEq` that ignores the stored error — an `Eq` that says two values are
equal while their sources differ.

**And a `source()` that returns `None` for every variant is worse than none**: it satisfies the
audit's checklist while adding nothing, which is the shape this project refuses elsewhere (RFC 127's
vacuous gate, RFC 126 §2's tautological property).

**So `source()` is not free, and it is not ruled here.** §5 makes it conditional on evidence that does
not exist yet.

## 3. What was verified before ruling

- **No exhaustive `match` on a `PrikkError` value exists anywhere in the workspace.** The one bare
  `match err {` (`lifecycle_cache/replay/tests.rs:304`) is on `LifecycleReplayError` and carries a
  wildcard. **`#[non_exhaustive]` is free.**
- **`Clone`/`PartialEq`/`Eq` have no found in-tree consumer** — no `assert_eq!` on a `PrikkError`
  value, no `err.clone()`. **Not proof they are unused**: five modules hold a `PrikkError` inside
  another type, and any of those deriving `PartialEq` would require it transitively. §5 requires that
  check before anything depends on the answer.
- **The CLI flattens every error to a string**: 240 `map_err(|err| err.to_string())` sites, and
  `CliError` is `Usage(String) | Failure(String)`. **Nothing in the CLI matches on a `PrikkError`
  variant**, so no exit-code mapping is affected — and equally, **none of this work improves what a
  CLI user sees.** It improves the library for an embedder. Saying otherwise would be a false promise.
- **`Display` output is documented, user-facing surface.** `docs/src/guide/troubleshooting.md:51` uses
  the literal rendered string `error: i/o error: repository mutation requires Linux, macOS, or
  Windows root-scoped filesystem capabilities` as a section heading.
- **`prikk-error` has zero dependencies** and `std::io::ErrorKind` is std, so the row's "no new
  dependency" constraint is satisfiable.

## 4. Why this is sequenced before a stability promise

`#[non_exhaustive]` is the item that matters for that constraint, and it is the cheapest of the three.
**Without it, every future variant added to `PrikkError` is a breaking change for every embedder.**
The crate is published — `prikk-error 0.29.0` is live on crates.io — so the window in which this
costs nothing is open only while the project is pre-1.0 and says so.

## 5. Ruling — two increments, and what separates them

**Increment 1 (ruled, and the subject of this RFC's first handoff):**

1. **`#[non_exhaustive]` on `PrikkError`.** Free, per §3.
2. **`Io { kind: Option<std::io::ErrorKind>, context: String }`.** `From<std::io::Error>` supplies
   `Some(kind)`; the 45 hand-built sites become `None`. **`Option` is transitional and deliberate** —
   it records honestly that most current uses have no kind, rather than inventing
   `ErrorKind::Other` for them. Increment 2 is expected to make it non-optional by moving those sites
   elsewhere.
3. **A classification of all 45 sites**, produced as evidence, not as a change.
4. **Whether `Clone`/`PartialEq`/`Eq` are load-bearing**, answered by checking the five modules that
   hold a `PrikkError` inside another type.

**`source()` is refused in increment 1.** It cannot be implemented meaningfully without storing a
source, storing a source cannot be costed without item 4's answer, and implementing it vacuously is
worse than leaving it absent.

**Increment 2 — RULED by the architect 2026-09-03: DEFERRED, with a named trigger.**

**The sequencing constraint that made `AUD-04` urgent is discharged.** *"Before any stability
promise"* applied to the changes that are **breaking**: `#[non_exhaustive]` and `Io`'s shape. Both
landed in increment 1. **With `#[non_exhaustive]` in place, adding descriptive variants later is no
longer a breaking change** — every downstream match must already carry a wildcard. Moving a site off
`Io` changes an observable *message*, not a compile contract.

**The remaining work has no consumer today, and that is measured rather than assumed:**

- **The CLI discards all of it.** `CliError` is `Usage(String) | Failure(String)`, reached through
  **126** `map_err(|err| err.to_string())` sites. Nothing in the CLI matches a `PrikkError` variant,
  so no re-classification changes any exit code or any control flow.
- **No embedder is relying on it.** `docs/src/reference/release-compatibility.md:14`:
  *"Cargo APIs, CLI behavior, object schemas, and repository formats are **not generally stable**."*
  Crate source APIs are named there as their own compatibility surface, deliberately unstable.

**The cost is immediate and the benefit is hypothetical:**

| Item | Cost |
|---|---|
| Re-classify the 29 production sites | ~29 user-facing message changes, a `troubleshooting.md` pass, release notes, and 2-3 new variants |
| Make `kind` non-optional | gated on the above |
| `source()` | rewriting **54** test assertions across 11 files in two crates |

**Doing that now is speculative API work with a measured cost against a hypothetical consumer**,
which is the trade this project refuses elsewhere. **Deferred.**

**AMENDED 2026-09-06 — the deferral's premise is false, and no trigger fired.** The stikk project
(a TUI/GUI front-end driving prikk through the public CLI) reported two preconditions surfaced under
unrelated variants, and named a machine-readable error surface as their highest-value ask. **The
"hypothetical consumer" this deferral was measured against now exists.**

**None of the three triggers below fires, and that is the finding.** stikk never touches the crates
(not trigger 1); the CLI still flattens to strings (not trigger 2); no stability promise has moved
(not trigger 3). **Trigger 2 names our own future change rather than the demand for it** — its
rationale is *"either creates the consumer that makes discrimination worth something"*, so a trigger
written to detect this consumer cannot fire until we have already done what the consumer is asking
for. **A trigger phrased as "when we do X" cannot detect "someone needs X."** Carry that into how the
next deferral is written.

**SECOND INSTANCE 2026-09-06, and it is now a pattern rather than an incident.** The same consumer
reported the identical class a second time, four days later, with **six more sites** — every
remaining `LockConflict` construction site that is not a lock
(`.git-exclude/external-communication/stikk/receive/004-four-more-preconditions-under-lock-conflict.md`).
The architect re-derived all ten sites independently and agrees on all ten, including the four the
reporter excluded. **Still no trigger fires**, for the same reason the amendment above already gave.
**Handoff issued:** `rfcs/handoffs/132-error-taxonomy-structure/six-preconditions-and-the-broad-arm-handoff-v1.md`.
**DELIVERED and ACCEPTED 2026-09-06** (`b5c2c4d`, `a050725`) — the six moved, the four stayed, and
`branch.rs` now names `PrikkError` nowhere at all. **One required follow-up**
(`guard-the-four-lock-conflicts-handoff-v2.md`): the four sites that must remain `LockConflict` had
**no assertion protecting them**, proved by perturbing `lock.rs:179` and watching the entire workspace
suite pass. **A round that guards what it changed and not what it deliberately preserved leaves the
easier half of the distinction unguarded** — and for these four the class word carries the real
instruction, *wait* rather than *change what you asked for*. **Delivered and accepted** (`0f264f3`);
verified by perturbing all four at once and confirming exactly four named tests fall over.

**That follow-up produced a finding worth more than the guards it added.** `RefStore::ensure_current_matches`'s
CAS refusal **cannot fire through `RefStore::publish`**: its only production call site is
`publish_locked`'s `Ready` branch, and `classify_state` reaches `Ready` only where the same
comparison already held, under container locks held continuously across both reads. **It is defence
against a lock-discipline regression, not a live CAS gate** — a reason to keep it, and now directly
tested. A third handoff (`record-the-cas-guard-reachability-handoff-v3.md`) puts that where the next
reader will be: the function itself carries no doc comment at all, and reads exactly like the
operative guard. **Delivered and accepted** (`587e84b`).

**ROUND CLOSED 2026-09-06.** Five commits — `b5c2c4d` (the broad arm narrowed onto a two-variant
answer type), `a050725` (six sites reclassified), `0f264f3` (the four that stay, guarded), `587e84b`
(the reachability finding recorded at the function). **Nothing outstanding.**

**Two things were found that the external reporter could not see and that their own letter did not
ask for**: the `branch.rs` broad-arm hazard, found in review before the round began, and the CAS-guard
unreachability, found by the implementer while building a test for a site nobody suspected. **Both
came from treating the reporter's six sites as a starting point rather than a work order.**

**That round carries something the reporter could not see, and it is the more important half.**
`crates/prikk-cli/src/branch.rs:290` matches `Err(PrikkError::Precondition(_))` — an open-ended arm
meaning *"this branch is uninvolved, closing may proceed."* True of exactly one condition, written as
a match on the whole variant. **Safe today** (verified: the only other errors that path produces are
`InvalidName` and `Integrity`), **and unsafe the moment `Precondition` is populated further — which is
what this very round does.** It is narrowed first, readers before writers. The fix applies RFC 138
§7.2's ruling internally: *"owned by a different ref"* is an answer, not a failure, and returning it as
`Err` is what forced the broad match.

**Increment 2 is still deferred; two of its 29 sites are not.** A `Precondition` variant for the two
sites the reporter named is handed over separately
(`rfcs/handoffs/132-error-taxonomy-structure/precondition-variant-handoff-v1.md`) — enabled by
increment 1's `#[non_exhaustive]`, committing to none of the other 27, and explicitly **not** opening
increment 2. The machine-readable surface is a design question the owner has not opened.

**What re-opens it — any one of these, and it should be taken up immediately:**

1. **A first real embedder** of `prikk-store`/`prikk-object` outside this workspace.
2. **The CLI stops flattening errors to strings** — JSON error output, or exit-code granularity
   beyond RFC 121's ruled `0`/`1`/`2`. Either creates the consumer that makes discrimination worth
   something.
3. **Any move toward a library stability promise**, which is the point at which the additive
   escape hatch stops being free for the *shape* changes.

**Not done, and deliberately:** `Clone` is measured dead (zero compile errors on removal) but is left
in place. Removing it is change without a beneficiary, and re-adding a derive later is not breaking —
so it costs nothing to keep and nothing to drop later. **Recorded so the next person does not
re-derive the measurement.**

### Increment 1's evidence, delivered 2026-09-03 (`264ba73`) and verified by the architect

**The 29 production sites classify as:** 12 caller-precondition violations, 6 validation failures,
5 "the entry disappeared" races, 4 real OS I/O failures, 2 platform-capability refusals. The
remaining 16 of the 45 are test-only — including `fsutil/anchored/failpoints.rs:256`, which is under
no `tests/` path but whose only call site is `#[cfg(test)]`-gated.

**The "entry disappeared" race is a sixth category increment 2 must carry**: five sites report a
directory entry or worktree file vanishing between one syscall and the next. They synthesize rather
than wrap an `io::Error`, but they describe a genuine external race, not a caller mistake.
`fsutil.rs:114` (a `getrandom` failure) is filed as a platform-capability refusal rather than I/O and
is explicitly flagged as arguable — **increment 2 rules it, rather than inheriting the placement.**

**The derive question is answered, by perturbation rather than by grep, and reproduced independently:**

- **`Clone` is dead.** Removing it produces **zero** compile errors across `--workspace
  --all-targets`. It can go whenever increment 2 wants it gone.
- **`PartialEq`/`Eq` are load-bearing — 56 compile errors — but not for the reason the question
  assumed.** Nothing compares two `PrikkError` values. The dependency is entirely transitive:
  `Result<T, E>: PartialEq` requires `E: PartialEq`, and **54 sites across 11 files in two crates**
  write `assert_eq!(result, Ok(x))`. Heaviest are `prikk-object/src/payload/tests.rs` (19),
  `prikk-store/src/wal/tests.rs` (10) and `refs/tests.rs` (10); all 11 are test-only, `vectors/hard.rs`
  included (`vectors.rs` is `#![cfg(test)]`).

**So `source()`'s real price is rewriting 54 test assertions**, not redesigning the type. That is the
number increment 2 is scoped against.

## 6. What must not change in increment 1

**`Display` output, exactly.** `Io` must still render `i/o error: {context}` — the structured field is
for programmatic access, not for changing what anyone reads. `troubleshooting.md:51` and every test
asserting on message text must pass untouched. **A diff that changes message text has left increment
1's scope**, whatever else it got right.

## 7. Scope

**In:** the four items in §5's increment 1.

**Out:** `source()`; re-classifying the 45 sites; any `Display` change; any CLI change. The CLI's
240-site string flattening is real and worth its own decision, but it is not this RFC's — recorded in
§3 so nobody mistakes this work for a user-visible improvement.

**Per-site, 2026-09-12 (`f06125f1`):** `checkout --snapshot-plan` on a block with no snapshot is `Precondition` with the `--patch-plan` route named; the adjacent missing-referenced-Blob arm stays `Integrity`. Delivered with `security-setup.md`'s AUTHOR sentence corrected against a measurement the architect repeated. Reviewed without a report (`rfc132-snapshot-plan-precondition-review-v1.md`).
