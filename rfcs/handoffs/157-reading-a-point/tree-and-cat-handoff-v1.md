# `prikk tree` and `prikk cat` — handoff v1

**Order of work for 0.46.0 (the three handoffs of 2026-09-17):**
1. `153-content-diff/point-resolver-handoff-v1.md`;
2. `157-reading-a-point/tree-and-cat-handoff-v1.md` (this one);
3. `153-content-diff/diff-handoff-v1.md`.

Take them one at a time, each reported and reviewed before the next. **This round starts after the resolver round is
reviewed**, and uses that resolver: no second one.

**Live 2026-09-17.** RFC 157 (`rfcs/accepted/157-reading-a-point.md`) is the specification: §3 `tree`, §4 `cat`, §5
failures, §6 security, §7 controls. Read it first. This handoff orders the work and names what the RFC leaves to
implementation.

## Stage 1 — `prikk tree`, then stop only if the cost stop fires

1. **Build `tree`** per RFC 157 §3:
   - `tree-listing-v1`: leaf paths in canonical order;
   - `kind` `file` (and `symlink` only when replay supports symlinks; until then a symlink operation fails the call);
   - `encoding` `text`/`binary` by checkout's own classification, **reused, not re-derived**;
   - full `mode`, exact `size`, and `content_id` for binary only;
   - `--prefix` by whole path components;
   - the implicit current branch, following 0.45.0's rules.
2. **Exact text sizes.** Measure `tree` against `checkout --patch-plan` on RFC 139's corpus at depths 64 and 256, both
   binaries built the same way. **If `tree` costs materially more than the replay** (above 1.25× the `--patch-plan` wall
   time at either depth), **stop and report** with the figures before continuing. Do not approximate sizes to make it
   fit.
3. **Add a `tree` row** to the RFC 133 memory instrument.

## Stage 2 — `prikk cat`

Per RFC 157 §4.
- **Bytes.** Fully resolved before anything is written.
  - `--max-bytes` refuses with nothing written: a binary blob is checked against its stored size before its bytes are
    read, text after reconstruction.
  - `--output` goes to a temporary sibling and is renamed into place. It refuses an existing file without `--force`,
    and a path inside `.prikk/`.
- **A terminal.** Binary content refuses a stdout that is a terminal (`std::io::IsTerminal`, stable within the 1.85
  MSRV), naming `--output`, with nothing written. Text is written.
- **`--format json`** writes `path-content-v1` and **no bytes**. Build it from the same entry code as `tree`, so the
  two cannot disagree.
- **Absence.** An absent path, and a directory prefix, refuse with "path <p> does not exist at <point>".
- **`crates` forbid `unsafe_code`.** Do not open a pseudo-terminal through FFI:
  - hold the terminal decision behind one function with a test-support seam, and control the decision there;
  - on Linux, add one CLI control through util-linux `script -qec` when it is present, skipped with a printed reason
    when it is not;
  - say what macOS and Windows cover.

## Controls

RFC 157 §7, all of them, each shown failing under a perturbation of the subject:
- the bound checked after writing;
- the terminal check skipped;
- a non-atomic `--output`;
- `content_id` emitted for text;
- `--prefix` matched by string prefix;
- `tree` and `cat --format json` computed separately (perturb the sharing).

**For the killed-process `--output` control,** use the existing failpoint mechanism, and count its ordinals against
unrelated writes. **Paths:** any printed absolute path is compared by identity (`support::assert_same_path`), never as
text.

## Docs and CHANGELOG

- `docs/src/guide/tree-and-cat.md`, `commands.md` and both synopses.
- `git-mapping.md`: `git ls-tree` → `prikk tree`, and `git show <rev>:<path>` → `prikk cat`.
- CHANGELOG `### Added — prikk tree` and `### Added — prikk cat`, with both schema names.

## Reports

- **Stage 1**, and the cost stop if it fires: `.git-exclude/review-request/tree-report-v1.md`.
- **Stage 2:** `.git-exclude/review-request/cat-report-v1.md`.

## Addendum 1 2026-09-21 — Stage 1 accepted; both questions ruled; Stage 2 proceeds

**Accepted** (review `tree-review-v1`): `490ab4fa`, `3f33c9f5`. The cost stop did not fire (1.00× at both depths), and
the architect's probes agree with the report on sizes, modes, order, `content_id`, the component-wise prefix and
block-id/ref identity.

**Confirmed as built, not merely allowed:** the prose shape (octal `mode`, `100755`-style, beside the decimal `mode`
in JSON, which RFC 157 §3 fixes). The guide must show both forms side by side, so a reader never takes the two
numbers for a disagreement.

**§5.1 — ruled (a), and RFC 157 §5 is amended (§5a).** A missing or non-recomputing blob **fails the whole call**.
There is no entry-level `unavailable` in `tree`, and `cat` refuses. Do **not** change replay. Your reading that
`show`'s rule does not transfer is right and is now written into the RFC.
- A replay-level per-entry "unavailable" state, shared by `checkout`, `bundle export` and `merge`, is recorded as a
  ROADMAP candidate. It is nobody's task this round.
- **One control is owed in Stage 2**, since the RFC now states this: post-seal blob damage (the report's
  `swap_two_equal_length_blob_frames` fixture) makes `tree`, `tree --format json` and `cat` each exit 1 and print
  nothing on stdout. *Perturb: let the listing skip the damaged entry.*

