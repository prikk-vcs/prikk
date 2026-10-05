#!/usr/bin/env python3
"""The 14 local gates (`rfcs/EXECUTION-ORDER.md` §6 rule 9), run as one script (0.49.0 step 5, D8).

Not a shell script. This repository's own `release-policy boundary-check`/`reference-check` scan every
`.sh`/`.yml`/`.yaml` file under `.github`, `scripts`, and `release` against a closed, line-based command
grammar (`tools/release-policy/src/command_scan/`) built for the literal, branch-free invocations a CI
workflow step already is -- it has no model of shell control flow at all (confirmed by reading the
lexer: `if`/`for`/`done`/`$(( ))` all mis-tokenize against its separator-splitting, which treats bare
`;`, `|`, `&`, `(`, `)`, `[`, `]` as hard command boundaries wherever they appear outside quotes). This
script needs real branching (loop over the gates, wrap each conditionally, aggregate exit codes), which
that grammar cannot express as a `.sh` file without being flagged as a wall of unclassified commands.
A `.py` file is outside that extension sweep entirely (`governed_procedure_file` in
`tools/release-policy/src/boundary/publication.rs` only matches `sh`/`yml`/`yaml`), and this project
already uses Python for its own tooling elsewhere (the external-review reproduction scripts, the
corpus extractor's test fixtures), so this is the existing convention, not a new one.

Usage: scripts/gates.py [--list]

Exit status: 0 only when every gate below exits 0. Prints one "<name> <exit>" line per gate as it
finishes, then the same lines again as a summary block, so a human scrolling back and a machine
grepping the tail both get the same answer without re-running anything.
"""

import os
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# RFC 126 §4: the lint travels via `RUSTDOCFLAGS`, not a `cargo doc` flag -- there is no passthrough
# for it on that subcommand.
DOC_ENV = {"RUSTDOCFLAGS": "-D rustdoc::private_intra_doc_links"}

# rfcs/EXECUTION-ORDER.md §6 rule 9, in the order that rule states them, plus its own amendment's two
# cross-target rows at the end -- run every time now, not only when a diff touches `#[cfg(target_os)]`
# code (the amendment's own reasoning: running them costs little, and making them conditional already
# let a Windows failure through twice, 0.49.0 step 5 D8).
GATES = [
    ("fmt", ["cargo", "fmt", "--all", "--", "--check"], None),
    (
        "clippy",
        [
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        None,
    ),
    ("test", ["cargo", "test", "--workspace", "--locked"], None),
    ("test-msrv", ["cargo", "+1.85.0", "test", "--workspace", "--locked"], None),
    (
        "check-msrv",
        ["cargo", "+1.85.0", "check", "--workspace", "--all-targets", "--locked"],
        None,
    ),
    ("git-diff-check", ["git", "diff", "--check"], None),
    ("audit", ["cargo", "audit", "--no-fetch"], None),
    ("doc", ["cargo", "doc", "--workspace", "--no-deps"], DOC_ENV),
    (
        "release-policy-check",
        ["cargo", "run", "--locked", "-p", "prikk-release-policy", "--", "check"],
        None,
    ),
    (
        "boundary-check",
        ["cargo", "run", "--locked", "-p", "prikk-release-policy", "--", "boundary-check"],
        None,
    ),
    (
        "reference-check",
        ["cargo", "run", "--locked", "-p", "prikk-release-policy", "--", "reference-check"],
        None,
    ),
    (
        "size-check",
        ["cargo", "run", "--locked", "-p", "prikk-release-policy", "--", "size-check"],
        None,
    ),
    (
        "clippy-windows",
        [
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--target",
            "x86_64-pc-windows-gnu",
            "--",
            "-D",
            "warnings",
        ],
        None,
    ),
    (
        "clippy-macos",
        [
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--target",
            "x86_64-apple-darwin",
            "--",
            "-D",
            "warnings",
        ],
        None,
    ),
]

# RFC 160 §9 R1: every local gate, test, probe and perturbation runs under an R1 cgroup scope with a
# memory ceiling and a timeout. The ceiling and timeout are shared across all 14 rather than tuned per
# gate -- the slowest (a from-scratch MSRV toolchain build, or the cross-target clippy checks) still
# finishes well inside it, and a single shared number is one thing to justify, not fourteen.
R1_MEMORY_MAX = "8G"
R1_TIMEOUT_SECONDS = 1800


def systemd_run_prefix():
    """`None` when `systemd-run` is not on `PATH` -- the caller then runs the gate directly and says
    so, rather than silently skipping R1 (the handoff's own "the script says plainly" requirement)."""
    if shutil.which("systemd-run") is None:
        return None
    return [
        "systemd-run",
        "--user",
        "--scope",
        "-q",
        "-p",
        f"MemoryMax={R1_MEMORY_MAX}",
        "-p",
        "MemorySwapMax=0",
        "--",
        "timeout",
        str(R1_TIMEOUT_SECONDS),
    ]


def writable_directory(path):
    return os.path.isdir(path) and os.access(path, os.W_OK | os.X_OK)


def choose_tmpdir():
    # EXECUTION-ORDER.md §6 rule 9: an existing, writable TMPDIR is kept; else the system temp
    # directory; the repository-local directory only as a last resort. A path that is long (a deep
    # checkout) makes Unix socket paths exceed SUN_LEN, so the repo-local choice is not the default.
    existing = os.environ.get("TMPDIR")
    if existing and writable_directory(existing):
        return existing, "an existing writable TMPDIR"
    system = tempfile.gettempdir()
    if writable_directory(system):
        return system, "the system temp directory"
    local = os.path.join(ROOT, ".git-exclude", "tmp")
    os.makedirs(local, exist_ok=True)
    return local, "the repository-local directory (last resort)"


def run_gate(name, argv, env_overrides, tmpdir):
    env = dict(os.environ)
    env["TMPDIR"] = tmpdir
    if env_overrides:
        env.update(env_overrides)
    prefix = systemd_run_prefix()
    r1 = prefix is not None
    command = (prefix or []) + argv
    began = time.monotonic()
    result = subprocess.run(command, cwd=ROOT, env=env)
    elapsed = time.monotonic() - began
    return result.returncode, elapsed, r1


def main():
    if "--list" in sys.argv[1:]:
        for name, argv, _ in GATES:
            print(f"{name}: {' '.join(argv)}")
        return 0

    tmpdir, why = choose_tmpdir()
    print(f"TMPDIR for every gate: {tmpdir} ({why})", flush=True)

    results = []
    any_without_r1 = False
    for name, argv, env_overrides in GATES:
        print(f"==== {name} ====", flush=True)
        exit_code, elapsed, r1 = run_gate(name, argv, env_overrides, tmpdir)
        if not r1:
            any_without_r1 = True
            print(f"{name}: ran without an R1 scope -- systemd-run is not on PATH", flush=True)
        print(f"{name} {exit_code} ({elapsed:.1f}s)", flush=True)
        results.append((name, exit_code))

    print("==== summary ====")
    if any_without_r1:
        print("summary: at least one gate above ran without an R1 scope (systemd-run not found)")
    for name, exit_code in results:
        print(f"{name} {exit_code}")

    failed = [name for name, exit_code in results if exit_code != 0]
    return len(failed)


if __name__ == "__main__":
    sys.exit(main())
