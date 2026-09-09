# RFC 133 §6d.5 — move the probe binary out of the published crate

**Ruled:** `rfcs/proposed/133-performance-cost-and-its-evidence.md` **§6d.5**. Follows `e36fd8b8`, which
is **accepted** — its measurement is not reopened. **Required before the next release that publishes
`prikk`.** Small round: a relocation, not a redesign.

## 1. What is wrong

`crates/prikk-cli` carries `[[bin]] rusage-object-index-probe` and `[features] rusage-probe = []`.
**`prikk` is published** — no `publish = false`, and it is last in the release publish order. So an
instrument-only binary ships in the `.crate` tarball, and **`rusage-probe` becomes a public feature name**
on a published crate: durable surface, visible on docs.rs, and a (minor) breaking change to remove later.

`required-features` correctly keeps it out of an ordinary `cargo install prikk`. That is not the point —
the point is that it is in the published package at all.

## 2. Where it goes — `tools/benchmarks`, and no new workspace member

**Move it into the existing `prikk-benchmarks` crate** (`tools/benchmarks`). Checked before ruling:

- it is already `publish = false`, and its own description already says *"Not part of the shipped
  product; not built by default"*;
- it already depends on **`prikk-store` and nothing else** — exactly what the probe needs;
- it is already a workspace member **and already in `boundary.rs`'s `check_members` allowlist**.

**So this needs no new crate and no allowlist edit.** Do not create `tools/rusage-probe`: a new workspace
member would require editing the hardcoded allowlist in `tools/release-policy/src/boundary.rs`, which is
real reviewed surface, for no benefit over a crate that already exists for exactly this purpose.

Add a `[[bin]]` to `tools/benchmarks/Cargo.toml`, move
`crates/prikk-cli/src/bin/rusage_object_index_probe.rs` under it, and **delete both the `[[bin]]` and the
`[features] rusage-probe` block from `crates/prikk-cli/Cargo.toml`.** The feature must be gone, not
merely unused.

## 3. How the test finds it — explicitly, with the identity recorded

`env!("CARGO_BIN_EXE_rusage-object-index-probe")` will not resolve once the binary lives in another
crate; that macro is defined only for test targets of the crate declaring the bin. **That is the same
wall RFC 139 increment 2 hit, and its ruling applies here: the executor takes the binary path explicitly
and records its identity.**

**Take the path from an environment variable** — name it clearly, e.g. `PRIKK_RUSAGE_PROBE_BIN` — and
**fail loudly with a message naming the variable and how to build the binary** when it is unset or does
not exist. The file already does exactly this for `rusage_child.zsh` and for `python3`; match that shape.

**Record the binary's identity in the report**, as RFC 139 increment 2 requires of its own executor: a
measurement taken with one binary is not comparable to one taken with another.

**This removes the feature gating entirely.** The 15 `#[cfg(feature = "rusage-probe")]` sites go away —
the driver is already `#[ignore]`d and the whole file is already `#![cfg(target_os = "linux")]`, so
run-time refusal is sufficient and simpler than compile-time gating. A welcome side effect: the driver
reappears in the default `cargo test` listing as `ignored` instead of vanishing under default features.

## 4. REQUIRED — a spot check that the relocation changed nothing

The binary's content is essentially unchanged, but its crate, and therefore its build, is not.

**Re-run the instrument at two points only — N=1,000 and N=64,000** — and confirm:

- the **standing control still holds** (resident cost >= `indexed_objects × 88`);
- the **resident-vs-minimum ratio still lands in the 2.7–3.0× band** §6d.4 recorded.

**A full seven-point re-run is not wanted.** If either point disagrees with §6d.4's recorded figures by
more than sampling spread, **stop and report it** — do not update the RFC's numbers yourself. A
relocation that moves the measurement is a finding, and it would mean the floor is sensitive to something
nobody has named.

## 5. What this round must NOT do

- **No production code change**, and nothing under `crates/prikk-store/src`.
- **Do not change the probe's logic**, its argument surface, or `rusage_child.zsh`.
- **Do not execute AUD-01** (§6d.2 stands), no `seen_ids` removal, no optimisation, no gate, no threshold.
- **Do not touch the other two drivers** or their series.
- **Do not edit `boundary.rs`'s allowlist.** If you find yourself needing to, you have chosen a new crate
  over `tools/benchmarks` — stop and report why rather than widening the allowlist.
- `MILESTONES.md` untouched; `NFR-PERF-01` status untouched.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit.

**Rule 9's cross-target addendum applies** — this diff touches `rfc133_node_count_memory.rs`, which
carries `#![cfg(target_os = "linux")]`, which is the widened-trigger shape exactly. Run both targets and
state the results, as the amended rule requires.

Report to `.git-exclude/review-request/`. State: the new run command verbatim (both steps — build the
binary, then run the instrument with the variable set), the binary's recorded identity, the two spot-check
points against §6d.4's figures, and confirmation that `crates/prikk-cli/Cargo.toml` no longer contains
either the `[[bin]]` or the `rusage-probe` feature.