**§5.2 — your proposal is accepted.** Give `durable_output` a `#[cfg(test)]` seam that fails between the temporary
write and the rename, and a unit test beside it asserting **the destination is absent and no temporary sibling is
left**. Keep the existing `--force` controls. **Say in the report what a real SIGKILL would leave** (the temporary
sibling, which nothing can clean up), rather than claiming more than the seam shows. The handoff's "use the existing
failpoint mechanism" was written without checking that those failpoints are anchored-writer only: that was the
architect's error, and your measurement of it is the right answer.

**Stage 2 (`cat`) proceeds** on RFC 157 §4 as written, plus the two controls above.

## Addendum 2 2026-09-22 — URGENT: `main` is red on Windows, from Stage 1's mode expectation

**CI on `64d253ed` (run 35562668263) failed the Windows mutation test suite**, 15 of 16 jobs green. The product is
right; the control is platform-shaped.

```
rfc157_tree.rs:131 control1_every_file_once_in_canonical_order_with_exact_sizes
assertion `left == right` failed: run.sh   left: Number("33188")  right: Number("33261")
```

**Cause, read at source:** the fixture sets `0o755` **only under `cfg(unix)`** (`rfc157_tree.rs:60-63`), while the
expectation at `:130` is unconditional. On Windows there is no executable bit, prikk records `33188`, and the test
contradicts its own fixture. The same class as the 0.45.0 path failures: **an expectation shaped by the developer's
platform**.

**Fix, in this round, before anything else:**
1. **Derive the expected mode the way the fixture writes it**: executable **and** `cfg(unix)` → `33261`, otherwise
   `33188`. Do not gate the whole control off on Windows: everything else in it (order, sizes, `encoding`,
   `content_id`) must keep running there.
2. **Sweep both new test files** (`rfc157_tree.rs`, `rfc157_cat.rs`) for every other expectation that could be
   shaped by the platform — modes, line endings, path separators, and anything derived from `std::fs` metadata — and
   say in the report what you checked, not only what you changed.
3. **Say in the docs what a reader will see.** `guide/tree-and-cat.md` shows `100755`/`33261` in its example and mode
   table; add the sentence that **Windows records no executable bit, so every file reads `100644` there**. A reader on
   Windows must not think prikk lost their mode.
4. Report it with the `cat` round (one report, `cat-report-v1.md` amended or a follow-up), since nothing is pushed
   beyond `64d253ed`.

**The architect pushed Stage 1 before its CI finished** — the push is allowed, the unread result was not. That is the
architect's error, recorded in the review.

## Addendum 3 2026-09-22 — the `cat` report's two questions, ruled

Both answers are independent of Addendum 2's Windows fix; apply them in the same round.

1. **§8.1, the bound.** Your measurement is right and the RFC's sentence was not. **RFC 157 gains §4a**:
   `--max-bytes` is exact and all-or-nothing **for what is written**, and is **not** a memory bound; peak memory stays
   the replay's own O(tree) cost. **No code change** — `cat` already does what §4a states.
   - **Docs must say both**, in `guide/tree-and-cat.md` and the CHANGELOG entry: what the flag guarantees, and that it
     does not bound memory. A caller bounding hostile input has to know which one it is getting.
   - Making it a memory bound is **RFC 158 Stage B** (streaming), owner-accepted 2026-09-21 for 0.48.0. Not yours now.
2. **§8.2, the implicit-branch parenthetical.** **Keep it.** "path `<p>` does not exist at heads/main (ref heads/main
   does not exist in this repository)" names both the question asked and the reason, and a fresh repository is exactly
   where a new user meets it. Add it to the control's asserted text so the wording is pinned.

**Nothing else in the `cat` round is contested**; the architect's own verification of it runs on the commit that
carries Addendum 2's fix, so that one gate run covers the whole round.

## Addendum 4 2026-09-22 — the `tree`/`cat` round is accepted

**Accepted** (review `cat-follow-up-review-v1`): `97143a4c`, `89972ac0`, `295fc57f`, `ffc83684`, `03bb9319`, with
Stage 1 already on `main`. Gates 14/14 on `03bb9319` (2,205 / 0 / 29 per toolchain), and the architect's probes agree
on bytes, the bound writing nothing, both `--output` guards, JSON/`tree` agreement, absence and the terminal rule.

**Your correction of the architect's docs sentence is accepted and is the better one.** Addendum 2 asked for
"Windows records no executable bit, so every file reads `100644` there". `platform-support.md:269-272` says worktree
authoring on Windows never derives a mode from the filesystem, carries an existing recorded mode forward untouched,
and defaults only a brand-new file to non-executable — so Linux-authored history still shows `100755` on Windows.
The handoff's sentence would have told a Windows reader that prikk lost their mode. **Checking a docs claim against
the reference page before writing it is exactly right.**

**The `--help` line (§2.1) stays.** `--help` is where a caller first meets `--max-bytes`, and a bound that is not a
memory bound is worth six words there.

**The macOS statement (§4.3) is accepted as reasoned, not observed**, and is labelled that way in the report. The
skip prints what it found, so a wrong assumption shows up as a printed reason rather than a silent pass.

**After the push, the architect reads the Windows and macOS jobs by name.** The whole round rides on that run: the
`cat` test file, the durable-output seam tests and the fixed mode expectation all execute there for the first time.

**Next: `153-content-diff/diff-handoff-v1.md`**, the last round of 0.46.0.
