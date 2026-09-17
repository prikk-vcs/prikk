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

`prikk setup` composes `init`, key generation for both roles, and `trust maintainer add` into one
command:

```sh
prikk setup ./my-repo
```

```
initialized Prikk repository at ./my-repo/.prikk
trusted maintainer key: ed25519-cd1bc694acba34fe
adopted maintainer keys: 1

your keys are in /home/you/.config/prikk
every new shell finds them -- nothing to export

next steps:
  prikk commit -m "<message>"
  prikk seal --allow-no-audit  # no audit trust policy is configured yet; see `prikk seal --help`
```

**There is nothing to export and nothing to copy.** Both seeds are written as files in your key
directory, and every later `prikk` run finds them there — this shell, tomorrow's shell, after a
reboot. Commit and seal directly:

```sh
echo "hello prikk" > ./my-repo/readme.txt
(cd ./my-repo && prikk commit --from-worktree -m "genesis")
(cd ./my-repo && prikk seal --allow-no-audit)
```

**Each new key gets its own key id.** A key id is the name a signature carries.
`setup` names each key it creates `ed25519-` followed by the first 16 hex characters of its public key,
and writes that id beside the seed. Your keys' ids are therefore yours alone: two people who each ran
`setup` can exchange history, and each adopts the other's maintainer key under its own id. The id is a
name, not a proof. What makes it an identity is that a repository binds one id to one public key.
`prikk key status` shows the id in effect and where it came from.

Neither command names a branch. Every command that takes `--ref` defaults to the branch named in
`.prikk/current-branch`, which `init` and `setup` set to `heads/main`; `--ref` given explicitly
always wins.

## Where your keys live

One directory, per platform, and prikk resolves it itself:

| platform | key directory |
|---|---|
| Linux, macOS, BSD | `$XDG_CONFIG_HOME/prikk`, or `$HOME/.config/prikk` when `XDG_CONFIG_HOME` is unset |
| Windows | `%APPDATA%\prikk` |

It holds `author.seed` and `maintainer.seed`, plus `author.key-id` and `maintainer.key-id`, each
holding the id of the seed beside it. **A key-id file must belong to its seed.** If the file does not
hold the id derived from the seed in use (a replaced seed, a copied file, or hand-edited content),
signing is refused, and `prikk key status` reports the key unusable, naming the file and both ids.
To sign under an id of your choosing, set `PRIKK_AUTHOR_KEY_ID` / `PRIKK_MAINTAINER_KEY_ID`; never edit
the file. On Unix the directory is mode `0700`
and each file `0600`; **prikk refuses to read a seed file that group or others can read**, naming the
mode and the `chmod` that fixes it. On Windows there are no mode bits: the directory relies on
`%APPDATA%` being per-user by platform ACL, which is stated here rather than assumed — it is the one
location where prikk accepts inherited permissions, because they are the right ones and they are
known.

**`--author-seed-out <path>` / `--maintainer-seed-out <path>` override the location.** A seed written
somewhere else is not found automatically, so `setup` prints the `PRIKK_AUTHOR_SEED_FILE` /
`PRIKK_MAINTAINER_SEED_FILE` line that points at it, and the key id. The id lives in `<path>.key-id`,
beside the seed, and is found from there. It is printed, not exported: an exported
`PRIKK_*_KEY_ID` would override the file.

**The seeds are not recoverable.** There is no copy in the repository, no keyring, no escrow. Lose
the AUTHOR seed and you can no longer sign patches with that identity; lose the MAINTAINER seed and
you can no longer seal to a repository that already trusts it. Back up the key directory the way you
back up anything else you cannot regenerate.

**The trust decision is always shown, never performed silently.** `trusted maintainer key: <id>`
is the same line `prikk trust maintainer add` itself prints — registering a maintainer key is a trust
act, and composing the steps removes the *typing*, never the *seeing*.

## After a reboot

