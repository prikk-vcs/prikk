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

**P1 ACCEPTED 2026-11-27** (`0587df84`, `c17c60be`, `af419c9a`, `dd2e9628`; review `release-0.59.0-prep-P1-review-v1`). **The theme is
P1's: "crash points swept, a batch writer that follows the store, and a writer that waits its turn".**

## P2 item 0, FIRST: three small changes, one commit each, then the gates (60 min, stop at 90)

1. **A mis-set `PRIKK_LOCK_WAIT_SECONDS` never makes prikk wait** (the review's defect section).
   - A value that is not a whole number from 0 to 300 refuses at once, when a wait would begin. It keeps the lock-conflict sentence,
     with the suffix `(not waited: PRIKK_LOCK_WAIT_SECONDS must be a whole number of seconds from 0 to 300, got "<value>")`.
   - **The stderr line after 1 s names the variable:** `(PRIKK_LOCK_WAIT_SECONDS sets this)`.
   - **Tests:** `off`, `0.5`, `-1`, empty and `301` refused at once with the suffix; `0` with no suffix; `7` waits; a command meeting no
     holder is unaffected.
   - **Red control:** the old fallback.
   - **CHANGELOG:** fold into the lock-wait entries (the feature is unreleased, so not `### Fixed`); correct the Upgrading sentence.
   - **Docs:** `concurrency-locking.md`, `commands.md`.
2. **`RefPublicationIssue` becomes `#[non_exhaustive]`.** The Upgrading sentence says both the literal break and the attribute.
3. **A short `### Added` entry for `doctor --remove-ref-candidates`,** pointing to its Fixed entry.

**Then P2's items 1–3, on the commit that holds item 0.** Report both in `release-0.59.0-prep-P2-report.md`. **The binary for every P2
measurement is built from that commit.**

**P2 ACCEPTED 2026-11-27** (`93ec4624`, `f80c31a9`, `a4b34104`; review `release-0.59.0-prep-P2-review-v1`). **P3 as written, with these
CHANGELOG edits first (the review's rulings 1, 3, 4, 7):**
1. **`### Security` states the release:**
   - growing 3/9/15/25/31 MiB; fixed 10k 23 MiB; fixed 10k CPU 60 → 6 s;
   - one clause for what the memory fix alone did (181 → 41 MiB growing);
   - one sentence at the head of the Changed entries: their figures are steps, each against the commit before it.
2. **RH1: drop "checkout path"** from the CHANGELOG sentence and the comment at `release.yml:65`. The `rust-src` clause stays only as RH1's
   measurement, said so.
3. **`sync accept`: 4.04 → 3.06** (the published 0.58.0 binary).
4. **The unreached bullet made exact:** the warning is replaced by the in-flight finding, and the count line can still show objects a
   running writer had not reached.

**The memory ratio is accepted as unmoved** (1.944 against the same-day 1.927). **The CHANGELOG date is given by the architect at the
cut:** put `2026-11-27` if P3 lands today, and the architect re-dates in the last commit if the tag falls later.


**P3 ACCEPTED 2026-11-27** (`6c3fafc4`, `8af5c1db`; review `release-0.59.0-prep-P3-review-v1`). CI on `8af5c1db`: run `45976657798`, 17/17.
**External review 058 answered 2026-11-27: nothing blocks the tag**
(`.git-exclude/upstream/external-architect/receive/058-reply-the-0-59-0-candidate-8af5c1db/`; reading
`.git-exclude/reviewed/external-058-reading-v1.md`). One item goes to 0.60.0, on the ROADMAP. **One note becomes P4.**

## Part P4: "up to 5 seconds" is a bound per stop, not per command (20 min, stop at 40; report `release-0.59.0-prep-P4-report.md`)

**The reviewer measured one `bundle import` at 20.7 s:** it met a held lock at four stops in turn, each with its own 5 s (058 §2.3). The
docs already say "5 seconds a stop". **The CHANGELOG's Upgrading line ("waits up to 5 seconds …") and its Output changes line read as a
bound on the command.**

1. **Add one clause to both lines,** for example "**for each lock or publication it meets**", so the sentence says what the mechanism
   does. The Changed heading ("a writer waits a few seconds …") is true as it stands.
2. **Measure it before you write it:** on the release build, make a command meet a live holder at two stops, and quote both waits in the
   report. Or cite the existing test that holds it.
3. **The date: re-date the heading to `## 0.59.0 — 2026-11-28` in this same commit.** The owner, 2026-11-28 (JST): *"Today, we will
   release it. The date will be 2026-11-28."* The tag goes on this commit today.
4. **No product file changes.** Rebuild the release binary from this commit (`cargo build --release --locked -p prikk`) and say its sha256
   is still `d299d133…`.
5. **The 15 gates on the final commit,** tree clean. **One commit.** The architect pushes and reads CI by name; **the tag goes on this
   commit.**
