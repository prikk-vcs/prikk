# RFC 139 increment 4 — the second profile

**Owner-approved 2026-09-09.** Reference: `rfcs/accepted/139-measurement-corpus.md` §4 (realism and the
provenance requirements), §9 increment 4, and RFC 136 §9.1 limit 2 — the limit this increment exists to
remove.

**Increments 1–3 are delivered.** The format, the extractor, the builder, the determinism property and
three measured results all exist. **This increment adds no mechanism.** It adds the one thing that
converts "measured against prikk" into "measured against more than prikk", and it is the last increment
RFC 139 defines.

## 1. What this is for, stated so it cannot be satisfied trivially

RFC 136 §9.1's limit 2, verbatim:

> It is one project with a disciplined one-theme-per-commit rhythm. A project that accumulates many
> small fixups against the same file would show a higher ratio. **This bounds *prikk's* case, not every
> case.**

**A corpus built only from prikk's own rhythm lets this project tune itself to itself.** The second
profile must come from a project with the **opposite** property: many small changes concentrated on few
files, rather than prikk's one-theme-per-commit spread across many.

**So a second profile that happens to resemble the first has not done this increment's job**, even
though it would be a valid profile by the format's rules. That is the failure mode to design against.

## 2. REQUIRED — prove the contrast with a number, before building the profile

**Do not select a project by reputation and assume its shape.** "Project X is known for small frequent
fixups" is source reading, which RFC 139 §2 ranks as worth nothing.

Define a **concentration metric** — a single number computed from the same `--name-status` extraction
both profiles already use, expressing how much change concentrates on few paths. Something in the spirit
of *changes per distinct path* or *the share of all changes landing in the top decile of paths*; the
exact choice is yours, but it must be computable from data the profile already carries, and it must be
stated in one sentence.

**Then compute it for `prikk-self.toml` first**, so there is a baseline to be different *from*. Report
both numbers. **If the candidate's number is not clearly on the opposite side of prikk's, it is the
wrong candidate — say so and pick another** rather than shipping it and noting the resemblance.

Report the metric, prikk's value, the candidate's value, and at least one rejected candidate with its
number if you tried more than one. A metric that no candidate fails is not discriminating.

## 3. Candidate selection — the criterion is mine, the choice is yours

**Criterion:** a real, public, git-hosted project, active enough to yield 600 non-merge commits, whose
change pattern concentrates on few files. Prefer something structurally unlike a Rust CLI/library
workspace — different language, different domain, different team size — since the point is a different
*rhythm*, and identical tooling tends to produce similar rhythms.

**Constraints that are not negotiable:**

- **`n = 600` non-merge commits**, matching `prikk-self.toml` exactly. Comparability is the entire
  purpose; two profiles at different `n` cannot be placed side by side, which is the incomparability
  defect RFC 139 §2 exists to retire.
- **The extraction commands go in the file verbatim**, as `prikk-self.toml` does, with the revision
  pinned and the extraction date recorded. A reader must be able to re-derive the numbers rather than
  trust them. Use the existing extractor; do not write a second one.
- **Aggregate distributions only.** RFC 139 §4 rules this admissible precisely because the profile stores
  no file contents and no paths from the source project. **Storing a path list would change that ruling's
  basis** — if you find the format tempts you toward one, stop and report rather than deciding it.
- Record the project's license and that only public history metadata was read. One line each.

If no candidate satisfies the criterion within reasonable effort, **report that instead of relaxing the
criterion.** A second profile that does not contrast is worse than none, because it would retire limit 2
on paper without retiring it in fact.

## 4. What this increment does NOT do

- **No builder change.** The builder already reads any profile of this format.
- **No new measurement, and no re-running of increments 2–3.** Whether the second profile changes those
  conclusions is a real question and it is **not** this increment's — it is the next thing to schedule
  once the profile exists, and it needs its own handoff.
- **No gate.** RFC 139 §7's two prohibitions still stand: the corpus is not a CI job and not a
  correctness fixture.
- **No `MILESTONES.md` edit.**

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, against your final commit.

The determinism test must still pass unchanged — it needs no binary and no repository, and this
increment must not disturb that.

Report to `.git-exclude/review-request/`. Include §2's metric and both values as the report's headline,
since that is the finding; the profile file itself is the artifact but the contrast is the result. State
plainly whether limit 2 is now retired, or only narrowed, and on what basis.
