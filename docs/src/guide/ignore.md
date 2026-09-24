# Ignoring Worktree Paths

`.prikkignore` (since 0.29.0) is a plain file at the repository root. Each non-blank line is one
rule: a literal repository-relative path prefix.

```
target
build/output
```

`target` matches `target` itself and everything under `target/` — never `target2` or `targetfoo`.
Matching is by whole path component, not by string prefix, so a rule can never accidentally cover a
differently-named sibling.

## What the syntax deliberately does not have

**No globbing, no negation, no comments, no per-directory files.** `*.log` does not work. This is a
stated limit, not a first step toward gitignore compatibility — RFC 124's own reasoning is that a
syntax which *nearly* matches gitignore's semantics would be worse than one that obviously does not
attempt to.

## Where it binds

`.prikkignore` is consulted in exactly three places: [`prikk commit`](patches/worktree-patch.md)'s
worktree walk, [`prikk worktree-status`](worktree-status.md)'s untracked-path scan, and the worktree
side of [`prikk diff`](diff.md), which reads the worktree the way `commit` does. A rule that matches
keeps a path out of discovery in all three.

**Nowhere else.** Applying a patch, replaying history, verifying a repository, and materializing a
checkout all ignore `.prikkignore` entirely. This is deliberate: if those steps consulted it, two
repositories holding the same signed history but different `.prikkignore` files could disagree about
what that history means — the ignore file governs what gets *proposed* for authoring, never what
already-sealed history *is*.

**A rule can never hide an already-tracked path**, or a path lying under one. Adding a line to
`.prikkignore` cannot make `commit` read an existing file as missing and author a deletion for it —
only untracked, undiscovered paths are affected.

One practical consequence worth stating: an ignored directory is skipped without being opened or
descended into. That is not only a speed benefit. `commit`'s own worktree walk fails closed on any
symlink or unsupported entry it encounters, and a real `node_modules/`-style directory is typically
full of exactly those — without skipping the whole directory rather than each file inside it,
ignoring it would not actually let such a worktree commit at all.

## A malformed file refuses; an absent one does not

A `.prikkignore` that is not valid UTF-8, or contains a line that is not a valid repository-relative
path once a trailing `/` is stripped, is refused outright — `commit` and `worktree-status` both fail
closed rather than silently treating it as empty. No `.prikkignore` at all is not malformed: it means
no rules, and every repository created before 0.29.0 behaves exactly as it always has.

## It is an ordinary tracked file, not configuration

There is no separate configuration mechanism for it. `prikk config` (since 0.47.0) exists, but its
one key bounds the size of incoming objects and has nothing to do with exclusion; a general
configuration file remains a deliberately deferred, future increment. `.prikkignore` is committed and signed like any other
worktree file, and it travels through `bundle export`/`import` and `sync` the same way. A repository
you receive carries the sender's own rules, and those rules shape *your* future commits from that
point on — they say nothing about, and change nothing in, the patches you already received.

## Limits

- **No globbing, negation, or per-directory files.** One flat list of literal path prefixes at the
  repository root is the whole mechanism.
- **A file swept into history by mistake still cannot be removed later.** `.prikkignore` prevents
  future capture; it does nothing about what a worktree already committed before the rule existed.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| `.prikkignore` rules are literal repository-relative path prefixes, matched by whole path component, with no globbing or negation. | [`ignore.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ignore.rs) |
| The rule set binds only at `commit`'s worktree walk and `worktree-status`'s untracked scan — nowhere else. | [`ignore.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ignore.rs), [`worktree_files.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_patch/node_authoring/worktree_files.rs), [`worktree_status.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_status.rs) |
| An already-tracked path, or one lying under an already-tracked path, is never hidden by a rule. | [`ignore.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ignore.rs) |
| A malformed `.prikkignore` is refused; an absent one means no rules and no behavior change for an existing repository. | [`ignore.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ignore.rs) |
| `.prikkignore` is an ordinary tracked, signed worktree file and travels through bundle export/import and sync like any other. | [`ignore.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ignore.rs) |

## Provenance

This guide documents RFC 124's worktree ignore mechanism
(`rfcs/done/124-worktree-ignore-mechanism.md`), shipped in 0.29.0. It is documentation-only and adds
no behavior: `.prikkignore`'s syntax, binding sites, and limits are exactly as `ignore.rs`'s own
module doc records them.
