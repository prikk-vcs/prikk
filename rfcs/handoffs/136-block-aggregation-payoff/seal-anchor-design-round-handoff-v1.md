# Sealing without re-walking the lineage — RFC 159 design round, handoff v1

**Live 2026-09-25, and it is next.** Filed under RFC 136 because it continues increment 2c's deferred option (i),
which was ruled to be *designed with `seal`'s lineage walk*. RFC 159 is still proposed, and a proposed RFC carries
no handoffs (`boundary-check`'s lifecycle rule). 0.47.0 is released. The owner scheduled this: *"Schedule it and start the design
when ready."* **This is a design round: measure, prototype, report, then stop. No product code lands.** The direction
needs the owner's acceptance before any implementation handoff (RFC 159 Status).

**Read first:**
- `rfcs/proposed/159-sealing-without-rewalking-the-lineage.md`, all of it;
- RFC 136 §10.3c (rulings 2 and 3);
- the 2c design-round review (`.git-exclude/reviewed/rfc136-2c-design-round-review-v1.md`), whose option (i) is the
  mechanism measured here.

**Next after this:** the gate plan (held). It is not live until it is next.

## 1. What to answer

**RFC 159 §5, all five questions.** Each needs an answer from source, and each answer backed by a measurement where it
can be measured:

1. **Every check the full walk makes that an anchored walk would skip.** Go through `validate_v2_lineage` and
   `verify_v2_lineage_roots` line by line. For each check, classify it:
   - **(a)** covered by the record, because the verification that wrote it ran that check;
   - **(b)** covered by the id-only walk;
   - **(c)** covered by nothing.

   **List every (c) first.** The architect rules on each before anything else proceeds.
2. **Received history.** Can a block from `bundle import` or `sync` be an anchor today? Answer from the record's
   writers (`seal_block`, `verify`). Should it be? Lay out the options; the architect rules.
3. **The id-only walk at depth 1,024**, in release. If it is significant against a 5 s seal, lay out options that
   need no format change.
4. **Peak RSS** by `getrusage`, before and with, at depths 256 and 1,024. It shows whether the per-block clone
   memo still matters once only ≤ 63 blocks fold.
5. **One mechanism, two users.** Which call sites one anchored derivation covers:
   - `seal_block` and `seal_block_classified`;
   - the cold baseline (rung 3 for **no cache only**; **never** the reanchor or `verify_divergence`);
   - `merge-evidence` (`merge/evidence.rs:56`, `:172`).

## 2. The prototype

**Build it in a separate worktree, with its own `CARGO_TARGET_DIR`, and never commit it to `main`.** Put its whole
diff in `.git-exclude/review-request/rfc159-prototype-v1.patch`. It anchors `resolved_parent_state` as RFC 159 §4
describes:
- the nearest block that has a snapshot, is in the record, and passes `validate_snapshot_manifest`;
- the id-only walk for the history fields;
- a fold of the ≤ 63 blocks after the anchor;
- otherwise, today's full walk.

**Identity is the first result, before any timing.** Block ids, state roots and signatures must be byte-identical
with and without the prototype:
- for every block of three independent builds to depth 1,024;
- for `seal`;
- for `merge` on a divergent pair;
- for seal-from-accepted, through `sync build` and `sync accept`.

A single mismatch stops the round and is reported as its first line.

## 3. Measurements — release, `build_cost_curve.rs`'s method

- **Before and with**, on the same plan: per-seal time at 32–1,024, and cumulative build time to 1,024.
- **Stop rule:** the prototype may reach 2,048. Continue past 1,024 while the build **to** the next checkpoint is
  projected under 2 hours, from times already measured.
- **Three samples** at every depth reached.
- **Report checkpoint blocks separately** from ordinary ones. A checkpoint seal costs about twice an ordinary one
  today; say whether the prototype changes that.
- **A seal-from-accepted cost:** time one `sync accept` then seal of a 64-block catch-up at depth 1,024, before and
  with.
- **The same rules as the re-measurement round:**
  - ratios within one session only;
  - every binary named by commit and sha256, and proven release (`opt_level`);
  - load recorded at each step;
  - **the architect runs nothing while you measure.**

## 4. What may land

Instrument changes, and new `#[ignore]`d measurement tests, both test-only. Feature-gated `test-support` probes, the
established pattern. **Nothing that runs in a shipped build.**

## 5. The report — `.git-exclude/review-request/rfc159-design-round-report-v1.md`

1. **§1.1's (c) list, first.** If it is empty, say so, and show how you checked.
2. The identity result.
3. The before-and-with tables, and the checkpoint-block split.
4. The answers to §1.2–§1.5.
5. The prototype's diff summary, and the functions it changes.
6. How each control in RFC 159 §6 would be built and shown red.
7. Load, and anything you could not stop.
8. **Anything in RFC 159 or this handoff that is not true at source.** The table of callers and the reading of the
   record are the architect's, from 2026-09-25. If either is wrong, say it first.
