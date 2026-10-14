#!/usr/bin/env python3
"""Tests for `gates.py`'s own argument handling (0.52.0 step 1 Part B, item 2).

Not a cargo test: `gates.py` is a standalone script outside the Cargo workspace (its own module
docstring explains why it is Python at all), and this repository has no other Python test runner --
confirmed before writing this file: `scripts/` held only `gates.py` itself, and the only other `.py`
test-shaped file in the whole repository, `crates/prikk-cli/tests/support/rusage_child.py`, is a test
*helper* a Rust test invokes, not a Python test runner of its own. Plain `unittest` (stdlib only, no
new dependency) is the smallest thing that works.

Loads `gates.py` by file path (`importlib`, not `sys.path` + `import gates`) so this file runs
correctly regardless of the caller's own working directory, and so the red/green demonstration below
can point it at an *older* copy of the script without colliding module names in `sys.modules`.

**Never actually runs a real gate.** The point of these tests is argument handling, decided before any
gate ever runs -- `gates.run_gate` is monkeypatched to a counting stub in every test, so even the "red"
run against the script as it stood before this part (which did not refuse an unknown argument at all)
finishes in milliseconds instead of discovering the bug the slow way, by waiting for `cargo fmt`
through `clippy-macos` to actually execute.

Usage:
    python3 scripts/gates_test.py
    GATES_PY_PATH=/path/to/an/older/gates.py python3 scripts/gates_test.py   # red-before-green
"""

import contextlib
import importlib.util
import io
import os
import sys
import unittest

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
DEFAULT_GATES_PY_PATH = os.path.join(SCRIPT_DIR, "gates.py")


def load_gates_module():
    path = os.environ.get("GATES_PY_PATH", DEFAULT_GATES_PY_PATH)
    spec = importlib.util.spec_from_file_location("gates_under_test", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GATES = load_gates_module()


class CountingRunGateStub:
    """Replaces `gates.run_gate` for the duration of one test -- every call is recorded, and none of
    them ever spawns a real subprocess. A script that correctly refuses an unknown argument must leave
    this stub's own call count at zero; one that does not (the bug this part fixes) calls it once per
    entry in `GATES.GATES` (14, on the real script)."""

    def __init__(self):
        self.calls = []

    def __call__(self, name, argv, env_overrides, tmpdir):
        self.calls.append(name)
        return 0, 0.0, True


class ListArgument(unittest.TestCase):
    """Regression check: the one argument this script is supposed to accept must still work exactly
    as before -- prints the gate list, exits 0, and (since `--list` returns before the gate loop even
    starts) never calls `run_gate` either."""

    def test_list_prints_every_gate_and_exits_zero_without_running_any(self):
        stub = CountingRunGateStub()
        original_run_gate = GATES.run_gate
        GATES.run_gate = stub
        original_argv = sys.argv
        sys.argv = ["gates.py", "--list"]
        captured = io.StringIO()
        try:
            with contextlib.redirect_stdout(captured):
                exit_code = GATES.main()
        finally:
            GATES.run_gate = original_run_gate
            sys.argv = original_argv

        self.assertEqual(exit_code, 0)
        self.assertEqual(stub.calls, [])
        output = captured.getvalue()
        for name, _argv, _env in GATES.GATES:
            self.assertIn(name, output, f"--list must print every gate's own name: missing {name!r}")


class UnknownArgument(unittest.TestCase):
    """The behaviour this part adds: an argument that is not `--list` must be refused outright, by
    name, at exit code 2 -- never silently accepted and never left to fall through into running the
    full 14-gate suite regardless (the exact shape Part W's own report found: `--only <gate-name>`
    looked like it selected a subset, and instead ran everything)."""

    def test_an_unknown_argument_is_refused_naming_itself_and_runs_no_gate(self):
        stub = CountingRunGateStub()
        original_run_gate = GATES.run_gate
        GATES.run_gate = stub
        original_argv = sys.argv
        sys.argv = ["gates.py", "--only", "clippy"]
        captured_out = io.StringIO()
        captured_err = io.StringIO()
        try:
            with contextlib.redirect_stdout(captured_out), contextlib.redirect_stderr(captured_err):
                exit_code = GATES.main()
        finally:
            GATES.run_gate = original_run_gate
            sys.argv = original_argv

        self.assertEqual(
            exit_code,
            2,
            f"an unknown argument must refuse at exit code 2, not run the suite (stub saw: {stub.calls})",
        )
        self.assertEqual(
            stub.calls,
            [],
            f"an unknown argument must run no gate at all; the stub recorded: {stub.calls}",
        )
        self.assertIn(
            "--only",
            captured_err.getvalue(),
            f"the refusal must name the unrecognized argument: {captured_err.getvalue()!r}",
        )

    def test_a_typo_is_refused_the_same_way(self):
        stub = CountingRunGateStub()
        original_run_gate = GATES.run_gate
        GATES.run_gate = stub
        original_argv = sys.argv
        sys.argv = ["gates.py", "--lsit"]
        captured_err = io.StringIO()
        try:
            with contextlib.redirect_stderr(captured_err), contextlib.redirect_stdout(io.StringIO()):
                exit_code = GATES.main()
        finally:
            GATES.run_gate = original_run_gate
            sys.argv = original_argv

        self.assertEqual(exit_code, 2)
        self.assertEqual(stub.calls, [])
        self.assertIn("--lsit", captured_err.getvalue())


if __name__ == "__main__":
    unittest.main()
