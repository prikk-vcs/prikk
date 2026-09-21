# Worktree Status

`prikk worktree-status` reports read-only worktree status against the same replay-derived baseline
`prikk commit` would author against (RFC 122, `replay-baseline-handoff-v1.md`): the sealed
node-addressed lineage for the selected ref, with any already-queued (unsealed) patches folded on
top. It answers "what would the next commit author?", not merely "what differs from the last seal."

```sh
prikk worktree-status [path] [--ref REF] [--format json]
```

It reports missing, modified, untracked, and unsupported paths. It does not write the worktree.
Patch generation is handled separately by `prikk commit --from-worktree`. To see the **content** of what
`commit` would author, use [`prikk diff`](diff.md): a file this command calls *untracked* is an `added`
entry there.

## What the next commit would refuse

Answering "what would the next commit author?" includes the paths it would *not* author. Every
change entry carries an **authoring** verdict alongside its change kind, and a refused entry carries
the reason `prikk commit` would print for it:

```
modified files: 1
unsupported paths: 0
refused paths: 1
worktree: changed against baseline
  modified a.txt — tracked path is not a regular file [refused: precondition not met: a.txt: worktree symlink authoring is out of scope]
```

Three things about that line are deliberate:

- **`refused` is not a change kind.** The entry is `modified` *and* refused. The refusal is a
  bracketed suffix, so the first word of an entry line is still the change kind — a reader that
  splits on it sees exactly what it saw before this field existed.
- **`refused paths` is not `unsupported paths`.** An unsupported path is an unrepresentable *name*;
  a refused path is a representable name whose worktree *entry* cannot be authored. The example
  above is one without being the other, which is why the two counters sit side by side.
- **The reason is `commit`'s own.** Both commands ask one shared classifier, so the string here is
  the string `prikk commit` prints, not a paraphrase of it.

In `--format json` the same facts are a top-level `refused_count` and, on each change,
`"authoring": "authored" | "refused"` with `"refusal": null` or the reason string. The fields are
additive within `worktree-status-report-v1`: a consumer written against the older field set reads
these documents unchanged.

A **dangling** symlink — one whose target does not exist — is refused here too. Presence is checked
without following the link, so the path reports as `modified` and refused rather than as `missing`,
which is what `prikk commit` has always seen. A path that is genuinely absent is still `missing`,
with no refusal.

**An unrepresentable name is refused too.** A file whose name no repository path can hold — a
backslash in it, or bytes that are not UTF-8 — stays an `unsupported-path` entry, and carries the
refusal `prikk commit` gives that name. In `--format json`, **`path` is repo-relative for every kind
except `unsupported-path`, where it is the file's OS name relative to the worktree root, rendered
lossily** (an invalid byte becomes `U+FFFD`): no repository path exists for it, and the absolute path
on this machine does not belong in a repository-scoped report.

**Every live declaration says what the commit will do with it.** A `prikk mv` declaration is not a
property of any single path's entry, so it is reported on its own line, with the resolution the next
`prikk commit` will reach:

```text
live rename declarations: 1
  notes.md -> docs/notes.md [refused: notes.md -> docs/notes.md: the source is present in the worktree again, so the declared move is not what the worktree holds. Run `prikk mv docs/notes.md notes.md` to drop the declaration, or `prikk mv notes.md docs/notes.md` to make the move again]
refused declarations: 1
```

The resolution is one of `rename` (authored as a rename, and the line says whether content or mode
also changed), `deletion` (the destination is gone, **or a directory stands there** — either way it is
no longer a file, so the source is authored as a deletion), `deletion-ignored` (`.prikkignore` excludes
the destination — decided from the rules, whatever kind of entry is there), `never-tracked` (the
source was never a node, so the declaration is dropped), or `refused` (the whole commit is refused,
with that message). A `rename` line also says `(content also changes)` or `(mode also changes)` when
the destination differs from the source node's baseline, because `commit` authors those beside the
rename.

In `--format json` each declaration carries `resolution`, `refusal`, `content_changed` and
`mode_changed`, and the top level
carries `refused_declaration_count`. These are additions within `worktree-status-report-v1`: against
0.42.0's output for the same repository, the only differences are the new keys — no field was
removed, renamed, or given a different value.

**`content_changed` and `mode_changed` are booleans only when the destination is a regular file.** They
are `null` when the resolution is not a rename, **and when the destination is a symlink, a FIFO or a
socket**: such a destination is never opened — reading a FIFO would wait forever — so there is no
difference to report, and `prikk commit` refuses over that path anyway (`worktree symlink authoring is
out of scope`, or `worktree entry is not a regular file`). A symlink is not followed to compare its
target.

**`refused_count` and `refused_declaration_count` are separate on purpose, and `clean` can be true
while a declaration is refused.** A worktree whose declared move was undone with a shell `mv` matches
its baseline byte for byte — nothing is missing, modified or untracked — and `prikk commit` still
refuses, because the declaration contradicts what the worktree holds. So read the commit's prospects
as "`refused paths: 0` **and** `refused declarations: 0`"; either one alone is only half the answer.
The resolution comes from the same classifier `prikk commit` obeys, so the two commands cannot
disagree about a declaration — see [Declared Moves](patches/declared-move.md).

The scanner is intentionally conservative:

- `.prikk/` metadata is ignored;
- existing path-safety validation is reused;
- non-ASCII paths remain unsupported until Unicode NFC normalization is implemented;
- no writes are performed.

For the exact repository path validator rules, see
[path and worktree safety](../reference/path-safety.md). A path matched by a `.prikkignore` rule at
the repository root never appears in the untracked list at all — see
[Ignoring Worktree Paths](ignore.md).

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| `worktree-status` compares the worktree against the replay-derived baseline `commit` shares — the sealed lineage with any already-queued patches folded on top — not a stored snapshot Blob. | [`worktree_status.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_status.rs), [`patch_replay.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_replay.rs) |
| It writes nothing and reports missing, modified, untracked, and unsupported-path changes. | [`worktree_status.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_status.rs) |
| Each entry's `authoring` verdict and `refusal` reason come from the same classifier `prikk commit` refuses with, so the two commands cannot disagree about a path. | [`node_authoring.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring.rs), [`worktree_status.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_status.rs) |
| A contradicted rename declaration is refused by `commit` and is reported as a refused *declaration*, not a refused path — and can sit in a `clean` worktree. | [`rfc147_authoring_refusal_field.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/tests/rfc147_authoring_refusal_field.rs), [`rfc147_declaration_resolution.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/tests/rfc147_declaration_resolution.rs) |
| Each declaration's `resolution` is what `commit` then does, and a refusal's text is commit's own message; every `prikk mv` a refusal names is run in the state that produced it. | [`declaration_resolution.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/declaration_resolution.rs), [`rfc147_declaration_resolution.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/tests/rfc147_declaration_resolution.rs) |

## Provenance

This guide covers RFC 122's rewire onto the replay baseline, RFC 147 §2e's per-entry authoring
verdict, and RFC 147 §2f's per-declaration resolution. It does not change repository state, signing, trust, or the bundle/sync formats.
