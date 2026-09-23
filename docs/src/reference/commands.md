# Command Surface

Every command prikk accepts, in one place. This page is an inventory, not a tutorial — each command's
own behaviour, refusals, and worked examples live in the [guide](../guide/tutorial.md), and the
[Git→prikk mapping](git-mapping.md) explains where the vocabulary differs from Git's.

Every command accepts `--help` for its own usage:

```text
prikk <command> --help
```

```text
prikk init [path]
prikk key generate [--out <path>]
prikk key public [--seed-file <path>] [--role author|maintainer]
prikk key status [path] [--role author|maintainer] [--format json]
prikk setup [repo-path] [--author-seed-out <path>] [--maintainer-seed-out <path>]
prikk trust maintainer add --key-id ID --public-key HEX
prikk trust maintainer remove --key-id ID
prikk trust maintainer list [--format json]
prikk trust maintainer check --key-id ID [--format json]
prikk commit [--from-worktree] [--text-edits] [--ref heads/<branch>] -m <message>
prikk mv <old> <new>
prikk seal --allow-no-audit [--ref heads/<branch>]
prikk status [--format json]
prikk log [path] [--limit N] [--ref REF] [--format json]
prikk checkout --plan-only [path] [--ref <ref|block-id>]
prikk checkout --snapshot-plan [path] [--ref <ref|block-id>]
prikk checkout --snapshot-materialize [path] [--ref REF]
prikk checkout --patch-plan [path] [--ref <ref|block-id>] [--format json [--content-path <path>]...]
prikk checkout --patch-materialize [path] [--ref REF]
prikk checkout --patch-delete-plan [path] [--ref <ref|block-id>]
prikk checkout --patch-materialize-delete [path] [--ref REF]
prikk show <block-id|patch-id> [--format json]
prikk tree [path] [--ref <ref|block-id>] [--prefix <p>] [--format json]
prikk cat --path <p> [--ref <ref|block-id>] [--output <file> [--force]] [--max-bytes N] [--format json]
prikk diff [path] [--from <ref|block-id>] [--to <ref|block-id>] [--path <p>]... [--format json]
prikk merge-evidence --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]
prikk merge-plan --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]
prikk merge --allow-no-audit --baseline-block ID --into REF --from REF [path]
prikk inverse-plan [path] [--ref REF]
prikk rollback-preview [path] [--ref REF]
prikk rollback-draft --append-inverse [path] [--ref REF] -m <message>
prikk rollback-draft-verify [path] [--ref REF]

prikk branch [list] [--all] [--format json]
prikk branch create heads/<name> [--from REF]
prikk branch close heads/<name>
prikk branch switch heads/<name>
prikk tag [list] [--format json]
prikk tag create tags/<name> --target <ref|block> [-m <message>]
prikk config get <key>
prikk config set <key> <value>
prikk config unset <key>
prikk config list
prikk bundle export --ref REF --output <file> [--force]
prikk bundle import --input <file> [--max-object-bytes N]
prikk bundle preview --input <file> [--ref REF] [--max-object-bytes N] [--format json]
prikk bundle verify --input <file> [--max-object-bytes N]
prikk sync summary --output <file>
prikk sync compare --summary <file>
prikk sync have <ref> --output <file>
prikk sync build <ref> --have <file> --output <file> [--force]
prikk sync accept <file> [--max-object-bytes N] [--claims-out <file>] [--force]
prikk sync pending
prikk sync seal <ref> --claim <id>
prikk sync seal <ref> --claims <file>
prikk sync tags
prikk sync adopt-tag <name>
prikk worktree-status [path] [--ref REF] [--format json]
prikk verify [path] [--stop-on-first-error] [--format json]
prikk doctor [path]
prikk doctor [path] --repair-wal-tail
prikk doctor [path] --repair-index
prikk doctor [path] --repair-main-ref
prikk format upgrade [path]
prikk unlock
prikk unlock --lock <path> [--yes|--force]
prikk compact --pointer-index|--received-index|--trust-policy|--all [--plan-only]
prikk --version
```

**What `--from` and `--target` accept.** `branch create --from REF` takes any published ref: a
branch ref names its Block directly, and a tag ref is dereferenced through its tag object, so
`--from tags/v1` publishes the new branch at the Block that tag names. `tag create --target` takes a
block id or a **branch** ref, and **refuses a tag ref** — a tag of a tag is outside the model (ref →
tag object → block, one hop), and resolving it silently would make `--target tags/v1` and `--target`
that tag's own block indistinguishable in history. The refusal names both accepted forms.

**Key ids.**
- `setup` (when it creates a seed) and `key generate --out` write the new key's id,
  `ed25519-<first 16 hex of the public key>`, beside the seed, as `<role>.key-id` in the key directory or
  `<seed path>.key-id`.
- Every signer, `setup` and `key status` resolve the id the same way: `PRIKK_<ROLE>_KEY_ID`, else that
  file, else the legacy role word.
