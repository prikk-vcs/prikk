# A declaration's destination is "present" by one definition, not by each caller's

**Live 2026-09-16**, from stikk's letter 014
(`.git-exclude/upstream/stikk/receive/014-a-directory-at-the-destination-resolves-rename-and-commit-records-a-deletion.md`).
Rulings recorded in RFC 147 §2g. **Ships in 0.44.0.** Order: after the round the owner has already handed you;
nothing is pushed until the architect cuts 0.44.0.

## 1. Measured by the architect

Released 0.43.0 asset, fresh repositories; `a.txt` sealed, then `prikk mv a.txt b.txt`, then:

| destination | `worktree-status` resolution | `commit` |
|---|---|---|
| `b.txt` replaced by an **empty directory** | `rename` | `delete-file a.txt`; *"declaration a.txt -> b.txt: destination is ignored; recorded as a deletion, not a rename"* |
| `b.txt` replaced by a **directory holding `q.txt`** | `rename` | `delete-file a.txt`, `create-file b.txt/q.txt`; the same "destination is ignored" line |
| `b.txt` listed in `.prikkignore` (control) | `deletion-ignored` | `delete-file a.txt`; the same line — correct here |

**Cause, at source.** `declaration_resolution.rs::resolve_one` is shared, but it asks each caller *"is this path
present?"* through a closure, and the callers answer differently:

- `worktree_status.rs:302-313`: present = `symlink_metadata` succeeds **for any entry kind**, and not ignored. A
  directory is present, so it resolves `rename`.
- `node_authoring.rs:471-…`: present = in commit's **regular-file** worktree walk. A directory is absent, so
  `resolve_one` takes the "not present" branch, where `symlink_metadata(on_disk).is_ok()` is read as *"it must be
  ignored"* (`:158-167`). That produces `deletion-ignored` and the `.prikkignore` message about a directory nobody
  ignored.

**RFC 147 §2f let each caller pass its own view, and its review verified parity only on regular files.** That is
the architect's miss, recorded in §2g.

## 2. Ruled (RFC 147 §2g)

1. **One presence definition, inside the classifier.** `resolve_one` decides presence itself, by one predicate
   both callers share. It must be the **same entry classification commit's worktree walk uses** — reuse it, do
   not restate it. Callers pass the baseline and the ignore rules, never a presence closure.
2. **A directory at the destination resolves `deletion`.** Commit's disclosure names the real cause —
   *"destination is a directory; recorded as a deletion, not a rename"* — not the ignore message. Commit's authored
   operations do not change.
3. **`deletion-ignored` only when the ignore rules actually exclude the destination.** It is decided from the
   rules, never inferred from "on disk but missing from the walk".
4. **No new resolution value** — this is a patch release. The documented meaning of `deletion` widens to "the
   destination is gone, or is no longer a file (a directory stands there)".
5. **Symlinks and special files: measure first, then keep or report.** Today a symlink at the destination resolves
   `rename`, and commit refuses over the path; stikk confirmed that reads correctly. If the unified predicate would
   change it, or a FIFO or socket behaves differently from a directory, **stop and report** before choosing.

## 3. Controls (each must be able to fail)

1. **A parity matrix across entry kinds at the destination**, with status's resolution and refusal compared with
   what `commit` then does and says:
   - a regular file;
   - an empty directory, and a directory holding a file;
   - a symlink to a file, and a dangling symlink;
   - a FIFO (Linux-gated);
   - an ignored file, and an ignored directory.

   Perturb by restoring a caller-supplied presence closure for one caller, and it must fail.
2. **The disclosure text** for a directory, asserted from commit's output, and absent the word "ignored".
3. **Every existing RFC 147 §2f control still passes**, including the two-node swap and the routes each refusal
   names.

## 4. Docs, CHANGELOG, report

- **Docs:** `docs/src/guide/worktree-status.md` and `docs/src/guide/patches/declared-move.md`, where `deletion` is
  defined, gain the directory case.
- **CHANGELOG:** `### Fixed`, naming stikk's letter 014 — status reported `rename` for a directory at the
  destination, and commit called it ignored.
- **Discipline:** the full gate set on the final commit; never push.
- **Report:** `.git-exclude/review-request/declaration-presence-report-v1.md`.

## 5. Rule 5, ruled — RFC 147 §2h (2026-09-16)

Your stop was right. **Option (A)**, with your recommendation taken whole:
- **FIFO and socket like a symlink:** `rename`, and commit refuses over the path.
- **`content_changed`/`mode_changed` are `null` whenever the destination is not a regular file**, symlinks
  included. Say so in the docs, the CHANGELOG `### Changed`, and the JSON field description.
- **Never open a non-regular destination.** One non-following stat decides presence and kind, and bytes are
  read only for a regular file.
- **§2g rules 1–4 unchanged.**

**Controls, added to §3:**
- FIFO and socket rows in the parity matrix, each under a timeout, so a regression **fails** rather than hangs.
- A control that the hang is gone: `worktree-status` on a FIFO destination returns within the timeout. Perturb by
  restoring the unconditional read, and it must fail by timeout.
- The symlink row's `content_changed` is asserted `null`.

**CHANGELOG `### Fixed`** also carries the hang, as a 0.43.0 defect. This round ships in 0.44.0, **on top of the
round you have already committed**. Nothing is pushed until the architect cuts 0.44.0.

