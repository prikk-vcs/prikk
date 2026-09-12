# RFC 150 — Signing readiness: `prikk key status`

**Status:** ACCEPTED by the owner 2026-09-12 (proposed the same day by the architect, on stikk's letter 007, received before
0.40.0 publishes). **Scheduled: 0.41.0, and the removal of
the retired `PRIKK_*_SEED` detection (RFC 148 rule 1's window) does not ship before it.**

## 1. The question a front-end cannot answer without copying our rule

RFC 148 moved seeds out of the environment. A front-end that used to compute *"can this user sign as
AUTHOR here?"* as `is_set(PRIKK_AUTHOR_KEY_ID) && is_set(PRIKK_AUTHOR_SEED)` is now wrong in both
directions: the correct 0.40 setup exports nothing and reads as not-ready; a stale export reads as
ready and is refused — and after the window closes, the stale export is simply ignored while prikk
signs with the key-directory seed. To answer correctly, the front-end would re-implement
`key_material::seed_path` — `XDG_CONFIG_HOME`, `HOME`, `APPDATA`, the `_SEED_FILE` overrides, the
set-but-missing refusal, the mode rule — a second copy of our rule that drifts the day we change it.
stikk said so plainly and asked, in the shape five earlier asks took: *expose what you already compute.*

We compute it on every `commit` and `seal`. This RFC exposes it.

## 2. `prikk key status [path] [--role author|maintainer] [--format json]`

**Per role, without signing anything:**

| field | meaning |
|---|---|
| `role` | `author` / `maintainer` |
| `source` | `seed-file-override` (`PRIKK_<ROLE>_SEED_FILE` set), `key-directory`, or `absent` |
| `path` | the path consulted (the override's, or `<key dir>/<role>.seed`), even when absent |
| `usable` | `true` only if the file exists, passes the Unix mode rule, and decodes as a seed |
| `reason` | present when `usable` is false: `missing`, `readable-by-others (mode 0644)`, `undecodable`, `override-missing` |
| `key_id` | the id in effect; `key_id_source`: `environment` or `default` |
| `public_key` | hex, when usable — what `key public --role` prints |
| `legacy_variable_set` | `true` when `PRIKK_<ROLE>_SEED` is set (the refusal RFC 148 rule 1 applies, for its window); removed with the detection |
| `binding` | **only when `path` names a repository**: author → `unrecorded` / `matches` / `mismatch` against the public key this repository has recorded for `key_id`; maintainer → `not-adopted` / `matches` / `mismatch` against the trust store |

**`binding` is the answer to stikk's second question, measured on the 0.40.0 binary:** an author id
with a different seed is refused only once the repository has recorded a key for that id
(`integrity error: author key_id … already has a different recorded public key`); a fresh id with any
seed is accepted and *becomes* the recording. A maintainer id is bound by adoption: a different seed
under an adopted id is refused at `seal` (`maintainer signer public key does not match trusted key`),
an unadopted id is refused as a precondition. So the id a front-end displays and the seed prikk will
use **can** disagree before first use for AUTHOR, and never silently for MAINTAINER; `binding` makes
both visible before the signature exists.

**Exit code:** 0 whenever the answer was computed, *including every not-ready state* — absence degrades,
it does not fail (RFC 140 §7b, RFC 142 §6b). 1 only for a real failure (an unreadable repository, an
I/O error). 2 for usage. `--format json` opens with `"schema_version": "key-status-v1"`, hand-built like
every other emitter, and never fails where prose succeeds. Prose is one block per role.

**Security posture, stated:** the command reads prikk's own seed files exactly as `commit` does, prints
public material only, never a seed, never a path derived from a seed, and creates nothing. A front-end
that calls it learns readiness without touching key material — which is the boundary stikk's `C-I1e`
protects and RFC 135 §9.6's refusal of a helper boundary left intact.

## 3. `key public --role` (stikk's third question)

It reads a seed file prikk owns and prints a public key. A front-end **may** call it; it neither creates
key material nor exposes a seed. But it is not the readiness probe: it cannot say *where* the seed came
from, whether the mode rule would refuse it, what id is in effect, or whether that id is bound to a
different key here. One consistency fix rides along: its missing-file message is `cannot read <path>`
where `commit` says `no seed at <path>. Create one with …`; both route through
`key_material::read_seed` after this RFC.

## 4. Sequencing rule

RFC 148 rule 1 removes the retired-variable detection in 0.41.0. **That removal must not precede `key
status`**: the day a stale `PRIKK_AUTHOR_SEED` becomes silently unused is the day a front-end without
this command shows a confident, wrong picture on the signing path. Both ship in 0.41.0; if `key status`
slips, the removal slips with it.

## 5. Not in this RFC

No key lifecycle (RFC-025). No `prikk config`. No change to how a seed is found (RFC 148 owns that). No
signing performed by `key status`.

**Delivered 2026-09-12** at `2212f7f9`, reviewed (`rfc150-key-status-review-v1.md`). Measured by the
architect in every state of §2's table against the signing commands' behaviour in the same state; the
shared-query control fails if and only if `commit` and `key status` stop sharing one computation.
`legacy_variable_set` is removed before 0.41.0 publishes the schema (retire-legacy-seed-detection
handoff), so `key-status-v1`'s first published form carries no field that means nothing.

**As shipped (0.41.0), corrected against stikk's letter 008, 2026-09-13.** `source` has **two** values,
`seed-file-override` and `key-directory`; an unusable key is `usable: false` with `reason` — the better
shape, accepted at review; §2's `absent` did not ship. `legacy_variable_set` never shipped (removed with the
detection before the schema published). **Contract for `key-status-v1`:** `public_key` and `binding` are
`null` when the seed is unusable, and `binding` is `null` when no repository was asked; `null` there means
*no claim* and will not be given another meaning within `-v1`. The architect's reply 007 described the RFC,
not the binary, and was sent after the binary had diverged from it — recorded as a process miss.
