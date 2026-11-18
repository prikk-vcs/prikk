# Release 0.58.0 preparation, as a candidate — the dev team's part

**LIVE: owner-authorized 2026-11-18** (*"Both accepted. Yes to the two."*, on `.git-exclude/scratch/0.58.0-cut-proposal-v1.md`).

**The architect's reading:**
- **This authorizes release prep P1–P3,** which ends in a candidate commit prepared locally. It does not authorize the tag,
  crates.io or the letters. Each still needs the owner's word.
- **The second "yes" widens the cut's external review:** the append path and S, **plus** the writers of the replay-verified
  record (S6, R2) and streaming `sync accept` (P3). The architect drafts the letter after P2, and the owner sends it.

**0.58.0's planned work is complete** as of A2 (`99c796be`). The theme is *"writes that follow size, continued"*. Every part's
history is in its handoff, under `160-costs-that-follow-the-store-and-lengths-read-from-disk/` (P1, P2, S, S2–S6, F, R, P3, P4,
B′, A) and `147-refusal-visibility/0.58.0-step-0-handoff-v1.md`.

Follow `release-prep-handoff-template.md`. This file fills in the blanks.

| | |
|---|---|
| version | **0.58.0**: a minor (v0 policy: runtime changes). **P1 says whether the public API also breaks.** Candidates: `admit_carried_signatures` and `CarriedAdmission` deleted (P3b); `ObjectReadSnapshot::check_write` added; new variants and report fields (`queue.already_sealed`, `import_refusals` ids, `import_adoption`, `PRIKK-VERIFY-REF-PUBLICATION-IN-FLIGHT`); `prikk-store`'s `prikk-ffi` and `rustix` edges target-gated |
| theme | **"writes that follow size, continued"** |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.57.0` on `149a9e3d`. The published Linux binary is at `~/.pgtmp/rel-0.57.0/prikk` (sha256 `f068706f…`) |
| memory ratio | against **0.57.0's release-gate profile, 1.865** (`release-0.57.0-prep-P2-review-v1`), measured the same day, alternated |
| smoke base | `.git-exclude/review-request/smoke-0.57.0.sh` (1,167 ok). 0.58.0 needs its own `smoke-0.58.0.sh` (P2) |
| gates | **15** (`reference-check` now holds the assurance-table rule, A2) |

## The working rule

As in 0.57.0's prep:
- one part per sitting, one report each, and no file in `review-request/` until a part is complete;
- `date` at the start and the end;
- every run in an R1 scope;
- keep every output the report cites;
- build before running anything that reports on a commit;
- alternate every timing, and state the filesystem and whether the machine was idle;
- `df` the scratch filesystem before the gates;
- no `rm -rf` on a variable;
- **kill a process only by its pid,** never by a name pattern (the owner's rule; the P3b report's `pkill -f` incident).

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job by name. **1.** `--help` against 0.57.0's binary: every changed line, named per command (`unlock`, `bundle preview`, `extract`'s refusal, …). **2.** CHANGELOG `## Unreleased` in RFC 161's shape (Security, Upgrading, Output changes, then Fixed, Changed, Added). Every user-visible commit since `149a9e3d` has an entry. **The `### Output changes` block lists every JSON field, id, exit code, refusal sentence and listing change since 0.57.0,** because the consumer letters are drafted from it. Cross-check it against `.git-exclude/scratch/0.58.0-release-letter-points.md`, and name anything in one and not the other. **3.** Docs currency for every changed message. **4.** Root-export diff from `0.57.0`, struct-shape changes, and `#[non_exhaustive]` on new report types: say whether the public API breaks, by crate. **5.** `cargo package --list -p prikk`, and `prikk-cli`'s `exclude` (the S2a fix): no test fixture ships. **6.** **`### Security`:** propose whether 0.58.0 needs one. Candidates: `sync accept`'s memory, which untrusted input drove to about 6× the bound in 0.57.0; `unlock --yes` on a live unknown holder. The architect rules. | 90 min | `release-0.58.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.865. Start it first, and run nothing else on the machine meanwhile. **2.** **Re-confirm, on the candidate, every figure the CHANGELOG states,** by the method that produced it: `sync seal` per claim (S6's table, 250 and 1,000 claims, plus 10,000 if the budget allows), `sync accept` peaks (P3b), a receiver's commits (R2), `bundle preview`'s time (B′b), and syncs per patch (P2a). **3.** **`smoke-0.58.0.sh`,** from 0.57.0's script, with a section for each feature of this release: the `unlock --yes` refusal of an unknown holder; `status --format json`'s `queue.already_sealed`; DEBRIS as info; `sync seal` of a batch and its re-run after a kill; an adopting import's record and `extract` on a receiver; the in-flight refusal; `bundle preview --adopt`; and the assurance table (`reference-check`). Run it on the release build of the candidate. Run the sections that must also pass on 0.57.0 on 0.57.0 (`passes-on-0.57.0.md`). | 120 min | `release-0.58.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's rulings on P1 and P2: **1.** The CHANGELOG in RFC 161's shape, with `### Security` only as ruled. **2.** The version bump to 0.58.0 and `Cargo.lock`. **3.** **On that commit:** `cargo test --workspace`, the 15 gates, `test-msrv` on 1.85.0, the smoke on its release build, and the release binary's sha256. **The commit is prepared locally, not pushed.** | 60 min | `release-0.58.0-prep-P3-report.md` |

## Not the team's

- pushing;
- **the external-review letter** (the architect drafts it against the candidate, and the owner sends it);
- the tag;
- verifying the shipped artifact;
- the release run's jobs;
- crates.io;
- **the consumer letters** (the owner sends them after the release). Their points are tracked in
  `.git-exclude/scratch/0.58.0-release-letter-points.md`, for planeter, inklkunn and stikk, and each claim is measured on the
  release binary first.
