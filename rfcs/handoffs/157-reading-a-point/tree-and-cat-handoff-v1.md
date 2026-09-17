# `prikk tree` and `prikk cat` — handoff v1

**Order of work for 0.46.0 (the three handoffs of 2026-09-17):**
1. `153-content-diff/point-resolver-handoff-v1.md`;
2. `157-reading-a-point/tree-and-cat-handoff-v1.md` (this one);
3. `153-content-diff/diff-handoff-v1.md`.

Take them one at a time, each reported and reviewed before the next. **This round starts after the resolver round is
reviewed**, and uses that resolver: no second one.

**Live 2026-09-17.** RFC 157 (`rfcs/accepted/157-reading-a-point.md`) is the specification: §3 `tree`, §4 `cat`, §5
failures, §6 security, §7 controls. Read it first. This handoff orders the work and names what the RFC leaves to
implementation.

## Stage 1 — `prikk tree`, then stop only if the cost stop fires

1. **Build `tree`** per RFC 157 §3:
   - `tree-listing-v1`: leaf paths in canonical order;
   - `kind` `file` (and `symlink` only when replay supports symlinks; until then a symlink operation fails the call);
   - `encoding` `text`/`binary` by checkout's own classification, **reused, not re-derived**;
   - full `mode`, exact `size`, and `content_id` for binary only;
   - `--prefix` by whole path components;
   - the implicit current branch, following 0.45.0's rules.
2. **Exact text sizes.** Measure `tree` against `checkout --patch-plan` on RFC 139's corpus at depths 64 and 256, both
   binaries built the same way. **If `tree` costs materially more than the replay** (above 1.25× the `--patch-plan` wall
   time at either depth), **stop and report** with the figures before continuing. Do not approximate sizes to make it
   fit.
3. **Add a `tree` row** to the RFC 133 memory instrument.

## Stage 2 — `prikk cat`

Per RFC 157 §4.
- **Bytes.** Fully resolved before anything is written.
  - `--max-bytes` refuses with nothing written: a binary blob is checked against its stored size before its bytes are
    read, text after reconstruction.
  - `--output` goes to a temporary sibling and is renamed into place. It refuses an existing file without `--force`,
    and a path inside `.prikk/`.
- **A terminal.** Binary content refuses a stdout that is a terminal (`std::io::IsTerminal`, stable within the 1.85
  MSRV), naming `--output`, with nothing written. Text is written.
- **`--format json`** writes `path-content-v1` and **no bytes**. Build it from the same entry code as `tree`, so the
  two cannot disagree.
- **Absence.** An absent path, and a directory prefix, refuse with "path <p> does not exist at <point>".
- **`crates` forbid `unsafe_code`.** Do not open a pseudo-terminal through FFI:
  - hold the terminal decision behind one function with a test-support seam, and control the decision there;
  - on Linux, add one CLI control through util-linux `script -qec` when it is present, skipped with a printed reason
    when it is not;
  - say what macOS and Windows cover.

## Controls

RFC 157 §7, all of them, each shown failing under a perturbation of the subject:
- the bound checked after writing;
- the terminal check skipped;
- a non-atomic `--output`;
- `content_id` emitted for text;
- `--prefix` matched by string prefix;
- `tree` and `cat --format json` computed separately (perturb the sharing).

**For the killed-process `--output` control,** use the existing failpoint mechanism, and count its ordinals against
unrelated writes. **Paths:** any printed absolute path is compared by identity (`support::assert_same_path`), never as
text.

## Docs and CHANGELOG

- `docs/src/guide/tree-and-cat.md`, `commands.md` and both synopses.
- `git-mapping.md`: `git ls-tree` → `prikk tree`, and `git show <rev>:<path>` → `prikk cat`.
- CHANGELOG `### Added — prikk tree` and `### Added — prikk cat`, with both schema names.

## Reports

- **Stage 1**, and the cost stop if it fires: `.git-exclude/review-request/tree-report-v1.md`.
- **Stage 2:** `.git-exclude/review-request/cat-report-v1.md`.
