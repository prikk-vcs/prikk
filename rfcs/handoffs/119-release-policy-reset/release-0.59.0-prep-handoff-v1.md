# Release 0.59.0 preparation, as a candidate — the dev team's part

**LIVE: owner-authorized 2026-11-27** (*"Yes, start prep"*).

**The architect's reading:**
- **This authorizes release prep P1–P3,** which ends in a candidate commit prepared locally.
- **It does not authorize the tag, crates.io or the letters.** Each still needs the owner's word at the cut.
- **The external review** covers RS, G, K and P5, as the scope ruled. The architect drafts its letter against the candidate, and the owner
  sends it.

**0.59.0's planned work is complete:** step 0 (RS, G, RH), K, M, P5 (built), and the H and J design rounds. **H and J built nothing
for 0.60.0,** except H2c–H2e's fixes and the guide page; their designs are proposed RFCs 172 and 174. Every part's history is in its
handoff:
- `rfcs/handoffs/160-costs-that-follow-the-store-and-lengths-read-from-disk/` (K, M, P5);
- `rfcs/handoffs/147-refusal-visibility/` (step 0, G3);
- `rfcs/handoffs/171-who-may-author-who-may-seal/` (H);
- `rfcs/handoffs/173-the-json-tier/` (J).

Follow `release-prep-handoff-template.md`. This file fills in the blanks.

| | |
|---|---|
| version | **0.59.0**: a minor (v0 policy: runtime changes). **P1 says whether the public API also breaks.** Candidates (the architect's list, to be checked against the code): `upgrade_repository_format_in`; `FormatUpgradeOutcome::MarkerRestored`; `RepositoryLayout::open_allowing_lost_marker`; `holds_nothing_yet`; `WayOut::RefLeadWithSealedQueue`; `prikk-replay`'s `PathChange`; the `current_branch` argument of `plan_seal_batch` / `SealBatchPlan::fold`; `SealBatchPlan::open` gaining the batch ids; library callers now wait on a live holder (`refuse_without_waiting`); `prikk_store::directory_accepts_new_entries` (additive); and the step 0 RS and G items |
| theme | **"crash points swept, and a batch writer that follows the store"** (P1 may propose better words) |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.58.0` on `64546e78`. The published assets are at `~/.pgtmp/rel-0.58.0/` |
| memory ratio | against **0.58.0's release-gate profile, 1.892** (`release-0.58.0-prep-P2-review-v1`; 0.57.0 1.881), measured the same day, alternated |
| smoke base | `.git-exclude/review-request/smoke-0.58.0.sh`. 0.59.0 needs its own `smoke-0.59.0.sh` (P2) |
| gates | **15** |
| letter points | `.git-exclude/scratch/0.59.0-release-letter-points.md` (the architect's tracked list) |

## The working rule

As in 0.58.0's prep:
- one part per sitting, one report each, and no file in `review-request/` until a part is complete;
- `date` at the start and the end;
- every run in an R1 scope;
- keep every output the report cites;
- build before running anything that reports on a commit;
- alternate every timing, and state the filesystem and whether the machine was idle;
- `df` the scratch filesystem before the gates;
- no `rm -rf` on a variable;
- **kill a process only by its pid.**

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job by name (the last run, `80993d74`, and the last code commit's, `0b4429bc`). **1.** `--help` against 0.58.0's binary: every changed line, named per command. **2.** CHANGELOG `## Unreleased` in RFC 161's shape (Security, Upgrading, Output changes, then Fixed, Changed, Added). Every user-visible commit since `64546e78` has an entry. **`### Output changes` lists every exit code, stderr line, refusal sentence, JSON field and listing change since 0.58.0**, and is cross-checked against `.git-exclude/scratch/0.59.0-release-letter-points.md`: name anything in one and not the other. **3.** Docs currency for every changed message (the wait's stderr line, the lost-marker sentences, the G3 sentences, the key-file sentences, the seed-directory refusals, the judge's reason). **4.** Root-export diff from `0.58.0`, struct-shape changes, and `#[non_exhaustive]` on new report types: **say whether the public API breaks, by crate,** and check the `### Upgrading` list against the version row's candidates. **5.** `cargo package --list -p prikk`: no test fixture ships. **6.** **`### Security`:** confirm the existing entry, its affected versions and its action, against the measured numbers. Propose anything else reachable by untrusted input since 0.58.0. The architect rules. **7.** Absence claims (template item 8). | 90 min | `release-0.59.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.892. Start it first, and run nothing else on the machine meanwhile. **2.** **Re-confirm, on the candidate, every figure the CHANGELOG states,** by the method that produced it: the P5 numbers (growing and fixed `sync seal` at 10k claims, the 100 MiB `sync accept`, lock syncs at 2k claims on disk), M's, and any K number. **3.** **`smoke-0.59.0.sh`,** from 0.58.0's script, with a section for each feature of this release, at least: the lock wait (5 s; the stderr line after 1 s; `PRIKK_LOCK_WAIT_SECONDS=0` refuses at once; `verify` never waits; a 0-byte lock refused after 250 ms); `init` over a populated lost-marker repository (exit 1) and `init` then `setup` (exit 0); the seed-directory refusals (missing, a file, mode 500); a torn key file's sentences; the G3 next-step lines; `trust maintainer remove`'s help line. Run it on the release build of the candidate. Run the sections that must also pass on 0.58.0 on 0.58.0 (`passes-on-0.58.0.md`). | 120 min | `release-0.59.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's rulings on P1 and P2: **1.** The CHANGELOG in RFC 161's shape. **2.** The version bump to 0.59.0 and `Cargo.lock`. **3.** **On that commit:** `cargo test --workspace`, the 15 gates, `test-msrv` on 1.85.0, and the smoke on the release binary built with **`cargo build --release --locked -p prikk`** (never a `cargo test` binary, which unifies `test-support`), with that binary's sha256. **The commit is prepared locally, not pushed.** | 60 min | `release-0.59.0-prep-P3-report.md` |

## Not the team's

- pushing;
- **the external-review letter** (the architect drafts it against the candidate, and the owner sends it);
- the tag;
- verifying the shipped artifact;
- the release run's jobs;
- crates.io;
- **the consumer release letters** (the owner sends them after the release; each claim is measured on the release binary first);
- **the five held RFC 172 and RFC 174 letters:** they are not release letters, and they go on the owner's word.
