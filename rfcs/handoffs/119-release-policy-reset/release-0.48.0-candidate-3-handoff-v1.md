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
