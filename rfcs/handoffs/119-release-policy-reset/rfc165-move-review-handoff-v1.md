# RFC 165 to done/ — a review before push

**Live 2026-10-10, and it is next.** 0.50.0 is released, and RFC 165's R5 amendment and the lost-generation-log rule
shipped in it. The architect moved RFC 165 to `done/` in commit `ef8a7faf`. It touches `docs/src`, so the
development team reviews it before it is pushed, as in 0.50.0 step 3.

**What the commit does:**
- **`rfcs/accepted/165-…` → `rfcs/done/165-…`.** Its status line gains *"DONE 2026-10-10: delivered in 0.49.0; the R5
  amendment and the lost-generation-log rule completed in 0.50.0"*.
- **`rfcs/README.md`:** the row leaves the Accepted table and joins the Done table, between 166 and 164.
- **Every link to the old path:** `docs/src/reference/durability-recovery.md` (two table rows), and three handoffs
  (`163-…/ref-publication-design-round-handoff-v1.md`, `165-…/round-1-handoff-v1.md`, which wraps across a line,
  and `165-…/round-2-handoff-v1.md`).
- **`ROADMAP.md`:** the 0.50.0 schedule row's status reads **RELEASED 2026-10-09**.

## Check (one sitting, 20 min, report `rfc165-move-review-report.md`)

1. **No old path remains:** `git grep -n "accepted/165"` prints nothing.
2. **Every new link resolves:** each `done/165-…` link opens the moved file.
3. **The README** lists 165 once, in Done, and the order is still descending.
4. **The status line and the ROADMAP row** say what shipped, and in which release.
5. **`scripts/gates.py`, all 14, on `ef8a7faf`.** Report every exit code.

The architect ran `boundary-check` and `reference-check`: both exit 0, no errors. **Do not push.** Report OK, or each
finding.
