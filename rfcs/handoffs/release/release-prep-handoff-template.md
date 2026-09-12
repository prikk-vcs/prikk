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
6. **A smoke script** exercising every shipped feature on a fresh fixture with a clean environment,
   runnable against any `prikk` binary path — the architect runs it against the published asset.

Report: `.git-exclude/review-request/release-<version>-prep-report-v1.md`, with anything found and fixed
(as its own commit, reviewed normally) separated from what remains.

## 2. The release commit (dev team)

Exactly three files: `Cargo.toml` (workspace `version` and the seven internal pins), `Cargo.lock`
(`cargo update --workspace --offline`; member versions only — paste the diff stat), `CHANGELOG.md`
(`## Unreleased` → `## <version> — <date the architect gives>`, em-dash). One-line message
`Release <version>: <theme>`. Full gate set on that commit; state it.

## 3. Not the team's

Pushing, tagging, publishing, and `release-signers.toml` — never.
