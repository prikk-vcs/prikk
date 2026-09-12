# Troubleshooting

Refusals a newcomer actually hits, in the [Tutorial](tutorial.md)'s own sequence, each quoted
verbatim from the CLI. If a message here reads as confusing, that is worth reporting — this page
explains what exists today; it does not change any wording.

## `error: author signing is required: no seed at <path>`

prikk looked for your AUTHOR seed and did not find it. Every commit produces a signed Patch; there is
no unsigned path. The message names the exact file and three ways to get one — create it with
`prikk key generate --out <that path>`, run `prikk setup` in a new project directory, or point
`PRIKK_AUTHOR_SEED_FILE` at a seed file you already have. `seal` says the same for the MAINTAINER
seed. See [First Run](first-run.md) for where the key directory is on each platform.

## `error: precondition not met: PRIKK_AUTHOR_SEED is no longer read`

Your shell still exports a seed as an environment variable. prikk stopped reading that channel in
0.40 and **refuses rather than ignoring it**, so a stale variable can never silently become the key
you are not signing with. Remove it from your shell profile; your keys are in the key directory the
message names, or wherever `PRIKK_AUTHOR_SEED_FILE` points. The refusal itself is removed in 0.41.0.

## `error: <path> is readable by group or other (mode 0644); run `chmod 600 <path>``

