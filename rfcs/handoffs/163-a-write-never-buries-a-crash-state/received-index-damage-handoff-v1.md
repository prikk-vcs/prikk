# RFC 163 — the received index refuses on damage, not only on a torn tail (external review 016, N9)

**Live 2026-09-30, and it is next.** The external architect reviewed the candidate `af2fc77e` (letter 016) and graded one
finding as blocking the tag. The architect reproduced it. Assessment: `.git-exclude/reviewed/external-review-016-assessment-v1.md`.
**A fix inside RFC 163 scope B, which the owner accepted. No new semantics.**

## 1. The defect

With 100 zero bytes or 100 random bytes after the received index's live slot, `verify` already exits 1 ("damaged
entry"). **`bundle import` exits 0 and appends 140 bytes behind them**, so the damage is buried for good. The same happens
on `ddf1e82a`, so it is not Addendum 1's tail walk alone. The received-index guard has only ever refused on
`trailing_partial_bytes`, never on a damaged entry. The four sibling files refuse because their writers read through a
lookup that fails on any damaged entry.

**The known limitations already claim the opposite:** that `bundle import` refuses with "has a damaged entry". The fix
makes that sentence true.

## 2. The fix

1. **`bundle import`'s pre-write check refuses on a damaged received index**, in the same pre-write phase as today, before
   the first object write. That covers any failed entry, and a frame with nothing sound after it that is not a torn
   prefix.
   - The refusal is the siblings' message: "received-ref index has a damaged entry; run doctor before reading".
   - **Keep the tail walk's cost property.** It already checks every checksum. Report the damage from it; do not add a
     full decode.
2. **Prove the rule at every guarded writer, not only this one.** For each of the six files RFC 163 guards, and the
   generation logs, give one write-first row: 100 zero bytes, then its ordinary write. Assert that the write exits
   non-zero and the guarded file is byte-identical. List any that do not hold; only the received index is expected.
3. **Tests:** the received index with 100 zero bytes, 4,096 zero bytes and 100 random bytes, then `bundle import`. It
   exits non-zero, and **every file under `.prikk/` is byte-identical**.
   - **Control:** restore today's arm. The rows go red.
4. **Text:**
   - the known-limitations sentence is now true; say so in the report, and check the troubleshooting entry for "has a
     damaged entry" names `bundle import`;
   - the CHANGELOG's RFC 163 entry gains one sentence, with no new output (the message already exists).

## 3. The candidate

- The final commit of this round is the candidate. The date stays **2026-09-30** if the round lands today (JST). If not,
  change the heading's date in the last commit, and say so.
- **Before proposing:**
  - the 14 gates on the final commit, in R1's scope;
  - the matrices green;
  - on a release build of the final commit:
    - **the reviewer's `matrix.py` version 4** (`receive/016-…/reproduce/`): the three N9 cells must be gone, and no
      cell worse than `af2fc77e`'s;
    - `reproduce.sh` v2;
    - **the architect's seven probes** in `/home/nabbisen/.pgtmp/arch-seal/`: the six named in the candidate-3 handoff,
      plus `rfc163_received_damaged_tail_probe.sh`, whose zero and random rows must now show the import refused and the
      file unchanged.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| P1 | `matrix.py` v4 and `reproduce.sh`, on the candidate | 45 min |
| P2 | the seven probes | 10 min |

**Report:** `.git-exclude/review-request/rfc163-received-index-damage-report-v1.md`.

**ACCEPTED 2026-09-30** (`f993bcee`; review `rfc163-received-index-damage-review-v1`). The architect ran seven probes on
the release build, with a control on the earlier builds. The reviewer's `matrix.py` v4 shows the three N9 cells gone and
none worse. **The 0.48.0 candidate is the pushed tip** that carries this code; letter 017 names it.
