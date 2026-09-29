# 0.48.0 candidate 2, part 1 — the release gate, and text that still describes replaced rules

**Live 2026-09-29, and it is next.** The external architect reviewed the candidate `41124dd2` (letter 015) and graded
four things as blocking the tag. The architect reproduced every finding. Assessment:
`.git-exclude/reviewed/external-review-015-assessment-v1.md`.

**This round is part 1:** N4 and N5 items 1–4. Both are fixes and do not depend on anything undecided. **Part 2**, N1
with the disclosures, waits for RFC 163 (`rfcs/proposed/163-a-write-never-buries-a-crash-state.md`), which the owner
reads first. Do not start RFC 163's work in this round.

**Read first:** `.git-exclude/upstream/external-architect/receive/015-review-of-the-0-48-0-candidate/015-review.md`, §2
N4 and N5.

## 1. N4 — the release gate refuses the release on its first run

**Confirmed by GitHub's record** (the architect's read-only `gh run list`): the commit of 0.47.0, `21895f46`, has two CI
runs, `36078094314` (`push`, branch `main`) and `36079577518` (`push`, tag `0.47.0`). The second was created in the same
second as the release run. `ci_status_check.rs`'s `latest()` takes the highest run id. On release day that is the tag's
own run, still queued, and the gate refuses.

1. **The gate judges only CI runs of the commit on `main`.**
   - Ask `gh` for `headBranch` and `event` too. Filter in Rust, not in `gh`'s arguments, so a test can see the filter.
   - Keep runs whose event is `push` and whose `headBranch` is `main`. Refuse when none exists; that commit never went
     through `main`'s CI. The most recent of them must be `completed` / `success`.
   - Update the module doc with the reason: a tag push is a push, and starts its own CI run for the same commit.
2. **Tests, from GitHub's real shape:** rows modelled on 0.47.0's record:
   - the tag's run **queued** and `main`'s run a success: passes;
   - `main`'s run failed, and the tag's run a success: refuses;
   - only a tag run: refuses;
   - two `main` runs, the newer failed: refuses.
3. **Control:** remove the filter. The first case goes red.
4. **Make the gate rehearsable without a release.** Move the gate job into its own workflow that `release.yml` calls
   (`workflow_call`). The same workflow is also `workflow_dispatch`-able with a commit input, and it publishes nothing.
   - `release.yml` keeps `build` depending on it.
   - The release-policy workflow checks must pass on the new file: timeouts, command scan and permissions
     (`actions: read`, `contents: read`).
   - **Rehearsal is the architect's, after the push:** dispatch it against `21895f46`, which has a tag run. It must pass
     by picking `36078094314`.
5. **Docs:** `release-compatibility.md`'s release steps do not mention the gate yet (checked by the architect: step 3
   says "CI green", and nothing says the workflow enforces it). Add one sentence to step 4: the Release workflow
   refuses to build unless the commit's most recent CI run on `main` completed successfully.

## 2. N5 items 1–4 — the notes and the recovery reference

1. **Affected versions of M2 and M3.** The 0.48.0 entry headed "an index repair could make `verify` blind …" says the
   three defects were "all introduced within this cycle".
   - **Only M1 was.** M2 and M3 are in 0.47.0 (the architect's run of the reviewer's script on the released 0.47.0
     binary) and in 0.46.0 (the reviewer's 014 output).
   - Say which release each first appeared in, **from history**: the commit that introduced the refusal or the shape
     rule, and the first tag containing it. If you cannot establish a first release, say "0.46.0 and 0.47.0 at least",
     and say so in the report.
   - A user of 0.47.0 must read that they are affected.
2. **One rule per section.** The entry "a repository that only crashed failed `verify` for good after its next write"
   still states F3 Addendum 1's rule ("no index entry names" → "never committed"; "a frame an index entry names is still
   a failed item"), which the RFC 162 entry above it replaced. Rewrite it to what 0.48.0 does. **Sweep the whole 0.48.0
   section for the same rule**, the `### Changed — breaking once` entry's `object_interrupted_appends` description
   included. List each place in the report.
3. **`durability-recovery.md`, "WAL Replay and Tail Handling":**
   - state RFC 162 rule 3 for the WAL and the pointer index: the tail is everything after the last sound record when no
     sound record follows, whatever its shape. A last record with a checksum mismatch is then a tail, and the repair
     truncates it, keeping its bytes (N6; its disclosure is part 2);
   - replace "applies to every framed file" with what holds per file: the WAL and the pointer index (rule 3), the object
     index (a cache, rebuilt), the object containers (connectivity), the ref log (its own positive rule), and the rest
     (the shape rule, unchanged).
4. **The entry title overstates.** "A repository that only crashed failed `verify` for good after its next write" is
   fixed for the object containers and the object index only. Scope the title to them. N1 and N2 show it is false for
   the pointer index and four more files, and their disclosure is part 2.

**Not this round:** N5 item 5 ("each is a cost or a silence" in the known limitations), and the disclosure of N2, N3 and
N6. Their text depends on RFC 163's scope.

## 3. The candidate

**This round does not produce the candidate.** Commit on `main`, locally, as usual. The next release commit comes after
part 2. The release commit `0f0ea817` stays as it is; part 2's handoff says how the version-bump commit is carried
forward.

## 4. Gates, units, report

- **The 14 gates on the exact final commit**, in R1's scope. A `docs/src` change runs the full test gate.
- **Units and budgets:**

  | unit | what | budget (stop at ×2) |
  |---|---|---:|
  | C1 | the history search for M2's and M3's first release | 30 min |

- **Report:** `.git-exclude/review-request/release-0.48.0-candidate-2-part-1-report-v1.md`, with:
  - the gate's diff, its tests and the control;
  - the new workflow file and the release-policy checks on it;
  - every CHANGELOG and docs place you changed, and the history evidence for item 1.

**ACCEPTED and CLOSED 2026-09-29** (`903c8f6f`; review `release-0.48.0-candidate-2-part-1-review-v1`). The architect ran
14/14 gates, a control on the filter (3 red), and the gate against GitHub's live record. The dispatched rehearsal run
`36563922118` picked `main`'s run for 0.47.0's commit. CI run `36563890481` went 16/16. Two follow-ups are carried into
part 2 as its item 0. **Next, live:** `rfcs/handoffs/163-a-write-never-buries-a-crash-state/implementation-handoff-v1.md`.
