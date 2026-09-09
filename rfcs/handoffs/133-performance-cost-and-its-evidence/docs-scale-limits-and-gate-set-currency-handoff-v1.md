# Docs — the scale limits an adopter needs, and the gate set a contributor needs

**Two honest-limits corrections, one round.** Both are **transcription from settled records**, not new
analysis: every number below is already measured, independently re-derived at review, and recorded. **No
code changes. No new measurements. Do not re-derive anything.**

`docs/book/` is not tracked — CI builds it — so there is no built output to commit. `mdbook build docs`
must still succeed, since `docs-pr.yml` runs it.

## 1. `reference/current-state.md` — "Not a good fit yet" is silent on scale

The section lists ten entries and **every one is a feature gap** (no remotes, no semantic merge,
`.prikkignore` has no globbing…). Nothing about repository size or history depth. **Someone evaluating
prikk for a repo with a few thousand commits gets no signal from the page whose whole job is that
decision.**

Add scale to that section — an entry or a short subsection, your call on shape; the page's voice is
yours to match. It must convey three measured facts:

**(a) Sealing cost grows quadratically with history depth.** Per-seal cost is linear in depth, so
cumulative build cost is quadratic — measured power-law exponent **2.03**.

**(b) Checkout and merge-evidence both cost about `depth^1.45`** (measured 1.446 and 1.445), from two
separate uncached history walks. Over the same range **tree size grew only `depth^0.859`** — so **cost
tracks history depth, not repository size.** That distinction is the single most useful thing on this
list for a reader deciding whether prikk suits them: a wide shallow repository is cheap; a deep one is
not.

**(c) Incremental commit memory is flat to about 4,000 tracked files, then grows linearly** at roughly
1.7 KiB per file — measured peak around **11.6 MiB at 100 files and 113 MiB at 64,000**.

### 1a. REQUIRED — the conditions ride with the numbers

**The 23-to-101-hour figure for depth 2,048 is an extrapolation, not a measurement.** That depth has
never been built; it was projected from the measured curve and the project's own record rules it
unreachable in practice today. **If you quote it, say "projected" in the same sentence.** Preferably
give the shape (quadratic) and let the reader apply it to their own depth, rather than quoting one
extrapolated headline.

**These were measured against synthetic ladders and a corpus built from a small number of profiles.**
Two profiles exist, deliberately chosen at opposite ends of one axis — but they also differ **46x in
breadth**, so results are **bracketed by two shapes, not interpolated across all shapes.** Say the
numbers are indicative of shape and order of magnitude, not a guarantee for any particular repository.
Do not present them as universal constants.

**Measured on Linux, release builds.** Say so once.

### 1b. What must NOT go on this page

- **No internal requirement or milestone status.** Whether any of this meets a stated internal
  performance requirement is milestone language and is not a user's concern. State measured behaviour
  only.
- **No RFC section numbers.** The book is user-facing; internal numbering does not belong in it. The
  conditions in §1a are what make a claim checkable to a reader, not a cross-reference.
- **No recommendation to avoid prikk**, and no reassurance either. State the shape and let the reader
  decide — that is what the rest of the section already does well.

## 2. `contributing/development.md` — the gate list is a subset, and omits the rule that turned CI red

Verified by counting each command in the file: it covers **six** of the ten gates — `cargo fmt --all
--check`, `cargo clippy --workspace ...`, `cargo test --workspace --locked`, `cargo +1.85.0 test`,
`cargo +1.85.0 check`, and release-policy `check` (the MSRV pair lives in its own section further down,
which is fine).

**Five are missing entirely:**

- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `boundary-check`
- release-policy `reference-check`

**And the cross-target addendum is absent**, which matters most: `rfcs/EXECUTION-ORDER.md` §6 rule 9
requires, for any increment touching `#[cfg(target_os)]`-gated code, both

```sh
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-apple-darwin -- -D warnings
```

**That rule's omission left `main` red on the macOS and Windows clippy jobs for three consecutive
rounds.** A contributor following the published page today cannot catch the failure it exists to
prevent.

**Also carry the amended trigger**, in one sentence: *"touching cfg-gated code" includes adding un-gated
code to a file that already contains cfg gating* — that adds no `cfg` line to the diff for anyone to
notice, and is exactly how the CI-red incident happened.

Keep the page's existing framing that a listed gate is not passing evidence unless it was observed for
the exact commit under review — that line is good and already there.

## 3. What this round must NOT do

- **No code changes at all**, and no changes under `crates/` or `tools/`.
- **Do not touch `MILESTONES.md`.**
- **Do not re-run or re-derive any measurement.** If a number here looks wrong to you, **report it
  rather than recomputing it** — every one was independently re-derived at review, and a disagreement is
  a finding worth surfacing, not a discrepancy to quietly fix.
- Do not add a performance page, a benchmarks section, or documentation of the measurement instruments.
  Those are `#[ignore]`d internal tools with `python3`/`zsh`/Linux prerequisites and their methodology
  belongs where the reasoning already lives.
- Do not document `prikk bundle preview` here — it ships with its own release and gets its own round.

## 4. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit, plus
`mdbook build docs`. Rule 9's cross-target addendum does not apply to a docs-only diff — **state that it
does not apply and why**, as the amended rule now requires.

Report to `.git-exclude/review-request/`. Quote the final text of both changes in the report, so the
wording can be reviewed without diffing — the wording *is* the deliverable here.
