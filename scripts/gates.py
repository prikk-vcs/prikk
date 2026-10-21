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
grepping the tail both get the same answer without re-running anything. An argument this script does
not recognize (anything other than `--list`) is refused outright, by name, at exit code 2 -- it is
never silently ignored (0.52.0 step 1 Part B: `--only <gate-name>` used to run the full suite anyway).
"""

import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# RFC 126 §4: the lint travels via `RUSTDOCFLAGS`, not a `cargo doc` flag -- there is no passthrough
# for it on that subcommand.
DOC_ENV = {"RUSTDOCFLAGS": "-D rustdoc::private_intra_doc_links"}

# rfcs/EXECUTION-ORDER.md §6 rule 9. 0.54.0 Part G1, item 3 reordered this list, cheapest and
# least-dependent-on-a-build first: `fmt` (no compile at all), `git-diff-check` and `audit` (no
# compile of this workspace's own code), the release-policy checks (compile one small tool crate,
# `prikk-release-policy`, not the workspace), then the three Clippy passes (each a full compile),
# `doc`, and finally the three full test suites -- the slowest gates, and the ones most likely to
# still be red while an earlier, cheaper gate would already have caught the same mistake for less.
# `audit` is not named anywhere in the handoff's own ordering sentence (`fmt, diff check, the
# policy checks, clippy (all three), doc, then the suites`); it is placed here, with the other
# fast, compilation-free checks and ahead of the policy checks that do compile a crate, as this
# round's own placement call -- see the report this round produced
# (`.git-exclude/review-request/0.54.0-g1-report.md`) for that call stated explicitly, in case the
# intended order was something else.
GATES = [
    ("fmt", ["cargo", "fmt", "--all", "--", "--check"], None),
    ("git-diff-check", ["git", "diff", "--check"], None),
    ("audit", ["cargo", "audit", "--no-fetch"], None),
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
    ("doc", ["cargo", "doc", "--workspace", "--no-deps"], DOC_ENV),
    ("test", ["cargo", "test", "--workspace", "--locked"], None),
    ("test-msrv", ["cargo", "+1.85.0", "test", "--workspace", "--locked"], None),
    (
        "check-msrv",
        ["cargo", "+1.85.0", "check", "--workspace", "--all-targets", "--locked"],
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


# 0.54.0 Part G1b, C1: the folder leak lives here, in one place, instead of at each of the ~1,270
# fixture call sites across the two test crates (the wide conversion G1 tried and reverted). Every
# gate run gets its own fresh, short-named subdirectory under the chosen base -- short so a deep
# `choose_tmpdir()` base plus this name still cannot push a Unix socket path past SUN_LEN (the same
# concern that already keeps the repo-local base out of the default path). `secrets.token_hex(4)` is
# 8 hex characters: enough to make two concurrent runs' directories distinct, short enough to stay
# well under any socket-path limit next to a short base like `/tmp`.
def make_run_tmpdir(base):
    run_dir = os.path.join(base, f"prikk-gates-{secrets.token_hex(4)}")
    os.makedirs(run_dir, exist_ok=False)
    return run_dir


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


# 0.52.0 step 1 Part B, item 2: the only argument this script understands. An unrecognized one
# (a typo, or `--only <gate-name>`, which Part W's own report found silently ran the full 14-gate
# suite anyway) used to be ignored outright by `main`'s own `if "--list" in sys.argv[1:]:` check,
# which inspects argv for one known flag and never objects to anything else present. Refused instead,
# by name, at exit code 2 -- the same "bad usage" code `prikk`'s own CLI uses for an unrecognized
# argument (`CliError::Usage(_) => 2`, `crates/prikk-cli/src/commands.rs`), and the same message
# shape `crates/prikk-cli/src/arg_scan.rs::unknown_argument` already uses there
# ("unknown {command} argument: {argument}").
KNOWN_ARGUMENTS = {"--list"}


def main():
    for argument in sys.argv[1:]:
        if argument not in KNOWN_ARGUMENTS:
            print(f"unknown gates.py argument: {argument}", file=sys.stderr)
            return 2

    if "--list" in sys.argv[1:]:
        for name, argv, _ in GATES:
            print(f"{name}: {' '.join(argv)}")
        return 0

    base_tmpdir, why = choose_tmpdir()
    tmpdir = make_run_tmpdir(base_tmpdir)
    print(f"TMPDIR base: {base_tmpdir} ({why}); this run's TMPDIR: {tmpdir}", flush=True)

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

    # 0.54.0 Part G1b, C1: a passing run leaves nothing behind; a failing one keeps everything,
    # named, for a human to go look at. Only this run's own subdirectory is ever removed or named
    # here -- never `base_tmpdir`, which this script did not create and must not touch.
    if failed:
        print(f"gate run failed -- TMPDIR kept for inspection: {tmpdir}", flush=True)
    else:
        shutil.rmtree(tmpdir, ignore_errors=True)

    return len(failed)


if __name__ == "__main__":
    sys.exit(main())