A seed file anyone but you can read is refused before signing. Run the `chmod` the message names.
(Unix only — on Windows the key directory relies on `%APPDATA%`'s per-user ACL instead.)

## `error: <path> must be 64 hex characters, got 8`

A seed file must hold exactly 64 lowercase hex characters (32 raw bytes) — the number after `got`
reports how many your file actually had, so it will differ from the `8` shown here. A shorter,
longer, or non-hex value is rejected before anything is signed; nothing is written to the repository
when this fires.

## `error: invalid name: worktree has no node-addressed changes to commit`

You ran `commit` with nothing changed since the last commit or seal — no new, edited, or deleted
files for Prikk to record. Change something first.

## `error: maintainer signing is required: no seed at <path>`

`seal` needs a *maintainer* key, distinct from the *author* key `commit` used — see
[why the tutorial needs a second key](tutorial.md#sealing--and-the-second-key). Same three routes as
the AUTHOR case above, with `maintainer.seed` in place of `author.seed`.

## `error: precondition not met: no maintainer key is adopted in this repository yet`

Your maintainer key is configured but not yet trusted by this repository. Run
`prikk trust maintainer add --key-id ID --public-key HEX` with the public half of your maintainer
key, then seal again. This is a repository-local, trust-on-first-use step — every fresh repository
needs it once, even with a key you have used elsewhere.

The message's parenthetical names the one other way this refusal can be reached: a trust policy
container damaged so badly it replays as empty is indistinguishable, at this read, from one never
written. If you know a key *was* adopted here before, run `prikk doctor`.

Earlier releases reported this same refusal as `error: integrity error: publication trust policy is
missing or unreadable`, which sent readers looking for corruption that was not there.

## `error: precondition not met: <path>: worktree symlink authoring is out of scope`

`commit` found a symlink where it needs a regular file — an untracked symlink in the worktree, or a
tracked file that has been replaced by one, including a **dangling** symlink whose target does not
exist. Symlink authoring is not implemented yet,
so the commit refuses rather than authoring something it cannot faithfully replay. Replace the
symlink with a regular file, or move it outside the repository (or add it to `.prikkignore` if it is
not meant to be tracked), then commit again.

You do not have to run `commit` to find these. `prikk worktree-status` reports the same paths, with
the same reason, under `refused paths:` — see [Worktree Status](worktree-status.md).

Earlier releases reported this as `error: integrity error: worktree authoring: unsupported symlink
authoring: ...`, which sent readers looking for repository corruption. Nothing is damaged.

## `error: precondition not met: <path>: existing TextFile cannot accept non-UTF-8 content`

A path tracked as a text node now holds bytes that are not valid UTF-8. Changing a node's kind from
text to binary (or back) is out of scope, so the commit refuses rather than silently changing what
the node is. Restore valid UTF-8 content at that path, or delete the path and add it again as a new
node, then commit again.

Earlier releases reported this as `error: integrity error: worktree authoring: unsupported kind
transition: ...`. Same refusal, correct class: it is a state you fix, not damage.

## `error: precondition not met: --target names a tag (tags/<t>); pass the block id it points at, or a branch ref`

You ran `prikk tag create --target` against another tag. A tag ref is one hop from its Block — ref →
tag object → block — and a tag of a tag is outside that model, so Prikk refuses rather than quietly
tagging the Block instead: that would make "tag this tag" and "tag this block" produce identical
history. Pass the block id (`prikk tag list` shows each tag's target block) or a branch ref.

`prikk branch create --from` is the other way round: it *does* dereference a tag, because `--from`
means "at the block this ref names".

Earlier releases reported this as `error: object type mismatch: expected block, got tag`, which read
as repository damage rather than as a correctable argument.

## `error: precondition not met: <path> already holds a repository`

You ran `prikk setup` on a directory that already has a `.prikk` in it. `setup` is the first-run
command: it initializes a repository *and* mints a fresh key pair, so there is nothing sensible for
it to do on a repository that already has adopted keys. It refuses before doing anything — no
`init`, no keys minted, no seed file written, even when you passed `--author-seed-out` or
`--maintainer-seed-out`.

Two ways on, both named in the message: to work in the existing repository with keys you already
hold, run `prikk trust maintainer add --key-id ID --public-key HEX` (use `prikk key public --role
maintainer` to derive the public half); to start a new project, point `setup` at a different
directory.

Earlier releases got further before failing: they printed `initialized Prikk repository at …`, minted
two keys, wrote any `--*-seed-out` file, and only then reported `maintainer key id maintainer is
already adopted with a different public key`. If you hit that version, the seed files it left behind
belong to keys **no repository adopted** — they are not your keys, and can be deleted.

## `error: integrity error: index entry for <id> resolves to an envelope with computed id <other>`

The object index points at the wrong record: the entry claims an object lives at a location that
actually holds a different one. Your objects are intact — the containers are self-describing and
carry every byte needed — it is only the index that is wrong. Run

```sh
prikk doctor --repair-index
```

which rebuilds the index by scanning the containers, touches no container bytes, and reports how many
entries it moved. Then `prikk verify` again.

This happened when two commands that both write objects ran at the same moment — for example a
`prikk commit` and a `prikk tag create`. Since prikk 0.40 that is prevented: the second command is
refused with `lock conflict` instead, so a repository cannot reach this state any more. The repair
exists for repositories damaged before that.

## `error: precondition not met: checkout target for <ref> does not contain a snapshot blob`

The block you asked to check out has no snapshot, which is the normal state of every block in every
repository today — no block-creating path writes one yet. Use the patch-replay route instead, which
does not need a snapshot:

```sh
prikk checkout --patch-plan --ref <ref>
```

Earlier releases reported this as `error: integrity error: checkout target for <ref> does not contain
a snapshot blob`, which read as damage. Nothing is damaged, and nothing was ever missing.

## `error: active WAL has no patch records to seal`

You ran `seal` with nothing queued — every commit since the last seal has already been published.
There is nothing to fix; commit something before sealing again.

## `error: invalid name: ref topic is not a local branch ref; expected heads/<name>`

You passed a bare name (`topic`) where Prikk expects a fully-qualified ref
(`heads/topic`). `branch create`, `branch close`, `tag create`, and any `--ref` flag all take the
qualified form — see [why refs are fully qualified](faq.md#why-is-it-headstopic-and-not-topic) for
the reasoning. Re-run the same command with `heads/` (or `tags/` for a tag) in front of the name.

## `error: i/o error: repository mutation requires Linux, macOS, or Windows root-scoped filesystem capabilities`

You are running `init`, `commit`, or `seal` on a platform other than Linux, macOS, or Windows.
Reading commands (`verify`, `log`, `status`, `doctor`) work anywhere Prikk builds; mutation does
not, by design — see [Platform Support](../reference/platform-support.md).

## Something not listed here

Run [`prikk doctor`](tutorial.md#doctor) — it is the diagnostic-first command, and its recommendation
lines usually name the next step. If it does not, the message is real (Prikk does not invent
placeholder diagnostics) but this page has not caught up with it yet.
