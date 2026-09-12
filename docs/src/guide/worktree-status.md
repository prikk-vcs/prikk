# Worktree Status

`prikk worktree-status` reports read-only worktree status against the same replay-derived baseline
`prikk commit` would author against (RFC 122, `replay-baseline-handoff-v1.md`): the sealed
node-addressed lineage for the selected ref, with any already-queued (unsealed) patches folded on
top. It answers "what would the next commit author?", not merely "what differs from the last seal."

```sh
prikk worktree-status [path] [--ref REF] [--format json]
```

It reports missing, modified, untracked, and unsupported paths. It does not write the worktree.
Patch generation is handled separately by `prikk commit --from-worktree`.

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

**One refusal is not yet reported here.** A worktree that contradicts a live `prikk mv` declaration
— the declared source back on disk, or the destination already occupied — is refused by
`prikk commit` but is not a property of any single path's entry, so it does not appear as a refused
path. The `live rename declarations:` lines in the same output are where that case is visible — see
[Declared Moves](patches/declared-move.md). So `refused paths: 0` means "no path's own entry is
unauthorable", not "the next commit will certainly succeed".

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
| A contradicted rename declaration is refused by `commit` but is not reported as a refused path. | [`rfc147_authoring_refusal_field.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/tests/rfc147_authoring_refusal_field.rs) |

## Provenance

This guide covers RFC 122's rewire onto the replay baseline and RFC 147 §2e's per-entry authoring
verdict. It does not change repository state, signing, trust, or the bundle/sync formats.
