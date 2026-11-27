# An Author Proposes, the Receiving Maintainer Seals

This guide is for the case where **one person or process writes changes** and **another person decides what enters their repository**: an author on one machine, a maintainer on another. The author's repository is the author's own. The maintainer's repository is the maintainer's own. What moves between them is a file, and **the maintainer seals what they accept under their own key.**

Every command below was run on a release build of the code this page describes, with test keys in two separate home directories. The output shown is that run's, with the machine's scratch path taken off. For what a seal is and what each key does, see [Security and Signing Setup](security-setup.md); for the commands used here, see [Sync](sync.md).

## The shape, in one paragraph

The author repository **seals under its own maintainer key**, a key made for that repository alone and adopted nowhere else. The author runs `sync build` and sends the file. The maintainer runs `sync accept` (which reads no seed), **reviews what is pending**, and runs `sync seal --claims`, which **re-seals under the maintainer's own key**. The author's seal reads **`Unverifiable`** on the maintainer's side, because the maintainer never adopted the author's key. **That is the expected answer, and it is the point:** the maintainer's repository trusts only the maintainer's key.

## The rules

1. **Never run a command that reads your maintainer seed in a repository someone else wrote or handed you.** **Every command that seals or signs a publication reads it**, for example `seal`, `merge`, `branch create`, `branch close`, `tag create`, `sync build`, `sync seal`, `sync adopt-tag`, `bundle import --adopt` and `ref complete` (even with `--plan-only`); what they write is signed by your key. A repository's trust policy is whatever its writer wrote: it can adopt your *public* key (a public key is public) and then your `seal` or `sync build` there lends your key to everything that repository holds. Receive another person's work by **`sync accept` into your own repository**, never by opening theirs.
2. **Never run `prikk setup` for an author repository in the receiving maintainer's key directory without both seed-out flags.** Without them `setup` reuses the keys already there (it says `using your keys in <dir>`) and adopts **the maintainer's own key** in the author repository: the author's seal is then the maintainer's. See [If the two must share an account](#if-the-two-must-share-an-account).
3. **The separation between an author's machine and a maintainer's is an operating-system boundary, not a prikk check.** Another account, another machine or a container keeps the author's process from reading the maintainer's seed. **A process of the same account can read every seed of that account** (a seed file is mode `0600`: private to the account, not to the processes in it). `prikk key status` tells you which key is in effect; it does not make a boundary.
4. **The bundle route is not for proposals.** `bundle import` lands what it receives under `remotes/` and never seals it; `bundle import --adopt` lands nothing for a key the receiver has not adopted. See [Why not a bundle](#why-not-a-bundle).

## 1. The author's repository, with its own keys

The author makes the repository **with both seed-out flags**, into a directory that **already exists** (prikk does not create the directory a secret lives in, and refuses before writing anything if it is missing or not writable):

```
$ prikk setup author-repo --author-seed-out keys/author.seed --maintainer-seed-out keys/maintainer.seed
initialized Prikk repository at author-repo/.prikk
trusted maintainer key: ed25519-7acc9f06da8228d9
adopted maintainer keys: 1

set these before committing:
  # author key id: ed25519-3b6f02756e8ea186
  export PRIKK_AUTHOR_SEED_FILE="keys/author.seed"
  # maintainer key id: ed25519-7acc9f06da8228d9
  export PRIKK_MAINTAINER_SEED_FILE="keys/maintainer.seed"

next steps:
  prikk commit -m "<message>"
  prikk seal --allow-no-audit  # no audit trust policy is configured yet; see `prikk seal --help`
```

The repository adopted **only** the key it just made. Export the two lines it printed. Then ask `key status` which key is in effect in this repository, and whether it is the one the repository adopted:

```
$ prikk key status author-repo --role maintainer
role: maintainer
source: seed-file-override
path: keys/maintainer.seed
usable: true
key id: ed25519-7acc9f06da8228d9 (key-file keys/maintainer.seed.key-id)
public key: 7acc9f06da8228d91e66413644d6c9e1c62d25b2ba05f9bceb5a0e5bfa271a23
binding: matches
```

**`binding: matches`** is the witness: the key in effect is the one this repository adopted. Without the exports it reads `binding: not-adopted` (see the last section), and a `seal` refuses.

The author works and seals as usual:

```
$ prikk commit -m "add the first page"
recorded worktree patch in active WAL
…
$ prikk seal --allow-no-audit
sealed active WAL into block
patches: 1
block id: 0cb9841a347ec690c485fe1757d1ed2edd154dc0075255db0f0459a8a46922c2
…
```

## 2. The exchange

The maintainer says what they hold, the author builds what is missing, the maintainer accepts it. Two files cross between the machines (`have.bin` and `proposal.bin`); the third (`claims.txt`) is written and read on the maintainer's side. None of them carries a secret (a sync artifact carries repository content **in the clear**: move it over a channel you trust).

```
maintainer$ prikk sync have heads/main --output ../exchange/have.bin
have-list for heads/main
wrote ../exchange/have.bin

author$ prikk sync build heads/main --have ../exchange/have.bin --output ../exchange/proposal.bin
built sync artifact for heads/main
delta patches: 2
claims: 2
tags: 0
blobs: 2 | author key material: 1
wrote ../exchange/proposal.bin
note: this artifact contains repository content in the clear -- prikk does not encrypt it; move it only over a channel you trust

maintainer$ prikk sync accept ../exchange/proposal.bin --claims-out ../exchange/claims.txt
accepted sync artifact
patches: 2
blobs: 2
claims: 2
tags: 0
new objects: 6
author key material: 1 recorded (continuity only, not a trust decision)
  claim 666b27f8…: Unverifiable (ed25519-7acc9f06da8228d9 not adopted here -- its order is unattributed)
  claim 188780f2…: Unverifiable (ed25519-7acc9f06da8228d9 not adopted here -- its order is unattributed)
wrote claim ids to ../exchange/claims.txt
```

**Each claim reads `Unverifiable`, naming the author repository's key.** That is the author's seal, **recorded and not trusted**. Nothing is sealed in the maintainer's repository yet: `sync accept` reads no seed and writes no block.

## 3. The review

Before sealing, the maintainer sees **every pending patch**, what it does, and the files it would produce. None of these commands reads a seed.

```
maintainer$ prikk sync pending
pending (accepted, unsealed) patches: 2
  b7ab0df9d0a9bd5d7c15e4499067064b19c5f22963173117c5e85929a6489b61
  d822cd37dea32c47c95cfb93ea3a111220ef567c6dd698b0faf76ede6a4dfba4
```

`sync pending --format json` carries, for each patch, the claims that name it and **each claim's own signature outcome** (`unverifiable` here). **`invalid` would be an alarm** (a claim whose signature does not verify), not the expected answer.

```
maintainer$ prikk show b7ab0df9d0a9bd5d7c15e4499067064b19c5f22963173117c5e85929a6489b61
patches: 1

patch b7ab0df9…
resolved through claim: 188780f2…
  operation 1: edit-text
    path: a.txt
    old:

    new:
a second change
```

`show` prints each operation, its paths and its content. To read the **files** a claim leads to, extract them somewhere harmless:

```
maintainer$ prikk extract --claim 666b27f8… ../exchange/first-claim-files
extract repository: receiver-repo/.prikk
claim: 666b27f8…
target block: 0cb9841a347ec690c485fe1757d1ed2edd154dc0075255db0f0459a8a46922c2
chain: 1 link(s), root to foot
  link 1: claim 666b27f8… (unverifiable)
unverified: at least one link's MAINTAINER signature names a key this repository has not adopted
files written: 2
```

And `prikk verify` on the maintainer's repository, which at this point still holds only what `sync accept` stored: `object items: 6 scanned, 0 failed`, **`unverifiable author signatures: 0`** (no patch signature was left unchecked), exit 0.

**What no command shows today:** *which author key signed a patch*. `sync accept` prints a **count** (`author key material: 1 recorded (continuity only, not a trust decision)`); `show`, `sync pending` and `log` do not print the key id. An author key is continuity, never a trust decision.

## 4. The seal

When the maintainer has read what they accept, they seal **under their own key**:

```
maintainer$ prikk sync seal heads/main --claims ../exchange/claims.txt
claim 666b27f8…: sealed 1 patch(es) into heads/main
  block id: 0cb9841a347ec690c485fe1757d1ed2edd154dc0075255db0f0459a8a46922c2
  heads/main RefState: a541dc7159fe1bf96ffa2185f1fee0107af6475501f022542e8987bfd8db515f
  claim signature: Unverifiable (ed25519-7acc9f06da8228d9 not adopted here)
claim 188780f2…: sealed 1 patch(es) into heads/main
  block id: 77773c5c230835719ca337750031f26bfe6d73f96cfe4c21f9a88b6643af20dd
  …
  claim signature: Unverifiable (ed25519-7acc9f06da8228d9 not adopted here)
materialized into the worktree: files written: 2, unchanged: 0, deleted: 0
```

The block ids equal the author's: the same patches, the same parents, the same block. **The signature on the block is the maintainer's.** `prikk verify` now reads `publication trust issues: 0`, and the sealed blocks are signed by the maintainer's key alone:

```
sealed-block 0cb9841a…: ed25519-9eac5f0e1caf5c6e
sealed-block 77773c5c…: ed25519-9eac5f0e1caf5c6e
```

`sync seal` is the only step on the maintainer's side that reads the maintainer's seed, and it reads it **in the maintainer's own repository**, over patches that repository accepted and the maintainer reviewed.

## Why not a bundle

A bundle is one ref's history with its signatures. `bundle import` **never seals**: it lands the history under `remotes/`, as received and untrusted.

```
maintainer$ prikk bundle import --input ../exchange/proposal.bundle --adopt
received remotes/heads/main
not adopted: RefState b1c54a36… is signed by ed25519-7acc9f06da8228d9 and this repository has not adopted that key; adopt it with `prikk trust maintainer add --key-id <id> --public-key <hex>` (…)
…
note: no local ref was created or advanced, and no MAINTAINER key was trusted; …
maintainer$ prikk log
history: <empty>
maintainer$ prikk verify
…
publication trust issues: 4
error: repository has publication-trust issues
```

`--adopt` lands a branch only if the key that signed it is **already adopted**; for a proposal it is not, so nothing lands, and `verify` fails until the maintainer adopts the author's key (a trust decision the proposal flow exists to avoid). **Use `sync build` and `sync accept`.**

## If the two must share an account

Same-account separation is the weakest the guidance allows: a process of the account can read every seed in it. If you must, keep the author's keys in **their own directory** and make the repository with both seed-out flags (§1). Two things to know:

- `setup` **without** the flags **reuses the key directory's keys** and adopts the maintainer's own key in the new repository:

  ```
  $ prikk setup shared-account-author-repo
  initialized Prikk repository at shared-account-author-repo/.prikk
  trusted maintainer key: ed25519-9eac5f0e1caf5c6e
  adopted maintainer keys: 1

  using your keys in home-receiver/.config/prikk
  every new shell finds them -- nothing to export
  ```

  The author's seal in that repository **is** the maintainer's seal. Do not use this for an author repository.
- A shell **without** the exports reads the maintainer's key from the default key directory, and the repository (which adopted only its own key) **refuses to seal under it**, with `key status` as the witness:

  ```
  $ prikk seal --allow-no-audit
  error: precondition not met: maintainer signer key id ed25519-9eac5f0e1caf5c6e is not trusted by policy; run `prikk trust maintainer add` for it, or export a key id this repository already trusts
  $ prikk key status --role maintainer
  source: key-directory
  key id: ed25519-9eac5f0e1caf5c6e (key-file home-receiver/.config/prikk/maintainer.key-id)
  binding: not-adopted
  ```

  **Do not follow the sentence's first suggestion** (`trust maintainer add` for that key): it adopts the maintainer's key in the author repository, which is exactly what the rules above forbid. Export the two lines `setup` printed instead, and `key status` reads `binding: matches`.

## What is proposed, and not yet part of prikk

How a repository's trust changes (who may adopt a key, retiring a key from a recorded point, keeping history verifying across a rotation) is **proposed in RFC 172**. It is a proposal: nothing in it is built, and nothing on this page depends on it.
