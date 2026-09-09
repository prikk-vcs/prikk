//! Minimal companion binary for RFC 133 §6d.3's resident-object-index probe
//! (`crates/prikk-cli/tests/rfc133_node_count_memory.rs`).
//!
//! **Lives here, not in `prikk-cli`, deliberately (RFC 133 §6d.5).** It was first built as a
//! `[[bin]]` on `prikk-cli` itself, gated behind a `rusage-probe` feature so an ordinary `cargo
//! install prikk` would not build it -- but `prikk` is a **published** crate, so that still shipped
//! the binary's source in the `.crate` tarball and put a public feature name on published surface,
//! however unreachable by default. `prikk-benchmarks` (this crate) is `publish = false`, already
//! depends on `prikk-store` and nothing else -- exactly what this probe needs -- and already exists
//! for precisely this "instrument, not shipped product" purpose, so relocating here needed no new
//! workspace member and no `boundary.rs` allowlist edit. The test file locates this binary by an
//! explicit environment variable rather than `env!("CARGO_BIN_EXE_*")`, which only resolves for
//! test targets of the crate that declares the `[[bin]]` -- the same wall RFC 139 increment 2's own
//! executor hit, resolved the same way it was there: take the path explicitly and record its
//! identity.
//!
//! **Why a genuinely separate binary, not another self-reexec worker.** The file's other probes
//! (`lifecycle_state_probe_worker`, the original object-index worker this one replaces) re-invoke
//! the whole compiled test binary by name-filter -- cheap to write, but that binary links every
//! crate the test file uses and pays the full `libtest` harness startup on every sample. This
//! binary links only `prikk-store` and does nothing else, which does shrink its own floor somewhat.
//!
//! **That turned out not to be the dominant effect, and the test file's own module docs record the
//! rest of the finding**: even this minimal binary, spawned through `rusage_child.py` (Python),
//! still read a floor of roughly 11 MiB -- because that floor tracks the *spawning parent's* own
//! resident size at fork time (a real Linux fork()+exec() characteristic), not this binary's own
//! content. The test file's own probe accordingly spawns this binary through
//! `tests/support/rusage_child.zsh` instead, whose own idle footprint is an order of magnitude
//! smaller. This binary being minimal is still worth doing (removes its own contribution from the
//! total), just not sufficient on its own.
//!
//! Usage: `rusage-object-index-probe <floor|read|write> [repo-root]`. `floor` takes no repository
//! and returns immediately -- process-startup cost only, this binary's own baseline. `read`/`write`
//! open the named repository's real object index through the same public API the commit/verify
//! paths use (`ObjectReadSnapshot::open` / `ObjectWriteSession::open`) and hold it until exit.

#![allow(clippy::expect_used)]

use prikk_store::{ObjectReadSnapshot, ObjectWriteSession, RepositoryLayout};

fn usage() -> ! {
    eprintln!("usage: rusage-object-index-probe <floor|read|write> [repo-root]");
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(mode) = args.next() else {
        usage();
    };
    if mode == "floor" {
        return;
    }
    let Some(repo_root) = args.next() else {
        usage();
    };
    let layout = RepositoryLayout::new(&repo_root).expect("opening the probed repository's layout");
    match mode.as_str() {
        "read" => {
            let snapshot = ObjectReadSnapshot::open(&layout).expect("ObjectReadSnapshot::open");
            std::hint::black_box(&snapshot);
        }
        "write" => {
            let session = ObjectWriteSession::open(&layout).expect("ObjectWriteSession::open");
            std::hint::black_box(&session);
        }
        other => {
            eprintln!("unrecognized mode {other:?}, expected floor, read, or write");
            std::process::exit(2);
        }
    }
}
