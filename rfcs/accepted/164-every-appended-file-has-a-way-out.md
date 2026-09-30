# RFC 164 — Every appended file has a way out: tails by position, nothing silent, one repair

**Status.** **ACCEPTED by the project owner 2026-09-30** (*"Yes. RFC 164 is accepted."*), after reading. Both §6
decisions were ruled first: one `--repair-tails` verb (*"Approved."*) and Rule E classification only (*"Accepted."*).
**The architect's reading, stated so it can be corrected:**
- **Rules A to E are accepted as written.** Rule A covers the trust keys, trust policy, author keys, received index and
  the three generation logs. The object containers, the object index and the ref log keep their own rules.
- **`--repair-tails`** covers every file with a position-defined tail: those of Rule A, plus the WAL and the pointer index
  (RFC 162 rule 3). It does not truncate the object containers or the ref log; Rule B makes them report.
- **Implementation is two rounds:** A, B and C first (`rfcs/handoffs/164-every-appended-file-has-a-way-out/
  round-1-handoff-v1.md`, live 2026-09-30), then D and E.
- **The ref log's repair and interrupted publications stay in the F1 round** (0.49.0 step 2).
- **2026-10-01: §9 corrects Rule A** (a complete record is never a tail), after round 1's review found that the rule as
  written can roll back trust and ref state. ACCEPTED by the owner; the reasoning is §9.1.
- **2026-10-01: §9.2 proposes a correction to §9** (the checksum decides whether a record is complete), after
  Addendum 1's review found that a flipped header byte still makes a complete record read as a tail, in the readers as
  well as the repairs, and that the trust-policy case has shipped silently. It awaits the owner's reading.

*History:* **PROPOSED 2026-09-30 by the architect** (0.49.0 step 1).

**Author-review independence.** The architect proposes, and the architect's own rulings left most of the gaps below:
RFC 162 rule 3 covered two files, and RFC 163 guarded writers but gave four files no repair. Two things compensate:
- every rule here extends a rule that already holds in the codebase for at least one file, and was tested there;
- the recommendation (§8) is that the 0.49.0 candidate goes to the external architect, whose matrix covers every file
  named here.

## 1. What 0.48.0 left

Measured, not argued, by the external reviews 015–017, the step-0 soak and the architect's probes:

