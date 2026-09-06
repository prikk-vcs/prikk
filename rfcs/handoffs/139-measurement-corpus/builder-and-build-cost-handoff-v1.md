# RFC 139 increment 2 — the builder, determinism, and the build-cost curve

**RFC:** `rfcs/accepted/139-measurement-corpus.md` — accepted in full. **§5a's determinism ruling is
settled input and is the hinge of this whole increment.**
**Base:** `main` at `70dd827`.
**Follows:** increment 1 (`735eee1`), which built `tools/corpus`, the profile format, the extractor,
and `profiles/prikk-self.toml`.

**§2 is the part to read first.** It resolves two obstacles in RFC 139 §7's own wording that would
otherwise stop this increment on its first day, and the resolution shapes everything else.

---

## 1. What to build

Three things, in the existing `tools/corpus`:

1. **A planner** — pure: profile + seed → an **action manifest**, the ordered sequence of operations
   a build would perform. Executes nothing.
2. **An executor** — drives the `prikk` CLI to turn a manifest into a real repository.
3. **The build-cost curve** — wall clock and peak RSS against sealed depth, and **a depth target
   confirmed or revised against it** (RFC 139 §6).

## 2. Two obstacles in RFC 139 §7, and how they resolve

**RFC 139 §7 says the builder must drive the CLI "through the existing `tests/support` surface".
That is not literally implementable, for two reasons I verified rather than assumed:**

- **`tests/support/mod.rs` is a test-only module inside `crates/prikk-cli/tests/`.** It is not a
  library and cannot be imported from `tools/corpus`.
- **It finds the binary with `env!("CARGO_BIN_EXE_prikk")`**, which Cargo defines only for the test
  targets of the crate declaring that binary. It does not exist for a separate tool crate.

**The requirement §7 was actually making still stands: drive the real CLI, not `prikk-store`.** The
`tests/support` reference was precedent, not a dependency.

### 2.1 The executor takes the binary path explicitly, and records it

**Do not search `PATH`, and do not guess `target/<profile>/prikk`.** Take the path as an explicit
argument.

**This is a provenance requirement, not plumbing.** A corpus built by one binary is not comparable to
one built by another, and RFC 139 §4's whole discipline is that a measurement records how it was
produced. **The manifest must record the binary's identity** — at minimum its path and reported
`--version`, and prefer its SHA-256, which costs one read.

### 2.2 Determinism is tested on the planner, with no binary and no repository

