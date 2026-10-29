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
prikk extract (--ref <ref|block-id>|--claim <claim-id>) <dir> [--allow-full-replay] [--plan-only] [--max-bytes N] [--max-files N] [--format json]
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
prikk bundle import --input <file> [--max-object-bytes N] [--allow-non-fast-forward]
prikk bundle preview --input <file> [--ref REF] [--max-object-bytes N] [--format json]
prikk bundle verify --input <file> [--max-object-bytes N]
prikk archive export <file> [--format json]
prikk archive verify <file> [--format json]
prikk archive import <file> [--adopt] [--format json]
prikk sync summary --output <file>
prikk sync compare --summary <file>
prikk sync have <ref> --output <file>
prikk sync build <ref> --have <file> --output <file> [--force]
prikk sync accept <file> [--max-object-bytes N] [--claims-out <file>] [--force]
prikk sync pending [--format json]
prikk sync seal <ref> --claim <id>
prikk sync seal <ref> --claims <file>
prikk sync tags
prikk sync adopt-tag <name>
prikk worktree-status [path] [--ref REF] [--format json]
prikk verify [path] [--stop-on-first-error] [--format json]
prikk doctor [path]
prikk doctor [path] --repair-wal-tail
prikk doctor [path] --repair-index
prikk doctor [path] --repair-pointer-index-tail
prikk doctor [path] --repair-tails [--plan-only]
prikk doctor [path] --repair-main-ref
prikk doctor [path] --rebuild-pointer-index [--plan-only]
prikk doctor [path] --discard-damaged-commits [--plan-only]
prikk doctor [path] --restore-queue-target --ref <ref> [--not-current-branch] [--plan-only]
prikk doctor [path] --recovery-list
prikk doctor [path] --recovery-restore <run id> [--plan-only]
prikk doctor [path] --recovery-clear [--plan-only]
prikk doctor [path] --cancel-import <id> [--plan-only]
prikk ref complete <ref> [--plan-only]
prikk format upgrade [path]
prikk unlock
prikk unlock --lock <path> [--yes|--force]
prikk compact --pointer-index|--received-index|--trust-policy|--all [--plan-only]
prikk compact --received-index|--trust-policy --keep-slot a|b [--plan-only]
prikk received remove <remotes/…> [--plan-only]
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
`prikk verify` and refuses — changing nothing — unless it would exit 0, then overwrites the one byte
of the `.prikk/FORMAT` marker in place and flushes it (no rename; RFC 168 §3.3, `crates/prikk-store/src/format_upgrade.rs:81-88`). Nothing stored is rewritten. It is idempotent on a format-7
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

**`bundle import` and `bundle preview` classify a delivery against the pointer it would replace,
in six classes:** `no-local-history` (nothing to compare against), `already-at-tip` (identical to
what is already there), `rollback` (the delivery is older than what is already there),
`fast-forward` (the delivery extends what is already there), `diverged` (both sides moved since a
shared point), and `does-not-connect` (no shared history at all). `bundle preview --format json`
reports this as a new, additive `"class"` field — its existing `"connectivity"` field keeps its
original five values unchanged (`already-at-tip` and `rollback` both still read
`"already-included"` there), so nothing a script already reads changes. Both commands also report
`"ref_state_chain"`: `"extends"`, `"older"`, or `"unrelated"` — how the delivery's own publication
history relates to the pointer it would replace, which can disagree with the class above (a
`fast-forward` by content can still be `"older"` in its own publication history).
- **`bundle import` refuses a `rollback`, a `diverged` delivery, or one that `does-not-connect`,
  whenever a received pointer already exists for that name** — naming the class, exit `1` — unless
  `--allow-non-fast-forward` is given. A delivery with no existing pointer to replace, or one that
  is `already-at-tip` or a genuine `fast-forward`, is never refused on this ground.
- **An `already-at-tip` delivery whose own publication history reads `"older"` writes every object
  it carries, but leaves the received pointer itself unchanged, and exits `0`.** The content is
  identical either way, and moving the pointer backward would only make its own publication
  history read incorrectly later; the text form says plainly that the pointer was left as it was.

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

**`log --ref` and a bare block id.** `log --ref` is resolved by the same one resolver described
above, so a value that is neither a ref name nor 64 lowercase hex refuses, and a block id naming
nothing in this repository refuses as a precondition — but `log` itself still refuses a block id
that does resolve: `<id> is a block id; log does not show history ending at a block` (exit `1`). A
block can have more than one parent, so there is no single next entry to walk the way
`RefState.previous_ref_state_id` gives one; `log`'s own history is the publication chain of a ref,
not a block's own ancestry. This closes a prior gap where `log --ref <block id>` exited `0` with an
empty history instead of refusing.

