# FAQ

The conceptual questions the [Tutorial](tutorial.md) raises but does not stop to answer, and the
ones that decide whether a first repository becomes a second one.

## What do I run first?

`prikk init`, then work through the [Tutorial](tutorial.md) — it is the shortest path from an empty
directory to a sealed, verified repository, and it is exercised by an automated test, so it will not
silently go stale.

## What is sealing, and do I have to do it?

**Yes, if you want the commit to count as history.** `commit` only appends a signed Patch to a local,
per-session write-ahead log — nothing is published yet, and nothing else can see it. `seal` is the
step that turns everything queued in that log into a signed `Block`, publishes a `RefState` pointing
at it, and makes it part of the ref's real history. You can commit many times before sealing (they
queue), but until you seal, there is no history to check out, verify against, or share — only a
pending queue `status` can show you.

There is no auto-seal. A workflow that only commits and never seals is a workflow that never
publishes anything.

## How many commits can I queue before sealing?

Two environment variables bound the queue, both fail-closed on a malformed value:

- `PRIKK_ACTIVE_PATCH_WARN` — warn at this many queued patches (default 800)
- `PRIKK_ACTIVE_PATCH_LIMIT` — refuse further commits at this many (default 1000)

The limit is checked before any write, so a refused commit leaves no partial state. Committing and
sealing one-for-one still works exactly as before; nothing forces accumulation.

## Why does this need keys at all?

Every commit and every seal in Prikk is a **signed** object, not a plain record — there is no
unsigned mode. Two different roles sign two different things:

- **Author** — signs the Patch a `commit` produces. Identifies who authored a change.
- **Maintainer** — signs the `Block`/`RefState` a `seal` produces, and must be a key the
  repository has been told to trust first (`trust maintainer add`). Identifies who published it.

Prikk keeps these separate even for a single-person repository, because the two signatures answer
different questions later: *who wrote this content* and *who decided it should become history*. See
[Trust and Threat Model](../reference/trust-threat-model.md) for what each signature is actually
checked against, and [Security and Signing Setup](security-setup.md) for how key material is
currently supplied — environment variables, not a keychain or config file, and intentionally minimal
at this stage.

## Why is it `heads/topic` and not `topic`?

Every ref name is fully qualified: branches are `heads/<name>`, tags are `tags/<name>`. A bare name
is refused:

```
error: invalid name: ref topic is not a local branch ref; expected heads/<name>
```

The qualification exists so a ref's *kind* is legible from its own name, without needing to look
anywhere else to know whether `v1` names a branch or a tag. There is currently no default or implicit
prefix — every command that names a ref (`branch create`, `tag create`, `--ref`) takes the qualified
form.

## How do I switch branches?

**There is no `branch switch` yet.** There is a current branch: `.prikk/current-branch` names one
local branch, `init` and `setup` set it to `heads/main`, and every command that takes `--ref`
defaults to it. This is the CLI's own answer, shown whenever you run `prikk` with no arguments:

```
note: no `branch switch` yet; `--ref` defaults to `.prikk/current-branch`
```

In practice: to work on another branch, name it with `--ref heads/<name>` on each command — an
explicit `--ref` always wins over the default. `prikk branch list` shows what exists and marks the
current branch with `*`.

## Is my data safe in a Prikk repository right now?

Prikk is early implementation software and is not a production Git replacement. Nothing in this
tutorial, FAQ, or troubleshooting page is a claim otherwise — work through them with content you can
afford to lose, and see [Platform Support](../reference/platform-support.md) and
[Non-Goals](../reference/non-goals.md) for what is and is not currently guaranteed.

## Something else?

If a real refusal sent you here and this page does not answer it, check
[Troubleshooting](troubleshooting.md) first — it is organized by the exact message you saw, not by
concept.
