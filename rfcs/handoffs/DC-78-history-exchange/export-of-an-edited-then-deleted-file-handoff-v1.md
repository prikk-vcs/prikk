# `bundle export` and `sync build` fail on any history that deletes a file it edited earlier

**Live 2026-09-16**, found by RFC 136 increment 3's measurement, reported and held; reproduced
independently by the architect. Review:
`.git-exclude/reviewed/rfc136-increment-3-review-v1.md`.

**Order:** this round, then the RFC 147 declarations round
(`147-refusal-visibility/declarations-say-what-commit-does-handoff-v1.md`). **0.43.0 does not cut without
both.**

## 1. The defect

A file-kind `DeleteNode` carries its preimage's `old_blob_id`. The exporter inserts that id into the blob
set (`crates/prikk-store/src/bundle.rs:677-683`) and then `read_required`s every id (`:690-692`, failing at
`:1176`). `crates/prikk-store/src/patch_exchange/artifact.rs:145-159` does the same for `sync build`.

**A text file whose content arrived by `EditText` has no stored content Blob** (DC-65: an edit carries a
span, not the file). So a deletion's preimage Blob is legitimately absent, `verify` passes, and `show`
already renders it as `<unavailable blob …>`. Export and sync build are the only readers that treat it as
required.

**Measured by the architect** (binary at `47fb6def`, fresh repository, one trusted key): create, commit,
seal; edit, commit, seal; delete, commit, seal.

| step | result |
|---|---|
| `bundle export` after the create | exit 0, `objects: 5` |
| `bundle export` after the edit | exit 0, `objects: 8` |
| `verify` after the delete | exit 0 |
| `bundle export --ref heads/main --output <f>` after the delete | exit 1, `integrity error: missing blob object: b7a720a5…` |
| `sync build heads/main --have <have-list taken at the create state> --output <f>` | exit 1, the same id |

Masked where a checkpoint happens to store the content: with the edit made before the first seal, the
same delete-then-export exits 0.

**Introduced** with DC-78 Stage 3 (`f8366189`, 2026-08-10), so every release since has shipped it.
**Consumers:** stikk (CLI) and planeter (sync) both depend on export of ordinary histories.

## 2. Ruled: carry the preimage only when it is stored

**Shape (a), ruled.** Export and sync build include a deletion's preimage Blob **when the repository holds
it**, and omit it otherwise. An absent preimage is by design, not damage.

**Shape (b) — store the preimage at deletion time — is refused.** It would change what `seal` writes and
grow every repository to serve an exporter's convenience, for data replay does not need.

**Before the fix lands, establish on the receiver side** (`import_bundle`'s closure validation,
`validate_bundle_contents` at `bundle.rs:926` with its "present" definition in `BundleAndLocalReader`,
`bundle.rs:1179-1191`, and `sync accept`) whether any
of them require a deletion's preimage to be present. **If one does, stop and report** rather than loosen a
validation to make an exporter pass.

## 3. Required

1. Export and sync build insert a file deletion's `old_blob_id` only when the object store holds it.
2. The receiver side is checked as above, and whatever it needs is stated in the report — with a
   measurement, not a reading.
3. `crates/prikk-cli/src/bundle.rs:116`: the import note advises `merge`, which does not work in a fresh
   repository. `docs/src/guide/backup-restore.md` was corrected this round; the message must match it. Say
   what the import gave (verifiable history at `remotes/…`, no local ref advanced, no key trusted) and
   stop there. Do not advise a route that refuses; do not promise RFC 155.
4. **The percentage labels from increment 3** are ambiguous and must be fixed in this round:
   `docs/src/reference/data-model.md`'s table labels the checkpoint cost `added … (31 %)` and the
   `### Added — sealed snapshots` CHANGELOG entry says four checkpoints "add 31 % to a 256-block one".
   1,356,017 bytes is 30.6 % of the repository **with** checkpoints and **44.2 %** of the one **without**,
   and "adds 31 %" reads as growth. Say which total each share is of, at both depths (128: 11.4 % vs
   12.9 %). Numbers themselves are correct; only the labelling changes.

## 4. Controls (each must be able to fail)

1. **Edit then delete, exported.** A store test: create a text file, seal; edit it, seal; delete it, seal;
   `export_bundle` succeeds, and the bundle omits the absent preimage. Perturb by restoring the
   unconditional insert, and it must fail.
2. **The same for `sync build`**, against a have-list from before the edit.
3. **The receiver accepts it.** Import that bundle into a fresh repository and `verify` it; `sync accept`
   the artifact. Both succeed, and the received history replays.
4. **A stored preimage still travels.** Create a binary file (or a text file whose content is stored), seal,
   delete it, seal, export: the preimage Blob is in the bundle. Perturb by omitting it always, and this
   must fail.
5. **CLI-level:** one test at the command layer for export and one for sync build, since the store tests
   alone did not catch this (`bundle/tests.rs:309` exports an `EditText` history but never deletes).
6. **The import note** is asserted by a test against the text the page states.

## 5. Docs, CHANGELOG, report

**Docs:** `docs/src/guide/backup-restore.md` and `docs/src/guide/sync.md` if either states what a bundle
carries for a deletion; `docs/src/reference/data-model.md` for item 4.

**CHANGELOG:**
- `### Fixed`: export and sync build of a history that deletes a previously edited file — say that every
  release since 0.28.0 refused it;
- `### Changed`: the import note.

**Discipline:** fmt, clippy with `-D warnings` gated on its exit code, workspace tests, and the staged list
before every commit; the full gates on the final commit, both cross-target clippies included. Never push,
tag or publish.

**Report:** `.git-exclude/review-request/export-of-an-edited-then-deleted-file-report-v1.md`.
