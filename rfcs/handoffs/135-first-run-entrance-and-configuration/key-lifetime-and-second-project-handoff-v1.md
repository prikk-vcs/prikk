# RFC 135 §9 — what `first-run.md` does not survive: a reboot, and a second project

**Ruled:** `rfcs/done/135-first-run-entrance-and-configuration.md` §9, which `docs/src/guide/first-run.md`
names as its own provenance. **Opened on the project owner's question**, 2026-09-10: *"Does it support
scenarios around local machine reboot and another project creation?"*

**The answer is no, twice.** Both scenarios are supported by the software and absent from the page.
**Everything below was verified against the shipped 0.38.0 binary**, not read off the source, and the
exact outputs are quoted so you do not have to rediscover them.

## 1. Gap one — the exports do not survive the shell, and the page never says so

`first-run.md` says *"Run the printed `export` lines"* and stops. **`export` is session-scoped.** After a
reboot, a new terminal, or a new tmux pane, prikk is unusable until the seeds are re-exported, and the
page says nothing about it.

**Verified** — with the four variables unset, in a working repository:

```
error: author signing is required: set PRIKK_AUTHOR_KEY_ID (no signing key configured)
```

**That error is good.** It names the variable and the cause. **The page is what is missing**, not the
message.

**And there is a sharper edge the page currently states as a feature.** It says, of default `setup`:

> *"With neither `--author-seed-out` nor `--maintainer-seed-out`, both seeds print once, here, and
> nowhere else."*

**True, and it means a user who closes the terminal without saving them has permanently lost both
keys** — the AUTHOR key that signs their patches and the MAINTAINER key their repository has already
trusted. Today that sentence reads as a security property. **It is also a data-loss warning and the
page does not say so.**

**What to add**, and keep it short — this page is an on-ramp, not a key-management manual:

- **That `export` lasts only as long as the shell**, said once, plainly.
- **What to do about it.** Do not invent a policy: state the options that exist and let the reader
  choose — `--author-seed-out`/`--maintainer-seed-out` write the seed to a file at mode `0600`
  (**and on Windows refuse outright**, which the page already documents), and a seed the reader already
  holds can be re-exported from wherever they keep it. **prikk never manages a secret's lifecycle**, and
  the page must not imply it does.
- **That an unsaved default-`setup` seed is unrecoverable**, next to the sentence that currently makes
  it sound safe.

**Do not recommend a specific secret store, dotfile, or `~/.profile` edit.** Naming one would be prikk
telling a reader where to keep a secret, which is exactly the commitment `key generate --out`'s design
refuses.

## 2. Gap two — a second project is entirely uncovered

**Verified, and it works — in four steps nobody is told about.** Reusing an existing key pair in a new
repository:

```sh
prikk init .
# export the same PRIKK_AUTHOR_* and PRIKK_MAINTAINER_* values
prikk commit --from-worktree -m "genesis"   # works: AUTHOR needs no trust registration
prikk seal --allow-no-audit                 # FAILS -- see §3
export MY_SEED="$(cat ./maintainer.seed)"
prikk key public --seed-env MY_SEED         # derive the public key
prikk trust maintainer add --key-id maintainer --public-key <hex>
prikk seal --allow-no-audit                 # now succeeds
```

**The fact that explains all of it, and the page never states it: trust is per-repository.** An AUTHOR
key is not registered anywhere, so it travels for free. **A MAINTAINER key must be trusted again in
every repository**, because adoption is a per-repository trust act. The page's "Which key is which"
section describes the roles correctly and **never says that one of them is repository-scoped.**

**What to add:** a short section — *"A second project"* — covering both routes and when each applies:

- **`prikk setup ./other-repo` again** — fresh keys for a separate project. Simple, and the right
  default when the projects are unrelated.
- **Reuse the keys you have** — the flow above. Right when it is the same person and the same trust
  domain, and the only extra step is one `trust maintainer add`.

**State the per-repository trust fact once, in "Which key is which".** It is the fact both routes rest
on and it belongs where the roles are defined, not only in the new section.

## 3. Two product findings, reported not fixed — rule on them separately

**Neither is in this round's scope.** Both were found while verifying §1 and §2 and are recorded so
they are not rediscovered.

### 3a. `seal` misreports an untrusted maintainer as a damaged trust policy

In a fresh repository with a valid maintainer seed exported but **no key trusted yet** — the guaranteed
state of every second project:

```
error: integrity error: publication trust policy is missing or unreadable
```

**The policy is neither missing nor unreadable. Nothing has been adopted yet.** `Integrity` reads as
corruption, and the message sends a reader looking for damage instead of running `trust maintainer add`.
Compare §1's author-key error, which names the variable and the fix.

**This may be an RFC 132 re-open trigger** — the error taxonomy was closed with named triggers, and a
`Precondition` misclassified as `Integrity` on a path every second project hits looks like one. **Check
RFC 132's trigger list before proposing anything.** Do not change the message in this round.

### 3b. `prikk setup` on an existing repository half-runs, then fails on the wrong axis

```
initialized Prikk repository at <path>/.prikk
error: invalid signature: maintainer key id maintainer is already adopted with a different public key
```

**Verified harmless**: the repository is undamaged (`verify` clean, refs intact — `init` is idempotent
here), and **no seed was printed before the failure**, so nothing leaked into scrollback. **But the
sequence reads as a partial write**, and `invalid signature` is the wrong axis — nothing's signature was
invalid; a key id collided. A reader trying `setup` for a second project **in a directory that already
has one** gets no usable next step.

## 4. Scope

- **`docs/src/guide/first-run.md` only.** If `security-setup.md` or `git-mapping.md` need a
  cross-reference, add the link and nothing else.
- **No new command, no behaviour change, no error-message change.** §3 is reported, not actioned.
- **Do not touch the Claim-to-Source Anchors table's existing rows**; add a row if you make a new claim
  that needs one, and **anchor it to source you actually opened.**

## 5. Controls

1. **Run every command sequence you document, on the shipped binary**, and paste real output. This
   handoff's own sequences were verified that way; **yours must be too, including any you rewrite.**
2. **The second-project sequence must be run end to end from a genuinely empty directory** — not from
   the repository you already have exported variables for. A stale export is exactly what would make a
   broken sequence look correct.
3. **Check the page's existing claims still hold** while you are in it. `docs/` has drifted before, and
   the round that finds a stale claim it was not looking for is the round that earns its keep.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim. **Cross-target addendum: state the
   outcome or state why it does not apply** — check this round's own diff, do not infer it from the fact
   that this is a docs round.

## 6. Report

- **The two new sections, in full.**
- **Every command sequence with its real output.**
- **Anything you found stale in the existing page**, or an explicit statement that you checked and found
  nothing.
- **Your reading of §3a and §3b against RFC 132's trigger list** — a recommendation, not a change.
