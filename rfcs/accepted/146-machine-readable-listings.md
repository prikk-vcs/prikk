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

## 7. What would make this not worth doing

If the answer to RFC 145 is **shape D** and the ecosystem is expected to build on `prikk-store`
directly rather than on the CLI, **the marginal value of these three drops** — a library consumer never
needed them. **It does not go to zero**, because the CLI is prikk's stable surface and the library is
explicitly not, but the case would weaken. **Recorded so that a later reader can see the argument's
dependency rather than infer it.**
