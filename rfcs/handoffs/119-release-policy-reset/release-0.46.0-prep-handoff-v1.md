# Release 0.46.0 preparation — the dev team's part

**Issued 2026-09-22. The owner authorized the cut:** *"Approved."* **The architect's reading, stated so it can be
corrected:** the 0.46.0 cut is authorized, with the version, theme and scope below; **crates.io publication is
not** — that needs the owner's own word at publication time, as every release. Follow
`release-prep-handoff-template.md`; this file fills in the blanks.

| | |
|---|---|
| version | **0.46.0**: a minor, because in v0 any runtime change is (`docs/src/reference/release-compatibility.md`) |
| theme | **"comparing"**: read a point (`prikk tree`, `prikk cat`) and compare two states (`prikk diff`, two points or the worktree), over one shared point resolver that also accepts a bare block id |
| CHANGELOG date | the day the release commit is prepared; if it slips past midnight JST, stop and ask |
| last tag | `0.45.0` at `27fcbd76` |
| memory ratio (template §1.6) | against `.git-exclude/measurements/rfc133/node-count-memory-measurement-27fcbd76db6c.md` (0.45.0's figure) |

## 0. Before the sweep — two things

1. **CI** on `origin/main`, job by job, per template step 0. It went red on **macOS** at `f2e0e200` this cycle
   (W13's fixture) and green again at `44e4ea70`; read the macOS and Windows mutation suites **by name**.
2. **The one line owed from the `diff` round** (handoff `153-content-diff/diff-handoff-v1.md`, Addendum 4): W13's
   `Err` arm must assert `!cfg!(target_os = "linux")` before printing its skip, so a skip cannot swallow the only
   platform where that half of the control has teeth. Two lines, its own commit, reviewed normally.

## 1. Readiness sweep — template §1, all eight steps

This release adds **three commands and three schemas** and changes how two existing families resolve a point.
Particular attention:

1. **`--help` versus the surface (§1.1).** Three new synopses (`tree`, `cat`, `diff`) and a changed one
   (`checkout … --ref <ref|block-id>`). `rfc146_help_inventory_matches_commands_md` passes today; that is not the
   check. The check is whether every synopsis *description* is **true**, read against the binary: `--max-bytes`
   bounds what is written and **not memory** (RFC 157 §4a), `--path` filters output and does **not** shrink the
   work, `diff` "writes nothing", and a bare `diff` compares against the tip **with its queued commits**.
2. **CHANGELOG completeness (§1.2).** Six `## Unreleased` sections today (`diff`, `cat`, `tree`, bare block id in
   checkout's read-only modes, the damaged-ref class change, merge's block resolution). Check every user-visible
   commit since `0.45.0` has one, and that the two `### Changed` entries say plainly what a consumer that
   text-matches will see change.
3. **Docs currency (§1.3).** Every message these commands print, quoted correctly in `troubleshooting.md`,
   `commands.md`, and the `tree-and-cat`, `diff`, `show`, `ignore`, `worktree-status` and `checkout` guides.
   `git-mapping.md` gained four rows this cycle — re-read them against the binary, not against the CHANGELOG.
4. **Root-export name diff from `0.45.0` (§1.4).** It must show at least the point resolver (`resolve_point`,
   `Point`, `ReceivedRefs`' use at these call sites), `tree`/`cat`'s reader and `PointEntry`,
   `diff_points_reporting_anchor`, `diff_worktree_reporting_anchor`, `DiffReport`, `DiffEntry`, `DiffPoint`,
   `DiffStatus`, `UnsupportedPath`, `WORKTREE_POINT`. Every one under `### Added`; **nothing removed without an
   entry**; `#[non_exhaustive]` on every new report type, proven from a downstream crate as in 0.41.0.
5. **`cargo package --list -p prikk` (§1.5)**: file count, and no unintended file.
6. **Memory ratio (§1.6)** against 0.45.0's figure above. This release adds three replay-shaped readers; the
   incremental-commit ratio is the gate, and the `tree`/`diff` rows measured at `5609efc6` are context, not the
   gate.
7. **The smoke script (§1.7)** extends 0.45.0's. Each assertion states its outcome:
   - **a point is a ref or a block id:** `tree`, `cat`, `diff` and `checkout`'s read-only modes each accept a bare
     block id and the ref pointing at it, with byte-identical output; an absent ref and an unheld block each
     refuse with their own wording; a `remotes/` ref is **read** and not adopted;
   - **`tree`:** exact sizes against materialized files, `content_id` on binary only, `--prefix` by whole
     components (`src/` never matches `srcx/`);
   - **`cat`:** bytes equal the materialized file for text and binary; `--max-bytes` one below the size refuses
     with **no file and no stdout bytes**, and at exactly the size succeeds; `--output` over an existing file
     refuses without `--force`; binary to a terminal refuses and names `--output`;
   - **`diff`:** `commit` then `diff` is empty; a new file is `added` **with content** and no `untracked` list; an
     ignored path never appears; a fresh repository lists everything `added` with exit 0; the printed hunks apply
     with `patch(1)` and reproduce the right side byte for byte; a bare diff with a queued commit names the queue
     instead of a block; an entry above the work bound says `minimal: false` and still applies;
   - **it writes nothing:** `.prikk/` is byte-identical across a `tree`, a `cat --format json` and a `diff`,
     including right after a `seal`.

   Keep it at `.git-exclude/review-request/smoke.sh`; the architect runs it against the published asset on Linux.
   Say which assertions are path-shaped, and compare those by identity.
8. **Absence claims (§1.8).** For each `### Added`, grep `docs/src`, `README.md` and `SECURITY.md` for a sentence
   saying that feature is missing. Known shapes to hunt: "comparing two points is a deliberately separate,
   not-yet-open question", "a bare block id is an open row", "there is no way to read a binary file's bytes",
   "`prikk diff` does not ship". **ROADMAP.md is the architect's and is already done** (`b1b2d762`).

Report: `.git-exclude/review-request/release-0.46.0-prep-report-v1.md`, with anything found and fixed (its own
commit, reviewed normally) separated from what remains.

## 2. The release commit — template §2

Exactly three files; `## Unreleased` → `## 0.46.0 — <date>`. Message: `Release 0.46.0: comparing`.
Full gate set on that commit, stated verbatim. **Never push.**

## 3. Not the team's

- the consumer letters to **stikk** and **planeter**, deferred by the owner to this release and covering
  0.44.0–0.46.0 (planeter's A1/A2 are `tree` and `cat`; stikk's content-at-a-point gains a two-point form);
- pushing, tagging, publishing and `release-signers.toml`: never.
