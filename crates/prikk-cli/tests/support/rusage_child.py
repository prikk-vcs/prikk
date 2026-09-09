#!/usr/bin/env python3
"""RFC 133 6b.3 step 1 -- exact peak RSS of one child process, via getrusage(RUSAGE_CHILDREN).

Used only by rfc133_node_count_memory.rs's own `#[ignore]`d instrument. This project's Rust code
forbids unsafe code workspace-wide (`unsafe_code = "forbid"`, only `prikk-ffi` exempted for its own
narrow, reviewed reason) -- calling getrusage from Rust would need it. Python's standard-library
`resource` module already wraps the same syscall safely, so this script is a small external tool
(the same category as `dc59_commit_benchmark.rs`'s own reliance on /proc, or git itself, both
already documented environmental prerequisites for a deliberately-run instrument), not a new
Cargo dependency and not a lint exemption.

RUSAGE_CHILDREN is a running maximum across every child *this process* has reaped -- never true
before the previous child terminated. Since this script's whole lifetime is "start, spawn exactly
one child, wait, read, exit", each invocation is its own fresh process by construction: the
"fresh process per measured commit" isolation the RFC's own method requires comes from being
invoked once per sample by the Rust harness, not from anything inside this script.

Usage: rusage_child.py <cwd> <binary> [args...]
Prints exactly one line to stdout: the child's peak RSS in KiB. Exits non-zero, with the child's
own stderr forwarded, if the child itself failed.
"""

import subprocess
import sys
import resource


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: rusage_child.py <cwd> <binary> [args...]", file=sys.stderr)
        return 2
    cwd = sys.argv[1]
    binary = sys.argv[2]
    args = sys.argv[3:]

    result = subprocess.run([binary, *args], cwd=cwd, capture_output=True)
    if result.returncode != 0:
        sys.stderr.write(result.stderr.decode("utf-8", errors="replace"))
        sys.stdout.write(result.stdout.decode("utf-8", errors="replace"))
        return result.returncode

    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    print(usage.ru_maxrss)
    return 0


if __name__ == "__main__":
    sys.exit(main())