Nothing to do. Your keys are files in the key directory, so a new shell, a new terminal, a reboot —
`prikk commit` and `prikk seal` keep working with no setup at all. This section used to explain how
to re-export two variables after every reboot; the variables are gone.

**If you moved a seed with `--*-seed-out`**, that one is the exception: prikk does not look there on
its own, so export its `PRIKK_*_SEED_FILE` in your shell profile —

```sh
export PRIKK_AUTHOR_SEED_FILE="$HOME/keys/author.seed"
```

— or move the file into the key directory and drop the variable.

### `PRIKK_AUTHOR_SEED` and `PRIKK_MAINTAINER_SEED` are no longer read

Before prikk 0.40 a seed was passed as an environment variable. That channel is removed: a seed in
the environment leaks into process listings, shell history, and any child process, and it silently
survived in profiles long after the key had changed.

**A stale export is now simply ignored.** prikk 0.40 *refused* while either variable was set — one
release wide, so that no automation could quietly start signing with a different key than it thought
it was using — and 0.41 stopped doing even that. Either variable may sit in your shell profile
indefinitely; prikk does not read it, and does not object to it.

You can still remove it, and should, because it is a secret sitting in a file for no reason. To see
which key is actually in effect before you commit anything:

```sh
prikk key status
```

That names the file each role resolves to and whether it is usable — see
[Can I Sign Here?](security-setup.md#can-i-sign-here).

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
key id: ed25519-2d353d98420bc75c
public key: ...

next steps:
  recommended: re-run with --out <path>, which writes the seed and its key-id file
  prikk trust maintainer add --key-id ed25519-2d353d98420bc75c --public-key ...
  or save by hand -- both files, mode 0600:
    /home/you/.config/prikk/maintainer.seed  (the seed above and a newline)
    /home/you/.config/prikk/maintainer.key-id  (exactly ed25519-2d353d98420bc75c and a newline)
note: a seed saved without its key-id file signs under the shared legacy id `maintainer`, which other installations may also use
note: the same seed works as an AUTHOR key instead -- name the files author.seed and author.key-id and skip the trust step
```

**Saving by hand needs both files.** The key id travels with the seed only through its key-id file; a
seed saved alone signs under the legacy id every installation made before 0.45.0 shares.

**`--out <path>` writes the seed instead of printing it — and then it is never printed at all**, only
the public key and the next steps are:

```sh
prikk key generate --out ./maintainer.seed
```

```
wrote seed to ./maintainer.seed (mode 0600)
key id: ed25519-2d353d98420bc75c (in ./maintainer.seed.key-id)
public key: ...
...
```

`--out` also writes the key's id to `<path>.key-id`, and the id is found there whichever role uses the
seed. `--out` refuses to overwrite an existing file (the seed or its key-id file), and refuses any path with a `.prikk` component — prikk
never invents a secret's location and never manages its lifecycle (writing it once, where you asked,
is the entire commitment). **On Windows, `--out` currently refuses outright**: Unix file permissions
(mode `0600`) have no portable equivalent here without unsafe code or a new dependency, and writing a
secret at whatever permissions the filesystem happens to inherit, silently, is not acceptable. Print
and place the seed yourself instead.

### `prikk key public` — derive a public key you already have

With no arguments it reads your key directory's `author.seed`; `--role maintainer` reads the other
one:

```sh
prikk key public --role maintainer
```

```
public key: ...
```

`--seed-file <path>` reads any other seed file instead — one you moved with `--*-seed-out`, or one
from elsewhere entirely.

**The seed is read from a file, never from an argument.** A `--seed <hex>` flag would put key
material into `/proc/<pid>/cmdline` (world-readable on Linux) and into shell history; a path is not a
secret, and the seed itself never appears on the command line. This replaces the older
`--seed-env <NAME>`, which took the name of an environment variable holding the seed — there is no
longer an environment variable to name.

### `prikk trust maintainer add` — the trust act itself

```sh
prikk trust maintainer add --key-id <the key id key generate printed> --public-key <the hex key generate printed>
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
saved as `author.seed` works too, with no trust step at all.

## A second project

```sh
prikk setup ./second-project
```

```
initialized Prikk repository at ./second-project/.prikk
trusted maintainer key: ed25519-cd1bc694acba34fe
adopted maintainer keys: 1

using your keys in /home/you/.config/prikk
every new shell finds them -- nothing to export
```

**`using` rather than `your keys are in` is the whole difference.** `setup` found the seeds already
in your key directory, so it minted nothing and wrote nothing — it initialized the repository and
adopted the maintainer key you already have, which is the one step a new repository genuinely needs.
Both projects sign with the same identity, which is usually what you want when it is the same person.

Pointed at a directory that *already holds a repository*, `setup` refuses before doing anything at
all:

```
error: precondition not met: ./second-project already holds a repository; to use your existing keys
here run `prikk trust maintainer add` (see `prikk key public`), or pick a different directory for a
new project
```

**If you want the second project to have its own identity**, give the new seeds their own paths:

```sh
prikk setup ./second-project \
  --author-seed-out ~/keys/second-author.seed \
  --maintainer-seed-out ~/keys/second-maintainer.seed
```

`setup` prints the `PRIKK_*_SEED_FILE` lines for them, since prikk will not find them on its own.

### The same thing by hand

`setup` composes it; these are the steps if you would rather run them yourself:

```sh
cd ./second-project
prikk init .
prikk key status --role maintainer        # the key id line names the id to adopt
prikk trust maintainer add --key-id <that key id> --public-key "$(prikk key public --role maintainer | sed 's/^public key: //')"
```

`prikk commit` needs nothing extra — an AUTHOR key is registered nowhere, so it needs nothing from
any repository. Only `seal` needs the trust act above; without it:

```
error: precondition not met: no maintainer key is adopted in this repository yet; run `prikk trust maintainer add` (a trust policy container that replays empty reads the same way -- run `prikk doctor` if a key was adopted here before)
```

**That is the state every new repository starts in**, not a fault. Adopt the key, deriving the
public half from the seed you already hold:

```sh
prikk key public --role maintainer
```

```
public key: 27b081593fa86489f9356ef4bc0cbf5f4a5a5b708aa1a10f1a8187fd56a34801
```

```sh
prikk trust maintainer add --key-id ed25519-27b081593fa86489 --public-key 27b081593fa86489f9356ef4bc0cbf5f4a5a5b708aa1a10f1a8187fd56a34801
prikk seal --allow-no-audit
```

```
trusted maintainer key: ed25519-27b081593fa86489
adopted maintainer keys: 1
sealed active WAL into block
```

The `--key-id` must be the id `seal` signs under, which `prikk key status --role maintainer` reports;
`seal` checks the signer against what this repository trusts under that id.

## Keys made before 0.45.0

A seed with no key-id file beside it signs under the **legacy** id: `author` or `maintainer`, the role
word. Keys made by `setup` before 0.45.0 look like that, and keep that id. Nothing about an existing
installation changes, and its repositories keep committing, sealing and verifying. `setup` reusing such
a key says so:

```
note: your author key uses the shared legacy key id `author`, which other installations may also use; for a distinct one, set PRIKK_AUTHOR_KEY_ID and adopt the key under that id in each repository
```

**Every such installation shares those ids, so two of them collide.** One key id binds to one public
key in a repository, so:
- history one installation signed under `author` cannot be imported into a repository where `author`
  is already bound to the other's key;
- a maintainer key cannot be adopted under `maintainer` beside a different one.

Both refusals name the route that works: sign **new** history under a distinct id, either a key made by
`prikk key generate --out <path>` (pointed at by `PRIKK_*_SEED_FILE`) or an id set with
`PRIKK_*_KEY_ID`, and adopt the maintainer key under that id. History already signed under the shared
id keeps it; there is no command that re-signs it.

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
