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

`show` never fails because there was nothing notable to report: exit `0` covers every case the
target resolves to, whether or not anything is unresolved. A malformed or missing id is a usage
error (exit `2`), caught before any repository work begins.

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
| Exit `0` whichever way the target resolves; a malformed or missing id is a usage error, exit `2`. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`args.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/args.rs) |

## Provenance

This guide covers RFC 142's `show` command. Comparing two arbitrary points in history is a
separate, unopened question with a materially different cost.
