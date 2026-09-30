# 0.48.0 candidate 3 — the prep sweep since `0f0ea817`, and the candidate

**Live 2026-09-30, and it is next.** The generation-log round is closed at `eeeb5b62` (review
`rfc163-generation-log-review-v2`).

The external review of candidate `41124dd2` (letter 015) blocked the tag. Everything it graded has since landed and been
accepted:
- candidate-2 part 1 (`903c8f6f`: the release gate, the notes text);
- RFC 163 at scope B (`ddf1e82a` + `acef8f3a`);
- the generation log (`7354feb3` + `eeeb5b62`).

**This round sweeps what changed since the last sweep (`0f0ea817`) and produces the candidate** that goes to the external
architect as letter 016. The reading from `release-0.48.0-prep-handoff-v1.md` still holds: a candidate and not a cut; not
called "stable"; no tag until the external answer; crates.io needs the owner's word.

| | |
|---|---|
| version | **0.48.0**, already in `Cargo.toml` since `0f0ea817` |
| CHANGELOG date | **2026-09-30**, the day this round runs. If it slips past midnight JST, stop and ask |
| last tag | `0.47.0` at `21895f46` |
| memory ratio | against 0.47.0's **1.916×** under the trimmed profile; `0f0ea817` measured **1.869×** |

## 1. The sweep, over `0f0ea817..HEAD` only

Follow `release-prep-handoff-template.md` §1. Earlier steps are not redone where nothing they check has changed; say
which steps changed nothing, and why.

1. **`--help`:** no flag was added since `0f0ea817`. Confirm it from `commands.rs`.
2. **CHANGELOG, RFC 161 shape:**
   - `### Security` is unchanged. The one import defect RFC 163 found (a refusal after the object writes) never
     shipped. Confirm that no released version has it.
   - `### Output changes`: every new refusal (the six RFC 163 files, the generation log) and the WAL repair's
     complete-record line, each with the before and the after.
   - **One attribution to correct:** the §9 bullet says the garbage-shaped generation-log tail was "measured by the
     external architect". **The project's architect measured it**, on `7354feb3` and on 0.47.0. The external architect
     did not run it. Correct it, and check every other "measured by" in the 0.48.0 section against who actually ran
     it.
3. **Docs currency:** every refusal message RFC 163 added is quoted in `troubleshooting.md` exactly as the binary prints
   it. Check each against the binary's output, not against the source string.
4. **Public API:** the root-export and struct-shape diff from `0.47.0`, redone on the final commit. For example,
   `WalRepair::complete_records_removed` and the RFC 163 tail types. Every addition is named in the CHANGELOG, and every
   new report type is `#[non_exhaustive]`.
5. **`cargo package --list -p prikk`:** the file count, against `0f0ea817`'s.
6. **The memory ratio (template §1.6):** the commit path changed, so the release-gate profile runs again. Compare it with
   1.916× and 1.869×. **Nothing else runs while it does, the architect's own gates included.**
7. **The smoke script gains the RFC 163 rows:** each of the six files with a torn tail, then the write that refuses, then
   the way out, then a commit. It also runs a refused `bundle import` that leaves every file identical.
8. **Absence claims (§1.8):** unchanged from `0f0ea817` unless a new `### Added` entry exists.

## 2. The candidate

- **One commit changes the heading's date** to `## 0.48.0 — 2026-09-30`. The sweep's fixes are separate commits before
  it. **The candidate is the date commit.**
- **Before it is proposed:**
  - the 14 gates on it, in R1's scope;
  - both matrices green (`rfc162_recovery_matrix`, `rfc163_*`);
  - `reproduce.sh` v2 and `matrix.py` (`receive/015-…/reproduce/`), on a release build of the candidate and on 0.47.0.
    Attach both outputs;
  - **the architect's probes, passing on the candidate's release build**, all in `/home/nabbisen/.pgtmp/arch-seal/`:
    `rfc163_import_writes_probe.sh`, `rfc163_append_only_probe.sh`, `rfc163_generation_log_probe.sh` and
    `rfc163_generation_log_tree_probe.sh`.

