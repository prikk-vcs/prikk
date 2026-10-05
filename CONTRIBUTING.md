# Contributing

Prikk is pre-1.0 experimental software, developed by one maintainer under an explicit design-review
discipline. This file exists because that discipline is not something you could guess by reading the
code, and it changes what a contribution should look like.

## How work is reviewed here

**A drive-by pull request is not the expected shape of a contribution.** Every change that has
landed in this repository went through a written proposal (an RFC, under `rfcs/`), design review,
and then implementation against a fixed gate set, reviewed independently of whoever wrote it — in
that order, not implementation first. `rfcs/EXECUTION-ORDER.md` is the single ordered view of what
is open and what it is waiting on.

**What "independent" covers, and what it does not.** Implementation is reviewed independently of
its own author: the person (or agent) that writes the code never reviews it. Design review is not
independent in that same sense today — the architect proposes an RFC and also rules on its design
— except for one external review of direction and process, commissioned before the 0.48.0 release
(review 014, pinned at commit `bb81b0fb`). There is no standing independent check of the architect's
own design judgment.

The sequence, for anything beyond a small, obviously-correct fix:

1. Requirements and an RFC (`rfcs/proposed/`).
2. External design, then internal design, then program design — reviewed before code is written.
3. Implementation and testing, one self-contained increment per review round.

This is unusual for an open-source project of this size, and it is deliberate: the product's whole
premise is verifiable history, and a change that skipped review would undermine the same guarantee
the project exists to provide. If you open a pull request for anything non-trivial without this
having happened first, expect it to be redirected into that process rather than merged directly —
not as a rejection, but because that is genuinely how a change becomes part of this project's
history.

For a small, self-contained fix (a typo, an off-by-one, a clearly wrong error message), a plain pull
request is fine. Say plainly in the description what you changed and why; you do not need to write an
RFC for it.

## Before you start on anything larger

Open an issue describing the problem and your proposed approach before writing code. This is not
bureaucracy for its own sake — it is cheaper for both of us to disagree about a paragraph than about
a diff, and a change that does not fit the project's design (see the [System
Architecture](docs/src/reference/architecture.md) and [Non-Goals](docs/src/reference/non-goals.md)
reference pages) is easiest to redirect before it exists.

## Building and testing

The [Development](docs/src/contributing/development.md) reference page has the full detail
(including how to build the documentation book).

Run the whole gate set with one script:

```sh
scripts/gates.py
```

It runs all 14 gates below, prints one `<name> <exit>` line per gate as it finishes (and again as a
summary), and exits 0 only when every one of them does. Each runs under an R1 (`systemd-run --user`)
scope when that is on `PATH`; the script says plainly when it is not, rather than silently running
without one.

The gate every candidate change must pass, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9 (what
`scripts/gates.py` runs):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo +1.85.0 test --workspace --locked
cargo +1.85.0 check --workspace --all-targets --locked
git diff --check
cargo audit --no-fetch
RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-apple-darwin -- -D warnings
```

Plus release-policy `check`, `boundary-check`, `reference-check`, and `size-check` — see the
Development page for what those verify. The two cross-target `clippy` runs above run every time, not
only on a change touching platform-conditional (`#[cfg(target_os)]`) code — this project has been
caught by CI going red from exactly that gap before, on a change that carried no `cfg(target_os)` of
its own, and running them always costs little next to the other twelve.

In restricted environments where the default temporary directory is read-only, use a workspace-local
one for the integration tests:

```sh
mkdir -p target/tmp
TMPDIR="$PWD/target/tmp" cargo test --workspace --locked
```

## Repository layout

- `crates/` — Rust workspace crates for the CLI, object model, crypto, repository store, replay
  semantics, hash primitives, and shared errors.
- `docs/` — mdBook documentation.
- `release/` — release-policy schemas and review fixtures; root `release-signers.toml` is the fail-closed
  official signer allowlist.
- `rfcs/` — design records and lifecycle state. `rfcs/done/000-rfc-lifecycle-policy.md` defines how
  `proposed/`, `accepted/`, `done/`, `archive/`, and `handoffs/` are used.
- `ROADMAP.md` — current release and upcoming theme summary.
- `CHANGELOG.md` — released changes.

## Reporting a security vulnerability

That is not what this file is for — see [SECURITY.md](SECURITY.md).

## What this file deliberately does not have

No issue templates, no code of conduct, no funding metadata. Those are governance choices for a
project with more than one maintainer to make when it needs them, not gaps to fill in now.
