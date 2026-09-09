# Show

`prikk show` reports what one sealed block or one patch changed: each operation, its kind, the
path(s) it affects, and the content it changed (RFC 142).

```sh
prikk show <block-id|patch-id> [--format json]
```

A patch is self-describing — the content is already in it. `EditText` carries the exact bytes
replaced and their replacement; `CreateFile`/`DeleteNode` carry (or point to) the file's content;
`RenamePath`/`ChangePerm`/`CreateSymlink` carry everything they need inline. Nothing is replayed to
find *what* changed.

A `RenamePath` also reports the AUTHOR key id that signed the patch asserting it — the signer is
recoverable in the same answer as the rename itself, never a fact a caller has to look up
separately. This is structural, not a rendering choice: `show`'s own read type cannot represent a
rename without it, so a patch whose AUTHOR signature is missing fails the command rather than
rendering a rename with no signer.

Three operation kinds — `EditText`, `ChangePerm`, `ReplaceBinary` — are node-addressed rather than
path-addressed, so finding *where* they apply needs one replay of the target block's own lifecycle
state. `show` pays that cost once per invocation, not once per operation, and there is no flag to
skip it: a surface that named a 32-byte node id where a path belongs would have failed at the one
thing it exists to do. If a block edits a node and then deletes it in the same block, the edit's
own path reports as unresolved rather than failing the command — every other operation in the block
still renders.

A block's output is the union of its patches, in the block's own canonical order. A bare patch id
works too, but has no block to resolve node-addressed operations against, so those report
unresolved.

`show` renders `EditText`'s own before/after span — it does not synthesize a unified diff. prikk's
edits are content-anchored spans, not line ranges, and a line-oriented rendering would assert a
structure the format does not have.

`ReplaceBinary` and any binary-kind blob report their object id and declared size, never content —
nobody has asked to see binary bytes in a terminal.

**A content reference is an id, not a guarantee.** `CreateFile`, `DeleteNode`'s file preimage, and
both sides of `ReplaceBinary` name a blob by content-addressed id — and once a text file has been
edited, its *pre-edit* identity is deliberately never written back to the object store (the content
is reconstructed by replay instead). If a referenced blob is simply **absent** from the object
store, that one piece of content reports as **unavailable** — a named state carrying the blob id,
distinct from real empty content — rather than failing the command. Every other operation in the
block still renders, and the command still exits `0`. This is not evidence of a damaged repository:
the overwhelmingly common cause is a deleted file whose most recent edit made its old identity
unbacked by design, and `show` cannot tell that specific case apart from any other blob that simply
never made it into the store; a `doctor`-level surface wanting to would need to check independently
whether the node's own history includes an edit since that blob was last live.

**Damage is different, and fails the command.** If the object store reports something more specific
than absence for a referenced blob — a content-hash mismatch, a stored type disagreement, a
malformed payload, or a `SNAPSHOT`-kind blob named where file content belongs — that is the object
store affirmatively reporting a problem, not an ordinary, by-design gap. `show` propagates it: the
command fails (exit `1`) with a message naming the integrity problem, rather than rendering the
affected content as unavailable and exiting `0`. Absence is ambiguous enough to degrade; a reported
error is not.

`show` never fails because there was nothing notable to report: exit `0` covers every case the
target resolves to, whether or not anything is unresolved or an absent content reference reports as
unavailable. A malformed or missing id is a usage error (exit `2`), a well-formed id naming no real
block or patch is a failure (exit `1`), and so is a referenced object the store reports as damaged
rather than merely absent — an unavailable *content reference* never softens any of those, which
stay what they were.

`--format json` carries the same content as the prose form, as `show-report-v1`. It settles the
format for `show` and nothing else.

Comparing two arbitrary points in history — not just one block or patch — means reconstructing both
and comparing them, a materially more expensive operation `show`'s own cost model does not cover.
There is no command for that today; it is a deliberately separate, not-yet-open question.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Content comes from the patch payload directly; only node-addressed operations need a replay, and it runs once per invocation against the target block's own lifecycle state. | [`show.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/show.rs) |
| An unresolved node-addressed operation is reported, not fatal; every other operation in the block still renders. | [`show.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/show.rs) |
| `EditText` renders its own before/after span; `ReplaceBinary` and binary blobs report id and size only, never content. | [`show.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/show.rs), [`output/show.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/output/show.rs) |
| An absent blob reference degrades that one piece of content to a named "unavailable" state rather than failing the command; every other operation still renders and the command still exits `0`. A blob the object store reports as damaged (hash mismatch, type mismatch, malformed payload, or `SNAPSHOT`-kind) propagates instead — the command fails rather than rendering it unavailable. | [`show.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/show.rs), [`object_store.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/object_store.rs) |
| Exit `0` whichever way the target resolves; a malformed or missing id is a usage error, exit `2`; a well-formed but nonexistent target id, or a referenced object the store reports as damaged, is a failure, exit `1`. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`args.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/args.rs) |
| `RenamePath` cannot be rendered without its asserting AUTHOR key id -- the read type has no construction that omits it, and a patch with no AUTHOR signature fails the command. | [`show.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/show.rs), [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author/author_signing.rs) |

## Provenance

This guide covers RFC 142's `show` command. Comparing two arbitrary points in history is a
separate, unopened question with a materially different cost.