| file | a crash tail today | a damaged-shaped tail (zeros, garbage) today | `verify` says | repair |
|---|---|---|---|---|
| WAL | tail, by position (RFC 162) | tail | a line | `--repair-wal-tail` |
| pointer index | tail, by position (RFC 162) | tail | a line | `--repair-pointer-index-tail` |
| trust keys, trust policy, author keys, received index | writes refuse (RFC 163); readers tolerate a torn prefix | **damage**: the reading command refuses, "has a damaged entry; run doctor" (N2's remainder) | **nothing** about a torn prefix (N7) | **none**; the way out is manual |
| the three generation logs | `compact` refuses (RFC 163 §9) | **damage**; on the pointer index's log **every command refuses** (N10) | nothing (N7) | none |
| ref log | positive rule (its own, RFC 162) | silent; a later `seal` appends behind it (M4) | nothing | none |
| the five object containers | connectivity (RFC 162 rule 2) | connectivity | a short tail: nothing (N7) | — |

**And two command-level gaps:**
- **RFC 163 §10.** A publication, or a new author's commit, refuses over its guarded file only after it has written its
  content objects.
- **Step 0's finding.** An interrupted `bundle import` left blocks naming missing objects. 0.49.0 now writes in dependency
  order, so it no longer happens. But a repository already in that state has one way out, re-running the same import,
  and a user without the bundle has none.

## 2. Rule A — a tail is defined by position, for every appended file that is not an object container

**For the trust keys, trust policy, author keys, the received index, and the three generation logs,** as RFC 162 rule 3
already holds for the WAL and the pointer index: **the tail is everything after the last sound record, when no sound record
follows, whatever its shape** (a torn prefix, zeros, garbage).
- **Readers tolerate a tail.** None of them refuses over it, so N10's "every command refuses" and N2's "run doctor" for a
  tail both end.
- **Writers refuse before appending behind it** (RFC 163's guard, now covering every shape).
- **Interior damage stays damage**: a sound record after the bad bytes is refused and named, as RFC 162 made it for the
  WAL.
- **Not changed:**
  - the object containers keep connectivity (RFC 162 rule 2);
  - the object index stays a cache (rule 1);
  - **the ref log keeps its positive rule.** Its tail and its interrupted publications (N3) are settled together in the
    F1 round (0.49.0 step 2). This RFC only makes the ref log speak (Rule B).

## 3. Rule B — nothing is silent

**`verify` and `doctor` report, for every framed file:**
- a tail (a warning, with the file, the offset of the last sound record and the byte count);
- interior damage (a failure, naming its offset).

That covers N7's short tails in the object containers, the ref log, the trust files, the author keys and the received
index, and M4's garbage in the ref log. **A repository whose only findings are tails still exits 0**, because a crash tail
is not damage. Each warning names the repair.

## 4. Rule C — one repair for every tail

**`prikk doctor --repair-tails`** truncates every tail Rule A defines, in every file it covers, and says what it removed
per file.
- **It saves the removed bytes** to `.prikk/recovery/`, as the WAL repair does, before truncating.
- **It is idempotent.** If any covered file has interior damage, it refuses before touching any file, naming the file
  and the offset.
- **It runs under the same locks as the files' writers.**
- `--repair-wal-tail` and `--repair-pointer-index-tail` stay, as the single-file forms.
- **Windows (D5):** the recovery file's save is claimed durable only where the platform makes it so, as today.

## 5. Rule D — refuse before the first write; Rule E — a remnant is not damage

**Rule D (RFC 163 §10).** A publication (`seal`, `branch create`, `tag create`, `merge`) and a commit by a new author check
their guarded files' tails **at the start of the command, before any content object is written**, as `bundle import`
already does. A refusal then writes nothing at all, for every guarded writer.

**Rule E (step 0's bundle-less case), extending RFC 162 rule 2 from frames to whole objects.** A stored object whose
references are missing is **damage only if something committed reaches it**: a ref, a received pointer, a queued patch, or
a sealed block reached from them. Otherwise it is an **unreferenced remnant with missing references**:
- a warning naming the object and what it lacks, and "re-run the import if you still have the bundle; otherwise it is
  harmless";
- `verify` exits 0 over it;
- **no command removes it in 0.49.0.** Containers are append-only, and removing a record needs object-container
  compaction, which is not built (`containers/generations.log` is reserved for it).

## 6. The owner's decisions

**Both are ruled.** RFC 164 as a whole is still PROPOSED, until the owner accepts it.

1. **One tail repair, `--repair-tails`, not one verb per file. APPROVED by the owner 2026-09-30** (*"Approved."*). The
   reasons, as the architect gave them to the owner:
   - **One rule, one mechanism.** Rule A gives every file the same tail definition. Per-file verbs are separate code paths,
     and every gap in the 0.48.0 cycle was a copy that drifted from the rule:
     - RFC 162 rule 1 applied to the object index only;
     - the RFC 163 guards differed per file;
     - the received-index guard alone ignored damage (N9).

     One implementation and one test set cannot leave a file behind.
   - **A crash leaves tails in more than one file.** An import touches the received index and the author keys; a
     publication touches the pointer index and the ref log. Per-file verbs make the user work out which files, and in what
     order, which is the "run doctor" dead end 0.48.0 disclosed. One verb repairs each tail it finds, per file under that
     file's lock, and reports per file.
   - **Discoverability.** Every `verify` warning names the same verb, and `troubleshooting.md` needs one entry, not seven.
   - **Future files join by construction.** A newly appended file joins the rule and the verb without a new flag. Each new
     flag is CLI surface for consumers, plus help text, docs and tests.
   - **Nothing breaks.** `--repair-wal-tail` and `--repair-pointer-index-tail` stay as the single-file forms.

   **The trade-off, stated:** one command touches several files, with less granular control. It is bounded three ways:
   - every removed byte is saved to `recovery/` first;
   - the verb refuses on interior damage in any file, before it touches anything;
   - it reports per file.

   A `--file <kind>` option can be added later without changing this design.
2. **Rule E: classification only in 0.49.0. ACCEPTED by the owner 2026-09-30** (*"Accepted."*). An unreferenced object
   with missing references is a warning, and `verify` exits 0 over it. **No command removes it in 0.49.0**: removal needs
   object-container compaction, a larger design for 0.50.0 or later.

## 7. The matrix, and what it must show

- `rfc162_recovery_matrix.rs` and `rfc163_*` gain, **for every file in §1**:
  - a torn prefix, 100 zero bytes and 100 random bytes;
  - both orders: the repair first, and the write first;
  - I1 to I5 asserted, and a `verify` line asserted where Rule B requires one.
- **Controls:** remove Rule A's reader tolerance, Rule B's line, `--repair-tails` and Rule D's early check, one at a time,
  at each site. Each removal must redden its rows.
- **The external architect's `matrix.py` v4 must show:**
  - the 21 damaged-shaped-tail I5 cells gone;
  - the N7 silent cells reduced to those this RFC leaves (the object containers' torn prefixes, if connectivity still
    calls them harmless, now with a line);
  - N10's cells gone.

## 8. Scheduling

1. **This RFC, read by the owner, then accepted or changed**, with §6 decided.
2. **Implementation, in two rounds:**
   - Rules A, B and C;
   - then Rules D and E.

   Each round is gated, probed and matrix-checked.
3. **Output changes** for the 0.49.0 notes and the consumer letters:
   - the new `verify` lines, and their JSON fields;
   - the new `doctor` verb;
   - fewer refusals.
4. **External review of the 0.49.0 candidate: recommended**, because damage is reclassified again. The owner decides when
   the candidate exists.

## 9. Amendment: a complete record is never a tail

**Status: ACCEPTED by the owner 2026-10-01** (*"Accepted. Record the reasoning and our consideration around it."*). It was
proposed by the architect on 2026-10-01 and read before acceptance. It changes Rule A as first accepted, and RFC 162 rule
3 for the pointer index. **The owner asked that the reasoning and the consideration around it be recorded: §9.1.**

**What went wrong.** Rule A says a tail is everything after the last sound record, "whatever its shape". That includes a
**complete** last record (a valid header, its full claimed body present) whose checksum fails. Such a record cannot be told
apart from a crash only when it is torn, and **a complete record is not torn: its bytes were all written.** For files
whose records carry state, removing the last one rolls that state back. Measured by the architect on round 1's release
build (`09f416af`, sha256 `578999d0…`, `arch-seal/rfc164_rollback_probe.sh`) and on the shipped 0.48.0
(`pointer_index_flip_probe.sh`):

| file | one byte flipped in the last complete record | `verify` | after the repair |
|---|---|---:|---|
| trust policy (round 1) | the snapshot that removed a maintainer | **0, silent** | **`--repair-tails` deletes it, and the removed key is trusted again** |
| pointer-index generation log (round 1) | the newest generation | 1 | `--repair-tails` deletes it; the live slot reverts, and **branch `heads/keep` is gone** |
| pointer index (**shipped 0.48.0**, RFC 162) | the newest publication | 1 | `--repair-pointer-index-tail` deletes it; **`main` reverts to the previous block**, and `verify` stays 1 |

The removed bytes are saved in each case, but saving them does not undo the meaning the repair changed. The first row is a
security defect: a single corrupted byte, then the documented repair, silently undoes a revocation.

**Proposed rule, replacing "whatever its shape" for the seven Rule-A files and for the pointer index:**
- a **tail** is a record whose header or body is incomplete (the bytes end first), or bytes that are not a record header
  at all (no magic, or an unknown version), **when nothing sound follows**;
- **a complete record whose checksum or envelope fails is damage, even when it is last.** `verify` fails and names it.
  `--repair-tails` and `--repair-pointer-index-tail` refuse, and change nothing. Readers treat it as they treat interior
  damage: **fail closed.** No reader reads past it to an older state;
- **the WAL keeps RFC 162 rule 3 as it is.** Removing its damaged last record loses a queued commit that is saved and
  disclosed (N6), not a rollback of trust or ref state. N6's witness (0.49.0 step 3) will close that case properly.

**What it costs.** A bit flip in the last complete record of one of these files now stops the commands that read that
file, instead of being repaired into a rollback. That is the old behaviour for damage, and the honest one. The way out is
restoring the file from a copy. Rebuilding the pointer index from the ref log belongs to the F1 round (0.49.0 step 2).
The measured crash shapes the external reviews found (torn prefixes, zeros, garbage) all stay tails, so N2's and N10's
fixes hold.

**For 0.48.0 users:** the known limitations disclose that `--repair-pointer-index-tail` can remove a damaged, not torn,
last record, and that the ref then reverts. The fix is this rule, in 0.49.0.

### 9.1 The reasoning, and what was considered (recorded at the owner's request)

**The principle.** A crash can only interrupt a write, so what a crash leaves at the end of an appended file is always
*incomplete*: a short header, a short body, or bytes that were never a record (zeros, garbage). **A record whose header is
valid and whose whole claimed body is present was fully written.** If its checksum fails, something changed it *after*
it was written. That is corruption, not a crash. Treating it as a crash tail lets a repair delete a record that was really
committed, and for files whose records carry state, the file then means an older state.

**Why "the bytes are saved" does not make it safe.** RFC 162 rule 3 accepted this trade-off for the WAL because a removed
WAL record is a queued commit, saved in `recovery/` and disclosed (N6). The user loses work in progress, not a decision.
In the files §9 covers, the last record *is* a decision:
- the trust policy's latest snapshot says who is trusted;
- a generation log's newest record says which slot is live;
- the pointer index's newest entry says where a ref points.

Removing it silently re-asserts the previous decision. The bytes in `recovery/` do not tell the repository to stop
trusting a key it had revoked.

**Considered, and not chosen:**
1. **Keep "whatever its shape", and rely on the recovery file.** Rejected, for the reason above. The trust-policy case
   was also silent: `verify` exited 0 before the repair.
2. **Refuse only for the trust files.** Rejected. The generation logs and the pointer index showed the same rollback: a
   lost branch, and a reverted ref. One rule for the whole class is the lesson of 0.48.0, where every gap was a
   per-file copy.
3. **A witness per file** (a count or end offset written with each record, as N6's witness will be for the WAL). It would
   tell a crash from corruption exactly. Not chosen for 0.49.0: it is a format change, a larger design, and §9 already
   closes the unsafe outcome. Recorded as format-8 input, beside RFC 162 §3.
4. **Readers tolerate the damage, reading up to the last good record.** Rejected: that *is* the rollback, done by every
   reader instead of by the repair. For the trust policy, a reader that skipped a damaged revocation would trust the
   revoked key.
5. **Chosen: fail closed.** A complete record with a bad checksum is damage wherever it sits. `verify` fails and names it,
   the repairs refuse and change nothing, and readers refuse as they do for interior damage.

**What it costs, accepted knowingly.**
- A single corrupted byte in the last complete record of one of these files stops the commands that read that file. For
  the pointer index's generation log, that is almost every command.
- The way out is restoring the file from a copy, until the F1 round (0.49.0 step 2) can rebuild the pointer index from
  the ref log.
- This is the behaviour 0.48.0 already had for damage in these files. It is honest, and it cannot be turned into a
  rollback.
- **What stays repairable is exactly what the external reviews measured:** a torn prefix, zeros and garbage. N2's and
  N10's fixes hold.

**The WAL is the one exception, and why.**
- Its damaged last record stays a tail (RFC 162 rule 3, N6), because what it loses is saved, disclosed, and not a
  decision.
- The repair's output already says when it removed a whole record (0.48.0).
- N6's witness (0.49.0 step 3) replaces the trade-off with an exact answer.

**The shipped 0.48.0 case, and no advisory.**
- `--repair-pointer-index-tail` can remove a damaged, not torn, last record, and the ref then reverts.
- It needs local corruption of the file, and no untrusted input reaches it. `verify` fails before and after, so it is not
  silent, and the removed bytes are saved.
- **The architect judged a disclosure in the known limitations enough, not a security advisory.** The owner was told, and
  may ask for one.
- The trust-policy rollback, which would have been silent, never shipped: it existed only in round 1, and is fixed before
  any release.

**How it is held.** Invariant I6, in every repair row of the matrix: a repair never changes the meaning of committed
state. The trust set, the ref tips and the live slots are identical before and after. Each file's §9 rule gets a control
that restores "whatever its shape" and shows the rollback rows go red.


### 9.2 Proposed correction, 2026-10-01: the checksum decides whether a record is complete (for the owner's reading)

**Status: PROPOSED by the architect, for the owner's reading.** It corrects §9's definition of a tail. §9's principle and
§9.1's reasoning are unchanged; this is the second correction in one round, so it is a design re-read (RFC 152 §7), not a
fix-round detail.

**What was found.** Round 1's Addendum 1 implements §9 as written. On its release build (`284a5308`, sha256 `331f04f5…`),
the architect flipped **each byte of the last record in turn**, header included (`arch-seal/rfc164_every_offset_probe.sh`,
`rfc164_header_flip_reader_probe.sh`). The header of these formats is magic (8 bytes), version (2), length (8), checksum
(32).

| file | bytes flipped | before any repair | after the repair |
|---|---|---|---|
| trust policy (80-byte last record) | checksum field or body: 62 of 80 | `verify` 1; readers refuse | refuses; nothing moves. **Correct** |
| trust policy | **magic, version or length: 18 of 80** | **`verify` 0; the removed maintainer is already trusted again** | `--repair-tails` truncates the record |
| pointer index (132 bytes) | **magic, version or length: 18** | `verify` 1; **`branch list` shows `heads/keep` at an older tip** | refuses; readers still show the older state |
| pointer-index generation log (51 bytes) | **magic or version: 10** | `verify` 1; **`branch list` drops `heads/keep`** | refuses; readers still show the older state |

**Why.** §9 decided "complete" from the header: bytes with no magic or an unknown version were "not a record", and a
length claiming more than the file holds made the record "incomplete". **But those header fields can be what got
corrupted.** A record whose magic byte, or length byte, was flipped was still fully written. §9's own probe flipped one
body byte only, and so did the matrix. One sample is not a measurement: the region is every byte of the record.

**The rollback happens in the readers, not only in the repairs.** Rule A makes every reader tolerate a tail. So a
complete record misread as a tail rolls back **every reader, before anyone runs a repair**, and silently wherever
`verify` reports a tail as a warning.

**It shipped.** Measured by the architect on released binaries:
- **The trust policy, 0.46.0 and 0.48.0:** a flipped length byte in the last snapshot. `verify` exits 0 (0.46.0 does not
  read the trust files at all), and readers trust the removed maintainer again. **Silent.** The partial-record arm that
  does this dates from the trust container's introduction (`2827fab7`, first released in 0.20.0); the dev team confirms
  the affected range from history.
- **The pointer index, 0.48.0:** *any* flipped byte in the last record, header or body. Readers show the ref at its
  previous tip before any repair. `verify` exits 1. §9's disclosure named only the repair.

**Proposed rule: the checksum decides.** Every one of these formats computes its checksum over the magic and version *as
the format defines them* (constants, not the stored bytes), the length and the body. At the first position after the last
sound record:
- **if the stored checksum verifies** over the claimed length, or over the length that reaches the end of the file, **the
  bytes are a complete record**, whatever the stored magic, version or length say. A complete record is never a tail
  (§9): damage, fail closed;
- **otherwise it is a tail.** A torn prefix, zeros or garbage cannot carry a SHA-256 that verifies over them.

**What it covers, and what it does not:**
- **Every single flipped byte in the last record is then caught:**
  - body and checksum field: already by §9;
  - magic or version: the checksum verifies over the claimed length;
  - length: it verifies over the length to the end of the file.
- **Cost:** one extra SHA-256 over at most the tail's bytes, and only when a tail is found. N2 and N10 are unchanged:
  their tails carry no checksum that verifies.
- **What remains:** corruption of more than one field of the last record still reads as a tail. For example, a zeroed
  sector covering the header and the checksum. Only a per-file witness closes that (§9.1, alternative 3, format 8). The
  known limitations say so.
- **The same files as §9:** the seven Rule-A files and the pointer index. The WAL stays under RFC 162 rule 3.

**For users of released versions.** The known limitations and CHANGELOG disclose:
- the trust-policy rollback, silent, from 0.20.0 (to be confirmed) through 0.48.0;
- the pointer index's readers and its repair, 0.48.0.

Fixed in 0.49.0 by this rule. **An advisory is the owner's decision.**
- **The architect recommends a disclosure, not a GHSA:**
  - it needs a corrupted local file, and no untrusted input can write one: bundles, sync and imports never write the
    trust policy;
  - anyone who can write `.prikk/` can change the trust policy with the CLI anyway, so an attacker gains nothing.
- **Unlike §9's case, this one is silent in shipped versions.** That is the argument for an advisory, and the owner may
  prefer one.
