# Development

The implementation follows the design-first sequence:

1. Requirements and RFCs.
2. External design.
3. Foundational Design Documents.
4. Program design.
5. Implementation.
6. Testing and evidence.

Release preparation follows the separate
[release, versioning, and compatibility](../reference/release-compatibility.md) policy. A listed gate is
not passing evidence unless it was observed for the exact commit or release under review.

Run the standard checks before submitting a source drop:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
git diff --check
cargo audit --no-fetch
RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps
cargo run --locked -p prikk-release-policy -- check
cargo run --locked -p prikk-release-policy -- boundary-check
cargo run --locked -p prikk-release-policy -- reference-check
```

### The cross-target addendum

If your change touches `#[cfg(target_os)]`-gated code, also run:

```sh
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-apple-darwin -- -D warnings
```

"Touching cfg-gated code" is broader than adding a `#[cfg(target_os)]` line yourself: it also covers
adding un-gated code to a file that already contains that gating. That shape adds no `cfg` line to
your diff, so it is easy to miss — and missing it once left the project's own main branch red on the
macOS and Windows Clippy jobs for three consecutive changes.

## Building the documentation

The book uses Mermaid diagrams, which are rendered by the `mdbook-mermaid` preprocessor. Both tools are
needed to build it:

```sh
cargo install mdbook --no-default-features --features search --vers "^0.5" --locked
cargo install mdbook-mermaid --vers "^0.17" --locked
mdbook build docs
```

`mdbook build` fails with a clear message if the preprocessor is missing, so a stale toolchain cannot
silently produce diagrams as code blocks. The Mermaid assets are vendored under `docs/`, so the built
book renders offline and fetches nothing.

The workspace declares Rust 1.85 as its minimum supported version. Verify that contract with the exact
minimum toolchain and locked dependency graph:

```sh
cargo +1.85.0 check --workspace --all-targets --locked
cargo +1.85.0 test --workspace --locked
cargo +1.85.0 build --workspace --locked
```

Strict Clippy remains a current-stable quality gate. It is not an MSRV gate because Clippy's lint set
changes with the toolchain.
