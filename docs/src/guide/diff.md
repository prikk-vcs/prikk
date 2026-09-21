# Comparing: `prikk diff`

`prikk diff` shows what changed between two states, where a state is a **point** in history (a ref or a
bare block id — see [Reading a Point](tree-and-cat.md)) or **the worktree**. It shows content: every text
change as unified hunks, the way `diff -u` prints them.

```sh
prikk diff                                      # the worktree against the current branch: what `commit` would author
prikk diff --from <ref|block-id>                # the worktree against that point
prikk diff --from <ref|block-id> --to <ref|block-id>   # two points
prikk diff --path src/main.rs --path README.md  # only these paths in the output
prikk diff --format json                        # diff-report-v1
```

There is no fourth shape: a lone `--to` is a usage error (exit `2`), because it names a right side and no left
one. Compare two points with `--from` and `--to`, or a point with the worktree with `--from` alone.

## What each side is

- **A point** is a ref — a received ref (`remotes/…`) too, read and not adopted — or a bare block id. `--from
  heads/main --to <block-id>` and `--from <block-id> --to <block-id>` are both fine, and a ref and the block it
  points at give the same answer.
- **The worktree** is **what `prikk commit` would author**, and the report is built from `commit`'s own rules so
  that it cannot disagree with the commit it predicts. Run `prikk commit` and then `prikk diff`: it is empty
  again.

That has consequences worth knowing:

- **A new file is `added`, with its content.** prikk has no index, and `commit` authors every worktree file
  `.prikkignore` does not exclude, so there is no separate "untracked" list here. **A file that
  [`prikk worktree-status`](worktree-status.md) calls *untracked* is an `added` entry in `prikk diff`.**
- **Ignored paths never appear** — as an entry or as anything else — and an ignore rule cannot hide a path that
  is already tracked: a tracked file that a later rule covers still shows its edits
  ([Ignoring Worktree Paths](ignore.md)).
- **The left side of a bare `diff` is the current branch's tip with its queued commits folded on top**, because
  that is what `commit` authors against. The `from:` line says so when commits are queued and not yet sealed:
  `from: heads/main (block 7c1e…, plus 2 queued commits not yet sealed)`. In JSON the left side carries
  `queued_patches`. `--from <point>` compares against that point exactly, queue or no queue.
- **A fresh repository** — a current branch that has never been sealed — has the **empty state** on the left:
  every file `commit` would author is `added`, and the exit code is `0`
  (`from: heads/main (not published: the empty state)`). An explicit `--from` naming a ref that does not exist
  refuses.
- **It writes nothing.** No lock, no cache, no index, no marker: `prikk diff` reads. It works while another
  process holds the active lock, and while the worktree is provisional (materialized from a snapshot and not yet
  verified) or an interrupted materialization left its marker — states in which `commit` refuses. It does not
  clear either marker. In an interrupted-materialization state, a file the interrupted write never created reads
  as `deleted`, which is what `commit` would have signed; that is the reason `commit` refuses there.

## Statuses

Each differing path has **one** status, in canonical path order. A path that is the same on both sides is not
listed.

| Status | Meaning |
|---|---|
| `added` | On the right only. |
| `deleted` | On the left only. |
| `modified` | A text file whose content differs. |
| `renamed` | A **declared** rename: the same file at a different path (below). |
| `mode` | Same content, different mode (the executable bit). |
| `binary` | A binary file whose content differs — ids and sizes, never bytes. |
| `symlink` | In the schema, **not produced today**: replay applies no symlink operation, so a history holding one fails the call, and a worktree symlink is an unsupported path (below). |

```text
diff repository: /path/to/repo/.prikk
from: heads/main (block 7c1e…)
to: worktree
entries: 3
modified src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,7 +10,7 @@
 …three lines of context…
-    let old = 1;
+    let new = 2;
 …three lines of context…
renamed docs/old.md -> docs/new.md
added notes.txt
--- /dev/null
+++ b/notes.txt
@@ -0,0 +1 @@
+a new file
```

### Hunks, and `patch`

Text changes are unified hunks with **3 lines of context**, headed by the `---`/`+++` lines `patch(1)` reads
(`/dev/null` on the side that has no file). A last line without a newline is marked
`\ No newline at end of file`, exactly as `diff -u` marks it, and a line ending in `\r\n` is a different line from
one ending in `\n`. Applying an entry's hunks to its left content reproduces its right content byte for byte, on
Windows as everywhere; `patch` on Unix applies the printed text.

### Renames are declared, never guessed

A `renamed` entry exists **only** where history (or a live `prikk mv` on the worktree side) **declares** a
rename. Deleting a file and creating another with the same bytes is a `deleted` and an `added`, never a rename:
nothing here infers one from similar content. A rename is the same *file* at a new path, so:

- a chain of renames `a → b → c` is **one** `renamed` entry from `a` to `c`, whatever the two points are;
- a file deleted and then created again — at its old path or at another — is a **different** file, so it is
  never `renamed`: at a new path it is `deleted` plus `added`, at the same path it is `modified`;
- a rename that also changes the content shows its hunks.

`--path` selects a renamed entry by **either** its old or its new path.

### Binary files

A binary entry shows the size of each side and an **id** — and never a byte. The prose names `prikk cat` for
reading a side, with the **block** each side resolved to rather than the name you gave (a ref may move before you
run the command). The worktree side has no block: it is the file itself. For a binary file on the worktree side,
the id is the one the file would have once committed. **`content_id` exists for binary files only**, exactly as it
does in [`prikk tree`](tree-and-cat.md): a text file is its own content.

