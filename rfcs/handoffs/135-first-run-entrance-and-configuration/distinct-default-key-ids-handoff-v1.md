# Distinct default key ids — handoff v1

**Order of work for 0.45.0 (the four handoffs of 2026-09-17):** 1. `136-block-aggregation-payoff/warm-cache-commit-anomaly-handoff-v1.md` (measure-only); 2. `135-first-run-entrance-and-configuration/distinct-default-key-ids-handoff-v1.md`; 3. `144-two-point-comparison/merge-with-renames-design-handoff-v1.md` (report only); 4. `132-error-taxonomy-structure/absent-and-received-ref-refusals-handoff-v1.md`. Take them one at a time, each reported and reviewed before the next.

**Live 2026-09-17.** 0.45.0 item 1, "working together" (ROADMAP schedule row; §C "The default key id collides across
installations"). A prerequisite of RFC 154 and of planeter's identity model: *"Distinct maintainer key-ids —
planeter's identity model assumes them"* (planeter, 2026-09-16).

## 1. Measured 2026-09-17, binary of `fab4aded`

Setup: two installations made by `prikk setup` with separate `HOME`/`XDG_CONFIG_HOME`, called A and B.

| Step | Result |
|---|---|
| `setup` in each | both print `trusted maintainer key: maintainer`; `key status` reports `key_id: "author"`, `key_id_source: "default"` |
| `PRIKK_MAINTAINER_KEY_ID=bob prikk setup .` | still `trusted maintainer key: maintainer`: **`setup` ignores the variable** (`setup.rs:180`, the `MAINTAINER_KEY_ID` constant) |
| A imports B's bundle | **refused**: `integrity error: author key_id author already has a different recorded public key (…) … this looks like a key-rotation attempt …` |
| A runs `trust maintainer add --key-id maintainer --public-key <B's>` | refused: `… adopt the new key under a different id …`. **That advice cannot work**: B's blocks carry `maintainer`, and adopting B's key under another id matches none of them |

**It is worse than the ROADMAP row says: authors collide too.** Two default installations cannot exchange any history
at all, because every patch from B names `author`.

## 2. Ruling (architect, 2026-09-17)

1. **A new key gets a distinct default id, derived from its public key:** `<role>-<first 16 lowercase hex characters
   of the Ed25519 public key>`, for example `maintainer-296c6e77232ffa57`. It is 27 bytes, within
   `validate_key_id`'s charset and 128-byte limit. It is a name, not a security property: binding (one id, one
   public key) stays what enforces identity.
2. **The id is persisted beside the seed that owns it**, in a key-id file:
   - `author.key-id` and `maintainer.key-id` in the key directory;
   - `<seed path>.key-id` for a seed given by `PRIKK_<ROLE>_SEED_FILE` or written by `--*-seed-out`.

   The file holds the id and one trailing newline, written like the seed (0600 on Unix; the key directory's ACL on
   Windows).
3. **Resolution order, one function, used by every signer and by `key status`:**
   - (a) `PRIKK_<ROLE>_KEY_ID`, if set and non-empty, as today;
   - (b) the key-id file beside the seed in use;
   - (c) the **legacy role word** (`author`, `maintainer`), when no file exists.

   An existing installation therefore keeps its id, and its repositories keep verifying and sealing.
4. **The key-id file must belong to its seed.** A file whose content is not exactly the id derived from the seed
   actually in use (a replaced seed, a copied file, or hand-edited content) is **refused for signing**. `key status`
   reports it as unusable, naming the file and both ids. A custom id is the environment variable's job, never the
   file's.
5. **Who writes the file:**
   - `setup` writes it **only when it creates a seed**;
   - `key generate --out <path>` writes `<path>.key-id`.

   A reused seed without a file keeps the legacy id. `setup` then prints one line saying the id is the shared legacy
   default and how to use a distinct one: set `PRIKK_<ROLE>_KEY_ID`, and adopt under that id in each repository.
   **No verb migrates an existing key**; that is not in this round.
6. **`setup` honours resolution** (fixing the ignored variable). It adopts the maintainer key under the id rule 3
   resolves, and prints that id.
7. **`key status`:** `key_id_source` gains the value `"key-file"`, additive within `key-status-v1`. Prose names the
   file.
8. **Two refusals get true advice**, and every command they name must run from the refusing state (a refusal's
   advice is a claim, tested by running it):
   - **`trust maintainer add` on an id already adopted with another key:** the id travels in every signature, so
     the other maintainer must sign under a distinct id (a key made by this version's `setup`, or
     `PRIKK_MAINTAINER_KEY_ID`). Adopting under a different local id does not work, so drop that advice.
   - **Author material conflict at import, accept or verify, when the id is a legacy role word:** say that both
     installations use the legacy default `author`, and name the route. Keep the rotation wording for any other id.

