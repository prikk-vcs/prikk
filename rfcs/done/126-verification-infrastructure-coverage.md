# RFC 126 — Four flanks the verification culture never reached

**Status.** **CLOSED 2026-09-04 and moved to `rfcs/done/`**, on the owner's authorization of the
2026-09-04 schedule (`ROADMAP.md`, "Proposed ordering", item 0). **No implementable work remains: all
four flanks are delivered.**

**Its one outstanding item was not work but a ruling, and that ruling now lives in exactly one
place.** §5a's question — does peak RSS get any standing protection — was tracked here *and* in
RFC 133 §6 simultaneously, which is how a ruling gets answered once and left open in the other. **RFC
133 §6 is now its sole home.** Nothing about the question changes; only where it is tracked.

Everything below is the record as it stood. Previous status: **Proposed; §6 ruled; §5a accepted by the
project owner 2026-09-03.** Raised by the
external architecture audit of 2026-08-31 (`audit-2026-08-31-task-2.md` §3 cross-cutting,
`task-3.md` §2c/§2d, Top-10 #10). All four independently confirmed at `3a8d730`.

**Delivered:** §2, §3, §4's CI half, §5 increment A, §6a, §6b. **Handed over:** §4's kernel doctests,
this RFC's last implementable item.

**ONE RULING OUTSTANDING, and accepting §5a did not answer it.** §5a escalated a question rather
than settling one: **does peak RSS get any standing protection, and in what shape?** Its three
options are stated there with no architect recommendation, deliberately — §6's reasoning does not
transfer, because for peak RSS there is no criterion equivalent to weigh a threshold against. **Until
that is ruled, `dc59_commit_benchmark.rs`'s `VmHWM` pass remains the project's only peak-RSS
measurement and remains `#[ignore]`d**, which is the exact condition §5 was written to complain
about.

**RULED by the project owner 2026-09-01: §6 option 4** — criterion enters the workspace in **its own
member, outside `default-members`**, the shape `tools/release-policy` already established. It appears
in no product crate's manifest and in no shipped dependency graph. The reversal of this RFC's first
recommendation, and the accessibility and drift arguments behind it, stand recorded in §6.

**Tracks.** Gates that do not exist. **One product behaviour change**, added by §6b: AUD-08's
`checked_add`, which turns a `u64` overflow from a wrap or debug panic into an `Integrity` error.
Every other item changes gates, attributes, or comments only.

---

## 1. The framing that makes this one RFC

This project verifies almost everything it does, unusually well: 76 failpoint sites each asserting
error *and* post-state *and* retry; decoder totality proptests; a frozen SHA-256 differential at
10,000 cases per run; cross-platform object-id diffing as a CI job; a policy binary that gates its own
boundaries. **The four gaps below are all the same shape — a place where that culture stopped —
and each is cheap relative to what already exists.**

## 2. The patch algebra has zero property tests

Confirmed: `grep -rl proptest crates/prikk-store/src/patch_algebra/` returns nothing.

**This is the subsystem where property testing pays most.** Commutation and confluence are
combinatorial; they are covered today by roughly 2,500 lines of hand-built fixtures which are
complete over the sealed/unsealed × malformed/missing matrix — but **nothing searches for a
non-commuting pair the fixtures did not imagine.**

And the harness is already built. `replay_oracle.rs` applies a pair in both orders over real
lifecycle state and real text bytes and compares final states. That is an executable specification,
which is exactly what a proptest needs:

> generate operation pairs and sequences; assert **classifier says `Commutes` ⇒ oracle states equal**.

**The failure this would catch is the only unsound one the design admits.** As the audit observes,
a gap in the pairwise theory can cause a spurious *refusal* safely — but a pair wrongly classified
`Commutes` is the one shape that produces an unsound merge, and it is precisely the shape a
fixture author is least likely to think of.

## 3. `cargo audit` runs in the gate set and not in CI

**The audit's framing is too broad and the gap is still real.** `cargo audit --no-fetch` is one of the
standing gate commands in `EXECUTION-ORDER.md` §6 rule 9 and runs on every gate pass. But
`grep -rn "cargo audit" .github/workflows/` returns nothing: **advisory monitoring depends on a human
running the gate set**, and it uses `--no-fetch`, so it is only as current as the local advisory
database.

A scheduled CI job that fetches is the missing half. It is also the only item in this RFC that can
find a problem *without anyone changing the code* — a new advisory against one of the 25 shipped
crates arrives on its own schedule, not on ours.

## 4. Documentation is never gated

Two distinct holes, confirmed:

- **`cargo doc` never runs in CI**, and `cargo doc --workspace --no-deps` currently emits **exactly 7
  `rustdoc::private_intra_doc_links` warnings** (`block_state.rs` ×5, `bundle.rs`, `worktree_patch.rs`).
  Each renders a public doc item's link as literal `[`Name`]` text. Gate: `-D rustdoc::private_intra_doc_links`.
- **`docs.yml` triggers only on push-to-`main`, filtered to `docs/**`.** So a code change that
  falsifies a documented claim triggers no docs build at all, and the book is never built on a PR.

**This matters more here than in most projects**, because RFC 118 ("derive, never transcribe") and the
§8 doc-coverage gate make documentation a checked artifact rather than prose — and those checks
themselves run only under `cargo test`, not against the rendered book.

**Separately: zero doctests.** `cargo test --workspace --doc` → `0 passed`. 566 public items are
documented and not one carries a compiler-verified example. Recommendation is deliberately narrow:
**the ten kernel entry points only** (`ObjectId::from_canonical_payload`, `validate_repo_path`,
`Ed25519KeyPair`, `verify_ed25519`, `CanonicalWriter`, `RefStore::publish`, and peers). A blanket
doctest campaign across 566 items would be a large ongoing cost for a small marginal gain.

**Delivered 2026-09-03 (`5a5315f`): ten doctests, `0 passed` → `10 passed`.** Two properties found
while landing them, recorded because a later reader would otherwise assume both the other way:

- **Doctest bodies are not linted.** rustdoc compiles each example as its own crate, outside cargo's
  clippy pipeline, so AUD-06's `deny`-level `unwrap_used`/`expect_used`/`indexing_slicing` **do not
  bind them** — confirmed empirically (`canonical.rs`'s example carries five `unwrap()` calls and
  `cargo clippy -p prikk-object -- -D warnings` exits `0`). This is fine and idiomatic for examples,
  but it means doctests are the one body of code in this workspace those three lints do not reach.
- **The cost is per-crate, not per-example.** Adding the first doctest to a crate costs roughly what
  several more in that same crate cost (`0.26s` → `0.86s` for the whole `--doc` run), so the ceiling
  that matters is filesystem-touching examples, not example count. Only `RefStore::publish` touches
  a repository; its own real work is ~`0.13s` of the total.

## 5. Benchmarks exist and never run

`dc59_commit_benchmark.rs` (1,064 lines) is `#[ignore]`d; `dc92_lineage_replay_benchmark.rs` runs
`SAMPLES = 2` and says of itself that it "decides nothing". There is no `benches/`, no criterion, and
no CI job. **DC-62's peak-RSS work — genuinely good measurement — can regress invisibly**, and so can
the two performance walls tracked in `ROADMAP.md`'s corrective program.

## 5a. Amended 2026-09-03 — increment B is not a migration; §5's remaining question moved to RFC 133

**SUPERSEDED in part, 2026-09-03.** On the owner's instruction the measurement concern was extracted
into **`rfcs/done/133-performance-cost-and-its-evidence.md`**, which carries the peak-RSS ruling
unchanged (RFC 133 §6) and adds what this section could not: **measured numbers**. The analysis below
stands as written and is left intact; only the open question moved. **RFC 126 no longer waits on it.**

**Increment A shipped** (`cb2e2a2`, amended `2fec604`/`fb107c7`): `tools/benchmarks`, criterion `0.7`
pinned against the `1.85.0` MSRV, one `commit` benchmark, outside `default-members`.

**The architect's own handoff described increment B as "migrating `dc59_commit_benchmark.rs`
(1,064 lines) and `dc92_lineage_replay_benchmark.rs` (718)". That framing was wrong**, and reading
what those files measure is what corrects it:

| Harness | Measures | Can criterion express it? |
|---|---|---|
| `dc59` | repository size **10 → 10,000 files** (3 samples at the top point), a change-count axis, **peak RSS by `.spawn()`ing the binary and polling `/proc/<pid>/status` for `VmHWM`**, plus memory and timing floors | **No.** Criterion samples many iterations of a small fixed operation; a 10,000-file commit cannot be iterated 100 times, and criterion has no memory axis at all |
| `dc92` | `verify`/`seal` against **sealed-history length to 160 blocks**, built to expose an O(N³)/O(N²) complexity question | **No**, for the same iteration-count reason |

**Ruling: neither harness is migrated, and neither is retired.** They are *on-demand instruments*,
a legitimate category distinct from a regression detector. They live under `tests/`, so the compiler
maintains them on every `cargo test --workspace` even while `#[ignore]`d — they cannot silently
bit-rot, which is the usual argument for retiring a dormant harness and does not apply here.

**Retiring `dc92` was considered and refused.** Its question is closed — MILESTONES criterion 3 is
**MET** (`verify` linear, 27.04 ms at N=160, ratio 1.97) — and both halves are now held by standing
gates rather than by measurement: `rfc111_index_decode_cost_gate.rs` for `verify`,
`rfc111_seal_decode_cost_gate.rs` for `seal`. **But a gate proves a property holds; it cannot
re-derive the curve if the gate itself is ever questioned.** Deleting 718 compiler-maintained lines
to buy tidiness is the wrong trade for a project that values robustness over initial effort.

**So increment B carries no dev-team work.** §5's "benchmarks exist and never run" is answered for
*regression detection* by increment A, and the two instruments are correctly on-demand.

### The one thing §5 still needs, and it is the owner's

**§5's stated motivation is that "DC-62's peak-RSS work — genuinely good measurement — can regress
invisibly." Nothing built under this RFC addresses that.** Criterion measures wall-clock time.
`dc59`'s `VmHWM` pass remains the project's only peak-RSS measurement and is `#[ignore]`d, so peak RSS
has **no standing protection of any kind**.

**This cannot be ruled by the architect, because the only available mechanism is the one §6 decided
against.** §6 chose criterion over a fixed-threshold job, and the deciding argument was that
hand-maintained thresholds drift into vacuous gates while criterion's stored baseline maintains itself
as a by-product of running. **For peak RSS there is no criterion equivalent** — the choice is a fixed
threshold or nothing, and §6's reasoning does not transfer to a question where the better option does
not exist.

**Three shapes, for the owner:**

1. **A scheduled CI job** running `dc59`'s RSS pass at one small fixed size against a recorded
   threshold. Catches order-of-magnitude regressions; carries exactly the drift risk §6 named, now
   with no better alternative to compare against.
2. **A local-only gate-set addition** — the RSS pass run on demand before a release cut rather than
   per increment. Cheaper, and it fails the "invisible regression" test between cuts.
3. **Record that peak RSS is unmeasured on a standing basis and stop implying otherwise.** §5's own
   option 3 for the timing axis, applied honestly to the axis that still lacks cover.

**No recommendation is offered here deliberately.** §6's reversal came from the owner questioning the
architect's first answer on accessibility and long-term stability grounds, and those same two
properties decide this one.

**That ruling now lives in RFC 133 §6, not here.** §4's kernel doctests landed at `5a5315f`, so
**RFC 126 has no work of its own left at all.**

## 6. The ruling this RFC needs: criterion as a dev-dependency

**The dependency gate permits it, and I verified why rather than taking the audit's word.**
`placement.rs:49-53` collects only `[dependencies]`, `[build-dependencies]`, and their `[target.*]`
forms — *"`[dev-dependencies]` is deliberately excluded everywhere… it is the sink this check
protects."* `proptest` already lives there in `prikk-object` and `prikk-store`.

**So the gate is not the question. The question is the workspace's own standard**, which is stricter
than its gate: exactly one third-party dev-dependency exists today, and criterion brings a
substantial tree into `cargo test` builds for developers and CI alike.

Four shapes:

1. **Criterion in a product crate's `[dev-dependencies]`.** Standard tooling, statistical rigor,
   baseline-relative regression detection. Puts a large tree into `prikk-store`'s own manifest.
2. **A fixed-threshold smoke job** on the existing harnesses in a scheduled workflow. Zero new
   dependencies; catches order-of-magnitude regressions only; thresholds are hand-maintained.
3. **Neither — record that performance is unmeasured** and stop implying otherwise.
4. **Criterion in a separate workspace member, outside `default-members`** — the shape
   `tools/release-policy` already established.

**The architect's recommendation is 4. An earlier draft of this RFC recommended 2, and that was
optimizing for the wrong thing.** Two properties decide it, and both were raised by the owner:

- **Accessibility.** Criterion is what a Rust contributor already knows: `cargo bench`, a standard
  report, a familiar statistical model. A bespoke threshold harness is one more thing that must be
  learned before anyone outside this project can evaluate a performance claim — and this project has
  to earn contributors against a novel model as it is. **On this axis option 2 is clearly worse, not
  marginally worse.**
- **Stability over a long-lived project.** Hand-maintained thresholds drift, and a threshold nobody
  recalibrates becomes a gate that passes vacuously — **the exact failure shape RFC 127 exists to
  correct**, reintroduced deliberately. Criterion compares against a stored baseline, so the
  maintenance is a by-product of running it rather than a separate discipline nobody owns.

**The dependency objection dissolves once placement is right, and that is why option 4 exists.**
`tools/release-policy` already carries 133 dependencies inside this workspace, excluded from
`default-members`, never shipped, with 13 duplicate-version crates confined there. **The project has
already solved "heavy infrastructure tooling without contaminating the product", and criterion is the
same problem.** A separate member keeps criterion out of every product crate's manifest and out of the
shipped dependency graph — the property `placement.rs` exists to protect. **Stated honestly: it does
not keep it out of build time**, since `cargo test --workspace` builds every member, exactly as it
already builds `tools/release-policy`.

**RULED 2026-09-01: option 4** (recorded in this RFC's Status). The paragraphs above are preserved as
the argument that was ruled on, not as an open question. The cost is one the project had already
priced once, in `tools/release-policy`, and judged worth paying.

## 6a. Recorded 2026-09-02 — workflow permissions are not declared least-privilege

Found by the architect while reviewing §3/§4's increment. **Not introduced by it, and not a defect
in it** — recorded here because this RFC owns the CI surface and the observation would otherwise
live only in a git-excluded review.

Of five workflows, **two declare `permissions:` and three do not**:

| Workflow | trigger | `permissions:` |
|---|---|---|
| `docs.yml` | push to `main` | declared (`pages: write`, `id-token: write` — it deploys) |
| `release.yml` | release | declared |
| `ci.yml` | push, pull request | **absent** |
| `docs-pr.yml` | pull request | **absent** |
| `security-audit.yml` | schedule | **absent** |

A workflow with no `permissions:` block inherits the repository or organisation default, which may
be read-write. **The three that omit it are all check-only workflows that need nothing beyond
`contents: read`** — and two of them run `cargo install` from crates.io, one of those on a schedule
with no human watching.

**`security-audit.yml` is the sharpest case**: it is the workflow whose entire purpose is telling
the project whether its dependencies are trustworthy, and it runs third-party build scripts with
whatever the default grants.

**This predates the increment** — `ci.yml` has always been in this shape — so it is one item across
three files, not a correction to §3/§4. `permissions: contents: read` on each is the whole change.
**Whoever takes it should check the repository's actual default first**, since if it is already
read-only the change is documentation of an existing property rather than a hardening.

## 6b. Adopted 2026-09-02 — four `ROADMAP.md` corrective rows with no RFC of their own

`ROADMAP.md`'s post-audit corrective program tracks **AUD-05 through AUD-08** under the heading
*"Tracked here rather than as RFCs — none needs a design decision"*, which was and remains true.
**But work reaches the dev team through `rfcs/handoffs/<dir>/`, and RFC 105's naming gate restricts
those directories to `^[0-9]{3}-[a-z0-9]+(-[a-z0-9]+)*$`** — so ROADMAP-tracked work with no RFC has
no conforming home, and the only way to give it one is a legacy-allowlist entry, which is exactly the
bypass that gate exists to prevent.

**Two of the four belong here on the merits and two are filed here out of that necessity. Stating
which is which, rather than inventing a common rationale:**

| Row | Fit |
|---|---|
| **AUD-05** — `prikk-crypto` is the only non-exempt crate without source-level `#![forbid(unsafe_code)]` | **On the merits.** A uniformity property that is currently held by review rather than by anything checkable. |
| **AUD-06** — `unwrap_used`/`expect_used`/`indexing_slicing` are `warn`, not `deny` | **On the merits.** This RFC's whole subject is verification the build does not enforce. |
| **AUD-07** — `refs.rs:508-509` says `tags/V1` and `tags/v1` "coexist as distinct refs"; publication refuses exactly that | **Filed here.** A false comment about a safety property, not a gate. |
| **AUD-08** — `merge_execute.rs:187` increments `update_seq` unchecked | **Filed here.** Arithmetic hygiene, not a gate, and the one behaviour change this RFC carries. |

**Delivered 2026-09-03** at `0fdde9a` (AUD-05), `b4bb0bd` (AUD-07), `d31071f` (AUD-08) and `e8d0cc5`
(AUD-06), alongside §6a at `c92c84e`. **`ROADMAP.md`'s four rows were retired on the owner's
instruction once the work landed**, so this section is now the design record for all four rather than
a second copy of a live tracker entry.

**What changed since the rows were written, and must not be transcribed from them:**

- **AUD-07's residual gap is narrower than the row implies.** `validate_no_ref_name_collision`
  (`refs/publication.rs:164-175`) folds through `prikk_object::ascii_fold`, so the ASCII case
  collision is genuinely refused at publication. What survives is (a) collisions that predate the
  validator or arrive by a path that does not publish, and (b) **NFC/NFD and non-ASCII case pairs**,
  which `ascii_fold` by construction does not see — a limitation DC-72 §3.5 already recorded. The
  corrected comment must name both, or it replaces one wrong sentence with another.
- **AUD-08 is not one site.** `merge_execute.rs:187` is the row's example, not the population.

## 7. Scope

**In:** the oracle-backed property tests (§2); a scheduled fetching `cargo audit` CI job (§3);
`cargo doc -D rustdoc::private_intra_doc_links` in CI plus the 7 existing fixes, and `mdbook build`
on PRs touching `docs/**` or the CLI (§4); doctests for the ten kernel entry points (§4); criterion in its own
member (§5, per §6's ruling); `permissions: contents: read` on the three check-only workflows (§6a);
AUD-05 through AUD-08 (§6b).

**Out:** coverage measurement, mutation testing, sanitizers, Miri (moot — no unsafe outside the
Windows FFI crate, which CI already builds and tests on Windows). Differential testing against Git —
there is no compatibility claim to differentiate against.

## 8. Ordering

**§3 and §4 are the cheap half and should go first** — a CI job and a lint flag, each independently
landable, each closing a hole that lets everything else drift silently. §2 is the valuable half and
needs real design. §5 waits on §6's ruling.
