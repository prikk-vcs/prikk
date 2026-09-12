# RFC 148 — a default key directory, two `0600` files, environment variables as overrides only

**Ruled:** `rfcs/accepted/148-key-material-across-sessions.md` §3 (as amended twice on 2026-09-12).

**LIVE — start.** RFC 148 was **accepted by the project owner 2026-09-12** and sits in `rfcs/accepted/`; the hold this handoff carried is lifted.

**Coordinate with `135-…/setup-existing-repository-handoff-v1.md`** — same command, same docs page. If
both are live when you start, do them as **one round**; if that one has already landed, this round
rebases its `first-run.md` changes on it.

## 1. What to build — RFC 148 §3, restated as work

**The reader.** `main.rs::read_seed_env` becomes the one place a seed is found, and it looks in exactly
two places per role, in order, and no third:
1. `PRIKK_AUTHOR_SEED_FILE` (a path), if set;
2. otherwise `<default dir>/author.seed`.
Same for `MAINTAINER`. **There is no environment channel for a seed.** `PRIKK_*_KEY_ID` default to
`author` / `maintainer` when unset.

**The default directory**, one per platform, resolved by prikk itself with no dependency:
`$XDG_CONFIG_HOME/prikk`, else `$HOME/.config/prikk` (Linux, macOS, BSD); `%APPDATA%\prikk` (Windows).
Never a repository, never a parent directory, never a second candidate.

**`prikk setup`** with no flags creates the directory (`0700` on Unix), writes `author.seed` and
`maintainer.seed` at `0600` (refusing to overwrite an existing file, as `--out` does), and prints *your
keys are in `<dir>`; every new shell finds them* — **nothing to export.** `--author-seed-out` /
`--maintainer-seed-out` remain as overrides and then print the corresponding `_FILE` line.

**Legacy refusal, one release.** If `PRIKK_AUTHOR_SEED` or `PRIKK_MAINTAINER_SEED` is set: refuse,
`Precondition`, *"no longer read; your keys are in `<dir>` (or set `PRIKK_*_SEED_FILE`)"*. **Silent
ignore is forbidden.** Mark the detection with a comment naming the release that removes it.

**Mode rule.** Unix: a seed file readable by group or world → refuse, naming the mode and `chmod 600`.
Windows: `%APPDATA%` is per-user by platform ACL; the default directory relies on that **and the docs say
so**; `--*-seed-out <arbitrary path>` keeps `key.rs`'s existing refusal.

**`prikk key public --seed-file <path>`** replaces `--seed-env <name>`; with no path it reads the
default-directory file for the role given.

## 2. The sweep — measured, so nobody rediscovers it

| where | occurrences | how |
|---|---|---|
| `crates/*/src` | 16 in 3 files | one reader; two callers |
| `crates/*/tests` | 118 in 36 files | **one shared helper** that writes a temp `0600` seed file and sets `_FILE` — not 118 edits |
| `docs/src` | 29 in 8 pages | `first-run.md` reboot section becomes one paragraph; tutorial, troubleshooting, security-setup, others follow |
| `.github/workflows/ci.yml` | 8 | the fixture writes its secret to a `0600` file and sets `_FILE`; **the release workflow is untouched** — it never signed via env |
| `tools/corpus` | 5 in 2 files | same helper shape |
| stikk's CLI seam | 5 | **not ours to edit**; the architect tells them in the next letter with the one-release window |

## 3. Controls

1. **Bare `prikk setup`, then a new shell with no `PRIKK_*` variable at all** → `commit` and `seal`
   succeed. Assert the environment is empty of `PRIKK_*_SEED` **and** that `~/.config/prikk/*.seed` are
   mode `0600`, the directory `0700`.
2. `XDG_CONFIG_HOME` set → that directory is used; unset → `$HOME/.config/prikk`. Both asserted.
3. `PRIKK_AUTHOR_SEED_FILE` set → it wins over the default file (put different keys in each; assert
   which signed).
4. Legacy `PRIKK_AUTHOR_SEED` set → refused with the migration message, exit 1; **assert the old
   `author signing is required` text is absent.**
5. Group-readable seed file → refused, naming the mode.
6. **Perturb** rules 1 (fall back silently to the default when `_FILE` is set but missing? must not),
   4 and 5 — one failure each, restored.
7. **`--help` says `--seed-file` everywhere**; grep the tree for `--seed-env` → 0.
8. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim, **plus the cross-target addendum run**:
   this round adds platform-conditional code (`XDG`/`HOME` vs `APPDATA`; `0700`/`0600` on Unix only) —
   state both outcomes.
9. **Changelog**: `### Changed — breaking once` for the removed environment channel, with the
   migration line and the release that drops the detection.

## 4. Not in this round

- No settings file, no `prikk config` (RFC 135 §9.1a — its trigger and format candidate are recorded
  there).
- No key lifecycle. No credential helper. No dependency.
- No change to `key generate --out`'s Windows refusal for arbitrary paths.

---

# v2 — 2026-09-12, one round: the second project, a flaky gate, and one number

**`ea0b16f2` + `a0f7b0a2` reviewed** (`.git-exclude/reviewed/rfc148-default-key-directory-review-v1.md`).
The core is accepted as measured. Three required items, one optional.

## 1. `setup` on a second project — RFC 148 §2c, ruled