## 3. Controls (each shown failing under its perturbation, then restored byte-identical)

1. **Two fresh `setup`s** get distinct author and maintainer ids. A adopts B's maintainer key under B's id, imports
   B's bundle, and `verify` exits 0. *Perturb: the derived default returns the role word.*
2. **An existing installation** (a seed with no key-id file, made the 0.44.0 way) keeps `author`/`maintainer`. A
   repository it set up before still commits, seals and verifies. *Perturb: derive when the file is missing.*
3. **`PRIKK_MAINTAINER_KEY_ID=bob prikk setup`** adopts `bob`. *Perturb: restore the constant.*
4. **A key-id file that does not match its seed** refuses signing, and `key status` reports it unusable naming both
   ids. *Perturb: skip the check.*
5. **`key generate --out p`** writes `p.key-id` (0600 on Unix), and `key status` with `PRIKK_AUTHOR_SEED_FILE=p` reports
   `key_id_source: "key-file"`. *Perturb: do not write the file.*
6. **Both refusals in §2 rule 8:** the test runs every command each one names, from the refusing state, and the route
   succeeds. *Perturb: the old text.*
7. **Resolution is one function:** give `key status` its own copy of the rule and a test must fail. That is,
   perturb the sharing, not the shared rule.

Controls 1–5 and 7 must not be Linux-gated unless the behaviour is Unix-specific (the mode check). Windows runs them in
CI.

## 4. Docs and CHANGELOG

- The key and setup guide, `commands.md` for `key status` and `setup`, and `trust-threat-model.md` (ids are names;
  binding is identity).
- CHANGELOG: `### Changed — new keys get distinct default ids`, stating that existing keys keep theirs. Also
  `### Fixed — setup ignored PRIKK_MAINTAINER_KEY_ID` and the two refusals.

## 5. Report

`.git-exclude/review-request/distinct-default-key-ids-report-v1.md`. **Stop and report** instead of proceeding if
anything in §1 does not reproduce, or if rule 3 turns out to change any existing installation's id.

## Addendum 1 2026-09-17 — the derived id is role-neutral

The stop report (`distinct-default-key-ids-stop-report-v1.md`) is right. `key generate --out <path>` outside the key
directory does not know the role, and today's route and `security-setup.md` let one seed serve either role. A
role-prefixed id checked strictly would refuse an honest use; checked loosely, it would sign a maintainer's blocks as
`author-…`. The fix is in rule 1, which was the architect's to get right.

**Rule 1, amended:** the derived id is **`ed25519-<first 16 lowercase hex characters of the public key>`**, for
example `ed25519-296c6e77232ffa57` (24 bytes). It carries **no role word**. One seed has one id, whichever role uses
it. `key status` and `trust maintainer list` already show the role beside the id.

Consequences:
- **Rule 4 stays strict and becomes simple.** The key-id file's content must equal the id derived from the seed in
  use, byte for byte, for either role.
- **Rule 5 unchanged.** `setup` (when it creates a seed) and `key generate --out` both write the file. Nothing needs
  to know the role, so no `--role` flag is added. The report's options A–D are all superseded.
- Control 1's "distinct author and maintainer ids" holds because `setup` makes two different seeds. Control 5 is
  unchanged.

**The two smaller points: both readings confirmed.**
1. `setup` stops printing `export PRIKK_<ROLE>_KEY_ID=…` for a seed it created, and prints the id itself. An
   exported id would win under rule 3(a) and bypass rule 4.
2. With a key-id file present and the seed unusable, `key status` reports the file's id with source `key-file` and
   the seed's own `reason`. The mismatch check runs only when the seed decodes.

**§3 readings, confirmed with one addition:**
- **One resolution.** `key_material::key_id` goes; the signers (`main.rs:1109`, `:1118`) and `setup` read the same
  function. `setup` passes the path it just wrote.
- **A stale file.** Before writing, check the seed path and the key-id path. Refuse if either exists, naming both
  paths, and write neither.
- **Rule 8's author refusal says plainly that history already signed under `author` cannot be imported** into a
  repository where `author` is bound to another key. The route that runs: the other installation signs new history
  under a distinct id (a key made by this version, or `PRIKK_AUTHOR_KEY_ID`), and that history imports.
- **Addition, from the report's §1:** the same conflict is reached by **`commit`** after an import bound `author` to
  the other installation's key. Rule 8's author refusal covers `commit` as well as import, accept and verify, and
  control 6 runs `commit`'s route too: sign under a distinct id, then commit succeeds.

Proceed with the handoff as amended. Report: `.git-exclude/review-request/distinct-default-key-ids-report-v1.md`.
