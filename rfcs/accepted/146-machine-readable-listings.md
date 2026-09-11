# RFC 146 — `--format json` for the three listing commands

**Status.** **ACCEPTED by the project owner 2026-09-10**, the same day it was opened. Written on the
owner's instruction to prepare work for an idle team. **Small and deliberately narrow.**

**The handoff is live**: `rfcs/handoffs/146-machine-readable-listings/listing-json-handoff-v1.md`. It was
written while this RFC was still `proposed/` and carried an explicit hold; that hold is now lifted.

**Author-review independence gap:** the architect authored this and will review its implementation.
Compensated by §5's controls being fixed before any code exists.

---

## 1. The gap, measured

**`prikk log`, `prikk branch` and `prikk tag` have no `--format json`.** Nine other commands have it:
`status`, `verify`, `worktree-status`, `show`, `bundle preview`, `checkout --patch-plan`,
`trust maintainer list`, `trust maintainer check`.

**These three are exactly what an index page needs first** — history, branches, tags. Any consumer that
wants them today must parse prose.

**This is not a defect report against RFC 121.** RFC 121 §117 put *"JSON output for commands that lack
it"* explicitly **out** of its scope. It was scoped out, not overlooked, and this RFC is the scope
being taken up rather than a contract being repaired.

## 2. Why now, and why it does not presuppose RFC 145

RFC 145 §6a recorded this gap while ruling on where a browse view should live, and noted that **it binds
a shell-out consumer and nobody else.** Today that means the stikk project, which drives prikk through
the CLI (`stikk-prikk` is *"the prikk seam … CLI backend"*) rather than by linking `prikk-store`.

**But this RFC stands on consistency alone.** Nine commands answer machine-readably and three do not,
with no principle separating them. **It is useful under every RFC 145 shape and under none** — an
`export`, a separate server, or nobody at all. **It must not be argued for as hosting groundwork**, and
scheduling it decides nothing about RFC 145.

**A request from a consumer would raise its priority, not its justification.** stikk was asked in
correspondence 005 whether the three are on their path; that answer is not a precondition.

### 2a. CONFIRMED 2026-09-10 — the consumer answered, and it is stronger than a preference

stikk letter 006 §1: **yes, all three, and "those three are the entire remainder of our prose
parsing."** They name their own call sites — `cli_backend.rs:209` (`log`), `:227` (`branch list --all`),
`:233` (`tag list`) — and every other prikk command they drive either has `--format json` or has its
output ignored.

**So this RFC does not merely add consistency for them: it closes their prose-parsing exposure
completely.** Each of the three has already cost them a re-baseline — `log` at 0.32 when a
`patch <id>: <message>` line appeared, `branch list` at 0.30, and `tag list` in a sweep that found their
ref picker had never shown tags at all.

**A second, independent argument arrived with it.** stikk reproduced a parser defect this project
predicted (letter 005 §3): a renamed path whose first token is `modified`/`missing`/`untracked`/
`unsupported` is taken for a change entry, making their view report **a file that does not exist**. Their
root cause is wider than the prediction — their entry scan reads indented lines from the whole document,
so **any future indented section of prikk's would break it identically.**

**Until this RFC lands, prikk's prose shape is load-bearing for a real consumer.** That is a cost the
project is carrying whether or not it chose to.

**The order this arrived in matters and is recorded deliberately: RFC 146 was opened, accepted and
handed off on consistency grounds *before* the answer came back.** The justification never depended on
a consumer; only the priority did.

## 3. What it adds

`--format json` on the three, following the convention the existing nine already establish:

| command | schema name |
|---|---|
| `prikk log [path] [--limit N] [--ref REF]` | `log-report-v1` |
| `prikk branch [list] [--all]` | `branch-list-v1` |
| `prikk tag [list]` | `tag-list-v1` |

**Every existing emitter carries a `"schema_version"` first field** — `status-report-v1`,
`verify-report-v1`, `worktree-status-report-v1`, `show-report-v1`, `patch-plan-content-v1`,
`bundle-preview-*`, `trust-list-v1`, `trust-check-v1` — and these three must too.

