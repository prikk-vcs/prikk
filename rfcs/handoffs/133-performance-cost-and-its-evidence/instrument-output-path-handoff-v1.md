# RFC 133 — the memory instrument stops writing into `rfcs/`

**Live 2026-09-13** (owner: proceed if cheap; it is). Found at the 0.42.0 prep: running
`rfc133_node_count_memory` rewrote `rfcs/handoffs/133-…/node-count-memory-measurement-report-v1.md`,
an architect-owned tracked file, stamped with HEAD at finish rather than the revision the binary was
built from.

1. The instrument writes its report under `.git-exclude/measurements/rfc133/` (create; untracked by
   construction) — file name carrying the built revision — and prints the path. It never writes under
   `rfcs/`.
2. The stamp is the **built** revision: the git revision of the tree the probe binary was built from
   (read `env!`/`build-info` if present; otherwise record HEAD at build start and refuse to run if HEAD
   changed by finish). Say which.
3. The tracked report keeps its last content as the historical record; a sentence at its top says where
   new runs write.
4. Release-prep template §1.6 gains the output path. Controls: the instrument's run leaves `git status`
   clean under `rfcs/` (assert in the test's own teardown); full gate set; no `cfg`.
5. Report: `.git-exclude/review-request/rfc133-instrument-output-report-v1.md`.
