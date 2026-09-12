# RFC 148 rule 1 — remove the retired-variable detection (0.41.0 item 0)

**Live** — RFC 150's `key status` is delivered (`2212f7f9`), which is the condition RFC 150 §4 set.
**Scheduled:** 0.41.0. Both ship in the same release.

## 1. The change

- `key_material::refuse_retired_env` and `Role::retired_seed_var` go; `read_seed` becomes `status` +
  `refusal_for`. A set `PRIKK_AUTHOR_SEED` / `PRIKK_MAINTAINER_SEED` is simply unread, as RFC 148 rule 1
  says for the release after the window.
- **`legacy_variable_set` does not ship.** `key-status-v1` is unpublished until 0.41.0; remove the
  field from the JSON and the prose now so the schema's first published form has no field that means
  nothing. (Field removal after publication would be a schema change; before it, it is design.)
- RFC 148 control 4 (`a_retired_seed_variable_is_refused`) and RFC 150 control 1's second half go; add
  one control in their place: a set `PRIKK_AUTHOR_SEED` with a usable key-directory seed → `commit`
  succeeds **and** `key status` reports the key-directory public key — the truth a front-end will now
  read from `key status` instead of from a refusal.
- Docs: `troubleshooting.md`'s "no longer read" entry retires the established way (old wording named
  inside the surviving entries); `first-run.md`'s migration note says a stale export is ignored and
  `key status` shows which key is in effect; CHANGELOG `### Changed`, one paragraph, naming the window
  that closed (0.40.0 refused; 0.41.0 ignores).

## 2. Controls

Full gate set; the isolation seam on every test; cross-target from the diff (`key_material.rs` is
platform-conditional — run both). Perturb: leave `refuse_retired_env` in place → the new control fails.

## 3. Not in this round

Nothing else about key discovery. RFC 148 is otherwise closed.