**Hand-built JSON, no serialization dependency.** All six emitters in `crates/prikk-cli/src/output/`
build their JSON as strings. **prikk's entire third-party runtime surface is five crates** and this
RFC does not spend any of it. A serialization dependency for three listings would be the most
expensive possible way to buy consistency.

## 4. The rules that bind the shape

**RULED, and these are the whole design:**

1. **`--format json` must not fail where the bare command succeeds.** RFC 140 §7b settled this for
   `status`: the JSON form failing in a state the prose form survives is a defect, not a stricter
   contract.
2. **Degrade absence, propagate error — and *absence only*.** RFC 142 §6b corrected an over-broad
   reading of exactly this: degrading every failure is the wrong behaviour. `previous-ref-state:
   <none>` becomes `null`; an unreadable object propagates as an error.
3. **Machine-branchable fields, not rendered prose.** A consumer must not have to parse a string to
   learn a fact the command already knows. `prikk branch --all` marks closed branches in prose; JSON
   carries a boolean, not the marker text.
4. **Empty is a value, not an error.** `prikk tag` prints `no tags`; the JSON form is
   `{"tags": []}` with exit `0`.
5. **The nesting is the data.** `prikk log` reports blocks each containing patches. The JSON mirrors
   that structure rather than flattening it.
6. **Exit codes are unchanged** — RFC 121's `0` ok, `1` operational failure, `2` usage error. Adding a
   flag must not move any of them.

## 5. Controls this must carry

- **Prose and JSON must agree, proven by a test that reads both** for the same repository state — not
  two tests each asserting its own expectation.
- **The `--format json` no-worse-than-prose rule (§4.1) must be perturbed**, not asserted: find a state
  where the command degrades in prose, and show JSON exits the same way. **A control that cannot fail
  is not a control** — this project has shipped three of those.
- **Empty and non-empty for each of the three.**
- **Exit codes checked, not assumed**, on the new flag.

## 6. Non-goals

- **No new command, no new capability.** Every fact these emit is already printed.
- **No `--format json` anywhere else.** `merge-evidence`, `merge-plan`, `inverse-plan`,
  `rollback-preview` and `unlock` also lack it; **they are out of scope and are not implied by this
  RFC.** If they are wanted, that is a separate decision.
- **No serialization dependency** (§3).
- **No prose change.** The existing human output is not touched.
- **No schema stability promise beyond what the other nine carry.** These are `-v1` names in the same
  namespace under the same terms.

## 8. DELIVERED 2026-09-10 (`3ccf6f69`) — and pushed before it was reviewed

**All three schemas shipped as ruled**: `log-report-v1`, `branch-list-v1`, `tag-list-v1`, hand-built,
crate count unchanged at 216. 1824 → 1837 tests. **Every field verified on the binary against the same
repository the shipped 0.38.0 binary populated.** Review: `.git-exclude/reviewed/rfc146-listing-json-review-v1.md`.

**A process breach, recorded against the architect.** `3ccf6f69` was the implementing team's local
commit. The architect read the ahead-list before pushing, saw two commits, and pushed both — carrying
unreviewed code to `origin/main` under a documentation commit. CI was green on it before it was read;
that is luck, not process. **The ahead-list check exists to be acted on, not run.**

### 8a. RULED — `branch` must filter by ref kind before `-v1` ships

The round found, correctly flagged as pre-existing and out of scope, that `prikk branch` lists tag refs:
`tag.rs:91` filters on `RefKind::Tag`, `branch.rs:101`→`:120` never filters. **Verified: the shipped
0.38.0 prose already prints `tags/v9` under `branch`.**

**RULED: fix it in this RFC's next increment, not later.** In prose it is a display slip; in
`branch-list-v1` it is **structured data asserting a tag is a branch, with a meaningless `closed`
field**. A shipped schema is fixed by changing what `-v1` returns or by minting `-v2`; an unreleased one
is fixed by a filter. Handoff: `rfcs/handoffs/146-machine-readable-listings/branch-ref-kind-filter-handoff-v1.md`,
carrying one question — whether `received` has the same hole — to be measured and reported, not fixed
silently, because fixing it would need a decode this RFC forbids.

### 8b. Two decisions the round made deliberately, both accepted

