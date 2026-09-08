<h1><a href="https://prikk.org/" target="_blank"><img src="https://raw.githubusercontent.com/prikk-vcs/prikk/main/assets/logo/prikk-header-520.png" alt="Prikk" width="360"></a></h1>

[![documentation](https://img.shields.io/badge/docs-prikk.org-brightgreen)](https://prikk.org/docs/)
[![license](https://img.shields.io/crates/l/prikk.svg)](LICENSE)
[![CI](https://github.com/prikk-vcs/prikk/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/prikk-vcs/prikk/actions/workflows/ci.yml)    
[Report a vulnerability](SECURITY.md)

**Prikk is a standalone distributed version control system built around block-oriented patch theory.**

Prikk uses a native `.prikk/` repository format. It is not a Git wrapper and does not use `.git/` as a
storage backend. The project aims to combine patch-based semantic precision with practical performance
by sealing history into immutable blocks and keeping expensive patch reasoning bounded to active work.

> **Prikk is an early implementation.** It suits architecture review, experimentation, and
> contribution. **Do not use Prikk as the sole store for important project history yet** — the
> repository format and command surface are still evolving, and future releases may require
> migration. See the [release, versioning, and compatibility
> reference](./docs/src/reference/release-compatibility.md) for the pre-1.0 boundary.

## Project Goals

Prikk is designed to be:

- easy to use for ordinary local development workflows;
- safe and secure by default, with role-bound signatures and fail-closed validation;
- resilient against corruption, interrupted operations, and lost mutable pointers;
- flexible enough for local, peer, and future hosted workflows;
- fast for long-lived repositories by separating active patch reasoning from sealed block history;
- explainable when patch reasoning cannot prove a safe result.

## Core Ideas

- **Patch**: an atomic logical change with ordered operations and an AUTHOR signature.
- **Block**: an immutable sealed collection of patches; blocks are the scalability boundary.
- **Ref state**: signed reference state; ref files are pointers, not the root of trust.
- **Ref update**: append-only publication evidence for a ref transition.
- **WAL**: active signed patch envelopes before sealing.
- **Repository layout**: `.prikk/` stores native Prikk objects, refs, active WAL state, and local trust
  data; see the [repository layout reference](./docs/src/reference/repository-layout.md).
- **Concurrency and locking**: local lock files guard active-session and ref publication writes; see the
  [concurrency and locking reference](./docs/src/reference/concurrency-locking.md).
- **Path safety**: repository paths use a conservative validated subset; see the
  [path and worktree safety reference](./docs/src/reference/path-safety.md).
- **Attestation**: future audit/policy evidence targeting blocks without defining block identity.

## Good Fit

Prikk may be a good match if you are:

- evaluating next-generation VCS architecture;
- interested in patch theory, commutation, conflict evidence, or signed history;
- building tools that need verifiable local history and conservative recovery behavior;
- contributing to a Rust implementation of a correctness-sensitive CLI and storage system;
- reviewing security, durability, and publication-trust boundaries.

## Install

```sh
curl -fsSL https://github.com/prikk-vcs/prikk/releases/latest/download/install.sh | sh
```

Downloads the prebuilt archive for your platform, verifies its checksum, and installs to
`~/.local/bin`. Also available as `cargo binstall prikk`, `cargo install prikk`, or a direct download
from the [release page](https://github.com/prikk-vcs/prikk/releases).

**Repository mutation runs on Linux, macOS, and Windows; read-only commands build and run on every
platform Rust targets.** A checksum proves integrity of transport, not authority of origin — no
release passes the signer-authority audit yet.

The [install guide](./docs/src/guide/install.md) covers pinning a version, verifying checksums by
hand, building from source, uninstalling, and the per-platform detail; the [platform support
reference](./docs/src/reference/platform-support.md) has the exact command-by-command matrix.

## Quick Start

```sh
prikk setup ./sample-repo
cd ./sample-repo
```

`setup` initialises the repository, generates **your own** author and maintainer keys, adopts the
maintainer key, and prints the four `export` lines you need. Run those, then:

```sh
echo "hello prikk" > readme.txt
prikk commit -m "genesis"
prikk seal --allow-no-audit

prikk log
prikk verify
prikk doctor
```

**The seeds `setup` prints are real signing keys and are yours alone.** It says so itself: at least
one of them lands in your terminal scrollback, so treat it as a secret. See
[Security and Signing Setup](./docs/src/guide/security-setup.md) for the current setup boundary.

The commands above, with explanation and the two refusals you will actually hit along the way, are
the [Tutorial](./docs/src/guide/tutorial.md) — this block is a copy-pasteable summary of it, not a
second, independent walkthrough; its authority for what each step means and why is the tutorial page.
**The tutorial uses a fixed public example seed instead of `setup`**, so its transcript is exactly
reproducible and you can compare your output to it line by line — that is the one place a shared,
publicly known key belongs, and it must never be used for real signing.
[First Run](./docs/src/guide/first-run.md) covers `setup` itself, including what it does not do.

Ran into a refusal, or wondering why a step works the way it does? The
[Troubleshooting](./docs/src/guide/troubleshooting.md) and [FAQ](./docs/src/guide/faq.md) pages cover
exactly this walkthrough — including why ref names are fully qualified (`heads/topic`, not `topic`),
why `commit` and `seal` each need their own key, and how commits queue across multiple `commit` calls
before one `seal`.

## Useful Commands

Every command accepts `--help` for its own usage:

```text
prikk <command> --help
```

The full inventory, with exit-code semantics, is the [command surface
reference](./docs/src/reference/commands.md).

## Current State

What works today, and the limits worth knowing before relying on it, are in the [current state
reference](./docs/src/reference/current-state.md).

## Crates

`prikk` is the command-line tool. The others are the libraries it is built from, published so the
CLI can be built from crates.io — **their APIs may change without notice before 1.0.**

| Crate | Purpose | Version | Docs | Dependencies |
|---|---|---|---|---|
| [`prikk`](https://crates.io/crates/prikk) | the command-line tool | [![crates.io](https://img.shields.io/crates/v/prikk.svg?label=%20)](https://crates.io/crates/prikk) | [![documentation](https://img.shields.io/badge/docs-prikk.org-brightgreen)](https://prikk.org/docs/) | [![Dependency Status](https://deps.rs/crate/prikk/latest/status.svg)](https://deps.rs/crate/prikk) |
| [`prikk-store`](https://crates.io/crates/prikk-store) | repository storage engine — layout, object storage, WAL durability, verification, patch replay | [![crates.io](https://img.shields.io/crates/v/prikk-store.svg?label=%20)](https://crates.io/crates/prikk-store) | [![docs.rs](https://img.shields.io/docsrs/prikk-store?version=latest&label=%20)](https://docs.rs/prikk-store) | [![Dependency Status](https://deps.rs/crate/prikk-store/latest/status.svg)](https://deps.rs/crate/prikk-store) |
| [`prikk-object`](https://crates.io/crates/prikk-object) | object identity, canonical encoding, and payload types | [![crates.io](https://img.shields.io/crates/v/prikk-object.svg?label=%20)](https://crates.io/crates/prikk-object) | [![docs.rs](https://img.shields.io/docsrs/prikk-object?version=latest&label=%20)](https://docs.rs/prikk-object) | [![Dependency Status](https://deps.rs/crate/prikk-object/latest/status.svg)](https://deps.rs/crate/prikk-object) |
| [`prikk-replay`](https://crates.io/crates/prikk-replay) | replay and lifecycle semantics | [![crates.io](https://img.shields.io/crates/v/prikk-replay.svg?label=%20)](https://crates.io/crates/prikk-replay) | [![docs.rs](https://img.shields.io/docsrs/prikk-replay?version=latest&label=%20)](https://docs.rs/prikk-replay) | [![Dependency Status](https://deps.rs/crate/prikk-replay/latest/status.svg)](https://deps.rs/crate/prikk-replay) |
| [`prikk-crypto`](https://crates.io/crates/prikk-crypto) | Ed25519 signing and verification | [![crates.io](https://img.shields.io/crates/v/prikk-crypto.svg?label=%20)](https://crates.io/crates/prikk-crypto) | [![docs.rs](https://img.shields.io/docsrs/prikk-crypto?version=latest&label=%20)](https://docs.rs/prikk-crypto) | [![Dependency Status](https://deps.rs/crate/prikk-crypto/latest/status.svg)](https://deps.rs/crate/prikk-crypto) |
| [`prikk-hash`](https://crates.io/crates/prikk-hash) | SHA-256 primitives | [![crates.io](https://img.shields.io/crates/v/prikk-hash.svg?label=%20)](https://crates.io/crates/prikk-hash) | [![docs.rs](https://img.shields.io/docsrs/prikk-hash?version=latest&label=%20)](https://docs.rs/prikk-hash) | [![Dependency Status](https://deps.rs/crate/prikk-hash/latest/status.svg)](https://deps.rs/crate/prikk-hash) |
| [`prikk-error`](https://crates.io/crates/prikk-error) | shared error taxonomy | [![crates.io](https://img.shields.io/crates/v/prikk-error.svg?label=%20)](https://crates.io/crates/prikk-error) | [![docs.rs](https://img.shields.io/docsrs/prikk-error?version=latest&label=%20)](https://docs.rs/prikk-error) | [![Dependency Status](https://deps.rs/crate/prikk-error/latest/status.svg)](https://deps.rs/crate/prikk-error) |
| [`prikk-ffi`](https://crates.io/crates/prikk-ffi) | Windows filesystem-identity FFI bindings | [![crates.io](https://img.shields.io/crates/v/prikk-ffi.svg?label=%20)](https://crates.io/crates/prikk-ffi) | [![docs.rs](https://img.shields.io/docsrs/prikk-ffi?version=latest&label=%20)](https://docs.rs/prikk-ffi) | [![Dependency Status](https://deps.rs/crate/prikk-ffi/latest/status.svg)](https://deps.rs/crate/prikk-ffi) |

## Development Gates

Every change this project accepts passes the full gate set in
[`rfcs/EXECUTION-ORDER.md`](./rfcs/EXECUTION-ORDER.md) §6 rule 9, which that document owns. The three
commands worth running constantly, the rest of the set, and how work is reviewed here are in
[`CONTRIBUTING.md`](./CONTRIBUTING.md).

## More Detail

The roadmap, RFCs, and mdBook docs are the best entry points for design details:

- [Documentation](https://prikk.org/docs/)
- [ROADMAP.md](./ROADMAP.md)
- [rfcs/README.md](./rfcs/README.md)
- [Current data model](./docs/src/reference/data-model.md)
- [Current trust and threat model](./docs/src/reference/trust-threat-model.md)
- [Security and signing setup](./docs/src/guide/security-setup.md)
- [Current patch algebra and merge evidence concepts](./docs/src/reference/patch-algebra.md)
- [docs/src](./docs/src)
