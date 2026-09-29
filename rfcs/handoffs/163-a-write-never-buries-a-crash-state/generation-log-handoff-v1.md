# RFC 163 §9 — the generation log: `compact` never buries a crash state

**Live 2026-09-30, and it is next.** The owner accepted RFC 163 §9 (*"Accepted."*). The RFC 163 implementation round
is accepted and pushed at `a884cfc1`, and CI run `36582775306` went 16/16. **After this round comes the 0.48.0 candidate.**

**Read first:** RFC 163 §9, and your own report v1 §3, which found this.

## 1. The rule at the generation log

- **Every production append to a generation log** (`foundation/generation.rs::append_generation_record`, called from
  `compact.rs`) confirms under its lock that the log ends at its last sound record. If it does not, it refuses **before
  `compact` writes anything**: before the new slot's first byte, not only before the generation record.
  - **The three compactions,** from source (`compact.rs`): the pointer index (`:85`, generation append `:115`), the
    received index (`:149`, `:179`) and the trust policy (`:212`, `:236`). For each, say where the check fires relative
    to its first write.
- **No new read:** `resolve_live_slot` already reads the log. Carry its tail status to the check, as the pointer index's
  lookup does.
- **The refusal** uses the shared `require_no_unclean_tail`. It names the log file, the offset and the byte count, and
  gives the manual way out.
- **Readers stay lenient.** `commit`, `seal`, `verify` and every read keep tolerating the tail, as they do today.

## 2. The way out, the notes, the documents

- **`troubleshooting.md`:** one entry for the refusal. Back the file up, truncate it to the named offset, run
  `prikk verify`, retry `compact`.
- **`current-state.md`:** `verify` says nothing about a torn generation-log tail; a repair verb and a `verify` line come
  in 0.49.0.
- **`durability-recovery.md`:** the generation log joins the RFC 163 list.
- **CHANGELOG:** in the RFC 163 `### Fixed` entry, with affected versions from history (0.47.0 is reproduced by the
  architect), and one `### Output changes` bullet.

## 3. Tests and controls

- **A write-first row per compacting container:** a torn prefix and 100 zero bytes on its generation log, then
  `compact`. Assert:
  - it exits non-zero, and every file under `.prikk/` is byte-identical afterwards;
  - after the documented truncation, `compact` succeeds, `verify` exits 0, and a `commit` is accepted.
- **Control:** remove the check. The rows go red.
- **The architect's probe to pass:** `/home/nabbisen/.pgtmp/arch-seal/rfc163_generation_log_probe.sh`. It must show
  `compact 2` refused and `commit after` 0.

## 4. Before proposing

- the 14 gates on the exact final commit, in R1's scope;
- both matrices green;
- `reproduce.sh` v2 and `matrix.py` on a release build of the final commit.

**Report:** `.git-exclude/review-request/rfc163-generation-log-report-v1.md`.

## Addendum 1 — 2026-09-30: disclose the garbage-shaped generation-log tail (text only)

Report `rfc163-generation-log-report-v1.md`, commit `7354feb3`. **The code is accepted** (review
`rfc163-generation-log-review-v1`). **Text only here: no code.**

**The case, measured by the architect** on `7354feb3` and on 0.47.0: 100 zero bytes after the **pointer index's**
generation log make every command refuse (`status`, `log`, `branch list`, `seal`, `commit`, `verify`, `doctor`) with
"generation log has a damaged record; run doctor before reading". `doctor` has nothing for it. On the received-index and
trust-policy generation logs, only `compact` is affected. This is long-standing, and it is N2's remainder in the
generation log. **Ruled: disclose in 0.48.0, fix in 0.49.0** with a positional tail and repair verbs for all six files.

1. **`current-state.md`, known limitations:** extend the garbage-tail bullet (or add one beside it) to the generation
   logs. Say that on the pointer index's generation log such a tail stops **every** command, and on the other two only
   `compact`. Say it is planned for 0.49.0.
2. **`troubleshooting.md`:** one entry for "generation log has a damaged record; run doctor before reading". Say what it
   means, and that no command repairs it in 0.48.0. **Give no truncation advice**, for the same reason as the
   damaged-entry entry: nothing tells the user where the last sound record ends. Point to restoring the repository from
   a copy or a clone.
3. **CHANGELOG:** name it where the RFC 163 entry lists what remains.

Full test gate (it is a `docs/src` change). **Report:** `.git-exclude/review-request/rfc163-generation-log-report-v2.md`.

**ACCEPTED and CLOSED 2026-09-30** (`7354feb3` + `eeeb5b62`; reviews `rfc163-generation-log-review-v1`, `-v2`). The
architect ran 14/14 gates on each commit, and probes on the release build at all three compactions, with a control on the
previous build. One CHANGELOG attribution is corrected in candidate 3's sweep. **RFC 163 is fully delivered, §9
included.** Next, live: `119-release-policy-reset/release-0.48.0-candidate-3-handoff-v1.md`.
