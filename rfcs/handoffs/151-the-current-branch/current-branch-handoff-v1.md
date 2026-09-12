# RFC 151 — the current branch: handoff v1

**Live 2026-09-13.** RFC 151 accepted with its three questions read as yes (status line says how). Two
increments, each its own commit and report; increment 2 does not start until 1 is reviewed.

## Increment 1 — the pointer and the default

1. `.prikk/current-branch`: one line, `heads/<name>\n`. Written by `init` and `setup` as `heads/main`,
   through the same atomic write every other repository file uses. Read by one function in the store
   (`current_branch(layout) -> Result<String>`): missing file → `heads/main` (repositories older than this
   RFC); present but not a valid local branch ref name → `Precondition` naming the file; naming a ref that
   does not exist or is closed → `Precondition` naming the ref and `branch switch`/`branch create`. **Never
   read by `verify`, trust, signing, `bundle`, `sync`** — assert with a grep-shaped test over the store's
   production tree that only the CLI's default-resolution path and `doctor` call it.
2. Every `--ref` default that is `heads/main` or `DEFAULT_CHECKOUT_REF` today resolves through it (list
   the sites in the report; expect `commit`, `seal`, `log`, `checkout` ×5, `worktree-status`, `bundle
   preview`, `history`, and whatever else the grep finds). `--ref` given explicitly is untouched.
3. `doctor` reports a pointer naming a missing or closed ref, as an issue with the two routes.
4. Visibility: `worktree-status` and `log` print the branch they resolved (prose) and carry
   `"current_branch"` in JSON where they already carry the ref name; `branch list` marks the current one
   (`*` in prose, `"current": true` in JSON). The `--help` note is unchanged until increment 2.
5. Controls: fresh repository → pointer present, `heads/main`; pre-RFC repository (fixture without the
   file) → every default still `heads/main`, no file written by a read; malformed file → the refusal; a
   pointer naming a closed branch → the refusal; each default site perturbed (point the file at
   `heads/other`, assert the command resolved `heads/other`). Isolation seam on every test. CHANGELOG
   `### Added`. Docs: `first-run.md` names the file once; `commands.md` unchanged.

## Increment 2 — `prikk branch switch heads/<name>`

1. Refusals, in order, each `Precondition` with the route: target not an existing local branch ref or
   closed; active WAL non-empty and owned by another ref (`ActiveRefOwnership`); worktree not clean
   against the current branch's baseline (reuse `worktree_status`; name the paths).
2. Then the transition: compute the target baseline (the replay baseline `checkout --patch-plan` already
   derives); create/modify through the existing safe materializer; **delete a tracked file only when its
   bytes equal the current baseline's** (which, after the clean check, is every tracked file the target
   lacks); untracked files never touched; refuse symlinked parents as the materializers do. Write the
   pointer **last**. Crash posture: interrupted before the pointer write, the worktree is a superset of
   both baselines and the pointer still names the old branch; `worktree-status` then shows the extra files
   as the difference — never a torn file. State the failpoint(s) used to prove it.
3. `--help`'s note is replaced by the synopsis; `commands.md` gains the line; `troubleshooting.md` gets the
   three refusals; CHANGELOG `### Added`.
4. Controls: switch between two branches with disjoint files → files appear and disappear, pointer
   updated, `verify` clean; dirty worktree → refused, nothing written, pointer unchanged; unsealed work on
   another ref → refused; closed target → refused; a tracked file *modified* on disk that the target lacks →
   the dirty refusal fires first (assert order); crash at each failpoint → the posture above; the
   full gate set; cross-target from the diff (materialization touches `fsutil`, so run both).
