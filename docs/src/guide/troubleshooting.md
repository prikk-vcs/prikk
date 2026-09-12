# Troubleshooting

Refusals a newcomer actually hits, in the [Tutorial](tutorial.md)'s own sequence, each quoted
verbatim from the CLI. If a message here reads as confusing, that is worth reporting — this page
explains what exists today; it does not change any wording.

## `error: author signing is required: set PRIKK_AUTHOR_KEY_ID (no signing key configured)`

You ran `commit` without `PRIKK_AUTHOR_KEY_ID`/`PRIKK_AUTHOR_SEED` set. Every commit produces a
signed Patch; there is no unsigned path. Set both variables — see
[Security and Signing Setup](security-setup.md) — and commit again.

## `error: PRIKK_AUTHOR_SEED must be 64 hex characters, got 8`

`PRIKK_AUTHOR_SEED` must be exactly 64 lowercase hex characters (32 raw bytes) — the number after
`got` reports how many characters your value actually had, so it will differ from the `8` shown
here. A shorter, longer, or non-hex value is rejected before anything is signed — nothing is
written to the repository when this fires. The same check applies to `PRIKK_MAINTAINER_SEED`.

## `error: invalid name: worktree has no node-addressed changes to commit`

You ran `commit` with nothing changed since the last commit or seal — no new, edited, or deleted
files for Prikk to record. Change something first.

## `error: maintainer signing is required: set PRIKK_MAINTAINER_KEY_ID (no signing key configured)`

You ran `seal` without `PRIKK_MAINTAINER_KEY_ID`/`PRIKK_MAINTAINER_SEED` set. `seal` needs a
*maintainer* key, distinct from the *author* key `commit` used — see
[why the tutorial needs a second key](tutorial.md#sealing--and-the-second-key). Set both variables
and seal again.

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
