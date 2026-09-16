# Five follow-ups to the deletion-preimage derivation

**Live 2026-09-16**, from the review of the v2 round
(`.git-exclude/reviewed/export-of-an-edited-then-deleted-file-review-v2.md`). The fix itself is accepted
and pushed (`1df0be33`, `5cc9138a`); these are its edges.

**Order:** after the declarations round (RFC 147 §2f). **0.43.0 does not cut without this** — item 4 is a
false statement in shipped documentation, and item 2 reports corruption as something else.

## 1. A merge history does derive, and a control must keep it true

The v2 report states that a merge history derives nothing because `single_parent_chain` refuses a fork.
**It is not so, measured by the architect:** `single_parent_chain` follows a `Merge` block's **mainline**
parent (`patch_replay/read.rs:37-53`), and a history that edits and deletes a file on `heads/main` and
then merges `heads/dev` into it exports cleanly (`objects: 18`).

- Add a control at the level the others live: a merged history whose mainline deleted a previously edited
  file exports, imports and verifies.
- Perturb by skipping the derivation, and it must fail.
- If some *other* merge shape genuinely cannot derive (a side-branch deletion the mainline walk never
  applies), **find it, state it, and control it**; do not restate the refuted claim.

## 2. A chain error is not "nothing to derive"

`derive_deleted_content` does `let Ok(chain) = read::single_parent_chain(…) else { return
Ok(BTreeMap::new()) }`, so a cycle (`Integrity`), an unreadable block, and an unsupported shape all end
as "derived nothing", and the caller then says `missing blob object: <id>`.

- Propagate a real failure. Only the **unsupported-shape** case (`UnsupportedObjectType`) may fall back to
  deriving nothing, and even that is worth saying in the resulting refusal.
- A control: a chain whose parent is absent gives the chain's own error, not a missing-Blob message.

## 3. Ask the index whether an object exists

All three sites probe with `read_typed(id, Blob)?.is_some()`, which reads the whole object.
`ObjectReader::has_object` (`object_store.rs:43`) answers from the index, and RFC 136 increment 1b added it
saying *"checking a snapshot's content Blobs must not read every file's bytes"*.

- Use it in `bundle.rs::export_bundle`, `patch_exchange/artifact.rs::export_exchange_artifact` and
  `block_state.rs::store_derived_content_for_candidate`.
- **The seal path is the one that matters:** every `CreateFile`'s Blob is currently read in full on every
  seal, which is new cost on an ordinary commit-and-seal. **Measure seal before and after this change** on
  the corpus at one depth (three samples), and report the number even if it is small.

## 4. A bundle receiver cannot roll back, so the page must not say it can

`docs/src/guide/backup-restore.md` now says the carried content means *"a receiver can read and roll back
that deletion"*. Measured: `rollback-preview --ref remotes/heads/main` and `inverse-plan --ref
remotes/heads/main` both exit 1 with `ref remotes/heads/main is not published`.

- Say what is true: the content is there to read, and to roll back once the history is local.
- `docs/src/guide/sync.md` is correct as written (`sync seal` makes the patches local blocks) — check it
  again rather than assuming.

## 5. The §4.7 control, at the layer that refuses

Dropping the CLI-level corruption attempt was right. The refusal lives in the delete arm's
`ensure_blob_matches_node_kind`, so the control belongs there: give `apply_decoded_operation` a
`DeleteNode` whose `old_blob_id` does not match the bytes in `files`, and require the `Integrity` error.
`patch_replay/tests/node_ops.rs`'s `patch_bytes` builds such an operation already. Perturb by removing the
check, and it must fail.

## Not in this round

**The derivation's pass shape stays anchored-first** (review ruling 2). The strictly better shape — decide
from each wanted deletion's block position whether to run the anchored or the full pass, and run exactly
one — is recorded there for whoever next touches it. If item 3 makes it cheap to try, measure it and
report; do not adopt it silently.

## Docs, CHANGELOG, report

**Docs:** item 4. **CHANGELOG:** nothing unless item 3 changes a measured cost, or item 2 changes a
user-visible message — then `### Changed`, naming it.

**Discipline:** fmt, clippy with `-D warnings` gated on its exit code, `cargo +1.85.0 check` in the
pre-commit run, workspace tests, and the staged list before every commit; the full gates on the final
commit, both cross-target clippies included. Never push, tag or publish.

**Report:** `.git-exclude/review-request/derivation-follow-ups-report-v1.md`.
