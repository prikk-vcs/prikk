# RFC 148 — Key material across sessions: a path in the environment, never a secret

**Status.** **PROPOSED 2026-09-12**, on the project owner's question — *"What happens when the local
machine is rebooted?"* — after the `first-run.md` round had documented the answer rather than changed
it. **This is the decision RFC 135 §9 deferred to the owner by name**, and the owner has now asked for
it.

**Author-review independence gap:** the architect authored this and will review its implementation.

---

## 1. What happens today, measured on the 0.39.0 binary

prikk reads exactly two secrets, both from the environment: `PRIKK_AUTHOR_SEED` and
`PRIKK_MAINTAINER_SEED` (plus the two `_KEY_ID`s). **No path in the codebase reads a seed from a file.**
`export` is shell-scoped, so after a reboot prikk has no key until the user re-exports — and the page
this project wrote for that case says *"re-exporting is yours to arrange."*

With `setup --*-seed-out`, the seed is in a `0600` file and `setup` prints
`export PRIKK_AUTHOR_SEED="$(cat ./author.seed)"`. That line contains no secret and *could* be persisted
in a shell profile — but nothing tells the user so, and every shell start then copies the secret into
the environment. Without `--*-seed-out`, the default path prints the seed once and nowhere else; after
a reboot it is gone unless the user saved it by hand.

**So the honest description is: prikk works after a reboot only if the user has arranged, unprompted,
something prikk never suggested.** That is a documented gap, not a working product.

## 2. What RFC 135 said, verbatim, and why this RFC exists

> *"Making a long-lived key easy to keep is not obviously an improvement while the machinery to retire
> one does not exist. … convenience that increases the population of long-lived keys raises the cost of
> not having lifecycle. **That trade is the owner's**, and this RFC must not smuggle it in as a
> by-product of a nicer first run."* — RFC 135 §9

RFC 135 was right not to decide it inside a first-run RFC. **The owner has now raised it directly**,
which is the trigger that section was waiting for. This RFC makes the trade explicit and small.

## 2a. AMENDED 2026-09-12 — the owner's question, and the answer is yes, remove the environment channel

> *"Is it necessary to preserve the environment variable(s)? I wonder if they can be removed when the
> permission 600 file(s) are introduced. If environment variables are unused, we won't care about them."*

**The owner is right, and §3 as first drafted was half a design.** Its rule 4 said *"the secret never
enters the environment"* — which is only true if the environment channel is gone. Keeping two channels
for one secret is what made rule 1 (the ambiguity refusal) necessary at all. **One channel needs no
ambiguity rule.** The security gain is real and specific: a secret in the environment is inherited by
every child process, visible in `/proc/<pid>/environ` to the same user, and copied into every shell
that sources the profile; a path is none of those things.

**Measured migration cost, so this is a decision and not a hope:** `PRIKK_AUTHOR_SEED` /
`PRIKK_MAINTAINER_SEED` appear at **16 sites in 3 source files** (all through one reader,
`main.rs::read_seed_env`, behind `author_signer_from_env` and `maintainer_signer_from_env`), **118 sites
in 36 test files**, **29 in 8 docs pages**, **8 in `ci.yml`** (the CI fixture exports secrets into the
environment; the release workflow does not sign via env at all), **5 in `tools/corpus`**, and **5 in the
stikk project's CLI seam.** Every source site is one function; every test site is one helper; the rest
is a sweep.

**§3 below is rewritten to a single channel.**

## 3. The design — one file, one non-secret line, no secret in the environment

The two seed environment variables are **removed**. In their place:

- **`PRIKK_AUTHOR_SEED_FILE`** and **`PRIKK_MAINTAINER_SEED_FILE`** — paths. prikk reads the seed from
  the named file at the moment of use. **These are the only seed channel.** `PRIKK_AUTHOR_KEY_ID` /
  `PRIKK_MAINTAINER_KEY_ID` are labels, not secrets, and stay as they are.

**Rules, each a security decision stated as one:**

