# Merge

DC-74 adds `prikk merge`, the first command that executes a merge rather than only reporting on one.
It builds on the same read-only evidence [merge evidence](merge-evidence.md) and
[merge plan](merge-plan.md) already report — see those pages, and
[Patch Algebra and Merge Evidence](../reference/patch-algebra.md), for the underlying confluence
concepts.

```sh
prikk merge --allow-no-audit \
  --baseline-block BLOCK \
  --into REF \
  --from REF \
  [path]
```

- `--baseline-block` is required and names the sealed baseline block confluence is proven against.
- `--into` is the ref the merge advances. It must currently be published and must be the branch
  the caller has maintainer signing authority to seal.
- `--from` is the ref merged in. Its patches since the baseline are what get adopted. It may be a
  local branch, or a received ref (`remotes/<name>`, DC-85) imported by `prikk bundle import` — see
  [Merging from a received ref](#merging-from-a-received-ref) below.
- `--allow-no-audit` is required, matching `seal`'s own flag: this command signs and publishes new
  sealed history, and audit plugins are not implemented.
- The optional positional argument is the repository root.

## What a merge does — and does not do

**A merge authors nothing.** `--from`'s patches since the baseline are adopted **verbatim**: the
exact same canonical bytes, the exact same `ObjectId`, the exact same author signature as when they
were originally sealed. `prikk merge` never decodes, re-derives, or re-signs a patch. Only the new
`Block`, `RefState`, and `RefUpdate` are signed — with the maintainer key, exactly as an ordinary
`seal` signs them.

The two sides must be **proven confluent** from the given baseline — the same evidence
`merge-evidence`/`merge-plan` already compute, reused rather than duplicated. Any outcome other than
`Confluent` (`Conflict`, `Deferred`, `NotConfluent`, `Unsupported`, `OrderedDependency`,
`EvidenceFailure`, `InvalidCandidate`) refuses the merge. **Refusal writes nothing**: no object, WAL
record, or ref update of any kind is created until confluence is confirmed, so a refused merge leaves
`--into` exactly where it was.

**What cannot be merged yet.** The confluence proof judges each side's changes against the baseline;
several changes to one file on one side are judged together by their net effect (an edit run, edits
then a delete, a permission-change run, a binary-replacement run). Some ordinary histories still refuse:
- **A side containing a rename** (`prikk mv`) is not merged yet, whether or not the renamed file was
  also edited. The proof cannot replay a rename, so it refuses as `unsupported_operation` (RFC 144's
  designed deferral) rather than guess how a rename commutes with the other side.
- **A side that creates a file and then edits it** refuses as `sequence_internal_dependency_deferred`.
- **A side that mixes kinds of change on one file** (an edit, a permission change, another edit), **or
  creates a file and later deletes it**, refuses as `pair_replay_failed`.

None of these is damage: the repository verifies clean, and the refusal is a precondition.

## What gets recorded

**Merge blocks are `BlockKind::Merge`, naming both parents (DC-75).** `parent_block_ids` holds
`--into`'s prior tip and `--from`'s adopted tip, sorted per the format's uniqueness invariant. A
separate `mainline_parent_id` field names which one is `--into`'s side, since sorted order carries no
positional meaning. State derivation and replay follow the mainline parent only — the same shape as
an ordinary single-parent block — while the secondary parent's own chain is verified independently by
the ordinary full-object-store scan every other block already gets.

**The baseline is recorded, and independently re-derived.** `merge_baseline_block_id` states what
`--baseline-block` was at seal time — a claim, not a trust boundary: ordinary `verify` computes the
true merge base itself (a full-parent reachability walk) and reports disagreement if the recorded
baseline is not it. Authorship is unaffected (the adopted patches still carry their original author's
signature).

This discharges the release condition DC-74 attached to this command: sealed history now structurally
records a merge, re-checkable by a later verifier from sealed history alone.

## Merging from a received ref

`--from` accepts `remotes/<name>` — a ref imported by `prikk bundle import --input FILE` (produced on
the other side by `prikk bundle export --ref REF --output FILE`) — exactly as it accepts a local
branch. `--into` never does: it must always be a genuine local branch, since publishing a ref only ever
writes the local ref store.

**Adopting content from a received ref requires the maintainer key that sealed it to already be
trusted here.** Received content arrives via `import_bundle` with no trust check at all — deliberate,
per DC-78 Stage 3: importing is not trusting. A local-to-local merge needs no equivalent check, because
every block reachable from a local ref was itself created through this repository's own `seal`/`merge`
path, each already gated by trust at creation. A received ref's blocks were never gated on the way in,
so `prikk merge` checks them itself, before `--into` advances: every block it would adopt must carry a
signature from a currently-adopted maintainer key, or the merge is refused with `no trusted MAINTAINER
signature` and writes nothing.

**If you meet that refusal, do not treat it as an error to clear.** Running `prikk trust maintainer add`
for whatever key the bundle happened to carry is exactly the decision this check exists to make you
take deliberately, not by reflex. Trusting a maintainer key means trusting every block that key has
ever sealed or ever will — confirm you mean to extend that trust to this specific origin before adding
it, the same judgment call `trust maintainer add` already asks of a purely local setup.

## Conflicts

Detection only. A resolution is itself a signed patch — a trust question `prikk merge` does not
decide. `patch_algebra`'s conservative subset (DC-16, DC-18) governs what can be proven confluent at
all; if it is too narrow to merge something that should be mergeable, that is its own finding, not
something this command works around.

## Compatibility

`verify`, `doctor`, `rollback-preview`, and DC-64's incremental lifecycle cache all continue to work
against a repository containing a merged block — tested, not argued
(`crates/prikk-cli/tests/dc74_merge_execution.rs`).

## Deferred

- Automatic merge-base discovery — `--baseline-block` stays explicit.
- Widening `patch_algebra`'s conservative subset.
- Merging more than two sides in one command.

**Conflict arbitration / resolution is not on this list.** "Deferred" means eventually built;
automatic conflict resolution is refused by the architecture and will not be built at all — see
[Conflict Resolution Is Refused By
Design](../reference/patch-algebra.md#conflict-resolution-is-refused-by-design).

**`PatchPayload.parent_patch_ids` is not on this list either.** It was removed at Patch schema 2
(`0.24.0`), not left unpopulated — there is no patch DAG because the field no longer exists, not
because it is inert. `BlockPayload.parent_block_ids`, which DC-75 records, answers a related but
different question: block-level parentage, not a patch DAG.
