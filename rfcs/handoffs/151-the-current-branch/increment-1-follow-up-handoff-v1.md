# RFC 151 increment 1 follow-up — three small commits before increment 2

**Live 2026-09-13**, from the review of `rfc151-inc1-rfc141-rfc120-report-v1.md`.

1. **Texts made true now** (ruled §1.7 (b)): `commands.rs:177`'s note becomes *"no `branch switch` yet;
   `--ref` defaults to `.prikk/current-branch`"*; `docs/src/guide/faq.md:66–71` and
   `docs/src/reference/git-mapping.md:28, 66–69, 111` say the same. The RFC 146 §8e synopsis control must
   stay green. CHANGELOG: the existing `### Added` entry gains nothing.
2. **RFC 141 §2.4 closed**: a required-procedure list in `command_scan`/`boundary-check` — the four
   policy `run:` lines must all be present in `.github/workflows/ci.yml`; deleting any one fails
   `boundary-check` naming it. Perturb by deleting the `size-check` step.
3. **RFC 120 §9.4a**: `git stash pop` and commit the gate unchanged. The architect refiled DC-43 (handoff
   appended to the RFC, directory and its allowlist entry removed), so `real_tree_passes_unchanged` passes.

Then **increment 2** per `current-branch-handoff-v1.md`, with one addition: **`status` reports the
current branch** (it prints `heads/main RefState` today); its JSON, if any, carries `"current_branch"`.
