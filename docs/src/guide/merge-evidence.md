# Merge Evidence

DC-22 (0.15.0) adds `prikk merge-evidence`, the first public read-only UX over the DC-21
merge/conflict evidence vocabulary. DC-23 (0.16.0) stabilizes its text output.

For the current concepts behind operation ordering, commutation, confluence, evidence outcomes,
reason codes, and proof phases, see
[Patch Algebra and Merge Evidence](../reference/patch-algebra.md).

For the planning classification layer over the same explicit-input evidence, see
[Merge Plan](merge-plan.md). Neither command executes a merge — for that, see
[Merge](merge.md) (DC-74), which reuses this same evidence to decide whether to seal.

```sh
prikk merge-evidence \
  --baseline-block BLOCK \
  (--left-block BLOCK | --left-ref REF) \
  (--right-block BLOCK | --right-ref REF) \
  [path]
```

The command derives the single-parent candidate sequences from an explicit sealed baseline to two
explicit left/right targets, runs the read-only merge/conflict evidence analysis, and prints the
resulting report.

Selector rules:

- `--baseline-block` is required and names the sealed baseline block.
- Each side must choose exactly one selector: `--left-block` or `--left-ref`, and `--right-block`
  or `--right-ref`.
- A `--left-ref`/`--right-ref` value may name a received ref (`remotes/<name>`, DC-85), previewing
  evidence against imported history exactly as it would for a local branch. This is read-only, so a
  preview can name a `remotes/` ref on either side even though `prikk merge` itself never accepts one
  as `--into` — the preview does not claim the plan it shows is executable as-is.
- The optional positional argument is the repository root, as with other commands. It is not a path
  filter.

The command is read-only. It does not infer merge bases, execute merges, publish merge commits, write
refs or WAL records, materialize worktree conflicts, or persist proof/witness objects.

## Output

Output is text-only and intended for human diagnostics. It is not a durable machine-readable schema.
DC-23 makes the shape easier to scan:

```text
merge evidence
baseline block: <block-id>
left selector: ref heads/topic-a
left target block: <block-id>
left operations: 3
right selector: block <block-id>
right target block: <block-id>
right operations: 2
outcome: Conflict
reason: pair_conflict
items: 1 displayed of 1

cross:
  left[0] op_seq=1 ChangePerm src/lib.rs
  right[0] op_seq=1 ChangePerm src/lib.rs
  outcome: Conflict
  reason: pair_conflict
  phase: classification

note: read-only evidence; no merge commit, ref update, WAL write, or worktree change was performed
```

Reading the output:

- both sides show the submitted selector text and the resolved target block identity;
- left and right operation counts are shown separately;
- the full-report `outcome` and `reason` are shown before item details, and are computed over the
  full candidate sequences;
- `items: N displayed of N` reports the displayed and total item counts (equal in this release; DC-23
  adds no display filtering);
- cross-side items render as a `cross:` block with separate `left[...]` and `right[...]` operation
  lines, rather than an ambiguous one-line form;
- report-level items render as `report:` without a fake operation label;
- DC-21 outcome and reason-code names are preserved exactly;
- a `RenamePath` operand's line also carries `asserted-by=<key-id>` — the patch's AUTHOR signature
  key id. This is structural, not a rendering choice: the underlying read type cannot represent a
  rename without its asserting signer, so a patch whose AUTHOR signature is missing fails the
  command rather than showing a rename with no signer.

Privacy: the output never includes raw text spans, replacement text, blob bytes, absolute host paths,
`.prikk` private paths, signer secrets, key material, or arbitrary object debug dumps. Displayed paths
are repository-relative.

## Exit Status

| Condition | Exit |
|---|---:|
| Valid request and a DC-21 evidence report was produced, for any outcome | 0 |
| Invalid CLI arguments, missing selectors, or ambiguous selectors | 1 |
| Selector, ancestry, object, or ref failure prevents building the report | 1 |
| Unexpected internal error | 1 |

Process success is independent of the evidence outcome: a produced report exits `0` even when the
outcome is `Conflict`.

## Deferred

**`prikk merge` (DC-74) executes confluent merges** — see the [merge guide](merge.md); this command
stays read-only regardless. Still deferred:

- conflict resolution;
- automatic merge-base discovery;
- branch merge semantics beyond a two-sided confluent merge;
- display-path filtering and scoped/path-limited merge analysis;
- persisted proof/witness/merge-evidence objects;
- JSON or other machine-readable output;
- public `prikk-replay` API stabilization.
