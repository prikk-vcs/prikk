# Release preparation — the dev team's part (template, copied per release)

**Owner's rule, 2026-09-12, after the 0.40.0 cut:** *the dev team helps release preparation, at least
partially.* The architect had done the whole 0.40.0 prep alone — readiness sweep, bump, lock, CHANGELOG
date, gates — and was told so. From 0.41.0 on, once the owner authorizes a cut, the architect issues this
handoff filled in, and the team delivers the prep commit for review like any other round.

## Split

| step | who |
|---|---|
| authorize the cut; set the version and the date | owner → architect |
| **readiness sweep** (below) and its report | **dev team** |
| **the release commit**, prepared locally on `main`, not pushed | **dev team** |
| review the sweep and the commit; gates on the exact commit | architect |
| push; wait for CI green; signed tag; push tag; Release workflow | architect |
| verify the shipped asset (checksum, build-info, `--version`, smoke of every shipped feature) | architect, with the team's smoke script from the sweep |
| crates.io publication — needs the owner's own word, every release | owner → architect |

## 0. Before the sweep: CI is green (dev team, first thing)

**Added 2026-09-16, after 0.43.0's prep found the Windows mutation suite had been red for twelve
consecutive runs — two days — while eleven architect pushes went by unnoticed.** The local gate set runs
cross-target *clippy*, which compiles other platforms and never runs their tests; `ci.yml` is the only
place the macOS and Windows suites execute.

Before anything else, check that the latest `ci.yml` run on `main` is green — every job, not the overall
badge (`gh run list --workflow ci.yml --limit 5`, then `gh run view <id>` for the job list). **If any job
is red, stop and report it instead of sweeping**: a release does not cut while its own CI is red on a
supported platform. Name the first red run and the commit that introduced it; `gh run list` and a bisect
over the runs give both in minutes.

## 1. Readiness sweep (dev team)

1. **`--help` versus the release's surface.** Every flag added since the last tag appears in its own
   synopsis; every synopsis *description* is still true (0.40.0 shipped `setup … and print the exports`
   after RFC 148 made that false for the default path). Diff `commands.rs` against the CHANGELOG's
   `Unreleased` entries.
2. **CHANGELOG `## Unreleased` completeness**: every user-visible commit since the last tag has an entry
   under the right heading (`### Added` / `### Changed` / `### Fixed`); internal-only commits have none;
   breaking-once changes say so.
3. **Docs currency**: every message the release changes is quoted correctly in `troubleshooting.md`,
   `commands.md`, and the guide pages that show it.
4. **Root-export name diff** from the last tag (`LC_ALL=C sort` + `comm`), plus `git diff <tag>..HEAD |
   grep '^+ *pub [a-z_]*:'` for struct-shape changes, plus `#[non_exhaustive]` on any new report type.
5. **`cargo package --list -p prikk`**: file count, and that no unintended file ships.
6. **Memory ratio (RFC 133 §6a).** Run the release-gate profile, not the full sweep — the prep step
   needs exactly one number, the incremental-commit peak-RSS ratio between the two largest N, and the
   profile gives it at a fraction of the full driver's points:

   ```text
   cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture rfc133_node_count_memory_release_gate
   ```

   Put the ratio in the report; a ratio that moved since the last release stops the cut until explained.
   It writes its report to
   `.git-exclude/measurements/rfc133/node-count-memory-measurement-release-gate-<revision>-<started-at>.md`
   (the trailing Unix-epoch timestamp so a second run cannot silently overwrite the first's evidence) and
   prints the path; it writes nothing under `rfcs/`, and the previous release's figure is the latest such
   file for the release-gate report name.

   **Start it first**, before the rest of this sweep. Items 1-5, 7 and 8 above are reading and grepping,
   not building — do them while the profile runs. **Do not start a build or another gate run alongside
   it**: the peak-RSS figures come from fresh child processes and survive load, but `/tmp` is a shared,
   size-capped tmpfs that a concurrent build or gate run can fill out from under it. **The measurement
   owns the machine while it runs — including gates, from the architect's own side, not only builds.**

   **Wall-clock cost: being measured, not yet recorded.** Two prior clean runs reported ~88-94 minutes
   for this profile, longer than the full sweep's own historical ~75 minutes — arithmetically impossible
   for a strict subset of that sweep's work, so that figure is **withdrawn**, along with the earlier
   "~10 minutes" estimate it replaced. Both entry points now print and report per-point elapsed time
   (an "elapsed (s)" column in the incremental table of both reports), so the next clean run — on a
   committed revision, on an idle machine, with nothing else running — records a real number here, and a
   run interrupted mid-sweep still leaves per-point timings for whatever it completed.

   **This profile is not a substitute for the full sweep everywhere.** A round that adds or changes a
   replay-shaped reader (a `tree` row, a `diff` row, anything the incremental or genesis series exercises
   structurally) runs the **full driver** instead —
   `cargo test -p prikk --release --locked --test rfc133_node_count_memory -- --ignored --nocapture
   rfc133_node_count_memory` — because that round measures the **shape** across all seven points and both
   series, and shape needs every point. Use the release-gate profile only for this step's ordinary ratio
   check. Nothing about the release commit's own gate set (item 2, below) is conditional on which memory
   path ran: it always gets the full 12 gates, plus cross-target clippy when its diff touches
   `cfg(target_os)`-gated code, whatever the sweep found.
7. **A smoke script** exercising every shipped feature on a fresh fixture with a clean environment,
   runnable against any `prikk` binary path — the architect runs it against the published asset.
8. **Absence claims** (owner's rule 2026-09-13, after `git-mapping.md` called four shipped features
   missing): for every `### Added` entry since the last tag, grep `docs/src`, `README.md` and
   `SECURITY.md` for a sentence that says that feature is missing, not yet available or not implemented,
   and fix it. Minutes, not hours; the pages' mechanical checks cover the rest.

Report: `.git-exclude/review-request/release-<version>-prep-report-v1.md`, with anything found and fixed
(as its own commit, reviewed normally) separated from what remains.

## 2. The release commit (dev team)

Exactly three files: `Cargo.toml` (workspace `version` and the seven internal pins), `Cargo.lock`
(`cargo update --workspace --offline`; member versions only — paste the diff stat), `CHANGELOG.md`
(`## Unreleased` → `## <version> — <date the architect gives>`, em-dash). One-line message
`Release <version>: <theme>`. Full gate set on that commit; state it.

## 3. Not the team's

Pushing, tagging, publishing, and `release-signers.toml` — never.
