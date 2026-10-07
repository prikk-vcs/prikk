# RFC 170 — Embedding prikk as a library: an investigation

**Status.** **PROPOSED 2026-10-07 by the architect, at the owner's request** (*"record this now as a proposed RFC
(investigation only, unscheduled)"*).
- **Investigation only:** it answers questions, and decides nothing. Not a design, and nothing is handed off.
- **Unscheduled.** It follows 0.52.0 at the earliest (0.50.0 is step 1's corrections, 0.51.0 is RFC 155, 0.52.0 is F4
  with M6).

**Origin.** A proposal the owner relayed on 2026-10-07: split `prikk` into an app crate and a `prikk-core` API crate,
so that app developers can embed prikk as internal storage with strong versioning and flexible patches. The proposal
names two further needs: a configurable storage path, and an easy API for the keys that sealing needs.

## 1. What exists today (facts, read 2026-10-07)

- **The crates:**
  - `prikk` (`crates/prikk-cli`) is the app;
  - `prikk-store` is a library, already published on crates.io;
  - beneath it are `prikk-object`, `prikk-crypto`, `prikk-replay`, `prikk-hash`, `prikk-error` and `prikk-ffi`.
- **Core writes live in the app crate (RFC 112, accepted 2026-08-18, unscheduled).** `seal`, `branch create`/`close` and
  tag creation cannot be called from the library. Any embedding starts here.
- **Signing is behind two interfaces,** `AuthorSigner` and `MaintainerSigner` (`prikk-store`). The app provides keys
  from the environment, files and configuration (RFC 148; `crates/prikk-cli/src/key_material*`).
- **A repository lives in a `.prikk/` directory under a working directory** (`foundation/layout.rs`: `REPO_DIR`,
  `init`/`open` take that root).
- **A commit scans a working tree** (`commit_boundary::worktree_patch::commit_worktree_changes_signed`).
- **No library stability promise.** `release-compatibility.md` says the promise arrives in layers at 1.0, *"the library
  API last if ever"*. No external library consumer is known: stikk drives the CLI.

## 2. The questions

1. **The use cases.** Which applications, storing what, through which operations?
   - write content at a path; read it at a point; history; diff; patches; branches;
   - which of `seal`, sync and bundles an embedder needs.

   The answer bounds the API.
2. **Storage without a working tree.** Can a commit take content from the caller (path → bytes), with no files on disk?
   - What does that do to the worktree model, the baseline, and RFC 102's container layout?
   - Is there a repository with no worktree (a bare store) at a caller-chosen directory?
3. **The storage path.** A caller-chosen repository directory: its name, its location, and whether the worktree is
   separate from it, or absent.
4. **Keys for embedders.** One simple way to create, load, persist and rotate an app's author and maintainer keys,
   built on the existing signer interfaces.
   - What an embedder must never be able to do by accident: lose a key; use a test seed; skip trust.
   - How this meets RFC 148 and the trust model.
5. **The facade and its promise.** A small crate (`prikk-core`, the proposal's name), or a stable subset of
   `prikk-store`?
   - What it exposes, its error type, its versioning promise, and how that sits with *"the library API last if ever"*.
     **Changing that policy is the owner's decision.**
6. **Running in-process.**
   - Concurrency within one process: today's locks assume one command per process.
   - Thread safety; cancellation; resource bounds.
7. **The order:** RFC 112 first (the core writes into the library), then the facade, then the worktree-less commit.
   Confirm or correct.

## 3. Constraints carried in

- **No format change for embedding's sake** (RFC 114). Embedded repositories are ordinary prikk repositories.
- **Security is not optional in-process:** signatures, trust and verification behave as they do in the CLI.
- **Every public type is `#[non_exhaustive]` until the promise is ruled.**
- **One way to do each thing** (the owner's philosophy): no second commit path that disagrees with the CLI's.

## 4. What an investigation round would deliver

- Answers to §2, from source and small prototypes, with no product code.
- A recommended facade, with its API sketch.
- The list of owner decisions: the stability promise; scope; schedule.
