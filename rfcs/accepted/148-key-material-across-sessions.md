# RFC 148 — Key material across sessions: a path in the environment, never a secret

**Status.** **ACCEPTED by the project owner 2026-09-12**, as amended twice that day (§2a, §2b); the handoff is live. Opened the same day on the project owner's question — *"What happens when the local
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

## 2b. AMENDED AGAIN 2026-09-12 — a default location, because requiring paths pushes the problem onto each user

> *"It will bring bad UI/UX and harm team productivity if it depends on an individual user. It should
> have the default and be integrated."* — the owner, on §3's rule that `setup` require two `--seed-out`
> paths.

**Right, and the error was mine twice over.** §2a removed the environment channel correctly and then
made every user type two paths to compensate — worse than today for the common case, and it puts the
"where do keys live" decision on each developer, which is the opposite of a team working the same way.
The cause was over-reading RFC 135's *"never invents a location for a secret."* That sentence was
written against prikk **managing** secrets — storing, searching, rotating. **A documented default
directory that the user can override is not that**; it is what `ssh`, `git` and `gh` all do, and it is
the only way "works after a reboot with nothing to persist" is true.

**On `app-json-settings`, which the owner raised (*"may be useful, or not"*), evaluated on merit and not
on posture** — RFC 135 §4 records the owner refusing the "out by construction" argument once already:
2.7.0, MSRV 1.85 (matches), normal dependencies `serde` + `serde_json` (+ optional `windows`). It
provides a JSON settings file at a platform path with load/save. **What this RFC needs today is a
directory convention, two `0600` files, and two default labels — none of which is a settings file.**
So it is not the tool for this increment. **It becomes a real candidate the day `prikk config` opens**
(RFC 135 §9.1, deferred on "a first real adopter" — and the owner's word *integrated* points there),
evaluated then against a hand-built reader as a reviewable `ALLOWED_THIRD_PARTY` decision per §4 — **now
recorded at RFC 135 §9.1a, where the deferral lives, on the owner's question** — with one benefit already
visible: the CLI's tests carry three copies of a hand-written JSON parser that
`serde_json` would retire.

**§3 is rewritten below: default directory, overrides, zero flags in the common case.**

## 3. The design — a default directory, two `0600` files, and nothing to persist

**The default location, one per platform, no search:**

| platform | default directory |
|---|---|
| Linux, macOS, BSD | `$XDG_CONFIG_HOME/prikk`, else `$HOME/.config/prikk` |
| Windows | `%APPDATA%\prikk` |

`prikk setup` with no flags creates it (`0700` on Unix), writes `author.seed` and `maintainer.seed` at
`0600`, and prints **nothing to persist** — *your keys are in `<dir>`; every new shell finds them.*
`--author-seed-out` / `--maintainer-seed-out` remain as **overrides** for CI, tests and anyone who keeps
keys elsewhere.

**How a seed is found — exactly two sources per role, in this order, and no third:**

1. `PRIKK_AUTHOR_SEED_FILE` (a path) if set;
2. otherwise `<default dir>/author.seed`.

The same for `MAINTAINER`. **No environment channel for the seed itself.** `PRIKK_AUTHOR_KEY_ID` /
`PRIKK_MAINTAINER_KEY_ID` **default to `author` / `maintainer`** — the ids `setup` mints — and the
variables become overrides. **A fresh shell on a machine where `setup` once ran needs zero exports.**

**Rules, each a security decision stated as one:**

1. **A set legacy `PRIKK_*_SEED` is a loud refusal for one release, then nothing** — *"no longer read;
   your keys are in `<dir>` (or set `PRIKK_*_SEED_FILE`)"*. Silently ignoring an exported secret is a
   footgun. Detection is removed the release after.
2. **A seed file must not be readable by others** — on Unix, refuse group/world-readable, naming the
   mode and `chmod 600`. The default directory is created `0700`.
3. **Windows: the default directory relies on `%APPDATA%`'s per-user ACL and says so.** That is the
   platform's own per-user boundary and prikk does not tighten it further; `--*-seed-out <arbitrary
   path>` keeps `key.rs`'s existing refusal, since there prikk cannot make the same statement.
4. **No search order beyond the two sources.** prikk never looks in a repository, a parent directory,
   or a second candidate path. Two places, documented, one of them the user's own choice.
5. **`prikk key public --seed-file <path>`** replaces `--seed-env`, with the default-dir file as its
   default when no path is given.
6. **CI and tools:** `ci.yml`'s fixture writes its secret to a `0600` file and sets `_SEED_FILE` (or
   uses `--*-seed-out`); `tools/corpus` likewise. The release workflow never signed via env.

**What this does not do.** No key lifecycle (RFC-025). No credential helper (RFC 135 §9.6). No settings
file and no `prikk config` — deferred on its own trigger, with `app-json-settings` noted as the
candidate when it fires. No dependency. No fallback to the environment for a seed, ever.

**Team productivity, stated as the goal it is:** onboarding a machine is `prikk setup` once; a
repository's trust store adopts the maintainer key as it does today (per repository, by design); a
second project on the same machine needs `init` + `trust maintainer add`, and the keys are already
where prikk looks.

### 2c. `setup` on a second project reuses the keys — RULED 2026-09-12 at implementation review

The first implementation (`ea0b16f2`) minted on every `setup` and refused to overwrite an existing seed
— **after `init`**, leaving a half-made repository, exactly the shape RFC 135's `d871f1e1` closed for
the other precondition. And §3's own sentence above — *a second project needs `init` + `trust
maintainer add`* — described the manual route, not the default the owner asked for.

**Rule 4. `prikk setup` reuses the keys already in the key directory.** Before `init`, before any
write or line of output, and after the already-holds-a-repository check: both `author.seed` and
`maintainer.seed` present → initialise, derive the maintainer public key from the seed (mode-checked
like any read), adopt it, print `using your keys in <dir>` and the trust line. Neither present → mint,
as before. **Exactly one present → refuse before anything**, naming the missing file and `prikk key
generate --out <that path>`. A user-named `--*-seed-out` path keeps its own behaviour; a role left to
the default follows this rule. `setup` is therefore the one command for the first project and every
project after it on the same machine; `init` + `key public` + `trust maintainer add` remains the manual
route and stays documented as such.

**Rule 1's window, restated with numbers:** 0.40.0 refuses a set `PRIKK_*_SEED`; 0.41.0 removes the
detection. Carried in `ROADMAP.md`'s 0.41 theme.

**Delivered 2026-09-12** at `ea0b16f2` + `a0f7b0a2` + `adde6e31`, reviewed
(`rfc148-default-key-directory-review-v1.md`, `rfc148-second-project-review-v1.md`). Measured by the
architect on the binary: bare `setup` to a sealed, verified commit with zero `PRIKK_*`; a second
project reuses the keys with both seed files byte- and mtime-identical; one-seed and bad-mode states
refuse with nothing created. Open from this RFC: the 0.41.0 removal of the retired-variable detection. **2026-09-12, later: CI's Windows suite failed on the pushed commit — the test isolation seam never redirected `APPDATA`; handoff v3, urgent.**

## 4. Why this shape and not the others

- **Require the user to name the paths** (this RFC's own §2a) — correct on security, wrong on the
  people: it makes every developer decide where keys live and type it twice. **Withdrawn on the owner's
  question.** A documented default with an override is the settled shape of every comparable tool.
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

- After a bare `prikk setup`, a **new shell with no `PRIKK_*` variable at all** commits and seals.
  After `setup --*-seed-out`, the same with only the two `_FILE` variables. **No `PRIKK_*_SEED` anywhere**
  — assert its absence. `XDG_CONFIG_HOME` set → that directory is used; unset → `~/.config/prikk`.
- A set legacy `PRIKK_AUTHOR_SEED` → refused with the migration message; the same for `MAINTAINER`.
  A group-readable seed file → refused, naming the mode.
- **The 36 test files move through one helper**, not 118 edits; `ci.yml` writes the secret to a file.
- The CLI's own `--help` says `--seed-file`, not `--seed-env`, everywhere it appears.
- `first-run.md`'s reboot section changes from "yours to arrange" to the one line; the tutorial and
  troubleshooting pages move with it. **The docs are not a follow-up** (RFC 135's own rule).
- Perturb each rule once.

## 6. Owner rulings

1. **Accept this RFC as amended twice** — a default directory per platform, two `0600` files, env
   variables as overrides only, no environment channel for a seed, a one-release loud refusal. The trade
   RFC 135 named is being taken deliberately, and the default directory is the part that makes it work
   for a team rather than a person.
2. **Rule 2 on Unix** — refuse a readable seed file, or warn. The architect recommends refuse.
3. **Release shape** — this is a pre-1.0 CLI change under `release-compatibility.md`'s policy; the
   architect recommends it ship as **0.40.0's own headline**, with the setup-existing-repository fix
   (same command, same page), and that the stikk project be told in the next letter with the
   one-release window stated.
