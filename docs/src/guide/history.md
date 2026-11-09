# History Inspection

PR-014 added a small read-only history view for early sealed repositories. PR-030 extends that view with rollback block classification.

```sh
prikk log [path] [--limit N] [--ref REF] [--format json]
```

The command follows the current `RefState` chain from newest to oldest and validates that each entry resolves to a persisted Block object.

`--ref` takes any published ref, not only a branch. A tag ref resolves through its tag object to the
Block that object names — so `prikk log --ref tags/v1` reports the tagged Block, which need not be
the branch tip. `--ref` does not take a bare block id: a block can have more than one parent, so `log`
has no single next entry to follow, and it refuses (``<id> is a block id; `log` does not show history
ending at a block …``, exit `1`) instead of printing an empty history. A value that is neither a ref
name nor a block id refuses too (`invalid name: … is not a point this command reads`). `prikk show`,
`tree` and `cat` read a block.

For each entry, the CLI reports:

- the target Block ID;
- RefState ID and update sequence;
- Block kind;
- parent and Patch counts;
- rollback block classification;
- rollback-marked Patch count;
- required attestation count;
- previous RefState ID.

Nothing in that list is a time: signatures and RefUpdates have a `created_at` field, every current writer sets it to 0, and so no block, patch or ref record carries one. The entries are ordered by update sequence, not by date.

**`--format json`** (`log-report-v1`) carries the same facts per block, plus two fields added in 0.57.0,
additive within the version (a consumer written against the older field set reads these documents
unchanged):

- **`parent_block_ids`**: the Block's own parents, as stored (the same field `sync` claims and `extract`
  links call `parent_block_ids`), `[]` for a root block. They are stored **sorted by object id**, so the order
  means nothing: a merge has no "first parent" to read from it, and a consumer must not infer one
  (`mainline_parent_id`, not the order, names the mainline). `parent_count` is kept and equals the length.
- **`sealed_by`**: the MAINTAINER key ids the Block's seal records, in stored order. It is **as recorded, not
  as trusted**: it lists the keys whether or not this repository has adopted them, so it reads the same
  before and after `prikk trust maintainer remove` of a sealer. A block sealed by two keys (a superseding
  record, RFC 156) lists both. `bundle preview`'s `sealed_by` is the same list, built by the same function.
  Which of those signatures verify against this repository's adopted keys is a different question, answered
  by `prikk verify` and, for a delivery, by `bundle preview`'s `sealed_by_signatures`.
  **It lists the Block's own signatures.** `prikk verify --format json`'s `block_seals[].sealed_by` is the
  same list from the same function; `bundle preview`'s `sealed_by` lists the **delivered tip RefState's**
  signatures instead, and the two can differ (another signer's copy of a stored Block merges as a
  superseding record carrying the union, and the local RefState gains nothing).

A Block is classified as a rollback Block when it references at least one Patch envelope carrying the current development rollback marker and that Patch payload decodes under the supported replay subset.

History inspection does not yet perform full block-DAG traversal, path-aware history queries, rollback authorization checks, or patch algebra.
