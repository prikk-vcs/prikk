#!/usr/bin/env python3
"""Print the RUSTFLAGS a release build of prikk is made with (0.59.0, RH1).

With these flags the binary's bytes depend on the source, the toolchain and the flags, and not on the directories the build ran in. Run it from the
root of the checkout (the release notes give the whole command):

    RUSTFLAGS="$(python3 scripts/reproducible_rustflags.py)" cargo build -p prikk --release --target <triple> --locked

Three absolute prefixes are remapped to fixed names. The workspace itself is not among the strings a build embeds (cargo gives rustc relative paths for
it); what is embedded is:

  * the registry the dependencies are read from: ``$CARGO_HOME`` (default ``~/.cargo``)  ->  ``/cargo``
  * the toolchain's own source, which is present when the ``rust-src`` component is installed: ``<sysroot>/lib/rustlib/src/rust``  ->
    ``/rustc/<commit>``, the form ``rustc`` itself gives the standard library when ``rust-src`` is absent, so a build with and without the component is
    the same bytes
  * the checkout, the current directory  ->  ``/src`` (nothing embeds it today; this keeps a dependency's build script from writing it out)

A path with whitespace cannot be passed through ``RUSTFLAGS``; the script refuses it rather than print flags that split in the wrong place.
"""

import os
import subprocess
import sys


def rustc(*args):
    return subprocess.run(["rustc", *args], check=True, capture_output=True, text=True).stdout


def flags(root, cargo_home, sysroot, commit):
    for path in (root, cargo_home, sysroot):
        if any(ch.isspace() for ch in path):
            raise SystemExit(
                f"reproducible_rustflags: the path {path!r} contains whitespace, which RUSTFLAGS cannot carry; build from a path without it"
            )
    if not commit:
        raise SystemExit(
            "reproducible_rustflags: `rustc -vV` prints no commit-hash (a rustc built without one): cannot name the toolchain's source"
        )
    return " ".join(
        [
            f"--remap-path-prefix={cargo_home}=/cargo",
            f"--remap-path-prefix={sysroot}/lib/rustlib/src/rust=/rustc/{commit}",
            f"--remap-path-prefix={root}=/src",
        ]
    )


def main():
    cargo_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    sysroot = rustc("--print", "sysroot").strip()
    commit = next((line.split(": ", 1)[1] for line in rustc("-vV").splitlines() if line.startswith("commit-hash: ")), "")
    sys.stdout.write(flags(os.getcwd(), cargo_home, sysroot, commit))


if __name__ == "__main__":
    main()
