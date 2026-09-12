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

## 3. The design — one non-secret line the user persists once

Two new environment variables, read wherever the two seed variables are read today:

- **`PRIKK_AUTHOR_SEED_FILE`** — a path. prikk reads the seed from that file at the moment of use.
- **`PRIKK_MAINTAINER_SEED_FILE`** — likewise.

**Rules, and each is a security decision stated as one:**

1. **Exactly one source per role.** If both `PRIKK_AUTHOR_SEED` and `PRIKK_AUTHOR_SEED_FILE` are set,
   refuse (`Precondition`, naming both). prikk never guesses which secret the user meant.
2. **The file must not be readable by others.** On Unix, refuse a seed file whose mode grants group or
   world read (`Precondition`, naming the mode and the fix). On Windows, where `--out` already refuses
   for the same reason, `_SEED_FILE` reads the file without a mode check **and says so** in the
   documentation — the same asymmetry `--out` already documents.
3. **prikk never invents the path.** The user names it — `setup --*-seed-out <path>` already does —
   and prikk only reads it. No default location, no search order, no `~/.config/prikk`. RFC 135's
   *"never invents a location for a secret and never reads one back"* becomes *"never invents; reads
   back exactly where told."*
4. **The secret never enters the environment.** The line a user persists in a profile is
   `export PRIKK_AUTHOR_SEED_FILE=/path/to/author.seed` — a path, safe to keep in a dotfile, safe in
   `ps`/`/proc/<pid>/environ`. The seed stays in the `0600` file.
5. **`setup --*-seed-out` prints the `_FILE` line** instead of `$(cat …)`, with one sentence: *put this
   line in your shell profile and prikk works after a reboot.* The default (print-once) path is
   unchanged in behaviour, and its existing "nowhere else" warning stands.
6. **`prikk key public` gains `--seed-file <path>`** beside `--seed-env`, under the same rules.

**What this does not do.** No key lifecycle — rotation and revocation remain RFC-025's. No credential
helper — RFC 135 §9.6's refusal and its trigger ("a user asking for one") stand. No config file —
`prikk config` stays deferred on its own trigger. No new dependency.

## 4. Why this shape and not the others

- **Default-write seeds somewhere** — violates rule 3 and RFC 135's settled position. Rejected.
- **Make print-once opt-in** (`--print-seeds` required) — removes the unrecoverable default but breaks
  the five-step entrance and still leaves the user with `$(cat …)` lines to persist. Not enough on its
  own; compatible with this design later if wanted.
- **A credential helper** — RFC 135 §9.6 refused it with a named trigger that has not fired.
- **Persist the `$(cat …)` line** — works today, undocumented, and puts the secret into every shell's
  environment. Rule 4 is the reason to prefer a path variable.

## 5. Controls the increment must carry

- After `setup --*-seed-out`, a **new shell** with only the two `_FILE` lines exported commits and seals.
- Both sources set → refused, naming both. A group-readable seed file → refused, naming the mode.
- `first-run.md`'s reboot section changes from "yours to arrange" to the one line; the tutorial and
  troubleshooting pages move with it. **The docs are not a follow-up** (RFC 135's own rule).
- Perturb each rule once.

## 6. Owner rulings

1. **Accept this RFC** — the trade RFC 135 named is being taken deliberately: long-lived keys become
   easy to keep before lifecycle exists. The architect recommends accepting, because the alternative is a
   tool that silently stops working after a reboot.
2. **Rule 2 on Unix** — refuse a readable seed file, or warn. The architect recommends refuse.
