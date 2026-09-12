# RFC 141 increment 4 (re-scoped) — CI runs the four policy gates

**Ruled yes 2026-09-13. Live; small.** Today no workflow runs any `release-policy` command; the fourteen
gates are local discipline (§7a closure, §7c).

- One new job in `.github/workflows/ci.yml`, `policy`, on `ubuntu-latest`, running the four gates in
  order — release-policy `check`, then `boundary-check`, `reference-check`, `size-check` — each as its
  own step in the invocation form EXECUTION-ORDER §6 rule 9 gives (the full command form is deliberately
  not spelled out here: this file is scanned, and an unregistered full invocation fails `reference-check`
  — rule 6 of the same section). Each exits non-zero on findings, which is what a step wants; no `||`,
  no `if`, no `$( )` (`command_scan` forbids them in workflows — RFC 121's lesson). `boundary-check` runs
  `cargo metadata --locked --offline` — the job needs the same fetch step the other jobs use first.
- `release/publication-command-inventory-v1.json` / `command_scan`: register the four invocations
  wherever the scanner requires; say which list.
- Controls: the job green on the pushed commit (the architect reads it); a deliberately red run is not
  required — the four commands' own controls already prove they fail. Full gate set locally; the addendum
  does not apply.
- Report: `.git-exclude/review-request/rfc141-policy-gates-in-ci-report-v1.md`. **Publication stays manual
  by the owner's word; this job publishes nothing.**
