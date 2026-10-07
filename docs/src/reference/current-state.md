# Current State

What prikk can do today, what it cannot do yet, and the limits worth knowing before you rely on it.
**This page describes deferrals — things not built yet.** Permanent refusals, which are a different
thing, are in [Non-Goals](non-goals.md).

## What works today

The local core can initialize a repository, author signed patches, seal them into blocks, inspect
history, verify integrity, diagnose common repository issues, perform safe checkout planning and
materialization for the supported subset, display merge evidence and merge plans for explicit sealed
candidates, and **execute a merge** when the two sides are proven confluent — refusing cleanly, with no
object, WAL, or ref write, when they are not.

**Cross-platform history identity is tested, not assumed.** Prikk authors, commits, and checks out on Linux, macOS, and Windows, and CI requires a repository authored on Linux, mutated on Windows, and verified back on Linux to produce byte-identical object ids — so the claim that anyone can verify anyone's history holds across the three.

Known limits worth stating up front: merge-base discovery is manual; conflicts are detected and refused
but never resolved; sync exists between repositories, but **prikk does not move the bytes itself** —
confidentiality is the user's channel's property, not prikk's — negotiation is branch-scoped (tags
travel and are adopted separately, under the receiver's own key), and there is no discovery or
remote-tracking; `verify` cost is linear in history length; `verify` checks author signatures
repository-wide, but only as trust-on-first-use continuity — it proves the same author signed as last
time, not who that author is on first contact; and `verify` checks a locally-published tag's
maintainer signature against this repository's own trust policy, but a received, not-yet-adopted tag
is deliberately exempt — its signature is the sender's, under a key this repository has not adopted.

Next increment candidates are tracked in `ROADMAP.md`.

## Not a good fit yet

Prikk is not yet the right tool if you need:

- a production replacement for Git;
- stable repository-format compatibility;
- Git object compatibility or transparent Git interoperability;
- hosted forge workflows, or remotes;
- complete branch management, or semantic merge;
- plugin/audit execution, attestations, or automated publication controls;
- mature key lifecycle features such as revocation, rotation, hardware signing, or thresholds;
- flexible exclusion of generated files — `.prikkignore` (since 0.29.0) takes literal repo-relative
  path prefixes, one per line, with no globbing, no negation, and no per-directory files, so
  patterns like `*.log` do not work; and a file swept into history by mistake still cannot be
  removed later.

## Known limitations, measured

Disclosed here rather than left implicit, each with the figure it was measured at and the release it is
planned for.