**This is what §5a's ruling buys, and it is the reason the ruling was made that way.** §5a: what must
be identical between two builds is *the sequence of operations the builder performs*, not the
repository's bytes — because a CLI-built corpus cannot have a stable sealed head while `NodeId` is an
OS-CSPRNG draw (`profiles/prikk-self.toml`'s own `known_nondeterminism_risks` records it).

**A manifest can be computed without executing anything.** So:

- **The determinism test compares two planner outputs.** Same profile, same seed → byte-identical
  manifest. **No binary, no repository, no seals** — genuinely shallow, and it belongs in the ordinary
  suite exactly as §5 requires.
- **The executor is exercised by the `#[ignore]`d cost work**, following `dc59_commit_benchmark.rs`
  and `dc92_lineage_replay_benchmark.rs`, which are `#[ignore]`d for precisely this reason.

**Separating planning from execution is therefore not a style preference — it is what makes §5's
"determinism test in the ordinary suite" possible at all.** Build it that way.

**Increment 1's own lesson applies to the determinism test:** Rust's default `HashMap` hasher is
randomized **per process, not per call**, so two in-process planner calls agree even under a genuinely
nondeterministic implementation. Increment 1 caught this and added a **subprocess** test. Do the same
here, or state why the in-process form suffices.

## 3. The three decisions increment 1 handed you

RFC 139 §9 names these as yours. **Make them, and report the reasoning — they are not implementation
detail.**

1. **The `--name-status` letter → `OperationKind` mapping.** The profile records git's vocabulary
   (`added`/`modified`/`deleted`/`renamed`/`copied`/`type_changed`) because `M` cannot be resolved to
   `EditText` / `ReplaceBinary` / `ChangePerm` without text-vs-binary and content-vs-mode information
   `--name-status` does not carry. **Increment 1 was right not to invent it. You have to decide it** —
   and a decision that says "treat all `M` as `EditText`" is acceptable **if you say so and say why**,
   because `EditText`'s cost is not `ChangePerm`'s and a reader must know what the corpus models.
2. **Application order.** The profile carries distributions, not a sequence. Choosing how to sample and
   order changes is design work — RFC 139 §3's "a profile plus a builder, not a recording".
3. **Whether the profile is missing anything.** Increment 1 named three candidates (§"What the format
   cannot express"): no per-path correlation between touch count and change kind, no sequencing beyond
   `commit_count`. **If you need one, that is a `schema_version` bump, which the format was built to
   take** — not a reason to work around the format.

## 4. The build-cost curve, and the number that may be unwelcome

RFC 139 §6 fixes a **floor** of 2,048 sealed blocks (32 `REANCHOR_BOUND` intervals) and deliberately
**no target**, because nobody has measured what building that costs.

**Measure the curve, then state a target against it.** Wall clock and peak RSS against sealed depth,
out to the depth the curve itself says is reachable.

**If 2,048 proves to cost hours, that is a finding about this project's seal cost — a result worth
having, not an obstacle to route around.** RFC 139 §6 says so explicitly, and §11 names "the build
cost may make the useful depth unreachable" as the most likely way this RFC disappoints. **Report the
number you got, not the number that would be convenient.** A shallower corpus with its limits stated
is an honest outcome; a corpus that quietly measures 200 blocks while the design says 2,048 is not.

**Peak RSS on Linux only**, following `dc59_commit_benchmark.rs`'s `/proc`-based pass and its own
"a missed sample is reported as not measured, never as zero" discipline. Do not fabricate a zero.

## 5. Out of scope

- **Any change under `crates/`.** In particular **do not** add a CLI-level deterministic-entropy
  override for `NodeId`. **RFC 139 §5a.1 refused it on security grounds** — a predictable-identity
  switch shipped to every user so an internal benchmark can diff two heads. If your work makes you
  want one, that is a finding to report, not a change to make.
- **The second profile.** Increment 4.
- **RFC 136's measurements.** Increment 3, and they consume this increment's output.
- **Committing a built corpus.** RFC 139 §3: the repository is derived, disposable, and never
  committed.

## 6. Controls

1. **Planner determinism.** Same profile + seed → byte-identical manifest, across **separate
   processes** (§2.2). Perturb toward a `HashMap` somewhere in the manifest path and confirm it fails.
2. **The seed actually drives it.** A different `generator_seed` produces a different manifest.
   Without this, control 1 passes for a planner that ignores the profile entirely.
3. **The manifest reflects the profile's shape.** A profile with a known, uneven
   `files_changed_per_commit` histogram produces a manifest whose per-commit file counts follow it.
   Hand-check against a small fixture, not a plausibility argument.
4. **The executor builds what the manifest says**, at shallow depth: the repository's file set and
   commit count match the manifest. **This is the join between the two halves** and the place a
   planner/executor split can silently drift.
5. **Binary identity is recorded** (§2.1), and a manifest built with a different binary is
   distinguishable.
6. **A shallow build is reproducible in the terms §5a rules** — two builds, identical manifests,
   and **explicitly not** identical sealed heads. **Assert the inequality too**: it documents §5a's
   ruling in the test suite rather than leaving a future reader to assume the heads should match.

**Each control seen to fail before it passes**, with the perturbation reported per control.

## 7. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

**If you add a dependency**, `tools/` declares literal versions in the tool's own manifest, not through
`[workspace.dependencies]` — the opposite of the product-crate convention. Increment 1 followed it.

**Peak-RSS work is `/proc`-based and Linux-only** — that is a `#[cfg(target_os = "linux")]` in your own
diff, so **cross-target clippy applies to this round** unlike the last two.

## 8. No `CHANGELOG.md` entry

`tools/corpus` is `publish = false` and ships to nobody. **Ruled here rather than left unsaid.**

## 9. Reporting

`.git-exclude/review-request/`. Include:

- **the three decisions from §3 and their reasoning** — this is the most important part of the report;
- **the build-cost curve and the depth target you derived from it**, stated plainly whether or not it
  is the number anyone hoped for;
- the per-control perturbations;
- **anything that made you want to change `crates/`** (§5), named rather than worked around.
