# RFC 120 §9.4a — the RFC-lifecycle gate

**Ruled yes 2026-09-13. Live; small.** *An RFC in `rfcs/proposed/` must not have a directory in
`rfcs/handoffs/`.* Measured 2026-09-13: it would have fired on RFC 133 and RFC 136, both accepted by the
owner and both filed as proposed until that day.

- A new category `rfc-lifecycle` in `boundary-check` (beside `rfc-naming` / `open-work-index`), one
  check: for every `rfcs/proposed/<NNN>-*.md` (and `DC-<NN>-*`), no `rfcs/handoffs/<NNN>-*` (or
  `DC-<NN>-*`) directory exists — match on the full number token, not a prefix (`DC-43` must not match
  `DC-10`; `13` must not match `133`). Failure names the RFC file and the directory.
- Controls: a fixture tree with a proposed RFC and a matching handoff directory fails naming both; a
  prefix-only collision does not fail; the real tree passes. `--graph` unaffected. Full gate set; tooling,
  no `cfg`.
- Report: `.git-exclude/review-request/rfc120-lifecycle-gate-report-v1.md`.
