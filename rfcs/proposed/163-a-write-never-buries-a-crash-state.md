# RFC 163 — A write never buries a crash state

**Status.** **PROPOSED 2026-09-29 by the architect, for the project owner's reading.** It is presented in this exchange
and accepted, changed or refused in a later one (RFC 152 §7). One decision in it is the owner's: the scope in §3.

**Why an RFC and not an addendum.** RFC 162 took one addendum already. By RFC 152 §7, a second one means the design is
re-read. The external review of the candidate `41124dd2` (letter 015) found the case RFC 162 did not cover, and it names
the missing piece as an invariant, not a row.

**Author-review independence:** the architect proposes. The architect also accepted RFC 162's implementation without
asking which other writers append (§1). Two things compensate:
- the external architect runs `reproduce.sh` v2 and `matrix.py` on the next candidate;
- the rule below already holds in this codebase, for the WAL, and is extended, not invented.

## 1. What was missed

RFC 162 rule 1 says a writer "never appends behind damage". The implementation applied it to the object index. The WAL
already refused to append past a tail. **Every other appended file appends blind**, so a crash state that `verify`
reports as harmless (exit 0) becomes permanent damage at the next ordinary write.

Measured by the architect on a release build of `41124dd2` (`9c64dca8…`), and identical on 0.47.0:

| finding | the crash state | the next write | afterwards |
|---|---|---|---|
| **N1** | a torn pointer-index append (a crash inside a publication) | `seal`, `branch create` or `tag create`: exits 1, having appended | `verify` 1; `--repair-pointer-index-tail` refuses (interior damage now); every command refuses |
| **N2** | a torn append on trust keys, trust policy, author keys or the received index | `trust maintainer add`, a commit by a new author, `bundle import` | `verify` 1 for good; `seal`, `commit` or `compact` refused, depending on the file |
| **N3** | the ref log's last record torn after `branch create` | a `seal` of another ref: exits 0 | the torn record is interior; `commit` refused |
| **N6** | a damaged last WAL record | — | `verify` 0; the repair removes a commit the user was told had succeeded (bytes kept) |

The missing invariant, in the reviewer's words: **a write that follows a crash state never buries it (I5).** Both
matrices ran every repair before the next write. None ran the ordinary command first.

## 2. The rule

**Before an append, a writer confirms under its lock that the file ends at its last sound record. If it does not, the
writer refuses before it writes anything.**

- **The refusal names** the file, the offset where its last sound record ends, how many bytes follow, and the way out
  (§4).
- **Refuse, not clear.** Clearing the tail inside an ordinary write would make every writer a repair verb, with the
  recovery-file save and its Windows caveat (D5) on every path. The WAL's precedent is a refusal. Clearing is 0.49.0
  design, with the repairs of N2.
- **No new whole read, where it can be avoided.** The check should use a read the writer already performs before it
  appends. A publication reads the pointer index for its compare-and-swap (`ensure_current_matches`), and the
  pointer-index decode already counts trailing partial bytes; whether that lookup exposes the count is for the round to
  find. Where a writer reads nothing first, it walks record headers with ranged reads. The P1 whole-read guard and the P2
  table apply. **The round lists every appender and what it reads before writing, from source.** (Checked by the
  architect: the `trailing_partial_bytes` that `publication.rs:82` already refuses on is the ref log's, not the pointer
  index's.)
- **What counts as the tail** is RFC 162 rule 3's: everything after the last sound record when no sound record follows,
  whatever its shape.
- **Not changed:** the object index keeps RFC 162 rule 1 (rebuild before appending); the object containers keep their
  classification; the ref log keeps its own positive truncation rule. **The ref log is not in scope** (N3 is below).

## 3. Scope in 0.48.0 — the owner's decision

| | A: the pointer index only | B: every appended file that lacks it (recommended) |
|---|---|---|
| files | pointer index | pointer index, trust keys, trust policy, author keys, received index |
| N1 | fixed | fixed |
| N2 | disclosed, with the manual way out | **the burying is fixed**; the write refuses instead. Repairs stay 0.49.0 |
| cost | one file | five files, one shared check; the round lists the append sites from source |
| risk | a consumer that imports on every push hits N2 first (the reviewer's point) | an import, a trust change or a new author's commit refuses until the file is repaired by hand |

**Why B.** A refusal is strictly better than today on every one of the four files: today the write succeeds and
`verify` then fails for good, and on trust keys `seal` is refused as well. The reviewer grades N2 for 0.49.0 only because
of how rarely the files are written, and says one rule in the shared append path brings them along at little cost.

## 4. The way out, per file, in 0.48.0

- **Pointer index:** `doctor --repair-pointer-index-tail`, which exists (RFC 162), keeps the removed bytes, and the
  write then succeeds.
- **Trust keys, trust policy, author keys, received index (option B):** no repair verb in 0.48.0. The refusal names the
  offset; `troubleshooting.md` gets one entry per refusal: back up the file, truncate it to the named offset, run
  `verify`. The verbs are 0.49.0 (N2's repairs), with the tail defined by position and a `verify` line for each file.

## 5. N6 and N3 in 0.48.0: disclosed, not fixed

- **N6:** the known limitations say it. `doctor --repair-wal-tail`'s output says when what it removed includes a whole
  record, naming how many, so a removed acknowledged commit is never silent. The witness (the count or end offset of
  committed records, written with each commit) is 0.49.0.
- **N3:** the known limitations say it. A way to complete or withdraw an interrupted `branch create` or `tag create`,
  and `seal` refusing while another ref's publication is incomplete, are settled with F1 in 0.49.0.
- **"Each is a cost or a silence, not a correctness defect"** leaves the known limitations, since N2 (under A), N3 and
  N6 are correctness defects.

## 6. The matrix gains I5

`rfc162_recovery_matrix.rs` gains the ordering the review added: **write first, then repair**.
- **Rows:** for each file in scope, a torn prefix and 100 zero bytes, then the ordinary write that appends to it (§1's
  table), then `verify`, the repair or the documented way out, and `commit`.
- **I5 asserted:** the write refuses before it writes, the file is byte-identical afterwards, and the way out reaches a
  repository that accepts a commit.
- **Controls:** remove the check at each site, one at a time; its rows go red.

## 7. Not in this RFC

- **N4** (the release gate against the tag's own CI run) and **N5 1–4** (text that still describes replaced rules): fixes,
  live now in `release-0.48.0-candidate-2-handoff-v1.md`.
- **N7** (short tails silent in ten files) with M4, and **N8**: 0.49.0.

## 8. Scheduling

1. **Now:** the candidate-2 handoff (N4, N5 1–4). It does not depend on this RFC.
2. **This RFC, read by the owner, then accepted or changed**, with §3 decided.
3. One implementation round: §2 at the chosen scope, §4, §5, §6, and the disclosures.
4. A new candidate: the gates, CI, the matrix. The external architect runs both scripts against it.
5. The cut.
