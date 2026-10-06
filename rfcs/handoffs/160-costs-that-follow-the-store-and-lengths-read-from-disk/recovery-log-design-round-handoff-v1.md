# A way back from every repair — RFC 168 design round, handoff v1

**Closed 2026-10-06** (see the closure line at the end). It was live from 2026-10-06, after 0.49.0 step 5 closed (review `step5-round-3-review-v3`).

## Task title and purpose

**RFC 168's design round:** answer its §4 questions with prototypes, so that the architect can rule and rewrite RFC 168
as a design for the owner. **Measure, prototype, report, stop. No product code lands.**

## Background and governing RFC

- `rfcs/accepted/168-a-way-back-from-every-repair.md`: read it all. It covers D6 (no reader of recovery files) and
  D5 (their save is not durable on Windows), from external review 014.
- It is filed here because the recovery file is RFC 162 rule 3's, under RFC 160's guards. A proposed RFC carries no
  handoffs.
- **Not in scope:** RFC 169 (the release key) has no handoff. It waits for the owner's decisions.

## Change scope (prototype only)

1. **Q1:** a framed `recovery/log` prototype (entry: source file, offset, length, repair, version, bytes), with
   `verify`'s view of it under RFC 164's tail rules and RFC 167's budget.
2. **Q2:** a prototype `--recovery-restore <entry> [--plan-only]` under RFC 168 C3, and the exact "nothing written
   since" condition. **Per writer:** the WAL, the pointer index, each of the ten `repair_tails` files, and the object
   index's lost ids.
3. **Q3:** from source and the existing rulings (`rfcs/handoffs/DC-87-windows-mutation/design-v1.md`,
   `narrow-round-ruling-v1.md`, `rfcs/archive/101-first-appearance-durability.md`), how `recovery/log` comes into being
   durably on Windows: at `init`, and in an existing repository.
4. **Q4:** the reader over the old `.bytes` files: what can be listed, what can be restored, and what cannot.
5. **Q5:** a table of every Windows `atomic_replace` site with non-rebuildable data:
   - `FORMAT`, the branch pointer, `ref-name`'s restore, the witness, and the recovery files;
   - **for each, run a failpoint that leaves the old value after the rename** (on Linux), and record what every reader
     then does.
6. **Q6:** a repair's cost with the log against `main` (`/home`, release build), and the log's growth.

## Explicit non-change scope

No product code, no format version change, no change to rebuildable caches, no change to `release-signers.toml`.

## Required tests (in the prototype)

- **A rehearsal for each writer:** repair, then restore, then a file byte-identical to before the repair, with a control
  (restore refused when the file was written since).
- The Q5 failpoints, one per site.

## Prohibited shortcuts

- Claiming Windows durability that only a Windows run could show. Say "from source" or "not verified".
- A restore that writes anywhere other than the exact file and offset the entry names.
- Choosing an option. Lay them out, and the architect rules.

## Compatibility and security constraints

- **Compatibility:** old `.bytes` files stay readable and are never deleted. A repository from 0.48.0 needs no
  migration.
- **Security:** a restore is a writer. It takes the same locks as the repair it undoes, and refuses over a file
  something else wrote since.

## Known risks

- A restore that silently brings back damage. C3 says `verify` must then report it; test that.
- The object index's lost ids are not bytes, so "restore" may not apply. Say so rather than force it.

## Required evidence and review request

- Answers to Q1–Q6, each option laid out, no option chosen.
- Each unit's real start and end; the prototype kept in `/home/nabbisen/Desktop/prikk/scratch-168/proto`.
- **Report:** `.git-exclude/review-request/rfc168-design-round-report-v1.md`.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | Q1, Q2: the log, the restore, the rehearsals | 120 min |
| U2 | Q3, Q4: Windows creation, the old files | 45 min |
| U3 | Q5: the replace sites and their failpoints | 60 min |
| U4 | Q6: cost | 30 min |

## Addendum 1 — 2026-10-06: round 2 (review `rfc168-design-round-review-v1`)

**Corrections Required.** Q1–Q4 are ruled (R1–R4 in the review). Build the prototype against the rulings, redo Q5 with the method below, and measure Q6. **Still a design round: no product code lands.**

1. **V1 — the prototype** (`scratch-168/proto`), under R1–R2:
   - the log, appended before the truncate, and never truncated itself;
   - `--recovery-list`;
   - `--recovery-restore <entry> [--plan-only]` under condition (ii).
   - **Rehearsals:** for the WAL, the pointer index and each of the ten `repair_tails` files: repair, restore, then a byte-identical file, then `verify`'s report.
   - **Controls:**
     - the file was written since;
     - a related file differs (R2's WAL-at-offset-0 example);
     - a damaged region in the log is followed by a sound entry that is still listed.
   - **A table, from source:** for each writer, the files that give the removed bytes meaning.
2. **V2 — Q5, redone.** For each site, run the operation **and the operations after it** to completion. Then put the old bytes back, or remove a new name, to model the lost rename. Then run `status`, `verify`, `commit`, `seal` and `switch`, and record what each does.
   - **Sites:**
     - worktree files after a switch, and after a checkout;
     - the branch pointer after the switch marker is cleared, and then after a commit;
     - `FORMAT` after a format-7 write (RFC 156);
     - the witness's first creation (the new name removed);
     - `ref-name`'s restore.
   - **The witness clear is already answered** by `row4_connectivity_finds_a_drain_not_damage`. Cite it; don't rerun it.
   - **The caches:** one row each saying why they are rebuildable.
   - **For any unsafe site,** lay out a fix and a disclosure, and choose neither.
3. **V3 — Q6, measured:** a release build on `/home`, inside the R1 scope, the repair's cost with the log against `main` at two file sizes, and the log's growth per repair.

**Prohibited:**
- claiming Windows durability from a Linux run;
- modelling a lost rename as a crash before it;
- setting aside the clock (it is correct; report the times it shows).

| unit | what | budget (stop at ×2) |
|---|---|---:|
| V1 | the prototype, the rehearsals, the controls | 120 min |
| V2 | Q5, redone | 90 min |
| V3 | Q6, measured | 30 min |

**Report:** `.git-exclude/review-request/rfc168-design-round-report-v2.md`, with each unit's start and end as the clock shows them.

**Design round CLOSED 2026-10-06** (reviews `rfc168-design-round-review-v1`, `-v2`: Approved). RFC 168 is rewritten as a design for the owner's reading. The evidence not delivered is required evidence in the implementation round (RFC 168 §7). **No implementation handoff until the owner accepts RFC 168.**