1. **A set legacy variable is a loud refusal for one release, then nothing.** If `PRIKK_AUTHOR_SEED` or
   `PRIKK_MAINTAINER_SEED` is set, refuse: `PRIKK_AUTHOR_SEED is no longer read; write the seed to a
   0600 file and set PRIKK_AUTHOR_SEED_FILE to its path`. **Silently ignoring a set secret is a
   footgun** — the user would see a seed-file error and not know why the value they exported went
   unused. The detection is removed the release after; a variable prikk does not read is then a
   variable nobody cares about, as the owner put it.
2. **The file must not be readable by others.** On Unix, refuse a seed file whose mode grants group or
   world read (`Precondition`, naming the mode and `chmod 600`). On Windows, where `--out` already
   refuses to *write* for the same reason, `_SEED_FILE` reads without a mode check **and the docs say
   so** — the same documented asymmetry.
3. **prikk never invents the path.** The user names it — `setup --*-seed-out <path>` and
   `key generate --out <path>` already do — and prikk only reads it. No default location, no search
   order, no `~/.config/prikk`.
4. **`setup` and `key generate` print the `_FILE` line** — `export PRIKK_AUTHOR_SEED_FILE=/path/to/
   author.seed` — with one sentence: *put this line in your shell profile; prikk then works after a
   reboot.* **`setup` without `--*-seed-out` can no longer print a usable export line**, since there is
   no environment channel to print for; it therefore **requires** the two `--*-seed-out` paths, and its
   first-run docs change accordingly. The five-step entrance keeps its length: the flags were already in
   the documented fast path.
5. **`prikk key public --seed-file <path>`** replaces `--seed-env <name>`.
6. **CI and tools follow the same rule.** `ci.yml`'s fixture writes its secret to a `0600` file in the
   job and exports the path; `tools/corpus` likewise. The release workflow is untouched — it never
   signed via env.

**What this does not do.** No key lifecycle — rotation and revocation remain RFC-025's. No credential
helper — RFC 135 §9.6's refusal and its trigger stand. No config file — `prikk config` stays deferred on
its own trigger. No new dependency. **No silent fallback to the environment, ever.**

## 4. Why this shape and not the others

- **Default-write seeds somewhere** — violates rule 3 and RFC 135's settled position. Rejected.
- **Make print-once opt-in** (`--print-seeds` required) — removes the unrecoverable default but breaks
  the five-step entrance and still leaves the user with `$(cat …)` lines to persist. Not enough on its
  own; compatible with this design later if wanted.
- **A credential helper** — RFC 135 §9.6 refused it with a named trigger that has not fired.
- **Persist the `$(cat …)` line** — works today, undocumented, and puts the secret into every shell's
  environment. The whole point of the path variable is that it does not.
- **Keep both channels** (this RFC's own first draft) — requires an ambiguity rule, keeps the secret
  in the environment for anyone who still uses it, and leaves "never enters the environment" as a
  sentence rather than a property. **Withdrawn on the owner's question.**

## 5. Controls the increment must carry

- After `setup --*-seed-out`, a **new shell** with only the two `_FILE` lines and the two `_KEY_ID`s
  exported commits and seals. **No `PRIKK_*_SEED` anywhere in the environment** — assert its absence.
- A set legacy `PRIKK_AUTHOR_SEED` → refused with the migration message; the same for `MAINTAINER`.
  A group-readable seed file → refused, naming the mode.
- **The 36 test files move through one helper**, not 118 edits; `ci.yml` writes the secret to a file.
- The CLI's own `--help` says `--seed-file`, not `--seed-env`, everywhere it appears.
- `first-run.md`'s reboot section changes from "yours to arrange" to the one line; the tutorial and
  troubleshooting pages move with it. **The docs are not a follow-up** (RFC 135's own rule).
- Perturb each rule once.

## 6. Owner rulings

1. **Accept this RFC as amended** — one file channel, no environment channel, a one-release loud
   refusal. The trade RFC 135 named is being taken deliberately.
2. **Rule 2 on Unix** — refuse a readable seed file, or warn. The architect recommends refuse.
3. **Release shape** — this is a pre-1.0 CLI change under `release-compatibility.md`'s policy; the
   architect recommends it ship as **0.40.0's own headline**, with the setup-existing-repository fix
   (same command, same page), and that the stikk project be told in the next letter with the
   one-release window stated.