### Paths `commit` refuses

Some worktree entries `commit` cannot author: a symlink (Unix), a FIFO or socket, a name that is not a safe
repository path, an existing text file whose content is no longer valid UTF-8. `prikk diff` **names** each one,
once, with the refusal `commit` prints for it, and shows no content:

```text
unsupported paths: 1
  link: precondition not met: link: worktree symlink authoring is out of scope
```

A tracked file replaced by a symlink is named here and is **not** also reported `deleted`. The rest of the diff is
still shown. `unsupported_paths` is **always empty between two points**; only the worktree form fills it.

A live `prikk mv` that `commit` would refuse (both paths present, say) refuses the diff too, with `commit`'s own
message and nothing on stdout: there is no honest answer to what a commit that cannot happen would author.

## The cost, and the bound on the line diff

**Two points** cost two anchored replays, as [`checkout --patch-plan`](../reference/commands.md) does. **The
worktree** costs one read of the baseline plus a read of every worktree file, as `worktree-status` does, and
materializes the baseline text of the files that changed only. `--path` filters the output; it does not shrink
the work.

The line diff finds the **shortest** edit script **within a work bound**: 45,000,000 search steps for one file
(a step is one diagonal visited or one line followed inside the search, so the bound counts work, never time, and
is the same on every machine). Almost every diff finishes far inside it. Two large files that share many lines in
very different orders — a file with its lines reversed, say — can reach it; the search then stops and the region
it had not resolved is shown as **deleted and re-added**. The hunks are larger than the shortest, and they still
apply: applying them reproduces the right side byte for byte.

**You are always told.** Every entry in `--format json` has a `minimal` boolean — `true` when its script is the
shortest, `false` when the bound engaged — and the prose puts a `note: not the shortest edit script` line under
that entry's header. The same two inputs give the same output on every run and every machine.

## `--format json`: `diff-report-v1`

```json
{
  "schema_version": "diff-report-v1",
  "from": {"point": "heads/main", "target_block_id": "7c1e…", "queued_patches": 0},
  "to": {"point": "worktree", "target_block_id": "worktree"},
  "entries": [
    {
      "path": "docs/new.md",
      "status": "renamed",
      "from_path": "docs/old.md",
      "from": {"kind": "file", "encoding": "text", "mode": 33188, "size": 812},
      "to": {"kind": "file", "encoding": "text", "mode": 33188, "size": 830},
      "minimal": true,
      "hunks": ["@@ -3 +3 @@\n-old\n+new\n"]
    }
  ],
  "unsupported_paths": [{"path": "link", "refusal": "precondition not met: link: worktree symlink authoring is out of scope"}]
}
```

- **`from` / `to`** — `point` is how the side was named: a ref, a block id, or `worktree`. `target_block_id` is the
  block it resolved to; for the worktree it is the string `"worktree"`; for the current branch of a fresh
  repository (the empty state) it is `null`. `queued_patches` appears only on the implicit left side of a bare
  `diff`: how many unsealed commits were folded on top of that block.
- **`entries`** — canonical path order. `path` is the right side's path (the left's for a deletion); `from_path`
  is present for a rename only. `from` and `to` are the fields a `tree-listing-v1` entry carries (`kind`, `encoding`, `mode`, `size`, and
  `content_id` for binary), emitted by the same code; the path is the entry's own. A side is absent when the path is not there (`added` has no `from`, `deleted` no
  `to`). `content_id` appears for binary sides only. `hunks` is a list of strings, each starting at its `@@`
  line and ending in a newline; empty for a binary, mode-only or content-identical entry. **`minimal` is always
  present.**
- **`unsupported_paths`** — objects with `path` and `refusal`, in path order; empty between two points.
- **No `untracked` list.** A new worktree file is an `added` entry.

`diff-report-v1` settles the format for `diff` and nothing else.

## When `diff` refuses

- **An absent ref, or a block id the repository does not hold** — precondition, exit `1`: `ref heads/x does not
  exist in this repository`, `block <id> is not in this repository`. An id naming a patch, not a block, refuses
  too.
- **A value that is neither a ref name nor a block id, a `--path` that is not a repository-relative path, a lone
  `--to`, a repeated flag** — usage, exit `2`.
- **An operation replay does not support, anywhere in either history** — the whole call fails, and nothing is
  printed.
- **A blob that is missing or no longer recomputes** — fails the whole call as `integrity error`: a comparison
  with a hole in it is not a comparison, so no entry is shown as unavailable (RFC 157 §5a).
- **A live rename declaration `commit` would refuse** — the same message `commit` prints.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| The line diff is a shortest edit script within a counted work bound, and marks what it could not finish. | [`line_diff.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/line_diff.rs) |
| Each entry is classified by path first, and a rename pairs only the same declared node. | [`diff.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/diff.rs) |
| The worktree side is read with `commit`'s own baseline, walk, ignore rules, declarations and refusals, and writes nothing. | [`worktree_read.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring/worktree_read.rs), [`worktree_files.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring/worktree_files.rs) |
| Each side of an entry is described by the function `tree` uses. | [`point_reading.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/point_reading.rs) |
| The prose and `diff-report-v1` are rendered from one report. | [`diff.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/diff.rs) |
