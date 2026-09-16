# The exporter supplies a deletion's preimage, and rollback can seal it — v2

**Live 2026-09-16.** Supersedes item 1 of
`export-of-an-edited-then-deleted-file-handoff-v1.md` (items 3 and 4 there are delivered at `ac5e530d`).
The v1 round stopped correctly at its own stop clause and proved shape (a) insufficient; review:
`.git-exclude/reviewed/export-of-an-edited-then-deleted-file-review-v1.md`.

**Order:** this round, then the declarations round (RFC 147 §2f). **0.43.0 does not cut without both.**

## 1. One root cause, three symptoms

A text file whose content arrived by `EditText` has no stored content Blob (DC-65). When such a file is
deleted, the deletion's preimage names a Blob id nothing stores. Measured on a binary from `ac5e530d`
(create, seal; edit, seal; delete, seal):

| command | result |
|---|---|
| `verify` | exit 0 |
| `bundle export --ref heads/main --output <f>` | exit 1, `integrity error: missing blob object: …` |
| `sync build <ref> --have <earlier have-list> --output <f>` | exit 1, the same |
| `rollback-draft --append-inverse -m …` then `seal` | exit 1, `integrity error: lifecycle replay: blob … required for a state effect is missing` |

The third is new in review and is **not** an exchange defect: undoing the deletion fails locally, because
the inverse is a `CreateFile` naming that Blob (`lifecycle_cache/replay.rs:146`).

## 2. Ruled

**Refused: making the preimage optional** in `bundle.rs::bundle_referenced_blob_ids`,
`accept.rs::referenced_blob_ids` and the offline check. Every released prikk requires that Blob, so
omitting it produces artifacts no shipped version can import — an interoperability break for stikk and
planeter — and the rollback finding shows the bytes are not unused after all.

**Ruled: whoever needs the Blob derives it.**

1. **Exporters** (`bundle.rs` export closure `:677-692`, `patch_exchange/artifact.rs:145-159`): carry the
   preimage Blob when the store holds it; otherwise **derive the content by replay and carry it in the
   artifact**. Export **writes nothing locally** — it is read-only, and stays so.
2. **The rollback seal**: when an appended inverse names a content Blob the store lacks, **derive and
   store it** at seal time. Sealing is a writer, and RFC 136 §10.5 1b already ruled that storing exactly
   this Blob (`text_blob_id(content)`) is additive.
3. **Both derivations share one function.** Replay already materializes these bytes
   (`patch_replay/anchor.rs:74`'s `files` map); do not write a second content reconstruction.
4. **A derived Blob must hash to the id the patch names.** If it does not, that is an `Integrity` error
   naming the block and the path — a real corruption signal, not something to paper over.
5. **Receivers are unchanged.** Closure stays "every referenced id resolves". A bundle from the fixed
   exporter imports into 0.42.0 as well as into this build; a control shows that.

## 3. Required

1. The shared derivation, anchored at the nearest checkpoint (RFC 136) rather than replaying from genesis.
2. The two exporter sites use it; the rollback seal path uses it.
3. **Measure export at depth on the RFC 139 corpus**, before and after, at depth 256, three samples.
   **Stop and report if export regresses beyond 2×** rather than shipping a slow exporter. Say what the
   cost is either way; the CHANGELOG names it if it is visible.
4. The four copies of the referenced-blob match (`bundle.rs:1211`, `accept.rs:357`, `artifact.rs:145`,
   and the export closure) are not unified by this round unless the fix needs it — if it does, say so and
   do it once.

## 4. Controls (each must be able to fail)

1. **Export, edit then delete.** Store test: the bundle carries a Blob for the deletion's preimage, and
   its id is the one the patch names. Perturb by skipping the derivation: the export fails as it does today.
2. **`sync build`** across a have-list from before the edit: the artifact carries it.
3. **Receiver, unchanged code path.** Import that bundle into a fresh repository and `verify`; `sync
   accept` the artifact. Both succeed, the history replays, and `show` of the deletion renders the content.
4. **Old-version interoperability.** A bundle from this build imports into a 0.42.0 binary (a release
   build is enough). This is the control that would have caught the refused shape.
5. **A stored preimage still travels**, unchanged: create a binary file, seal, delete, seal, export.
6. **Rollback.** On the history above: `rollback-draft --append-inverse` then `seal` succeeds, and a
   checkout materializes the restored file with its original bytes. Perturb by removing the derive-and-store
   step, and the seal fails with today's message.
7. **A genuinely corrupt history still refuses:** a patch naming a blob id that replay's content does not
   hash to gives an `Integrity` error naming the block.

## 5. Docs, CHANGELOG, report

**Docs:** `docs/src/guide/backup-restore.md` and `docs/src/guide/sync.md` say what an artifact carries for
a deletion, now that it is always complete; `docs/src/reference/data-model.md` if it states the rule.

**CHANGELOG:**
- `### Fixed`: export and sync build of a history that deletes a previously edited text file — broken in
  every release since 0.28.0;
- `### Fixed`: sealing the inverse of such a deletion;
- `### Changed`: only if export cost changes measurably.

**Discipline:** fmt, clippy with `-D warnings` gated on its exit code, workspace tests, and the staged list
before every commit; the full gates on the final commit, both cross-target clippies included. Never push,
tag or publish.

**Report:** `.git-exclude/review-request/export-of-an-edited-then-deleted-file-report-v2.md`.