- `setup` adopts the maintainer key under the resolved id and prints it.
- `key status` reports `key_id_source` as `environment`, `key-file` or `default`, and a key-id file that
  does not hold its seed's id as `reason: key-id-file-mismatch`, which refuses signing.

**`format upgrade` is explicit, verified and one-way.** A repository created by 0.44.0 or earlier is
format 6; new repositories are format 7, which is format 6 plus "an object id may hold several records,
the last authoritative". `prikk format upgrade` takes the writer lock, runs the same verification as
`prikk verify` and refuses — changing nothing — unless it would exit 0, then rewrites the one-line
`.prikk/FORMAT` marker atomically. Nothing stored is rewritten. It is idempotent on a format-7
repository, never runs by itself, and has no inverse: prikk 0.44.0 and earlier refuse a format-7
repository at open. See [Release Compatibility](release-compatibility.md).

**Refusals about refs and repositories.**
- **A ref that does not exist.** A command that reads an existing ref's state refuses an absent name
  before anything else with `precondition not met: ref <name> does not exist in this repository` (exit
  `1`), adding a sentence when `remotes/<name>` exists. That covers `log` and `worktree-status` given
  `--ref`, every `checkout` mode, `inverse-plan`, `rollback-preview`, `rollback-draft`,
  `rollback-draft-verify`, `merge-evidence`, `merge-plan`, `merge --into`/`--from`, `bundle export`,
  `branch create --from`, `tag create --target`, `branch close` and `branch switch`, which keeps its own
  route sentence.