- **`log`'s `repository` field carries the absolute `.prikk` path.** The handoff named it as the field to
  decide rather than copy; they decided: it is the existing convention (`status-report-v1`,
  `worktree-status-report-v1`, both verified at source), repositories have no other identity, and a third
  convention would be the surprising choice. **Accepted.** Whether the convention itself should carry an
  absolute path is a separate question about the convention, not about this emitter.
- **Received refs carry no `closed` field** — a separate array, not a `null`. The listing path never
  decodes a received ref's state and §4.2 forbids new computation. **This is rule 5 applied, not rule 2
  avoided**: the missing fact is a different kind of entry, not an absence on a uniform one.

### 8c. Two things the handoff got wrong, recorded

- **It omitted the changelog.** The round left the entry to review, correctly. **A handoff that omits
  the changelog ships an undocumented feature** — 0.33.0's lesson, repeated here. Closed by the
  architect under `## Unreleased`.
- **`prikk tag [list]` in §3's table implied a bare form that did not dispatch.** `tag` had no
  leading-flag arm; `branch` did. The round found it by running the RFC's own example and fixed it as a
  necessary parity change, with no prose line touched. **Verified on the shipped binary: the bare form
  errors at 0.38.0 and works at `3ccf6f69`.**

### 8d. DELIVERED and RULED 2026-09-12 (`fa872c19`) — `branch` filters; `received` stays kind-agnostic, by design

**`prikk branch` now lists only `RefKind::Branch`, in both forms**, on the payload already decoded for
`closed`. Verified on a fresh fixture: the tag is absent from prose and from `branch-list-v1`, present
in `tag-list-v1`. 1837 → 1838 tests. Review:
`.git-exclude/reviewed/rfc146-branch-ref-kind-filter-review-v1.md`.

**§8a's attached question, measured end to end: a received *tag* does appear in `branch-list-v1`'s
`received` array** — `bundle export --ref tags/v1`, `bundle import` into a fresh repository, and the
receiver reports `{"ref_name": "remotes/tags/v1", …}` under `received`.

**RULED: `received` is kind-agnostic under `-v1`, and that is its contract, not a gap.** Three findings,
each verified at source, decide it:

1. **There is no kind to filter on.** `ReceivedPointer` and `ReceivedIndexEntry` carry `ref_name` and
   `ref_state_id` only. Kind-partitioning means decoding each received `RefState` — new computation
   §4.2 forbids, and, more to the point, **decoding an untrusted received object in a listing in order
   to label it.** Classification of received material belongs to `verify`, not to `branch`.
2. **A name-prefix filter is ruled out.** `validate_received_ref` enforces the `remotes/` prefix and
   nothing about what follows — the remainder is the origin's own ref name, which this repository does
   not control. **Filtering on `remotes/tags/` would trust a convention nothing enforces.** Recorded
   because it is the fix an engineer reaches for first.
3. **Dropping the row would lose the only listing of a received tag.** `prikk tag` reads
   `list_ref_pointers()` only, never the received namespace.

**So `received` means exactly what its name says: every pointer that arrived, in the form it arrived,
with no claim about what it is.** That is consistent with the settled position that repositories are
anonymous and peers do not exist — a received ref is an artifact, not a branch or a tag until this
repository says so. **The schema's contract, stated so a consumer branching on it knows: `received` is
not kind-partitioned and will not become so under `-v1`.** A received-ref listing surface of its own
(the round's option c) is real scope and is not opened here; nothing asks for it.

**The round edited the architect's changelog entry, and was right to.** The entry's *"known and
carried … fixed in the next increment"* became false in the same unreleased window; shipping it would
have advertised a fixed defect as carried. Replaced with `### Fixed` and one paragraph on the received
tag, flagged in the report's own section. Accepted as written.

## 7. What would make this not worth doing

If the answer to RFC 145 is **shape D** and the ecosystem is expected to build on `prikk-store`
directly rather than on the CLI, **the marginal value of these three drops** — a library consumer never
needed them. **It does not go to zero**, because the CLI is prikk's stable surface and the library is
explicitly not, but the case would weaken. **Recorded so that a later reader can see the argument's
dependency rather than infer it.**