- **Trust keys, trust policy, author keys, the received index, and the three generation logs now have
  a tail defined by position, a `verify`/`doctor` line, and a repair (fixed in 0.49.0, RFC 164 Rules
  A/B/C, then corrected by §9).** Before this round (N2's remainder, N7, N10, M4): a torn prefix at
  these seven files was a repairable-looking tail, but 100 or more zero or random bytes at the same
  position was *damage* under their own pre-0.49.0 shape rule -- `trust maintainer add`, a commit, or
  `bundle import` refused with "\<container\> has a damaged entry", a generation log's own reader
  refused every command that resolves a ref (on the pointer index's own log) or just `compact` (on the
  other two), and none of the seven had a repair verb -- the way out was a manual backup-and-truncate.
  Rule A first defined a tail as "everything after the last sound record, whatever its shape"; **§9
  corrected this the same round, before release, once measurement showed it let a complete last record
  with a failed checksum be truncated as a tail** -- for a file whose last record carries a decision
  (the trust policy's latest snapshot, a generation log's live slot, the pointer index's newest
  pointer), that silently re-asserted the previous decision. A single flipped **body** byte in a real
  trust policy snapshot, followed by the pre-§9 `--repair-tails`, silently re-trusted a key that
  snapshot had just revoked, `verify` exiting 0 throughout — measured on a release build before this was
  closed. **§9 was itself incomplete: it decided "complete" from the header's own magic, version, and
  length fields, and any one of those three can itself be the single corrupted byte.** A flipped
  **header** field rolled every reader back to the state before the last record, before any repair even
  ran -- §9.2 corrects this: completeness is decided by the checksum, recomputed with the format's own
  real magic and version constants over either the claimed length or the length to the end of the file,
  whatever the stored header fields say. Verified exhaustively, not sampled: every single byte offset of
  a last record, across the trust keys, trust policy, author keys, the received index, the generation
  logs, and the pointer index, is now asserted (a cheap, decode-only test per format) to never decode as
  a tail. Now: a tail is an incomplete record or bytes that are not a record header, when nothing sound
  follows; a *complete* record (its checksum verifying one of the two ways above) is damage, even when
  last, however its header got corrupted. `verify` and `doctor` report a tail as a warning (never failing
  on it alone) and interior damage (including a complete, corrupted last record) as a failure; `prikk
  doctor --repair-tails` truncates every genuine tail these seven files (plus the WAL and the pointer
  index) have, in one run, saving what it removes first, and refuses -- unconditionally, changing nothing
  -- if any covered file has interior damage, including a complete corrupted record. **What remains:**
  interior damage still has no automatic repair -- by design, the same as every other covered file -- so
  the way out for it is still manual (back the file up first, ask before truncating; a complete corrupted
  record has no offset a repair can safely act on at all). **Corruption spanning more than one field of
  the last record** (for example a zeroed sector, touching the checksum and other fields together) can
  still read as a tail: the checksum-decides rule above only resolves a single field's own corruption
  against an otherwise-intact record. Telling that shape apart reliably needs a per-record witness
  (format 8), not yet built. The object containers' own short tails (N7) are now reported too
  (`verify`/`doctor` print a trailing-partial byte count per persisted object type), reporting only, no
  repair, per the review's ruling. **The ref log's own tail (M4) and N3's interrupted publications are
  now settled (RFC 165, 0.49.0):** the ref log is Rule B's eighth reported file and `--repair-tails`'s
  tenth covered one (§9.2 applied to it the same as the other seven); a torn last record with a
  completable pointer lead is an interrupted publication, completed by `prikk ref complete <ref>`
  (any adopted key, not only the one that started it), never truncated as a tail. **Disclosure, released versions
  (no advisory, per the owner's ruling); code history kept apart from measurement:** the code shipped in
  0.20.0 for all three files (trust policy `2827fab7`, the pointer index `0550e340`, the three generation
  logs `b33d1942`), confirmed from history, not measured directly. **Measured on 0.46.0, 0.47.0, and
  0.48.0** (0.20.0-0.45.0 not independently confirmed), with one reader per file -- `trust maintainer
  check` for the trust policy, `branch list` for the pointer index and its generation log, not every
  reader (round 2's own exhaustive, all-reader sweep is against the *fixed* 0.49.0 build, not these
  historical releases): a **length-field** flip in the trust policy's last snapshot read straight through
  to the previous one on all three measured versions, `verify` exiting 0 throughout (0.46.0's and 0.47.0's
  own `verify` does not read the trust files at all; 0.48.0's reads them but still exits 0 for this
  shape). A length-field flip in the pointer index's or its generation log's own last record rolled
  `branch list` back to the previous tip or slot on 0.46.0 and 0.47.0, `verify` exiting 1; on 0.48.0 the
  generation log refuses instead (its own one-byte-body rule rejects any length but exactly one), but
  **every byte, header or body**, flipped in the pointer index's own last record rolls `branch list` back
  with no repair involved, and a **body**-byte flip (checksum mismatch) is separately truncatable by
  `doctor --repair-pointer-index-tail` as a tail, reverting the ref's tip while `verify` stays failed both
  before and after. A flipped **magic** or **version** byte is refused on every
  measured released version -- that rollback shape existed only in this release's own, unreleased Rule A,
  never shipped. All fixed in 0.49.0 by §9 and §9.2.
- **A `bundle import` interrupted by a crash could leave a block durable while the patch, blob, or
  parent block it names is not, and no repair cleared it (fixed in 0.49.0).** In 0.48.0 and earlier,
  objects were written in the bundle's own carried order, not in dependency order -- across kinds,
  and, within one kind, a child block could be listed before its own parent. An interruption partway
  through could leave `verify` failing for good with one of: "object \<id\> references missing
  snapshot blob \<id\>" / "... missing block patch \<id\>" / "... missing parent block \<id\>";
  "snapshot of Block \<id\> names Blob \<id\> for \<path\>, which is missing"; or "lifecycle replay:
  blob \<id\> required for a state effect is missing" -- while a commit was still accepted. None of
  the three `doctor` repairs cleared it -- **re-running the same import cleared it in every case
  reproduced (25 of 25)**. Reproduced at 89 of 300 kills on 0.48.0 and 136 of 300 on 0.47.0, with an
  80-file bundle; present since `bundle import` was first introduced (0.20.0). Fixed in 0.49.0:
  every writer that lays down more than one object in a single command -- `bundle import`, `sync
  accept` -- now writes them in dependency order, both across and within kinds, so a new interrupted
  write can no longer produce this shape. **What remains, after RFC 164 Rule E (0.49.0): `verify`
  still needs the original bundle to make such an object whole, but no longer fails forever over one
  nothing in the repository actually needs.** `verify`/`doctor` now classify a stored object's own
  dangling reference by reachability from committed state (a ref, a received pointer, a queued
  patch, or a sealed block reached from them): reachable, it is still the same hard failure named
  above; unreachable, it is an **unreferenced remnant** -- a warning, naming the object and what it
  lacks, and `verify` exits `0`. No command removes a remnant in 0.49.0; re-running the same `bundle
  import` is still the only way to make a still-needed one whole, exactly as before this round.
- **Existence checks on RefState and Tag references (0.49.0 step 5, round 2).** `verify` now requires each
  attestation a RefState's `required_attestation_ids` names to be present as an Attestation object (a typed read).
  No producer in this repository writes a non-empty list, so an honest repository cannot fail it. **Not checked, and
  disclosed as gaps:** `Attestation.target_block_id` (there is no `AttestationPayload` decoder, and no producer, so
  this is a format decision, not debt); and a received ref's `previous_ref_state_id` is not required by `bundle import`. `verify` does
  check a received tip's previous state (one read), and `bundle import` refuses a bundle whose ref chain is not carried.
- **A crash inside `branch create` or `tag create` now has a command that completes it (N3, fixed in
  0.49.0).** Before this round: the ref
  log's last record is torn; `verify` fails with `PRIKK-VERIFY-REF-DIVERGENCE` and `doctor` recommends
  manual recovery, but retrying the same `branch create`/`tag create` answers "already exists" rather
  than finishing the interrupted publication -- DC-38's own retry exists for `seal` only. **Reached
  through `merge` too**: a crash mid-publication leaves the same `PRIKK-VERIFY-REF-DIVERGENCE` state,
  and re-running the same merge answers "not confluent" rather than finishing it. Measured for `merge`:
  10 of 300 kills on 0.48.0, 16 of 300 on the 0.49.0 build carrying the dependency-order fix above.
  **Fixed in 0.49.0 (R1-R3): every publication (`seal`, `branch create`, `branch close`, `tag create`,
  `sync adopt-tag`, `merge`) now refuses before its first write while *another* ref's publication is
  incomplete -- the same refusal `commit` already gave -- so a `seal` (or any other publication) can no
  longer bury an unrelated ref's torn record behind its own, appended content.** A publication's own
  retry of *its own* interrupted state is never blocked by this (`seal`'s DC-38 retry still completes).
  **Fixed in 0.49.0 step 3 (R4-R6):** `prikk ref complete <ref>` completes an interrupted `branch
  create`/`branch close`/`tag create`/`sync adopt-tag`/`merge` publication the same way `seal`'s own
  DC-38 retry always completed its own -- by any adopted maintainer key, not only the one that started
  it. Every condition evaluated before any write (the leading `RefState` verifies under current trust,
  chains cleanly, names a target that exists, and, for a WAL-consuming publication, retained WAL
  evidence still matches); a lead that fails is left alone (not completed), and the entry point's own
  "already exists"/"not confluent" refusal now names `prikk ref complete <ref>` when that is why.
  **`sync seal` remains locked out of its own interrupted publication**: its own precondition cannot
  yet tell "my own retry" apart from "another ref's incomplete work," so it refuses both -- a known,
  disclosed gap, not worked around in this round. **A lead that fails R4's rule entirely** (an
  untrusted or revoked signer, a broken chain, a missing or wrong-kind target, or mismatched WAL
  evidence) has no automatic completion; `prikk doctor --rebuild-pointer-index` re-derives the
  pointer index from the ref log instead, dropping that one lead (named) while leaving every other ref
  untouched -- it refuses outright if *any* lead anywhere is completable, since completing it is the
  correct fix and a rebuild would otherwise discard an authorized transition. **Addendum 1: a ref-log
  tail with no pointer lead (zeros, random bytes, or any torn prefix left for a reason unrelated to a
  pending write) is not an incomplete publication** -- `commit` and every other writer that does not
  append to the ref log proceed over it; every publication still refuses over it (RFC 164 Rule D: a
  writer refuses over a tail in a file it appends to), naming the tail's own offset and byte count; its
  repair is `prikk doctor --repair-tails`, the same as the other nine files it covers.
- **Fixed in 0.49.0 (N6): a damaged last WAL record is no longer removed uncritically as a tail.**
  RFC 162 rule 3 used to define the WAL's tail by position, not shape: a last record whose own bytes
  are all present but whose checksum fails was indistinguishable, once nothing sound followed it, from
  a genuine crash-torn append -- so `--repair-wal-tail` removed it either way, even when it was a commit
  the user had already been told succeeded. A commit now writes a small local witness alongside the WAL
  (the count, Patch id and frame hash of the last record it acknowledged); `verify`, `status` and every
  writer read it before a tail speaks. A genuine, never-acknowledged crash tail is still removed exactly
  as before (`--repair-wal-tail`, `--repair-tails`). An *acknowledged* commit that is now damaged or
  altogether missing refuses instead (`verify` exits 1; `commit`/`seal`/`rollback-draft` refuse, naming
  it) -- `prikk doctor --discard-damaged-commits [--plan-only]` is its own way out, never folded into
  the tail repair's single meaning.
- **Fixed in 0.49.0: a crash during a commit after the first one in a session could leave `ref-name`
  empty while the queue still held records (0.20.0-0.48.0).** Every commit, not only the first, used to
  rewrite this small file by a durable truncate then a durable append; a crash between the two left
  nothing durable naming which ref owned the queue. The file is now written once per session, by the
  first commit only -- a later commit never touches it again, so no commit past the first can tear it.
  `verify` and `doctor` exit 1, and `commit`/`seal`/`rollback-draft` refuse, over any session still
  stranded from before this fix (or over the same ref-name-vs-witness disagreement by a different
  route). `prikk doctor --restore-queue-target --ref <ref> [--plan-only]` is the way out -- the ref
  always comes from the caller, never from the witness; see [the troubleshooting
  entry](../guide/troubleshooting.md) for the exact text.
- **Fixed in 0.49.0 (R1): a ref publication now reads the whole ref log once, not three times.**
  `classify_state`'s own replay is threaded through the append's idempotency check and the post-write
  agreement check (a ranged read-back of just the bytes appended, not a fourth whole read). Measured
  at generations 4, 64 and 1,024: 3 whole reads before, 1 after, at every depth; about 1.8× faster at
  depth 1,024 on a release test binary. **What remains**: the write-path precondition (M8, below) used
  to share this same three-reads shape but is now fixed separately; reconciling this round's own
  ~417 B/generation figure against an earlier ~4.3 KB/generation measurement (likely counting more than
  the ref log alone) was not settled.
- **Fixed in 0.49.0 (R2): a commit's own write-path precondition no longer checks every ref's whole
  history.** `ensure_no_incomplete_publication` used to run a full `verify_refs`, replaying the ref log
  once per ref -- a commit touches no ref, yet paid refs × log size. Now one pass over the pointer
  index and one over the ref log, comparing each ref's newest pointer with its newest log record.
  Measured (release build): 62 µs at 1 ref before and after is noise-level, but 6.3 ms → 108 µs at 100
  refs, 74 ms → 417 µs at 400, and **6.8 s → 4.4 ms at 4,000**. `verify`/`doctor` keep the fuller check
  (chain continuity, signature-envelope structure, missing-object detection) this precondition never
  needed.
- **Listing objects by type reads that type's whole container**, including on `sync seal`'s own path.
  Deferred to 0.50.0 or later (ROADMAP.md, the 0.49.0 schedule).
- **A one-file `commit` reads every stored blob to learn its kind**: 1 MiB of stored content read 1.1 MB,
  16 MiB read 16.8 MB, 64 MiB read 67.1 MB. Not yet fixed; deferred to 0.50.0 or later (ROADMAP.md).
- **Fixed in 0.49.0 (RFC 165, M4): `verify` reports garbage bytes in the ref log.** Before 0.49.0, 100 zero bytes appended to the ref log
  container: `verify` and `doctor` exit 0 with no warning, a `seal` appends behind them, and `verify`
  stayed 0 with the garbage in the middle of the file. The state was harmless (ref-log records are signed
  and chained), but nothing reported the bytes. The ref log is now a reported file, and `--repair-tails` covers it.
- **Fixed in 0.49.0 (RFC 167): resynchronisation over hostile content is linear, not quadratic.** `verify`
  over a WAL torn tail packed with fake frame headers used to grow from 0.24 s at 256 KiB to 13.98 s at
  2 MiB, extrapolating to hours at 64 MiB (a plain, unpacked tail stayed 0.01–0.02 s at every size, so the
  cost was specific to the hostile shape). Six readers shared the defect: the WAL, object containers,
  trust policy, the received index, the ref log container and the pointer index. Each now carries a work
  budget (bytes hashed, at most 8x the input) shared by every candidate the resynchronisation scan visits;
  a scan the budget cuts short is always reported as damage, never silently read as a tail. Measured at
  64 MiB: 0.25–0.29 s, down from a projected 2.8–4.6 hours. **Not a self-vouching header** (a header-only
  checksum, which would remove the re-parse entirely): that is a format change, format-8 input, still
  planned for a future increment, not this one.
- **A `commit` holds all new file content in memory at once.** 64 files of 4 MiB each peaks at about
  284 MiB. Streaming and chunking (large objects, Stages B–D) are not yet built.
- **The object index's own lookup is a linear scan**, and the whole index is held resident in memory for
  the duration of a write session. Not yet fixed.

- **A WAL whose damage exhausts the resynchronisation scan budget has no repair command** (RFC 167). `prikk doctor`
  reports the failed record and recommends preserving the repository and inspecting it; no command in 0.49.0 removes
  the damage, so the way out is a backup or a clone. (Recorded from the doctor's own recommendation in source;
  not yet reproduced with a live damaged WAL.)

- **Three Windows residuals of the recovery log (RFC 168 §6, accepted).** Each is a new-name or rename case
  that Windows has no primitive for, so none is closed in 0.49.0:
  - **(a)** worktree files after a completed `branch switch` or `checkout` may not be durable after a power loss.
    The route: move the files `prikk worktree-status` lists as modified out of the way, then
    `prikk checkout --patch-materialize --ref <current branch>`. Missing files are written by that checkout; a
    file the user deleted and the checkout writes back is removed by the user.
  - **(b)** a repair's save in the same Windows boot as the log's creation, in a repository created before 0.49.0.
    The log is created by the first write command, so this is rare.
  - **(c)** the witness's first creation, in a repository created before 0.49.0. A lost creation falls back to the
    older classification (RFC 166 rule 3), the one a session written by an older binary gets today.

## What scale to expect

The list above is about missing features. Scale is a separate question, and worth stating on its
own: what happens as a repository's history gets deep, or its tracked file count gets large. Every
figure below was measured with a **release build** (the build you install) on Linux, on the
development state that shipped as 0.47.0 (a build that reported 0.46.0, plus the changes the CHANGELOG lists under 0.47.0), on
the RFC 139 corpus profile `prikk-self` (a real project's shape; 155 tracked files at depth 256). Each figure names the depth range it rests on. Depth is the number
of sealed blocks.

- **Sealing cost grows roughly quadratically with history depth, and per-seal cost is worse than
  linear.** `seal` derives the next state root by walking the ancestor lineage, so each seal costs
  more the deeper the history is. Three independent builds, each grown to depth 1,024 (the deepest
  measured), median of three, mean over the 16 blocks ending at each depth:

  | depth | one `seal` | cumulative time to build to it |
  |---:|---:|---:|
  | 32 | 0.09 s | 4 s |
  | 64 | 0.18 s | 11 s |
  | 128 | 0.30 s | 30 s |
  | 256 | 0.64 s | 98 s |
  | 512 | 1.4 s | 6.4 min |
  | 1,024 | 5.0 s | 34 min |

  The per-seal exponent is 1.2 over depth 32–1,024 (1.15–1.24 across the three builds) and rises
  with depth: it is about 1 up to 256, and 1.6–1.8 between 512 and 1,024. The cumulative exponent is
  2.0 over depth 128–1,024 (1.9 over 64–1,024). **Quadratic is the release build's shape too, but
  only from about depth 128 on**: over depth 32–128, the only range a debug build measured, the
  cumulative exponent is 1.4 in release against 1.96 in debug (the cause was not investigated). A release seal is 15–33 times cheaper than a debug one at
  the same depth (0.30 s against 9.9 s at depth 128). **Every 64th seal also writes a checkpoint and
  costs more than an ordinary one**: about twice as much near depth 128 (the seal of block 129 took 0.61 s against
  0.31–0.33 s for blocks 128 and 130), less at depth, because the lineage walk dominates both (1.6× at block 513, 1.4× at
  961). The per-seal figures above are means over
  windows that end at every 64th block and so never include one of these; the cumulative column does include
  them. **This is the figure that decides
  how long it takes to build, or import, a deep history**, and it has been measured only to depth
  1,024; the build to 2,048 was not attempted (the 2-hour rule stopped it, its build having been
  projected at 2.3 hours). Nothing here is projected past 1,024.
  **0.48.0 changes this.** `seal`, `merge` and
  `sync seal` start from the nearest checkpoint that an adopted maintainer signed and fold at most 63 blocks
  instead of walking the lineage. Release build, alternating with 0.47.0's, three samples each (a shared machine,
  1-minute load 1.0–2.8 at each step's start), medians:

  | | 0.47.0 | 0.48.0 |
  |---|---:|---:|
  | one `seal` at depth 256 (an ordinary block) | 0.64 s | 0.07 s |
  | one `seal` at depth 1,024 (an ordinary block) | 4.5 s | 0.12 s |
  | the last ordinary block before a checkpoint, depth 1,024 | 4.3 s | 0.46 s |
  | the `seal` that writes a checkpoint, depth 1,024 | 6.1 s | 1.8 s |
  | `sync seal --claims` of a 64-block catch-up, depth 1,024 | 254–280 s | 17.9–18.5 s |
  | cumulative time to build to depth 1,024 (three builds) | 1,947 s | 279 s |

  **It is not flat.** A 0.48.0 build to depth 2,048 (three builds; 0.47.0 was not built that deep) averaged 1.7 s
  per ordinary seal over its last 16 blocks and 1,308 s cumulative: the fold and the checkpoint write grow with the
  size of the tree, not with depth. A checkpoint that no adopted maintainer signed, or that lies more than 63
  blocks back, is not used, and the command does the full walk, as 0.47.0 does.
- **Sealing also needs memory that grows with the square of depth.** One `seal`'s peak resident memory, measured with
  `getrusage` on the 0.47.0 release build: about 124 MiB at depth 256 and **about 1.8 GiB at depth 1,024** (1,874,000 KiB
  sealing block 1,025, three samples). `seal` keeps a copy of the tree's state for every block of the lineage it walks. At
  depths past a few thousand blocks, memory rather than time is the first limit. **In 0.48.0 it does not**: one `seal`
  at depth 1,024 peaks at about 30 MiB (29–30 MiB over ordinary blocks, 53 MiB for the block that writes a checkpoint;
  three samples each), against 1.8 GiB, and a 64-block `sync seal` catch-up at that depth peaks at 56–57 MiB against
  2.0 GiB.
- **Checkout is close to linear in depth; `merge-evidence` is a little worse, and both are cheap at
  these depths.** From depth 32 to 256: `checkout --patch-plan` 36 → 152 ms and
  `checkout --patch-materialize` 125 → 591 ms (exponents 0.70 and 0.77; the tree itself grew as
  depth<sup>0.84</sup>, 29 → 155 files), and `merge-evidence` 34 → 455 ms when the two sides only add
  files (exponent 1.28; the baseline's replay is all it does). When each side edits files that were
  edited before, `merge-evidence` costs 69 → 473 ms (0.97). Peak memory at depth 256 is about 11 MB
  for all of them (polled `VmHWM`). **Cost tracks history depth, not repository size**, but at these depths depth is
  cheap: a `commit` that has to replay the whole history (no cache) measured 69 ms at depth 33 and
  494 ms at depth 256 (exponent 0.96). *The depth<sup>1.45</sup> figures this page used to give were
  measured before 0.43.0 (checkout was not yet anchored), at checkpoints that a divergence had
  shifted, in a debug build. A debug build measured today gives nearly the same shapes as release
  over the same depths (0.68 and 1.26), and costs 8–30 times more at each depth.*
- **A `commit` that can use the cache costs about the same at any depth we measured.** Between
  seals, the second and third `commit`, and a `commit` after `worktree-status`, take 39–44 ms at
  depth 256, against about 475–507 ms before 0.47.0 (they replayed the whole history each time). A
  `commit` at a tip that edits text an earlier block edited, 487 ms → 52 ms; the same `commit`
  editing that file, 936 ms → 53 ms. The cache's independent full replay still happens every 64
  uses. Depth range 32–256; three interleaved runs each.
- **Incremental commit memory is flat up to a few thousand tracked files, then grows linearly** at
  roughly 1.7 KiB per file beyond that — measured peak around 11.6 MiB at 100 files and 113 MiB at
  64,000 (release, three samples at each of seven sizes).
- **Before 0.48.0, a command's memory also followed the size of what the repository stored.** Every object write read its whole object
  container into memory only to learn its length, and reading an object back read the whole container to decode one record. A `commit` that
  added one small file to a repository with a 256 MiB blob container peaked at 266 MiB on 0.47.0; **on 0.48.0 it peaks at 22 MiB**, the same
  as with an 8 MiB container (22 MiB; release build, three samples each, container sizes 8, 64 and 256 MiB). A first `commit` of 4,000
  files of 20 KB read 162 GB on 0.47.0 and reads 81 MB on 0.48.0 (16.2 s → 0.3 s), about the size of the files themselves: what a first
  commit reads now doubles when the file count doubles (1,000 files 20 MB, 2,000 files 40 MB, 4,000 files 81 MB). The writer used to re-read
  the whole object index after every object write, which was quadratic in the number of objects (a first commit of 8,000 files of 40 bytes
  read 9.4 GB; it now reads 1.6 MB).

A few things worth knowing about these numbers before relying on them:

- They describe *shape and order of magnitude*, not a guarantee for any particular repository.
  They were measured against a small corpus of profiles — two profiles, chosen deliberately at
  opposite ends of one axis but also differing 46x in breadth from each other, so results are
  bracketed by two shapes rather than interpolated across every shape a real repository might
  have. The timing figures above use one of them, `prikk-self`.
- The evidence is not equally deep. The sealing figures are three builds to depth 1,024; the
  checkout and `merge-evidence` figures are five depths (32 to 256), three samples each, so their
  exponents are direction, not a fitted curve; the cache figures are five cells, depth 32–256; the
  commit-memory figures reach 64,000 files.
- The machine was shared with other work (load average 1.7–13 during the sealing builds), which is
  why each sealing figure is the median of three and why absolute times can move by up to about
  two times between runs. Ratios within a table are the sturdier reading.
- All of the above was measured on Linux.
