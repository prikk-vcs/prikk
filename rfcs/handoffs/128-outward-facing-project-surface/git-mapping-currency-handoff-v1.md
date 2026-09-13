# `git-mapping.md` currency — five corrections, three rows, one test

**Live 2026-09-13** on the owner's word (`.git-exclude/reviewed/git-mapping-currency-review-2026-09-13.md`).
Docs and one test; no product change. Verify every sentence you write against the `main` binary's
`--help`, not against this handoff. Small; may run beside RFC 136 increment 0.

## 1. Corrections (each was false on 2026-09-13)

1. Intro: *"no branch switching"* → gone; `branch switch` exists (0.42.0, RFC 151). Keep "no `HEAD`,
   no staging area" and say a current branch exists (the page's own later section already does).
2. Row `git show` / `git diff` → split into two rows. `git show` → `prikk show <block-id|patch-id>`
   (0.36.0, RFC 142): what one block or patch changed, spans not unified diffs, no replay for content.
   `git diff` → still missing; the closest are `checkout --patch-plan --format json --content-path`
   (content at a point, RFC 143) and `worktree-status` (names, no content); note that a two-point diff
   is RFC 153 (proposed), and link nothing that does not exist.
3. Row `prikk doctor` → two real repairs: `--repair-wal-tail` and `--repair-index` (0.40.0, RFC 102);
   `--repair-main-ref` still recognized and refused.
4. Row `prikk key` → `generate` / `public` / `status` (RFC 150); row `git config` → the verdict stays
   "missing" (no config file or command; RFC 135 §9.1a), the reason becomes: keys live in a per-user
   key directory by default (RFC 148, `security-setup.md:61-65`) and the `PRIKK_*_SEED_FILE` /
   `PRIKK_*_KEY_ID` variables are overrides.
5. RFC 113 link → `rfcs/accepted/113-history-import-foundations.md`; "nothing implements it yet"
   stays true.

## 2. New rows

- `git fetch` → `sync have <ref>` (you) → `sync build` (them) → `sync accept <file>` (you), or
  `bundle import`: what arrives is an untrusted `remotes/<name>` pointer, never merged. File-based;
  no sockets. Point at `sync.md`'s loop section.
- `git pull` → fetch as above, then `sync seal <ref> --claim <id>` when the received patches extend your
  ref, or `merge-evidence → merge-plan → merge` when the histories diverged. No single verb, by design.
- `git push` → **no counterpart**: you produce the artifact (`sync build`, `bundle export`) and the
  other side accepts it; prikk never writes into another repository.

`git submodule` and multiple `git worktree`s stay "Missing"; do not add a reason (the non-goals page
requires a citable owner decision, and there is none yet).

## 3. The test that stops the class

`commands/tests.rs` has rule (A) (every command the docs name exists) and rule (B) (every command is
explained somewhere). Neither catches a page that says "Missing" about a command that exists. Add
**rule (C): every `COMMANDS` entry's name appears in a code region of `git-mapping.md`**, with an
allowlist `GIT_MAPPING_EXEMPT: &[(&str, &str)]` (name, reason) that must be empty or name real
entries (mirror `declared_undocumented_names_are_real_registry_entries`). Perturb: remove `show` from
the page and the test must name it. Subcommands (`key status`, `doctor --repair-index`) are below the
registry's granularity; they are the manual step in §4, not this test.

## 4. The rule (already applied to the template; read it)

`119-release-policy-reset/release-prep-handoff-template.md` §1 step 8: at each release prep, for every
`### Added` entry since the last tag, grep `docs/src`, `README.md` and `SECURITY.md` for a sentence
that calls that feature missing, not yet available, or not implemented, and fix it. Minutes per
release; the test above makes the largest page mechanical.

## 5. Controls and report

Rule (A)/(B)/(C) green; `reference-check` green (the page is a declared document); every `prikk`
invocation on the page pasted into a shell against the `main` binary and exiting 0 or with the usage
error the row describes. Full gate set (the test is Rust). Report:
`.git-exclude/review-request/git-mapping-currency-report-v1.md`.