**`show`'s own `state` field, and resolving a pending text edit's path.** Every patch
`show --format json` reports (`show-report-v1`, additive) carries `"state"`: `"queued"` (found
only in this repository's own active WAL), `"pending"` (accepted and not sealed by any history
this repository holds, its own or received — the same membership `sync pending` itself reports),
or `"sealed"` (reached from a block, through this repository's own local ref history or a received
pointer's). `state` is the same whichever id names the patch, a block's own bare id included: a
patch no tip reaches reads `"pending"` — the same membership `sync pending` itself lists — even
while its own block is still held and readable by its own id directly, since a bare-id read never
checks reachability. `"pending"` and `"sealed"` cannot both hold for the same patch; `"queued"` and
`"pending"` cannot either, under ordinary operation, since a queued patch has no written object yet
at all. `"queued"` and `"sealed"` can briefly overlap across the window between sealing a block and
clearing the queue afterward — `show` resolves this by looking for a real object before ever
falling back to the queue, so `"sealed"` wins.

A pending patch's own node-addressed operations (`edit-text`, `replace-binary`, `change-perm`)
resolve a real path when a recognition claim naming it names a single parent block this
repository already holds, sealed or received — folding that claim's own patches onto that
parent's state, the same way a sealed block's own patches fold onto its parent's. When a claim
resolves the path, that patch's own `"resolved_through_claim_id"` names the claim's id (`null`
otherwise); a tool can look up that claim's own signature outcome in `sync pending`'s own
`sync-pending-v1` listing. A claim whose own MAINTAINER signature is invalid is skipped outright,
never used to choose a path; a claim whose own patches do not actually fold against its own
declared parent is skipped the same way, its own fold failure named as the reason — in neither
case does `show` itself refuse. **The path stays unresolved** when every claim naming the patch
fails: no held claim names it at all; every claim's declared parent is itself only pending, part
of a chain of claims that is not yet realized; or every claim names more than one parent, with no
mainline to pick. `"unresolved_node_id"` carries a `"reason"` alongside it in every such case,
naming which applies, rather than guessing a path — and when more than one claim fails for a
different reason, the most specific one is reported: an invalid signature first, then a fold
failure, then a chain, then a merge. A bare sealed patch id shown directly stays unresolved too,
also naming why: `show` does not search for the block that sealed a patch shown by its own id;
`prikk log` or `prikk tree` finds it.

**`sync pending` and its own `--format json`: `sync-pending-v1`.** `sync pending` lists patches
accepted into this repository (`sync accept`) and not sealed by any history it holds, its own or
received: patch objects present, minus those reachable from a local ref's own tip block, further
reduced by every patch also reachable from a **received** pointer's own tip block — a patch the
sender's own history already sealed, even one this repository has never adopted into a local ref,
is not something an operator still needs to act on. This is a listing-only reduction: `sync seal`
still seals a claim's patches into this repository's own history, even when a received history
already sealed them, since sealing is about this repository's own ref history, not what some other
repository's received history claims.
`--format json`'s one document, `sync-pending-v1`, adds the recognition claim(s) naming each pending
patch: `patches[]`, each `{"patch_id", "claims"}`; `claims[]`, each `{"claim_id", "target_block_id",
"parent_block_ids", "signature"}`, where `signature` is `{"outcome": "sound"|"unverifiable"|"invalid", "key_id"}` from
the claim's own MAINTAINER signature. **`"invalid"` is the one value a tool consuming this listing
must treat as an alarm:** a detected forgery under a key this repository has adopted, a
MAINTAINER signature whose algorithm is not Ed25519, or no MAINTAINER signature at all — carried
alongside `"reason"` (the refusal's own text) and, for the first two cases, `"key_id"`; the last
case's `"key_id"` is `null`, since there is no signature to name one from. This is reported for
that one claim only, beside every other pending patch — it never refuses the whole listing, unlike
`sync accept`/`sync seal`, which still refuse outright on the identical signature check.
`target_block_id` is the block this claim recognizes — the same field and meaning as
`extract-report-v1`'s own `target_block_id` (see `extract`'s own `--format json` below).
`parent_block_ids` is the block(s) this claim declares as `target_block_id`'s own parents — the
same meaning as `extract-report-v1`'s own `claim_parent_block_id`, but as a **list**: `[]` for a
root claim, one entry ordinarily, two for a claim about a merge. This listing reports a
merge-shaped claim in full rather than refusing one, unlike `extract --claim`, which has no
mainline to pick from and refuses such a claim outright — a listing has no such need to pick one.
**A patch no held claim names appears with `"claims": []`:** ordinary, since one exchange artifact's
patch and claim sections are independent — a patch can arrive without, or before, any claim naming
it. `patches[]` is ordered by patch id, the same order the text form already uses; each entry's own
`claims[]` is ordered by claim id. The text form itself (the count line and one id per line) is
unchanged in shape, though its own count and list now reflect the same received-history reduction.

```json
{"schema_version": "sync-pending-v1", "patches": [{"patch_id": "<id>", "claims": [{"claim_id": "<id>", "target_block_id": "<id>", "parent_block_ids": ["<id>"], "signature": {"outcome": "sound", "key_id": "<key-id>"}}, {"claim_id": "<id>", "target_block_id": "<id>", "parent_block_ids": [], "signature": {"outcome": "invalid", "key_id": "<key-id>", "reason": "<the refusal's own text>"}}]}]}
```

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

**Writing a point into a new folder: `extract`.** `extract --ref <ref|block-id> <dir>` writes every file
at a point into a new directory that does not exist yet, no repository history and no worktree state, the
same shape `git archive | tar -x` produces. `<dir>` must resolve outside this repository's own root
(`.prikk/` included) — compared by identity, never by spelling, so a relative path, a `..` that lands back
inside, a symlinked parent, or a differently-cased or short-name spelling of the root on Windows all refuse
just as a plain path inside the worktree does; a path outside the repository, including a sibling
directory whose name merely starts with the repository's own name, is unaffected. The reason is the same
one `commit --from-worktree` already gives for its own worktree content: a copy left inside the repository
would be picked up as worktree content by the very next commit. Unlike `cat`/`tree`, **it anchors only
where a writer would**:
the same admitted-anchor rule `checkout`'s own worktree writes use, never the weaker rule that lets `cat`
and `tree` display any snapshot that merely decodes correctly. A `remotes/...` ref under a key this
repository has not adopted therefore always replays fully from its own genesis; past 63 blocks with no
admitted snapshot in reach, `extract` refuses by name, giving the distance and `--allow-full-replay` to
read it anyway. Every file whose content has not changed since the admitted anchor streams straight from
its stored blob id, nothing is held resident past the one file's own size, and a path renamed after the
anchor still streams from that same id, at its new path. The target must not already exist; a leftover
`<dir>.prikk-extract-tmp` from an interrupted run refuses by name rather than being reused or deleted; the
whole write is atomic (a temporary sibling, then one no-replace rename), and every file and directory is
synced before it, so a crash never leaves a target with missing or partial content. `provisional: yes|no`
says whether an admitted anchor was actually found and used, or the replay fell back to genesis instead.

**`--plan-only`** reports the point's own file count, total bytes, `largest file: N bytes` and
`provisional`, and writes nothing at all — no target, no temporary sibling. **`--max-bytes N`/`--max-files
N`** refuse the whole point, by name, before the first byte is written — learning the point's own total
size this way costs what it costs (each file not already resident is read once to measure it), the same
cost a real write already pays; the *pre-walk's* own bound (refusing a point whose nearest admitted
snapshot is unreachably far back) is the separate, cheap guard against unbounded history depth, checked
before a single Patch or Blob is read, never relaxed by these flags. **`--format json`** (`extract-report-v1`) carries `file_count`, `total_bytes`, `largest_file_bytes` (the single
largest entry's own byte length, always present, the same number the text form's `largest file: N bytes`
line gives), `provisional`, `written` (whether this call actually wrote the target), `point` (the ref name or
the block/claim id given, the one fixed key `tree`'s own `tree-listing-v1`/`cat`'s own `path-content-v1`
both use), `point_kind` (`"ref"`, `"block"`, or `"claim"`), `target_block_id`, and `claim_parent_block_id`
(always present; `null` unless `point_kind` is `"claim"`, in which case it is the claim's own declared
parent — the block its own patches would be sealed *onto*, not `target_block_id`, which is the block the
claim *recognizes*; no command printed a claim's parent before this round) — the same hand-rolled,
`schema_version`-named convention those commands already use. **`--claim <claim-id>`** (mutually exclusive
with `--ref`) previews a pending `RecognitionClaim` — the same id `sync seal --claim` takes — as if its
own patches were sealed onto its own declared parent, without sealing anything: useful to inspect what a
claim would produce (its size, or its content) before deciding to accept it. A claim naming more than one
parent (one built from an ancestor `Merge` block) is refused by name — there is no field in the claim
saying which parent is mainline, so this is refused rather than guessed. `--claim` takes the identical
pre-walk bound as `--ref`/a bare block id: past the trusted distance from the claimed parent's own nearest
admitted checkpoint, it refuses by name unless `--allow-full-replay` is given. `extract`'s own peak memory,
whole command, tracks the largest single file it actually reads (roughly twice that file's own size, not
once — see [Current State](current-state.md)), not the point's own total content size; this holds the
same way whether the point is named by `--ref`, a bare block id, or `--claim` — `--claim`'s own own
derivation step (`prepare_extract_plan_for_claim`) costs a little more than the ref/block-id path's own
equivalent planning step (about 1.1–1.3x, measured), but that planning cost is small next to the one
shared, dominant cost every path pays: streaming the point's own largest file once. See
[Current State](current-state.md) for the measured figures, both the planning-only comparison and the
whole command. See [Reading a point](../guide/tree-and-cat.md).

**`refusal-v1`: a machine-readable refusal document.** Under `--format json`, *every* refusal `extract`
exits `1` for — never a usage error, exit `2`, which prints no document — writes exactly one JSON document
to **stdout** (never stderr; stderr's own text and the exit code are entirely unchanged by this). This is a
structural guarantee at `extract`'s own command boundary, not merely a property each refusal happens to
have today: a refusal reached by a path this document's own list does not yet name still gets a document,
classified by a fallback, rather than exiting `1` under `--format json` with nothing on stdout.

```json
{"schema_version": "refusal-v1", "command": "extract", "refusal": "<id>", "reason": "<id>|null", "message": "<the stderr text>"}
```

`refusal` is one of fifteen stable, kebab-case ids: `over-max-bytes`, `over-max-files`,
`destination-exists`, `destination-inside-repository`, `temporary-sibling-exists`, `point-not-found`,
`claim-not-found`, `not-a-claim`, `claim-has-several-parents`, `claim-parent-not-sealed`, `case-collision`,
`no-checkpoint-in-reach`, `not-a-repository`, `damaged-repository`, `io-failure`. `claim-parent-not-sealed`
means the claim's own declared parent has not been sealed or received into this repository yet — seal or
receive it first, then extract the claim again; this repository is not damaged. `reason` is set only for
`no-checkpoint-in-reach`, naming which of
three conditions the admitted-anchor search found in reach — **one cause at a time**: the first blocker
the search meets, not necessarily the only one. Fix it and run again; the next refusal names the next
cause, if any. In priority order: `signer-not-adopted` (a recorded checkpoint whose signer this repository
has not adopted — adopt the key), `not-recorded` (a checkpoint in reach, never recorded by `verify`/`seal`
— run `verify`), or `no-checkpoint` (none at all — `--allow-full-replay`). Every other id carries
`reason: null`. `message` is stderr's own text, without its leading `error: ` prefix — the generic framing
applied to every command's own error, never part of the refusal's own text itself; the JSON document never
paraphrases it.

**This is this project's own, reusable form for a JSON refusal** — `refusal-v1` is `extract`'s own
document today, but its shape (`schema_version`, `command`, `refusal`, `reason`, `message`) is designed as
a precedent other commands adopt when next touched, not a one-off specific to `extract`.

**Removing a received pointer: `received remove`.** `received remove <remotes/…> [--plan-only]`
removes one received ref's own pointer from the received-index container — the same container
`bundle import`/`archive import` write and `cat`/`tree`/`checkout`/`extract` read at a bare
`remotes/…` point. **The objects it reached are not removed, only the pointer**: a stored `Block`
or `RefState` nothing else reaches any more is reported by `prikk verify` as an **unreached
object**, a non-blocking warning, not a failure (see `verify`'s own entry above). When `verify` also
reports a damaged object, the walk cannot read past it: objects reached only through the damaged
one are counted as unreached, and their publication trust is not checked, so the count can include
objects that are in fact reached. `verify` still fails on the damage itself
(`verify/reachability.rs`, `verify/objects.rs`). `--plan-only`
reports what would be removed — the pointer's own current target, or that there is nothing named —
writing nothing, but it still takes the container's lock for the duration of the read: the same
reason `compact`'s own `--plan-only` variants do — an operator acts on what a preview reports, and
the read should not go stale against a concurrent writer before it reaches the terminal; the lock
is released once the read is done. A real run saves the removal to the recovery log
unconditionally, naming the run id so `prikk doctor --recovery-restore <id>` can undo it, as long
as nothing has since written to the same slot (a later, sound re-import of the identical name makes
the restore refuse, rather than silently discarding it). Refuses a name that does not start with
`remotes/`, a name with no received pointer, and a damaged received index, the same way `compact
--received-index` already does.

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

Four size bounds (three overridable, the have-list fixed) and three count bounds exist — six
variables in all (DC-86), none previously documented. The size bounds:

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
is not bounded by default. It measures an object's *encoded* size as it travels — its content plus
a header — checked on the length prefix before the object's bytes are copied or decoded. **So a
file of exactly the bound you set refuses:** the header adds **69 bytes** (measured, for text and
binary files alike, and pinned by a test), so a file of N bytes needs a bound of at least N + 69.
The refusal names the size it saw, which is that N + 69. Default: 256 MiB, the same as the total
bundle/exchange bound, so it is inert at the defaults — it only acts once an operator lowers it.
Resolved highest-precedence first: `--max-object-bytes N` on the command itself (on `bundle import`,
`bundle preview`, `bundle verify`, and `sync accept`), else the repository's own
`incoming.max-object-bytes` in `prikk config` (not available to `bundle verify`, which opens no
repository), else the default. The bound is never taken from the input it bounds — nothing inside a
bundle or exchange artifact can raise it, and nothing an import or accept writes can reach
`.prikk/config`.

**All three bundle-reading commands now stream a regular file** rather than buffering it whole
before the store sees a byte, but by different amounts, because each needs a different amount held
in memory while it works:

- **`bundle import`** stays an open file the store reads and re-reads in place, deciding and
  writing one object at a time; its own memory follows roughly the size of the largest single
  object the bundle carries, not the whole bundle (measured, one 16 MiB object: about four times
  the object's own size).
- **`bundle verify`** decodes one object at a time and keeps nothing past its own turn, the same
  shape as `bundle import` — its own memory also follows roughly the largest single object, a
  little lower than `import`'s own figure since it never writes (measured, one 16 MiB object:
  about 2.3 times the object's own size, down from about 3.3 times before).
- **`bundle preview`** reads an object by seeking to its own range too, and never keeps a decoded
  `Blob` past the one call that read it (a `Blob` is file content, not closure metadata, so
  holding every one read would rebuild the whole bundle in memory one blob at a time). What is
  left is the comparison's own real working set: for one object, that is the object itself
  (measured, one 16 MiB object: close to `import`/`verify`'s own figure on the identical bundle);
  for a bundle that creates many files at once, the comparison genuinely holds each file's own
  content once while it decides what changed, so its own peak still grows with how much a bundle
  changes, not with the bundle's own size on disk (a mixed 158 MiB bundle peaks around 162 MiB,
  down from about 640 MiB before this release's own streaming work, where `import`/`verify` on the
  identical bundle peak under 15 MiB).

A pipe or FIFO input to any of the three is unchanged: a non-seekable input cannot be read twice,
so it is still read into memory whole before anything else happens.

**`prikk config`** is a repository's own small, durable settings file at `.prikk/config` — never in
the worktree, so a checked-out or imported file can never set one. One `key = value` per line; blank
lines and `#` comments are allowed. An unknown key, a duplicate key, or an invalid value (zero,
negative, non-integer) each refuse, naming the line, never a silent fall back to the default. The
only key today is `incoming.max-object-bytes`. `get`/`list` print the effective value and its source
(`(default)` or `` `.prikk/config` ``); `set` validates then writes atomically; `unset` restores the
default. **`set` rewrites the whole file and `unset` removes it, so a comment you wrote by hand — which
the format allows — is dropped without a word:** keep notes somewhere else. An older `prikk` build simply does not read this file — it has no effect on that build's
own behavior, and no error results from a repository holding one.

**Exit codes.** `0` — the operation succeeded and did what was asked. `1` — operational failure:
verification findings, an integrity failure, a refusal, a dirty worktree. `2` — usage error: an
unknown argument, a missing required flag, a duplicate flag. Graded verification results are in
`prikk verify --format json`, not in the exit code.
