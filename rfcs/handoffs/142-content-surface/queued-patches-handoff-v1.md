# RFC 142 §7a / RFC 140 — a queued patch says why, and `show` can address it

**Live 2026-09-13**, on stikk's letter 011 (measured by them on 0.41.0; reproduced by the architect on the
tree). One round, one commit; after RFC 151's follow-up, before RFC 151 increment 2 or beside it.

## 1. The queue entry carries the message (RFC 140, additive within `status-report-v1`)

`worktree_status.rs:490-501` already decodes each active-WAL record's envelope for its operations; the
same envelope carries the message `log` prints (`decode_patch_message`, RFC 123). `QueuedPatchEntry`
gains `message: Option<String>` — the decoded message, `None` where the patch has none, exactly as `log`
distinguishes. JSON: `"message": "…" | null` after `"patch_id"`; prose: the message line under the id when
present. `#[non_exhaustive]` already applies to `QueuedPatchEntry` (RFC 147 ruling 2) — confirm.

## 2. `show` addresses a queued patch (RFC 142 §7a)

`show.rs:181`: a miss in the object store is `Integrity("no object …")` for every id, including one the
user typed wrong and one `status` just printed as queued. Two changes, one lookup:

- **When the object store misses, look in the active WAL** — the same records the queue enumeration
  reads. A queued patch renders as the same `show-report-v1` it will render as once sealed, plus one
  additive field `"queued": true` (false for a stored object) and the prose line `queued: yes` — a
  front-end can offer the queue's content, not only its paths.
- **A miss in both is `Precondition`**, naming both places looked and the route: *"no object <id> in the
  object store or the active WAL; `prikk status` lists queued patch ids, `prikk log` sealed ones"*. An id a
  user supplied that resolves nowhere is caller-fixable, not damage (RFC 132; RFC 147 §2d). An object a
  *ref* names and the store lacks stays `Integrity` — that path is `verify`'s, not this one's.

## 3. Controls

- Commit with `-m` → `status --format json` entry has the message; commit without → `null`; prose shows
  the line only when present.
- `show <queued id>` → rc 0, `show-report-v1` with `"queued": true`, operations equal to the queue
  entry's; `seal` → `show` the same id → `"queued": false`, content byte-identical apart from that field.
- `show <unknown id>` → rc 1, `precondition not met:` with both places and both routes; **no
  `integrity error:`** in stderr (assert absence).
- The RFC 142 agreement/perturbation shape: flip the miss back to `Integrity` → one test fails; drop the
  WAL lookup → the queued `show` test fails.
- Full gate set; the isolation seam; addendum from the diff (none expected — say so).
- Docs: `commands.md` synopsis unchanged; `show` and `status` reference pages gain the field; the
  `troubleshooting.md` entry for `no object` carries the old wording; CHANGELOG `### Added` (message,
  queued `show`) and `### Changed` (the miss is a precondition), naming stikk's letter 011.
- Report: `.git-exclude/review-request/rfc142-queued-patches-report-v1.md`.
