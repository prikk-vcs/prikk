# Tutorial

This is the first thing to read after [Install](install.md) — one continuous path from an empty
directory to a sealed, verified repository: create it, make a commit, seal it, then look at what you
built. It confronts key setup directly, because `commit` and `seal` both need one and a tutorial that
hides that fails at your second command.

Prikk is early implementation software and is not a production Git replacement. Work through this
tutorial with throwaway content — nothing here is a claim that Prikk is ready to hold history you
care about yet.

## Create a repository

```sh
mkdir sample-repo && cd sample-repo
prikk init
```

```
initialized Prikk repository at /path/to/sample-repo/.prikk
```

`.prikk` is the only thing Prikk ever creates. There is no separate config step.

## Your first commit — and why it needs a key

Add a file and commit it:

```sh
echo "hello prikk" > readme.txt
prikk commit -m "genesis"
```

```
error: author signing is required: set PRIKK_AUTHOR_KEY_ID (no signing key configured)
```

**This refusal is the point, not a mistake in this tutorial.** Every commit in Prikk produces a
signed Patch — there is no unsigned-commit mode to fall back to. `commit` reads the signing key from
two environment variables, so set them and try again:

```sh
export PRIKK_AUTHOR_KEY_ID="dev-author"
export PRIKK_AUTHOR_SEED="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
prikk commit -m "genesis"
```

```
recorded worktree patch in active WAL
baseline ref: heads/main
patch id: ...
WAL sequence: 1
operations: 1
referenced blobs: 1
  create-file readme.txt
```

**The seed above is a public example, shown in this documentation and in the project's README —
never use it, or any seed you have shown anyone else, for real signing.** See
[First Run](first-run.md) for how to generate your own, and
[Security and Signing Setup](security-setup.md) for how real key material is meant to be handled;
this tutorial only needs you to get past the refusal, not to understand key management yet.

The commit is recorded, but nothing has been published — `readme.txt`'s patch sits in the active
write-ahead log until you seal it. You can commit again before sealing; Prikk queues patches and
seals them together. This tutorial seals right away.

## Sealing — and the second key

```sh
prikk seal --allow-no-audit
```

```
error: maintainer signing is required: set PRIKK_MAINTAINER_KEY_ID (no signing key configured)
```

**A different role, a different key.** `commit` signs as the *author* of a change; `seal` signs as
the *maintainer* who publishes it — Prikk keeps the two separate even when one person holds both
roles, because the trust boundary they express is different (see
[Trust and Threat Model](../reference/trust-threat-model.md) for why). Set the maintainer key and try
again:

```sh
export PRIKK_MAINTAINER_KEY_ID="dev-maintainer"
export PRIKK_MAINTAINER_SEED="111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000"
prikk seal --allow-no-audit
```

```
error: precondition not met: no maintainer key is adopted in this repository yet; run `prikk trust maintainer add` (a trust policy container that replays empty reads the same way -- run `prikk doctor` if a key was adopted here before)
```

**One more refusal, and it is also expected.** Having a maintainer key configured is not the same as
Prikk *trusting* it — a repository only publishes under keys it has been told to trust, explicitly,
once:

```sh
prikk trust maintainer add \
  --key-id "dev-maintainer" \
  --public-key "a00899dfd3357aee69729405913f9324dfc033cec04a2215239eda64ae6d9d91"
```

```
trusted maintainer key: dev-maintainer
adopted maintainer keys: 1
```

The public key above is the matching half of the example seed — trust-on-first-use means whichever
public key you adopt for a key id here is the one Prikk holds that id to from now on. Now seal
succeeds:

```sh
prikk seal --allow-no-audit
```

```
sealed active WAL into block
patches: 1
block id: ...
heads/main RefState: ...
```

Your commit is now published: a sealed `Block` on `heads/main`, with a `RefState` recording exactly
which block that ref points at.

## Look at what you built

```sh
prikk log
```

```
ref: heads/main
block ...
  update-seq: 1
  kind: Root
  parents: 0
  patches: 1
```

`Root` means this is the first block on this ref — nothing came before it.

## Check its integrity

```sh
prikk verify
```

```
verified repository: /path/to/sample-repo/.prikk
...
checked blocks: 1
...
issue summary: errors=0, warnings=0, info=1
```

`verify` re-derives every check from the objects on disk — signatures, structural links, WAL
replay — rather than trusting any cached summary. A clean repository still prints one `info` line;
that is not a problem, only a note.

## Doctor

```sh
prikk doctor
```

```
doctor repository: /path/to/sample-repo/.prikk
...
issue summary: errors=0, warnings=0, info=1
```

`doctor` runs the same structural checks as `verify` and adds repair-oriented diagnostics — this is
the command to reach for when something looks wrong, not `verify` again. On a healthy repository the
two report the same thing.

## Where to go next

- Hit a refusal this tutorial didn't cover? See [Troubleshooting](troubleshooting.md).
- Wondering *why* Prikk works this way rather than *how* to make it work? See the
  [FAQ](faq.md).
- Ready for the real key-handling story — not the public example seeds above? See
  [Security and Signing Setup](security-setup.md).
- Want the full command reference? The rest of this guide is organized one page per command.

---

**This tutorial's own command sequence is exercised by an automated test**
(`crates/prikk-cli/tests/beginners_tutorial.rs`), which runs every command above, in order, against
the compiled binary, and asserts on the same output lines quoted here — including both refusals. A
change to the CLI that alters any of them fails that test, not just this page.
