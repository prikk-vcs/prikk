# 0.49.0 step 5, round 3 — Linux-only tests that can run on Windows and macOS (D4)

**Live 2026-10-06, and it is next.** Round 2 is closed: reviews `.git-exclude/reviewed/step5-round-2-review-v1.md` and
`-v2.md`.

## Purpose and background

- **External review 014's D4:** 42 `cfg(all(test, target_os = "linux"))` gates leave hundreds of tests uncompiled on
  Windows and macOS. Among them are the WAL, refs recovery, locks, bundle, trust and `format_upgrade` tests.
- The documents were corrected in 0.48.0. The tests themselves are 0.49.0 (plan, step 5).
- **Your inventory so far:** `.git-exclude/review-request/step5-round-2-u4-inventory-v1.md`. It counts 28 test hooks
  and 14 `mod tests;` modules.

## Change scope

1. **Classify every gate,** from source, one row each:
   - **Linux by necessity:** names the mechanism (a Linux syscall in a failpoint, signals, `/proc`, `RLIMIT`, cgroups,
     `O_TMPFILE`, and so on);
   - **portable:** says why.
2. **Port the portable ones,** in this order: the WAL, refs recovery, locks, bundle, trust, `format_upgrade`, then the
   rest.
   - **Narrow** a gate to the part that needs Linux, splitting a module where needed, rather than dropping a test.
3. **`platform-support.md`:** what is still Linux-only, by name, with the reason.

## Explicit non-change scope

- No product code changes, except where a test cannot run without a test-support hook. Then say so before landing it.
- No test is weakened to pass on a platform.

## Prohibited shortcuts

- `#[cfg]`-ing a test away from a platform without naming the mechanism it needs.
- Text-matching printed paths, modes or line endings (`support::assert_same_path`; derive expectations as the fixture
  writes them).

## CI time

- **The "before" figure** is the Windows mutation job's duration on the architect's push of round 2. The architect
  records it in review `step5-round-2-review-v2`'s commit record.
- **The job's timeout is 65 minutes.** If the ports would take it past 50, stop and ask before landing them.
  - Estimate from your local test times per ported module, then the architect pushes, and the "after" figure is read
    from that run.

## Known risks

- Tests that pass on Linux and fail on Windows or macOS for platform-shaped reasons: paths, modes, line endings,
  sharing violations on open files. **Each ported test is a new chance of a red `main`.** Port in the order above,
  in small commits, so that a failure points at one module.

## Required evidence

- The classification table.
- The list of ported modules, with test counts.
- `scripts/gates.py`'s summary on the final commit.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | the classification, all 42 | 90 min |
| U2 | the ports, in order, with the CI-time estimate | 120 min |
| U3 | `platform-support.md` | 20 min |

**Report:** `.git-exclude/review-request/step5-round-3-report-v1.md`, with each unit's real start and end.

## Addendum 1 — 2026-10-06: Windows red on the push (review `step5-round-3-review-v2`)

**Corrections Required.** macOS passed. Windows failed one test on `aa7851a5`: the witness source scan skips test files
with a `"/tests/"` text match, which never matches a Windows path. **This is a fix round:** fixes only.

1. **One shared `is_test_source`** (by `Path::components()`, from `block_state/anchored_parent/tests.rs`) in a shared
   test-support module, used by every source scan in `prikk-store`. List each scan from source.
2. **Keep its control** with the shared function, adding rows for `caller_tests` and `test_support`.
3. **Sweep** `crates/` and `tools/` tests for text matches against `/` in paths, and fix each.
4. **Commit alone, first.** The architect pushes it through the service and reads the Windows job.

**Prohibited:** `cfg(not(windows))` on any scan.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| A1 | items 1–4 | 45 min |

**Report:** `.git-exclude/review-request/step5-round-3-report-v2.md`, with `scripts/gates.py`'s summary.