Measured: in a fresh directory with both seeds already in the key directory, `setup` printed
`initialized Prikk repository at …` and then `error: refusing to overwrite an existing file:
…/author.seed`; the repository was left with no maintainer adopted; a second `setup` refused for the
other reason. Same shape as the half-run `d871f1e1` closed.

- **Order**: already-holds-a-repository check (existing) → **key-directory check (new)** → only then
  `create_dir_all` and `RepositoryLayout::init`. Nothing is written and nothing is printed before both
  checks pass.
- **Both seeds present → reuse.** Init; derive the maintainer public key from `maintainer.seed`
  through `key_material::read_seed` (so the mode check and the retired-variable refusal apply); adopt
  it; print `using your keys in <dir>` in place of `your keys are in <dir>`, then the trust line and
  `every new shell finds them -- nothing to export` as today. No seed is written.
- **Neither present → mint**, unchanged.
- **Exactly one present → refuse before anything**, `Precondition`, naming the missing file and
  `prikk key generate --out <that exact path>`.
- **User-named paths**: a role given `--*-seed-out` mints to that path as today; a role left to the
  default follows the rule above.
- **Controls**: (a) two `setup`s in two directories under one isolated key environment with no
  `PRIKK_*` — both reach `seal` + `verify` rc 0; the second wrote no seed (bytes and mtime of both files
  unchanged) and printed no 64-hex run; (b) exactly one seed → refusal, **no `.prikk` created**, no
  file written, nothing on stdout; (c) reuse with a `0644` maintainer seed → the mode refusal, before
  `init`. Perturb (a) by moving the key check after `init` — (b) must fail.
- **Docs**: `first-run.md` "A second project" becomes one route (`prikk setup ./second-project`,
  reuses your keys) with the manual `init` + `key public` + `trust maintainer add` route kept beneath
  it; drop *"export the same PRIKK_* values"* and *"saving the printed seeds"*, both pre-RFC 148.
  `troubleshooting.md`: the `refusing to overwrite an existing file` message from `setup` is no longer
  reachable; if an entry quotes it, retire it the established way. CHANGELOG: one sentence under the
  RFC 148 entry.

## 2. The RFC 102 race control flaked in the architect's gate run

`two_racing_object_appends_serialise_and_never_share_an_offset`: `exactly one racer must be refused`,
`left: 0, right: 1`, both `Ok` — the loser reached the lock after the winner released it. Change the
assertion to `conflicts <= 1 && succeeded >= 1` and keep the shared-offset check; the deterministic
`an_append_meeting_a_held_object_store_lock_is_refused` already proves the refusal. Update the doc
comment that promises "exactly one".

## 3. The window

`key_material.rs`: `REMOVE THIS DETECTION IN 0.42.0` → **0.41.0** (0.40.0 refuses; the release after
removes). `ROADMAP.md`'s 0.41 theme now carries the line.

## 4. Optional

`key generate --out <keydir>/author.seed` prints the maintainer next step and then suggests renaming
the file to `author.seed`. Inside the key directory, read the role from the file name.

## 5. Gates

The full set, verbatim, against the final commit; the addendum applies (this round's diff is
platform-conditional by design). The first full `cargo test` must be green — that is what §2 is for.

---

# v3 — 2026-09-12, URGENT: `main` is red on Windows; the isolation seam does not redirect `APPDATA`

CI run 34686656667 on `0609ee51`: **"Windows mutation test suite" failed**, three tests in
`rfc135_key_and_setup.rs`:

```
stdout: initialized Prikk repository at C:\Users\RUNNER~1\AppData\Local\Temp\…\r\.prikk
stderr: error: refusing to overwrite an existing file: C:\Users\runneradmin\AppData\Roaming\prikk\author.seed
```

`support::isolate_key_environment_for` sets `XDG_CONFIG_HOME` and `HOME` and removes the `PRIKK_*`
variables — and on Windows `key_material::default_key_dir` reads **`APPDATA`**, which the seam never
touches. Every test's `setup` on the Windows runner wrote into the runner's real
`%APPDATA%\prikk`, in parallel, and the second and later ones collided. The same hermeticity problem
you solved for Unix in v1 §4, one platform over. Every other job is green, including the Windows
build and the Windows read-only conformance job.

## Required, before anything else

1. `isolate_key_environment_for` also sets `APPDATA` to the isolated config home (unconditionally —
   harmless on Unix), so the Windows key directory is `<home>\prikk`, which is what
   `isolated_key_dir` already computes. Remove `USERPROFILE`-derived surprises only if a test reads
   them; none should.
2. Any test file that builds its own `Command` and was given the seam in v1 gets this through the
   same function — confirm none re-sets `APPDATA` itself.
3. **This cannot be verified here.** The cross-target addendum compiles Windows code; it does not run
   Windows tests. The check is CI's "Windows mutation test suite" on the pushed commit, and the
   architect will read it before the 0.40.0 cut. Say in the report that this is the case.

## Note, not required this round

The Windows transcript also shows the mint path can still print `initialized` and then fail — when
two `setup`s share one key directory *concurrently*, classification in both sees no seeds and the
second's `create_new` loses. The v2 precondition phase closes every single-process case; this one is
two processes racing on the user's own key directory, which the isolation seam is what prevents in
tests. Recorded as the one remaining path to that message.
