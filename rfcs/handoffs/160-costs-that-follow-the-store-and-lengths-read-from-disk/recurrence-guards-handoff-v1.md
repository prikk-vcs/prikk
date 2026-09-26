# Guards against costs that follow the store, and against lengths read from disk — RFC 160, handoff v1

**Live 2026-09-26, and it is next.** The RFC 102 append-length round closed at `e21c674f`: CI run `36242389896` went
16/16, the Windows mutation suite green. **This round is in 0.48.0** (owner, 2026-09-26: *"1. Accepted. 2. 0.48.0"*),
and 0.48.0 release prep follows it.

**Read first:**
- `rfcs/accepted/160-costs-that-follow-the-store-and-lengths-read-from-disk.md`, all of it, including §7;
- `rfcs/accepted/102-container-based-durability.md`, its last two sections;
- `.git-exclude/reviewed/append-length-without-reading-review-v1.md` and `-v2.md`.

## 1. What lands: P1–P4 (P5–P7 are the architect's)

1. **P1 — the whole-read guard (`cfg(test)` only).**
   - Under `cfg(test)`, the anchored reader fails the test that performs a **whole** read of a store-growing file: an
     object container, the object index, a ref log container. The exception is a read inside a declared scope.
   - The scopes are **one table** in one file, each row with its reason and its RFC or ROADMAP reference. Start from
     RFC 160 §3.1's list, then add what the inventory finds.
   - **Inventory first:** run the whole store suite with the guard in report-only mode, and list every whole read it
     sees, by file and caller. That table is part of the report. Every row either becomes a declared scope with its
     reason, or is a defect named for a ruling. Do not declare a scope just to make the suite pass.
   - Shipped builds compile none of it.
2. **P2 — store-size independence, one table of operations.** Each row runs one operation on a small store and a large
   one (RFC 160 suggests 1 MiB and 16 MiB of content; say what you chose). It asserts that the bytes the read tally sees
   under `.prikk/` differ by at most the operation's own records. Rows:
   - object append;
   - object read;
   - the index refresh after an own write;
   - a one-file commit through the store API;
   - a one-block seal.

   The table's doc says a new per-object operation joins it in the round that adds it.
3. **P3 — the allocation-bound source scan.**
   - A test reads `prikk-store` and `prikk-object` production sources. It fails on `with_capacity(`, `vec![0…; …]` and
     `reserve(` / `reserve_exact(` whose size comes from decoded data (`as usize` on a value read from bytes, or a
     cursor read) unless the site is in an allowlist naming its bound.
   - Add a `ByteCursor` helper: capacity = min(count, remaining bytes / smallest encoded element).
   - **Fix `trust_index.rs:344` with it.**
   - List every site the scan inspected and how each was classified. A scan that matches nothing proves nothing.
4. **P4 — the hostile-length damage suite.**
   - For **every** on-disk record format with a length or count field (RFC 160 §3.4's list; say if you find more),
     one test sets that field to its maximum **with a matching checksum where the format has one**, so the length is
     what is tested, not the checksum. The decode must be refused with an error, never an abort, and never an
     allocation of the claimed size.
   - Cases that could abort run in a child process.
   - **At the CLI:** `prikk verify` and `prikk doctor` on a repository whose first record in each container type claims
     2⁶² bytes exit non-zero with a finding, never on a signal. Run each case on a **sealed** repository **and on an
     unsealed one**. State 0.47.0's behavior on the same repositories, from the 0.47.0 binary.
   - **Fix `verify`'s objects stage (RFC 160 §7).** An index entry whose record cannot be read is reported as a
     `Failed` item naming the object id and offset, unless the container scan already reported a failure at that
     offset. Today such an entry is skipped, and the container scan calls the frame a torn tail: on 0.47.0 and on
     `9137125e`, a lone blob whose header claims 2⁶² bytes gives `object items: 0 scanned, 0 failed` and exit 0.
     **Perturb:** restore the `continue`. The unsealed CLI case goes red.

## 2. Controls: each guard red by its own defect (RFC 160 §5)

Put each defect back, one at a time, and show which guard goes red. **Restore by copy and `touch`.**
- **P1 and P2:** `index.rs`'s whole-container length read in the append; S-2's whole read in the object read; site C's
  whole index read.
- **P3:** the ranged reader's clamp removed; `trust_index.rs:344` reverted.
- **P4:** the clamp removed, which must redden the container-frame case and the CLI `verify` case without killing the
  suite; `trust_index.rs:344` reverted.

**A guard that stays green on its own defect is not accepted.**

## 3. Measurement: units and budgets, up front

| unit | what | budget (stop at ×2) |
|---|---|---:|
| G1 | the full store test suite with P1 on, against the suite without it: wall-time overhead, one run each | 10 min |
| G2 | P4's CLI cases on this build and on 0.47.0 | 5 min |

**Acceptance:** P1 adds at most 10 % to the store suite's wall time, or the report says what it costs and why.

## 4. The report

`.git-exclude/review-request/rfc160-recurrence-guards-report-v1.md`:
- the gates on the exact final commit;
- P1's inventory table;
- P3's site list;
- P4's format list, with the 0.47.0 comparison;
- each control with the defect that reddened it;
- G1 and G2;
- anything in RFC 160 that is not true at source.

**Windows:** P1–P4 must run on Windows too wherever the platform allows. The architect reads the Windows mutation suite
on the pushed commit, and the round closes only when it is green.
