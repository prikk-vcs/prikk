# Reading a Point

A **point** is a place in history: a ref (`heads/main`, `tags/v1`, `remotes/heads/main`) or a bare block
id, 64 lowercase hex characters, as `prikk log` prints it. Any block the repository holds is a point,
including an older block on a branch and a block only received history reaches. Reading a point adopts
nothing and trusts nothing (RFC 157).

## `prikk tree` — the files at a point

```sh
prikk tree [path] [--ref <ref|block-id>] [--prefix <p>] [--format json]
```

`tree` lists every file present at the point, in canonical path order (byte order of the path). There
are no directory entries: prikk records paths, not directories, so a directory is just the shared
leading components of the paths under it.

```text
tree repository: /path/to/repo/.prikk
ref: heads/main
target block: 7c1e…
entries: 3
  100755 text 18 run.sh
  100644 binary 240128 src/big.bin
  100644 text 1834 src/main.rs
```

Each line is the file's mode (octal), its encoding, its exact size in bytes, and its path. With a block id
the header reads `block: <id>` instead of `ref: <name>`.

- **Encoding.** `text` or `binary`, by the same classification `checkout --patch-plan --format json
  --content-path` reports. A file is text when it was committed as text.
- **Size** is exact: a text file's content is reconstructed by replay, and its length is counted, not
  estimated.
- **`--prefix <p>`** keeps the paths at or under `p`, by whole path components: `--prefix src/` (or `src`)
  lists `src/main.rs` and never `srcx/a.txt` or `src.rs`. The replay is whole-tree either way, so a prefix
  shortens the output, not the work.
- **Without `--ref`** it lists the current branch. In a fresh repository whose branch has never been
  sealed, that is an empty listing with exit `0`. An explicit `--ref` that does not exist refuses with
  `ref <name> does not exist in this repository`.
- **A received ref** (`remotes/…`) lists like any other.
- **Cost.** One replay of the point, anchored at its nearest checkpoint, like `checkout --patch-plan`.

### `--format json`: `tree-listing-v1`

```json
{
  "schema_version": "tree-listing-v1",
  "point": "heads/main",
  "target_block_id": "7c1e…",
  "prefix": "src/",
  "entries": [
    {"path": "src/big.bin", "kind": "file", "encoding": "binary", "mode": 33188, "size": 240128, "content_id": "9f02…"},
    {"path": "src/main.rs", "kind": "file", "encoding": "text", "mode": 33188, "size": 1834}
  ]
}
```

- `point` is the point exactly as given; `target_block_id` is the block it resolved to, so a consumer can
  cache by block. For an unpublished current branch it is `null`, with no entries.
- `prefix` is `--prefix` exactly as given, or `null`.
- `kind` is `file`. (`symlink` is reserved for when replay supports symlinks; until then a history holding
  a symlink operation fails the whole call rather than listing part of the tree.)
- `mode` is the full file mode, the same number `checkout` reports (`33188` is `100644`, `33261` is
  `100755`).
- **`content_id` is present only for a binary file**: its stored blob id. A text file never has one, even
  when a checkpoint happens to store its content, so a listing does not change with checkpoint cadence.
  Treat it as a cache hint, not as a way to fetch content.

### When `tree` refuses

- An absent ref, a block id the repository does not hold, or an id naming another kind of object: the
  point resolver's preconditions, exit `1` — see [Commands](../reference/commands.md).
- A `--ref` value that is neither a ref name nor a block id, or a `--prefix` that is not a
  repository-relative path: usage, exit `2`.
- An operation replay does not support, anywhere in the history: the whole call fails, and nothing is
  listed.
- Damage the store reports: `integrity error`.