## 3. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| P1 | the release-gate memory profile | 20 min |
| P2 | the reviewer's scripts, candidate and 0.47.0 | 45 min |
| P3 | the architect's four probes | 10 min |

**Report:** `.git-exclude/review-request/release-0.48.0-candidate-3-report-v1.md`.

## Addendum 1 — 2026-09-30: one half-applied refusal, and what each refusal leaves (review `release-0.48.0-candidate-3-review-v1`)

**The sweep is accepted.** `14918dea` is not yet the candidate. **Fixes only here.**

1. **`trust maintainer add` refuses before its first append** (review §2.2, code).
   - Today, with a torn **trust-policy** tail, it appends the new key to `trust/keys.container` and then refuses over
     the policy.
   - Decide both tails before the first append: the trust-key file's, if the key will be appended, and the policy's, if
     a snapshot will be.
   - **Test:** a torn policy tail, then `trust maintainer add` of a new key. It exits non-zero, and every file under
     `.prikk/` is byte-identical.
   - **Control:** restore today's order. The test goes red.
2. **Say exactly what each refusal leaves** (review §2.1, text). Use the review's table, measured on the candidate's
   binary:
   - `bundle import`, `compact` and `trust maintainer add` write nothing at all;
   - `seal`, `branch create`, `tag create` and `merge`, and a new author's commit: **the guarded file is untouched**.
     Content objects and caches the command had already written are left unreferenced; `verify` still exits 0; a retry
     after the way out reuses them.
   - Correct the CHANGELOG's `### Output changes` bullet (today "refuse before writing anything"), `troubleshooting.md`
     (the pointer-index and author-key entries), and `durability-recovery.md`'s RFC 163 section wherever it says the
     same.
   - **Add a test** that holds each publication's claim as the text will state it: the pointer index byte-identical,
     `verify` 0, and a retry after the repair succeeds.
3. **The candidate** is the final commit of this addendum. The date stays **2026-09-30** if the work lands today (JST).
   If it does not, change the heading's date in the last commit, and say so.

**Before proposing:**
- the 14 gates on the final commit, in R1's scope;
- both matrices green;
- the architect's six probes pass on its release build, all in `/home/nabbisen/.pgtmp/arch-seal/`: the four named in §2,
  plus `rfc163_publication_writes_probe.sh` (whose output the new text must match) and `rfc163_other_writers_probe.sh`
  (whose trust-policy row must now show `changed: none`);
- `reproduce.sh` v2 on a release build of the final commit.

**Report:** `.git-exclude/review-request/release-0.48.0-candidate-3-report-v2.md`.

**ACCEPTED 2026-09-30: `5299fd1b` is the 0.48.0 candidate** (reviews `release-0.48.0-candidate-3-review-v1`, `-v2`).
The architect ran 14/14 gates and six probes on its release build, and the reviewer's scripts; the results are
attached to letter 016. Next: push, CI, then letter 016 for the owner to send. **No tag until the external answer.**

**Letter 016 SENT and ANSWERED 2026-09-30: do not tag `af2fc77e`.** One blocker, N9: `bundle import` appends behind a
damaged-shaped received-index tail. Reproduced by the architect, also on `ddf1e82a`. **Next, live:**
`163-a-write-never-buries-a-crash-state/received-index-damage-handoff-v1.md`; then a new candidate and letter 017.

**0.48.0 RELEASED 2026-09-30.** The external review 017 found nothing blocking. The owner authorized the cut and crates.io
(*"Both authorized."*).
- Signed tag `0.48.0` on `5e50a661`, with the Release-page link in its message.
- Release run `36681456886`, 6/6. **The gate chose `main`'s CI run `36676818049` while the tag's own run was still in
  progress**: N4, proven in production.
- The downloaded Linux asset: checksum and build-info OK, `prikk 0.48.0`, smoke 252/252.
- The Release page's notes are byte-identical to the CHANGELOG section at the tag.
- **crates.io:** 0.47.0 and 0.48.0 published for all eight crates.
