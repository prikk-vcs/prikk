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
