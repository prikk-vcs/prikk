# Install

See the root [README](https://github.com/prikk-vcs/prikk#install) for what to run: the shell
installer below, `cargo binstall prikk`, a direct download from the [release
page](https://github.com/prikk-vcs/prikk/releases), or `cargo install prikk` from crates.io. This
page covers what comes after — the steps a new user usually gets stuck on.

## The shell installer

```sh
curl -fsSL https://github.com/prikk-vcs/prikk/releases/latest/download/install.sh | sh
```

**What it does, exactly**: detects your OS and CPU architecture (`uname -s`/`uname -m`); downloads
that platform's release archive and its `.sha256` file from the same release page URLs the manual
path uses; verifies the checksum with `sha256sum -c` (or `shasum -a 256 -c` on macOS, which has no
`sha256sum` by default) and **installs nothing if verification fails**; extracts the `prikk` binary
to `~/.local/bin` (`chmod +x`); and, if that directory is not already on `PATH`, appends one clearly
marked block to your shell's startup file so a later uninstall can find and remove exactly that
block, and nothing else.

**Generated, not hand-maintained.** `install.sh`/`uninstall.sh` are produced by
`prikk-release-policy generate-installer` and attached as release assets at publish time, the same
way the tarballs and their checksums are — never committed to the repository as tracked files.
Their source template lives in `tools/release-policy/templates/`, reviewed like any other change to
this project.

**What it claims, and what it does not.** A passing checksum proves the file you received matches
what the release page published; it does not prove *who* published it.
In v0 one maintainer key signs every release tag; `release-signers.toml` is empty because no
multi-signer policy exists yet — see
[how prikk releases](../reference/release-compatibility.md#how-prikk-releases). The script prints this
same statement when it finishes, rather than only stating it here.

**Supported today**: Linux (`x86_64`, `aarch64`) and macOS (Apple Silicon only — there is no
prebuilt binary for Intel Macs). **Not yet supported**: Windows — the script detects it and refuses
with a pointer to the manual `.zip` download or `cargo install prikk`, rather than silently doing
nothing or guessing. A PowerShell equivalent is a separate, later increment.

**Version selection**: defaults to the latest release. Pin one with `sh install.sh --version X.Y.Z`
or `PRIKK_INSTALL_VERSION=X.Y.Z` — the form a CI pipeline typically wants, so a workflow does not
silently pick up a new release mid-pipeline. Override the install directory with `--prefix DIR` or
`PRIKK_INSTALL_DIR`.

**Read it before you run it, if you prefer**: `curl -fsSL <url> -o install.sh`, open it in an
editor, then `sh install.sh`. It is a single self-contained file — nothing it does is hidden behind
a second download.

**Re-running is safe.** It overwrites the binary with the same (or a newly pinned) version and does
not duplicate the marked `PATH` block if one already exists.

**Uninstalling**:

```sh
curl -fsSL https://github.com/prikk-vcs/prikk/releases/latest/download/uninstall.sh | sh
```

Removes the binary from its install directory and the marked `PATH` block from whichever shell
startup file has it, and nothing else — an unrelated file in the same directory, or unrelated lines
in the same startup file, are left untouched. If you installed with a custom `PRIKK_INSTALL_DIR`,
pass the same value to the uninstaller too, the same way: `PRIKK_INSTALL_DIR=/custom/path curl ...`.
Neither script touches any repository's own `.prikk` directory — delete those yourself if you no
longer want that history tracked.

**One real caveat about the `PATH` edit**: macOS Terminal starts a login shell, which reads
`~/.bash_profile`, not `~/.bashrc`, unless your own `~/.bash_profile` already sources it. The
installer still writes to `~/.bashrc` (or `~/.zshrc` for zsh) — the file an interactive non-login
shell reads on Linux, and the common case this script optimizes for — and says so when it runs; if
your `~/.bash_profile` does not source `~/.bashrc`, add the `PATH` line manually, or add
`. ~/.bashrc` to `~/.bash_profile` yourself.

## Verify the checksum

Every downloaded release archive ships beside a `.sha256` file recording its SHA-256 digest —
for example, `prikk-x86_64-unknown-linux-gnu.tar.gz` ships beside
`prikk-x86_64-unknown-linux-gnu.tar.gz.sha256`. Download both into the same directory, then verify
from inside it, substituting the archive name for your platform.

**Linux**:

```sh
sha256sum -c prikk-x86_64-unknown-linux-gnu.tar.gz.sha256
```

**macOS** (no `sha256sum` by default; `shasum` reads the identical file format):

```sh
shasum -a 256 -c prikk-aarch64-apple-darwin.tar.gz.sha256
```

Both print `<archive name>: OK` on success and exit non-zero on a mismatch.

**Windows** (PowerShell):

```powershell
$expected = (Get-Content prikk-x86_64-pc-windows-msvc.zip.sha256).Split(' ')[0]
$actual = (Get-FileHash prikk-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower()
$expected -eq $actual
```

This should print `True`. **Unverified on a Windows machine** — this command was derived from how
the release build produces the checksum file (a lowercase hex digest, matching `sha256sum`'s own
format), not confirmed by running it.

A passing checksum proves the download matches what was published; it does not prove *who*
published it — see [Release, Versioning, and Compatibility](../reference/release-compatibility.md#core-caveats).

## Build from source

Prebuilt archives cover four targets: `x86_64` and `aarch64` Linux, `aarch64` macOS, and `x86_64`
Windows. Anywhere else, build it yourself — there is no separate porting step:

```sh
cargo install prikk            # from crates.io, and puts it on PATH for you
```

or from a clone, which is also what you want when working on Prikk itself:

```sh
git clone https://github.com/prikk-vcs/prikk
cd prikk
cargo build -p prikk --release --locked   # binary at target/release/prikk
```

### Other Linux architectures

**Fully supported.** Nothing in Prikk is gated on CPU architecture — only on the operating system —
so any architecture Rust targets on Linux builds and runs with no reduction in capability.

### FreeBSD, OpenBSD, and other platforms

**They build, but they are read-only.** Prikk compiles for FreeBSD, and nothing in it is
OpenBSD-specific, so the same applies there — but **repository mutation is refused at runtime on any
platform other than Linux, macOS, and Windows**:

```
repository mutation requires Linux, macOS, or Windows root-scoped filesystem capabilities
```

So `init`, `commit`, and `seal` will not work. Reading an existing repository does — `verify`, `log`,
`status`, `doctor`, and the other read-only commands.

**This is a review boundary rather than a technical one.** FreeBSD has the filesystem primitives
Prikk's POSIX path uses; what it does not have is a durability implementation anyone has reviewed, or
CI that exercises one. Prikk refuses rather than writing history through a path nobody has audited.
Support is [recorded as a future
theme](https://github.com/prikk-vcs/prikk/blob/main/ROADMAP.md#future-themes) and is not scheduled. [Platform Support](../reference/platform-support.md) states what each supported platform
guarantees, including two narrower guarantees on Windows.

## Put the binary on `PATH`

`cargo binstall`/`cargo install` place the binary in Cargo's own bin directory (`~/.cargo/bin` on
Linux/macOS), which is normally already on `PATH` once Rust is installed.

For a direct download, move the extracted `prikk` (or `prikk.exe` on Windows) into a directory
already on your `PATH` — `~/.local/bin` is a common choice on Linux/macOS if it's already there —
or add the directory you placed it in to `PATH` yourself.

## Confirm it worked

```sh
prikk --version
```

prints the version you installed, in the form:

```
prikk <version>
```

Check that `<version>` matches the release you downloaded — that match is the actual
verification, not any particular number this page could show.

If the shell reports "command not found" instead, the binary is not on `PATH` yet.

## Next: Tutorial

Installing the binary configures no signing keys. Continue with the [Tutorial](tutorial.md) — it
walks through your first commit and seal, key setup included, using disposable public example keys.
[Security and Signing Setup](security-setup.md) covers real key handling once you are past that.

## Uninstalling

Prikk places nothing outside its own binary and, inside each repository it manages, a `.prikk`
directory. Delete the binary, and remove `.prikk` from any repository whose history you no longer
want tracked — there is nothing else to clean up.
