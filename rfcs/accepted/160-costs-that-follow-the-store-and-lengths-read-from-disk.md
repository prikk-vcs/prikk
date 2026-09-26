# RFC 160 — Guards against costs that follow the store, and against lengths read from disk

**Status.** **ACCEPTED by the project owner 2026-09-26** (*"1. Accepted. 2. 0.48.0"*, answering §6's two decisions).
**The architect's reading, stated so it can be corrected:**
- all seven measures of §3 (P1–P7) are accepted as written, with nothing dropped;
- they ship in **0.48.0** as **one round**, live when the RFC 102 append-length round closes (Addendum 1 accepted and the
  Windows mutation suite green), and before 0.48.0 release prep;
- 0.48.0 is not cut without this round;
- P5 (the template's store-size row) and P7 (the architect's own discipline) are the architect's to apply, P5 at 0.48.0
  prep and P7 from now on; P6's reopening of RFC 133's residual is reported by 0.48.0 prep.

*History:* **PROPOSED 2026-09-26 by the architect**, on the owner's instruction: *"I want you to consider carefully
recurrence prevention measures on the case(s)."*

Author-review independence: the architect proposes and will review. Every guard in §3 must be shown red by putting
back the defect it exists to catch (§5). A guard that cannot fail that way is not accepted.

## 1. The cases

1. **Operations whose cost followed the whole store** (RFC 102, `MEASURED and RULED 2026-09-26`). There were three
   sites, all in the object store:
   - **A. The append.** It read its whole container to learn its length, from RFC 102 Stage 3 (`d260e340`,
     2026-08-14) until the fix round.
   - **B. The object read.** It read the whole container to decode one record.
   - **C. The index tail replay after every own write.** It read the whole object index.

   Consequences: a commit adding one small file peaked at the blob container's size (265 MiB at 256 MiB), and a first
   commit of 4,000 × 20 KB files read 162 GB. Every release from Stage 3 to 0.47.0 had it, about six weeks.
2. **An allocation sized by a length read from disk.**
   - The fix round's own positioned read (S-2) allocated the frame header's `body_len`, a `u64` from disk, before
     reading. A damaged header made `prikk verify` abort (`memory allocation of 4611686018427387954 bytes failed`,
     exit 134). It was caught in review before any push.
   - **The same shape is already shipped:** `trust_index.rs:344`, `Vec::with_capacity(count as usize)`, where `count`
     is a `u32` read from the trust-policy record. The record's checksum is verified first (`trust_index.rs:450`), so
     random damage is refused cleanly. A record whose checksum **matches** is decoded, though, and one claiming 2³²
     keys asks for about 100 GB; by reading, that aborts. Such a record could be written by anyone who can write the
     repository's files (the checksum is unkeyed), or by a faulty writer. Found by the architect's sweep on 2026-09-26.
     This is less exposed than S-2, whose length was trusted before any checksum could be checked.
3. **The architect's own misses, which let case 1 live:**
   - RFC 133 §6d (2026-09-09) named the blob and patch containers among the five paths growing with N, then set them
     aside as "the repository's own content… correctly not counted". The round closed with **four-fifths of the
     memory growth unattributed**, and the architect accepted that. Site A was inside that residual.
   - The RFC 102 fix handoff attributed the one-small-file commit's peak to the append alone. The team found site B.
     The outcome-based acceptance figure (M2) is what caught the error, not the stated cause.

## 2. Why each case lived

- **Nothing stated the invariant, so nothing tested it.** An operation on one object should cost the same whatever the
  store holds. No test asserted this.
- **The measurements confounded store size with node count.** RFC 133 scales N with content 1:1, so a per-operation
  read of the store grows with N and reads as "memory grows with N". This is RFC 139's lesson again, profiles
  confounded on breadth.
- **Bytes read were never measured.** Timings run on tmpfs, where re-reading the page cache is cheap. Only peak RSS
  and wall time were recorded.
- **The easiest primitive reads the whole file.** `read_file_if_exists` is the default, and using it for a length or a
  slice looks harmless in review.
- **A comment stood in for a count.** Site C's code says its whole read runs when something else grew the index, "which
  is expected to be rare". It runs after every own write. Nobody counted.
- **An implicit bound was lost when the primitive changed.** A whole-file read cannot allocate more than the file
  holds. A positioned read allocates whatever it is told.
- **Allocation failure in Rust aborts.** A bad length therefore bypasses every error path, `verify`'s included.
- **Local files were treated as trusted input.** RFC 158 Stage A bounded untrusted input (bundles). The damage tests for
  local containers cover checksums, magic and running past the end, never an absurd length.
- **An unattributed residual was accepted as a result**, where it should have stayed a question.

## 3. The measures

Structural guards first; they fail a build. Process rules last; they depend on the architect.

1. **P1 — whole reads of store-growing files are declared (a `cfg(test)` guard).**
   - The anchored reader, under `cfg(test)`, fails the test that performs a **whole** read of a file that grows with
     the store, unless the read happens inside a declared scope. The files: object containers, the object index, and
     the ref log containers.
   - The declared scopes are one table, each with its reason: `verify`'s scans, `rebuild_index_from_containers`,
     `doctor --repair-index`, `IndexSnapshot::open` (once per session; AUD-01), and whatever the inventory finds.
   - **The whole store test suite, about 2,400 tests, becomes the detector.** Any production path that starts reading a
     container whole fails some test, naming the path.
   - Shipped builds are untouched.
2. **P2 — store-size independence, one table of operations.** For each per-object operation (object append; object
   read; the index refresh after an own write; a one-file commit through the store API; a one-block seal), run it on a
   small store and on a large one (for example, 1 MiB and 16 MiB of content). Bytes read from `.prikk/` must differ by
   at most the operation's own records. It uses the read tally the fix round added. A new operation joins the table in
   the round that adds it.
3. **P3 — no allocation sized by a decoded length without a bound (a source-scan test).**
   - A test reads the production sources of `prikk-store` and `prikk-object`. It fails on
     `with_capacity(…)`, `vec![0…; …]` and `reserve(…)` whose size comes from decoded data (`as usize` on a read
     value, or a cursor read) unless the site is listed with its bound.
   - A `ByteCursor` helper gives the bound mechanically: capacity = min(count, remaining bytes / smallest encoded
     element).
   - **`trust_index.rs:344` is fixed in this round** with it.
   - The ranged reader clamps to the file inside the primitive (RFC 102 Addendum 1 item 1), so no caller can forget.
4. **P4 — a hostile-length damage suite.**
   - For **every** on-disk record format with a length or count field, a test sets that field to its maximum. The
     formats: container frame, object index, WAL, trust index, author-key index, pointer index, received index, ref
     containers, lifecycle cache, verified-blocks record, commit index, generation file.
   - Each must be refused with an error: no abort, and no allocation of the claimed size. Cases that could abort run in
     a child process, so a regression fails one test instead of killing the suite.
   - **At the CLI:** `prikk verify` and `prikk doctor` on a sealed repository whose first record in each container
     claims 2⁶² bytes exit non-zero with a finding, never on a signal. **A damaged repository never crashes the tools
     that diagnose it.**
5. **P5 — release prep measures the store-size axis and bytes read.** The template's memory step gains two things:
   - the one-small-file commit's peak RSS **and bytes read** at an 8 MiB and a 256 MiB blob container, flat or
     explained;
   - bytes read beside peak RSS in the release-gate profile.

   It costs about 15 s. The template edit is the architect's.
6. **P6 — an unattributed residual stays open (amends RFC 133's method).**
   - An attribution that leaves more than a fifth of an effect unexplained ends with a named open finding and its next
     probe, never with "the residual is the result".
   - Attribution traces **bytes read by path** (as the fix round did with its read trace), not only sizes on disk and
     structures in memory.
   - **RFC 133's residual is reopened:** 0.48.0 prep reports how much of the N = 64,000 residual the RFC 102 fix
     removed.
7. **P7 — the architect's handoffs and reviews:**
   - a cause stated in a handoff says **"confirmed by <measurement or prototype>"** or **"suspected"**;
   - acceptance is the measured outcome, never "the named line is gone";
   - in review, every replaced whole read gets one question: *what bound did the whole read give for free?*
   - every "rare", "once" or "cheap" in a comment on a hot path gets one question: *counted where?*

## 4. What each measure would have caught

| case | P1 | P2 | P3 | P4 | P5 | P6 |
|---|---|---|---|---|---|---|
| A. append read (2026-08-14) | yes: the first test to append through the file store | yes | — | — | yes, as M2 in the next release prep | likely: the containers were in the residual's own sweep |
| B. object read | yes | yes | — | — | yes | likely |
| C. index tail after own write | yes | yes | — | — | via bytes read | — |
| S-2 unbounded allocation | — | — | yes | yes, at the CLI | — | — |
| `trust_index.rs:344` | — | — | yes | yes | — | — |
| the misattributed M2 | — | — | — | — | — | P7's "confirmed by" |

**No single guard covers every case.** P1 and P3 are the structural core: they fail the ordinary test run with no
measurement at all.

## 5. Controls: each guard is shown red by its own defect

- **P1 and P2:** put back `index.rs:555`'s whole-container read. Both go red. Then put back S-2's whole read, then site
  C's, one at a time.
- **P3:** remove the clamp in the ranged reader. The scan goes red. Revert the `trust_index.rs` fix, and it goes red
  again.
- **P4:** remove the clamp: the container-frame case goes red, and so does the CLI `verify` case, without killing the
  suite. Revert the `trust_index.rs` fix: its case goes red.
- **P5:** run the new step on the 0.47.0 binary. It reports the growth: 22 → 266 MiB peak.

## 6. Scheduling and what the owner decides

1. **Accept the direction** (P1–P7), or name what to drop.
2. **When:**
   - **(a) 0.48.0**, one round after the RFC 102 fix round, before release prep. This is the architect's
     recommendation: 0.48.0 is the release both consumers are waiting for, and these guards protect exactly what it
     fixes. The round is tests, one source-scan, one small helper and one decoder fix.
   - **(b) 0.49.0**, with `trust_index.rs:344` alone pulled into 0.48.0 as a one-line fix.

**Not in scope:**
- AUD-01, the object index's linear lookup: a CPU cost with its own ROADMAP row and a memory condition;
- fuzzing the decoders: a later option, which P4's fixed cases do not replace;
- RFC 158 Stage B's commit holding all new content at once: noted in that RFC.

## 7. Architect's addendum, 2026-09-26 — `verify` does not report an indexed object whose record cannot be read

Found while reviewing RFC 102's append-length round (review `append-length-without-reading-review-v2`); present in
0.47.0 too. `verify/objects.rs` does the following:
- the index pass skips any entry whose record fails to decode, on the stated assumption that the container scan below
  reports it;
- the container scan (`decode_container_records`) classifies a frame whose header claims more bytes than remain as a
  **torn tail**, the harmless remnant of an interrupted append, and tolerates it.

So a damaged header on the last record goes unreported by either pass. Measured, on 0.47.0 and on `9137125e` alike: a
repository whose only blob has a header claiming 2⁶² bytes; `prikk verify` prints `object items: 0 scanned, 0 failed`
and **exits 0**. The damage surfaces only when a sealed block's state replay happens to touch the object.

**A genuine torn tail can never have an index entry**, because the index is appended only after the container record is
durable (RFC 102's write protocol). So an indexed entry whose record cannot be read is always damage.

**Ruled, as part of P4 in this RFC's round:**
- the objects stage reports such an entry as a `Failed` item, naming the object id and offset, unless the container scan
  already reported a failure at that offset;
- P4's CLI control covers an **unsealed** repository as well as a sealed one;
- **perturb:** restore the `continue`; the unsealed CLI control goes red.

This is within the accepted direction ("a damaged repository never crashes the tools that diagnose it"): a tool that
exits 0 over damage it could see is the same failure, quieter.
