# First Run: Keys and Setup

Read this before [Install](install.md)'s next step, [Tutorial](tutorial.md) — this page is about
getting your *own* signing keys, not the shared example seed the tutorial deliberately reuses so its
own walkthrough is reproducible.

Every `prikk commit` and every `prikk seal` needs a real Ed25519 key. Before `prikk key` and `prikk
setup` existed, obtaining one meant inventing 32 bytes by hand and then having no way at all to derive
the matching public key — reproduced first-hand while preparing this project's own release testing,
by copying a public key out of a CI configuration file rather than deriving it. That gap is what this
page closes.

## The fast path: `prikk setup`

`prikk setup` composes `init`, key generation for both roles, `trust maintainer add`, and the export
lines you need, into one command:

```sh
prikk setup ./my-repo
```

```
initialized Prikk repository at ./my-repo/.prikk
trusted maintainer key: maintainer
adopted maintainer keys: 1

export these before committing:
  export PRIKK_AUTHOR_KEY_ID="author"
  export PRIKK_AUTHOR_SEED="..."
  export PRIKK_MAINTAINER_KEY_ID="maintainer"
  export PRIKK_MAINTAINER_SEED="..."
note: at least one seed above is now in your terminal scrollback -- treat it as a secret

next steps:
  prikk commit -m "<message>"
  prikk seal --allow-no-audit  # no audit trust policy is configured yet; see `prikk seal --help`
```

**The `...`s draw fresh from your OS's CSPRNG every run — copy your own, never anyone else's.**
Run the printed `export` lines, then commit and seal as usual:

```sh
export PRIKK_AUTHOR_KEY_ID="author"
export PRIKK_AUTHOR_SEED="<the value setup printed>"
export PRIKK_MAINTAINER_KEY_ID="maintainer"
export PRIKK_MAINTAINER_SEED="<the value setup printed>"
echo "hello prikk" > ./my-repo/readme.txt
(cd ./my-repo && prikk commit --from-worktree -m "genesis")
(cd ./my-repo && prikk seal --allow-no-audit)
```

**`setup` never invents a location for a seed and never reads one back.** With neither
`--author-seed-out` nor `--maintainer-seed-out`, both seeds print once, here, and nowhere else. Give
either flag a path and that seed is written there instead (mode `0600`, refusing to overwrite) and
**never printed** — see below.

**"Nowhere else" cuts both ways: nothing can recover a printed seed once the terminal is gone.**
There is no copy anywhere — not in the repository, not in a keyring, not on disk. Lose the AUTHOR
seed and you can no longer sign patches with that identity; lose the MAINTAINER seed and you can no
longer seal to a repository that already trusts it. Save both before you close the terminal, or use
the `--*-seed-out` flags below so they are written instead of printed.

**The trust decision is always shown, never performed silently.** `trusted maintainer key: maintainer`
is the same line `prikk trust maintainer add` itself prints — registering a maintainer key is a trust
act, and composing the steps removes the *typing*, never the *seeing*.

## After a reboot: `export` lasts only as long as the shell

`export` sets a variable for *that shell session*. A reboot, a new terminal window, or a new tmux
pane starts with none of them, and prikk stops working until they are exported again:

```sh
prikk commit --from-worktree -m "second change"
```

```
error: author signing is required: set PRIKK_AUTHOR_KEY_ID (no signing key configured)
```

**The error names the variable and the cause; what it cannot tell you is where your seed went.**
prikk never manages a secret's lifecycle — it does not store your seed, look it up, or know where
you keep it — so re-exporting is yours to arrange. Two facts are all you need to arrange it:

