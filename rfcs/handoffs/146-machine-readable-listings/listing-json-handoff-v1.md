# RFC 146 — `--format json` for `log`, `branch` and `tag`

**Ruled:** `rfcs/proposed/146-machine-readable-listings.md`.

**LIVE — start.** RFC 146 was **accepted by the project owner 2026-09-10**, the same day it was opened,
and now sits in `rfcs/accepted/`. The hold this handoff carried while the RFC was `proposed/` is lifted.

## 1. What to build

`--format json` on three commands, and nothing else:

| command | schema name |
|---|---|
| `prikk log [path] [--limit N] [--ref REF]` | `log-report-v1` |
| `prikk branch [list] [--all]` | `branch-list-v1` |
| `prikk tag [list]` | `tag-list-v1` |

**Follow the convention already in `crates/prikk-cli/src/output/`, do not invent one.** Six emitters
live there (`status.rs`, `verification.rs`, `worktree.rs`, `show.rs`, `patch_plan.rs`, `trust.rs`,
`bundle_preview.rs`); read at least two before writing the first line. **Every one builds its JSON as a
string and opens with `"schema_version"`.**

**No serialization dependency.** prikk's whole third-party runtime surface is five crates. **If your
design wants `serde`, the design is wrong** — say so and stop rather than adding it.

## 2. The fields come from the prose, which already exists

Captured from the shipped 0.38.0 binary, so this is the real shape and not a sketch:

```
$ prikk log
history repository: <path>/.prikk
ref: heads/main
block 06fb7daf…
  ref-state: e44f0462…
  update-seq: 1
  kind: Root
  rollback-block: false
  parents: 0
  patches: 2
  rollback-patches: 0
  required-attestations: 0
  patch 433c6d50…: add a.txt
  patch a8ed3166…: rename a to b
  previous-ref-state: <none>

$ prikk branch
heads/main e44f0462…

$ prikk tag
no tags
```

**Every fact above is already computed.** This round emits what the command knows; **it must not
compute anything new**, and a design that needs a new query is out of scope — report it instead.

## 3. The six rules, and they are the whole design

RFC 146 §4, restated because each has bitten this project before:

1. **`--format json` must not fail where the bare command succeeds.** RFC 140 §7b settled this on
   `status`: JSON failing in a state prose survives is a defect.
2. **Degrade absence, propagate error — absence only.** RFC 142 §6b is the correction to the
   over-broad reading: **degrading every failure is wrong.** `previous-ref-state: <none>` → `null`; an
   unreadable object → an error.
3. **Machine-branchable fields, not rendered prose.** `--all` marks closed branches in prose; **JSON
   carries a boolean, not the marker text.** A consumer must never parse a string for a fact the
   command already has.
4. **Empty is a value.** `no tags` → `{"tags": []}`, exit `0`.
5. **The nesting is the data.** A block contains its patches; mirror that, do not flatten it.
6. **Exit codes unchanged** — RFC 121's `0`/`1`/`2`. Adding a flag moves none of them.

## 4. Controls

1. **One test per command reading *both* forms for the same repository state**, asserting they carry
   the same facts. **Not two tests each asserting its own expectation** — that shape passes while the
   two drift.
2. **Perturb rule 1.** Find a state where the command degrades in prose — a missing object, an
   unresolvable ref — and show the JSON form exits the same way. **Then break it deliberately and show
   the test fails.** Three rounds in this project have shipped controls that could not fail; do not add
   a fourth.
3. **Empty and non-empty for each of the three.** `prikk tag` on a fresh repository is the cheapest
   empty case.
4. **Exit codes checked on the new flag**, not assumed: `2` for a malformed `--format`, `0` for the
   empty listing.
5. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim.
6. **Cross-target addendum: check this round's own diff and state the outcome, or state why it does not
   apply.** Do not infer it from this sentence. `crates/prikk-cli/src/output/` is not obviously
   `cfg(target_os)`-free — count at source.

## 5. Report

- **The three schemas, in full**, as they will ship — a reviewer should not have to run the binary to
  see what a consumer gets.
- **Which prose facts you chose not to emit, and why.** `prikk log`'s headline prints an absolute
  repository path; **decide deliberately whether that belongs in machine output** and say what you
  decided. It is the kind of field that looks harmless and leaks a home directory into a log.
- **Both control-2 outcomes**: the passing run and the deliberate break.
- **Test counts before and after**, per rule 10.

## 6. What this round must NOT do

- **No fourth command.** `merge-evidence`, `merge-plan`, `inverse-plan`, `rollback-preview` and
  `unlock` also lack `--format json` and are **explicitly out of scope**.
- **No new dependency**, `serde` above all.
- **No prose change.** The human output is not touched, not even to align wording with the JSON.
- **No new computation.** Emit what exists.
- **No schema stability promise** beyond what the other nine carry.