- **Not refused:** `sync have`, `sync build` and `bundle preview` answer an absent ref as a state ("none
  of it"), and `commit --ref`, `seal --ref` and `branch create` create the ref. In a fresh repository,
  `log` and `worktree-status` without `--ref` still report the unpublished current branch.
- **A received ref given to a command that cannot use one** (`checkout`, `inverse-plan`,
  `rollback-preview`, `bundle export`, `branch create --from`, `tag create --target`) refuses with
  `precondition not met: <name> is a received ref`, naming the commands that read received refs (`log`,
  `merge-evidence`, `merge-plan`, `bundle preview`) and the one that takes it into a local branch
  (`merge --from`).
- **A path with no repository** refuses as a precondition naming that path ("no prikk repository at
  <path>"), decided when the repository is opened, in every command that opens one.

**A point: a ref or a bare block id.** `checkout`'s read-only modes (`--plan-only`, `--snapshot-plan`,
`--patch-plan` with or without `--content-path`, `--patch-delete-plan`) take `--ref` as a ref name or a
bare block id, 64 lowercase hex characters, resolved by one resolver.
- A block the repository holds resolves whether or not any ref reaches it: an older block on a branch,
  or a block only received history holds. Reading it adopts nothing.
- `block <id> is not in this repository` and `object <id> is a patch, not a block` (naming the type it
  is) refuse as preconditions, exit `1`. A `--ref` value that is neither a ref name (`heads/…`,
  `tags/…`, `remotes/…`) nor 64 lowercase hex is a usage error, exit `2`.
- The modes that write the worktree (`--snapshot-materialize`, `--patch-materialize`,
  `--patch-materialize-delete`) refuse a block id: writing the worktree needs a branch, because the
  next `commit` authors against one.
- The prose header reads `block: <id>` for a block id and `ref: <name>` otherwise; `--plan-only` prints
  `ref-state: <none>` for a block id. `patch-plan-content-v1`'s `ref` holds the value as given, now
  possibly a block id; its schema version is unchanged.

**Reading a point: `tree`.** `tree` lists the files present at a point — a ref, or a bare block id — with
each one's mode, exact size and whether it is text or binary, by the same classification `checkout
--patch-plan --format json` reports. It reads received refs. Without `--ref` it lists the current branch,
and an unpublished current branch lists nothing, exit `0`; an explicit absent ref refuses. `--prefix`
filters by whole path components; the replay is whole-tree either way. `--format json` is
`tree-listing-v1`. See [Reading a point](../guide/tree-and-cat.md).

**Reading a point: `cat`.** `cat` writes one file's bytes at a point, text or binary, to stdout or to
`--output`. The content is resolved in full before anything is written, so a refusal writes nothing.
`--max-bytes` bounds what is written — exactly, and all or nothing — but **not memory**, which is the shared
replay's own cost; `--output` refuses an existing file without `--force` and any path inside `.prikk/`,
and is written to a temporary sibling then renamed into place. **Binary content refuses a terminal**, naming
`--output`; text is written. A path that is not a file at the point refuses with `path <p> does not exist at
<point>`. `--format json` is `path-content-v1`: the same fields as one `tree-listing-v1` entry, and no bytes.

**Comparing: `diff`.** `diff` shows what changed between two states. A bare `prikk diff` compares the current
branch's tip (with any queued commits) against the **worktree** — exactly what `commit` would author, so `commit`
then `diff` is empty again; `--from <point>` compares that point against the worktree; `--from` and `--to`
together compare two points, each a ref (a received ref is read, not adopted) or a bare block id. A lone `--to`
is a usage error. Each differing path has one status: `added`, `deleted`, `modified`, `renamed`, `mode` or
`binary`; a new worktree file is `added`, with its content, and ignored paths never appear. Text changes carry
unified hunks with 3 lines of context, and a last line without a newline is marked as `diff -u` marks it. A
rename is reported only when history (or a live `prikk mv`) **declares** one, never inferred from similar
content. A binary file shows ids and sizes and never its bytes; `prikk cat` reads either side. A worktree path
`commit` would refuse is named under `unsupported_paths` with `commit`'s own refusal and no content. The line
diff is shortest within a work bound, above which the entry says `minimal: false` and its hunks, still valid,
are larger. `--path` (repeatable, exact) filters the output, and the work is whole-tree either way. Two states
that are the same give `no differences` and exit `0`; `diff` writes nothing. An unsupported operation, or a
blob that is missing or does not recompute, in either history fails the whole call. `--format json` is
`diff-report-v1`; see [Comparing](../guide/diff.md).

**Size bounds on incoming artifacts.** RFC 158 Stage A: `bundle import`, `bundle preview`, `bundle
verify`, `sync compare`, `sync build` and `sync accept` all read a file arriving from outside the
repository, and every one of them refuses **before reading it** when its size is over the bound —
on the open file's own metadata, so nothing large is allocated to discover it is large, then
enforced again while it streams, so a lying declared size is still caught. Every refusal names what
was too large and its size, the bound applied (in bytes and the nearest binary unit), where that
bound came from, and how to change it; none of them says "malformed" or a bundle is "persisted"
data, since neither is true of an oversized but otherwise well-formed input.

Six total-artifact bounds exist (DC-86), none previously documented:

| bound | default | applies to | override |
|---|---|---|---|
| bundle | 256 MiB | `bundle import`/`preview`/`verify` | `PRIKK_BUNDLE_MAX_BYTES` |
| sync exchange artifact | 256 MiB | `sync accept` | `PRIKK_EXCHANGE_MAX_BYTES` |
| sync summary | 16 MiB | `sync compare` | `PRIKK_SYNC_SUMMARY_MAX_BYTES` |
| have-list | 64 MiB | `sync build --have` | fixed — no override exists |

A bundle and a sync exchange artifact also each declare a count of objects they carry (bundles) or
of patches/blobs/author keys/claims (exchange artifacts), bounded by `PRIKK_BUNDLE_MAX_OBJECTS` and
`PRIKK_EXCHANGE_MAX_OBJECTS` respectively (default 100,000 each), and the sync summary's ref count
by `PRIKK_SYNC_SUMMARY_MAX_REFS` (default 100,000).

**The per-object bound** applies to `bundle import`, `bundle preview`, `bundle verify` and `sync
accept` — everything that decodes objects from outside, never to a repository's own `commit`, which
is not bounded by default. It measures an object's *encoded* size as it travels (its content plus a
small header), checked on the length prefix before the object's bytes are copied or decoded, so a
file of exactly the bound you set is not a surprise refusal. Default: 256 MiB, the same as the total
bundle/exchange bound, so it is inert at the defaults — it only acts once an operator lowers it.
Resolved highest-precedence first: `--max-object-bytes N` on the command itself (on `bundle import`,
`bundle preview`, `bundle verify`, and `sync accept`), else the repository's own
`incoming.max-object-bytes` in `prikk config` (not available to `bundle verify`, which opens no
repository), else the default. The bound is never taken from the input it bounds — nothing inside a
bundle or exchange artifact can raise it, and nothing an import or accept writes can reach
`.prikk/config`.

**What is still true until RFC 158 Stage B (streaming)**: an artifact under its bound is still read
whole into memory, and decoding costs a multiple of that — measured on a bundle carrying one 255 MiB
blob, at the defaults, three samples each: `bundle verify` peaks at a median of 787,400 KiB (≈3.0×
the bundle's own size), `bundle import` at 1,570,984 KiB (≈6.0×). Streaming removes this multiplier;
it does not exist yet.

**`prikk config`** is a repository's own small, durable settings file at `.prikk/config` — never in
the worktree, so a checked-out or imported file can never set one. One `key = value` per line; blank
lines and `#` comments are allowed. An unknown key, a duplicate key, or an invalid value (zero,
negative, non-integer) each refuse, naming the line, never a silent fall back to the default. The
only key today is `incoming.max-object-bytes`. `get`/`list` print the effective value and its source
(`(default)` or `` `.prikk/config` ``); `set` validates then writes atomically; `unset` restores the
default. An older `prikk` build simply does not read this file — it has no effect on that build's
own behavior, and no error results from a repository holding one.

**Exit codes.** `0` — the operation succeeded and did what was asked. `1` — operational failure:
verification findings, an integrity failure, a refusal, a dirty worktree. `2` — usage error: an
unknown argument, a missing required flag, a duplicate flag. Graded verification results are in
`prikk verify --format json`, not in the exit code.