- **A seed written to a file re-exports from that file.** Both `prikk setup --author-seed-out
  <path>`/`--maintainer-seed-out <path>` and `prikk key generate --out <path>` write the seed at mode
  `0600` and print the matching `export` line for it, ready to re-run:

  ```
  export PRIKK_AUTHOR_SEED="$(cat ./author.seed)"
  ```

  (On Windows these refuse outright — see [`--out`](#prikk-key-generate--a-fresh-seed) below.)
- **A seed you already hold re-exports from wherever you keep it.** Any value that reaches the
  environment variable works; prikk only reads the variable. Choosing where a secret lives — a
  password manager, an encrypted file, a secrets service — is a decision prikk deliberately does not
  make for you, so this page does not make it either.

Whichever you choose, the public key never needs saving: `prikk key public --seed-env` derives it
from the seed again at any time.

## The commands `setup` composes — and when you'd use them directly

`setup` is not the only way in. Each step is a first-class, documented command in its own right, and
understanding them is what lets you reason about what `setup` actually did.

### `prikk key generate` — a fresh seed

```sh
prikk key generate
```

```
seed: ...
note: this seed is now in your terminal scrollback -- treat it as a secret
public key: ...

next steps:
  prikk trust maintainer add --key-id maintainer --public-key ...
  export PRIKK_MAINTAINER_KEY_ID="maintainer"
  export PRIKK_MAINTAINER_SEED="..."
note: the same seed works as an AUTHOR key instead -- export PRIKK_AUTHOR_KEY_ID/PRIKK_AUTHOR_SEED and skip the trust step
```

**`--out <path>` writes the seed instead of printing it — and then it is never printed at all**, only
the public key and the next steps are:

```sh
prikk key generate --out ./maintainer.seed
```

```
wrote seed to ./maintainer.seed (mode 0600)
public key: ...
...
```

`--out` refuses to overwrite an existing file, and refuses any path with a `.prikk` component — prikk
never invents a secret's location and never manages its lifecycle (writing it once, where you asked,
is the entire commitment). **On Windows, `--out` currently refuses outright**: Unix file permissions
(mode `0600`) have no portable equivalent here without unsafe code or a new dependency, and writing a
secret at whatever permissions the filesystem happens to inherit, silently, is not acceptable. Print
and place the seed yourself instead.

### `prikk key public --seed-env` — derive a public key you already have

If you already hold a seed — from `key generate --out`, from `setup`, or from anywhere else — derive
its public key without regenerating anything:

```sh
export MY_SEED="$(cat ./maintainer.seed)"
prikk key public --seed-env MY_SEED
```

```
public key: ...
```

**The seed is read from the named environment variable, never from an argument.** `--seed-env` takes
the variable's *name* — `MY_SEED`, not the seed itself — because a `--seed <hex>` flag would put key
material into `/proc/<pid>/cmdline` (world-readable on Linux) and into shell history. The name is not
a secret; the value never appears on the command line at all.

### `prikk trust maintainer add` — the trust act itself

```sh
prikk trust maintainer add --key-id maintainer --public-key <the hex key generate printed>
```

Registering a maintainer key is what lets `prikk seal` publish — see
[Security and Signing Setup](security-setup.md) for the full trust model, revocation, and what is and
is not enforced today.

## Which key is which

Two independent roles, two independent seeds:

- **AUTHOR** signs the Patch a `commit` queues. No trust registration needed for it at all.
- **MAINTAINER** signs the Block, RefState, and RefUpdate a `seal` publishes, and must be registered
  with `trust maintainer add` first.

**Trust is per-repository.** The trust policy a maintainer key is adopted into belongs to one
repository, so **the same maintainer key must be trusted again in every repository you seal in** —
adoption is a trust act about *this* repository, not a machine-wide or account-wide setting. An
AUTHOR key has no such step anywhere, which is why it travels for free.

`setup` generates one of each, but a single seed works as either role — `key generate` prints the
maintainer framing because that is the one role requiring a visible trust step, but the same seed
exported as `PRIKK_AUTHOR_KEY_ID`/`PRIKK_AUTHOR_SEED` works too, with no trust step at all.

## A second project

Two routes, and which one is right depends on whether the projects share a trust domain.

**Fresh keys — `prikk setup` again.** The right default when the projects are unrelated:

```sh
prikk setup ./other-repo
```

Everything above applies unchanged, including saving the printed seeds. Pointed at a directory that
already holds a repository, `setup` refuses before doing anything at all — no `init`, no keys, no
seed file — and tells you which route you probably wanted:

```
error: precondition not met: ./other-repo already holds a repository; to use your existing keys here
run `prikk trust maintainer add` (see `prikk key public`), or pick a different directory for a new
project
```

**Reuse the keys you already have.** Right when it is the same person and the same trust domain.
Only one extra step over the first project — the maintainer key must be trusted here too:

```sh
cd ./second-project
prikk init .
# export the same PRIKK_AUTHOR_* and PRIKK_MAINTAINER_* values as before
prikk commit --from-worktree -m "genesis"
```

The commit succeeds: an AUTHOR key is registered nowhere, so it needs nothing from this repository.
The seal does not, yet:

```sh
prikk seal --allow-no-audit
```

```
error: precondition not met: no maintainer key is adopted in this repository yet; run `prikk trust maintainer add` (a trust policy container that replays empty reads the same way -- run `prikk doctor` if a key was adopted here before)
```

**That is the state every new repository starts in**, not a fault. Adopt the key, deriving the
public half from the seed you already hold:

```sh
export MY_SEED="<your maintainer seed>"   # or: export MY_SEED="$(cat ./maintainer.seed)"
prikk key public --seed-env MY_SEED
```

```
public key: 27b081593fa86489f9356ef4bc0cbf5f4a5a5b708aa1a10f1a8187fd56a34801
```

```sh
prikk trust maintainer add --key-id maintainer --public-key 27b081593fa86489f9356ef4bc0cbf5f4a5a5b708aa1a10f1a8187fd56a34801
prikk seal --allow-no-audit
```

```
trusted maintainer key: maintainer
adopted maintainer keys: 1
sealed active WAL into block
```

The `--key-id` must match the `PRIKK_MAINTAINER_KEY_ID` you export; `seal` checks the exported
signer against what this repository trusts under that id.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| `prikk key generate`/`prikk key public`/`prikk setup` exist and compose `init`, key generation, and `trust maintainer add`. | [`key.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/key.rs), [`setup.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/setup.rs), [`commands.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/commands.rs) |
| A generated seed draws from the OS CSPRNG and is never accepted on argv; `key public` reads it from a named environment variable. | [`prikk-crypto/src/lib.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-crypto/src/lib.rs) (`generate_seed`), [`key.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/key.rs) |
| `--out` writes the seed at mode `0600`, refuses to overwrite, and refuses a path inside `.prikk/`; it refuses outright on Windows. | [`key.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/key.rs) (`write_seed_to_path`) |
| `setup` shows the trust decision it makes, and prints nothing that reproduces without your own OS entropy. | [`setup.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/setup.rs) |
| Maintainer trust is per-repository: `seal` checks the exported signer against the trust policy of the repository being sealed, so the same key must be adopted again in each one. | [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs) (`verify_signer_trusted`, which resolves the policy from the `RepositoryLayout` it is given) |

## Provenance

Written for [RFC 135](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/135-first-run-entrance-and-configuration.md)
§9, which measured the unfamiliar-step count to a first sealed commit at eleven, with the third step
(deriving a maintainer public key) impossible before this page's own commands existed. See
[Git → prikk](../reference/git-mapping.md) for how prikk's commands relate to Git's, including
`git config`'s row, which is where this page is cross-linked from.
