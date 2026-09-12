# RFC 135 — `prikk setup` on a directory that already holds a repository must refuse before touching anything

**Ruled:** RFC 147 §2d's per-site practice fixed the *class*; this fixes the *behaviour*, which RFC 135's
own §(c) — *"a flow, not storage"* — never specified for the re-run case. **Live.** Raised by the project
owner: *"`prikk setup` did not work properly."*

**Measured on the published 0.39.0 binary:**

```
$ prikk setup ./r                 # second time, same directory
initialized Prikk repository at ./r/.prikk
error: precondition not met: maintainer key id maintainer is already adopted with a different public key; …
rc=1
```

`setup.rs:74-78`: `create_dir_all`, `RepositoryLayout::init` (idempotent, so it succeeds), `println!
("initialized …")`, then key minting, then the trust step collides. **A command that prints "initialized"
and then fails has done something it should not have started.** Verified: no seed is printed and the
existing repository is undamaged — but two fresh keys were minted for nothing and the user is told the
directory was initialized when it already was.

## 1. The change

**Before `create_dir_all`, if `root.join(".prikk")` exists, refuse** — `PrikkError::Precondition`,
naming both routes:

> `./r already holds a repository; to use your existing keys here run \`prikk trust maintainer add\`
> (see \`prikk key public\`), or pick a different directory for a new project`

Nothing else runs: no `init`, no key generation, no seed file written, no `initialized` line.

## 2. Controls

1. **Re-run on an existing repository**: rc 1, the precondition message, **no `initialized` line**, **no
   new seed file** when `--*-seed-out` paths are given (assert they do not exist afterwards), **no 64-hex
   run in stdout**, and `prikk verify` on the repository clean and unchanged.
2. **Fresh directory** and **fresh nested directory that does not exist yet** both still work — the
   `create_dir_all` property RFC 135 added must survive.
3. **Perturb**: remove the check, control 1 fails on the `initialized` line; restore.
4. **`first-run.md` "A second project"** currently documents this half-run as a hazard clause — **it
   moves in the same round** to say `setup` refuses. `troubleshooting.md` gets the new message with the
   old wording named, per the established pattern.
5. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; cross-target: state the outcome from this
   round's own diff.

## 3. Not in this round

- No change to `prikk init`'s own idempotence; this is `setup`'s flow only.
