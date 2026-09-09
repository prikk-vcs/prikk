# Changelog

## Unreleased

### Added — `prikk mv <old> <new>`: declared move and rename authoring

The first prikk command that authors a `RenamePath`. Renames are never inferred from content — the
machine never compares bytes to guess that an untracked file is really a missing one, moved; a
rename only ever comes from this explicit declaration. `prikk mv` keys its behavior on existence
only: if `<old>` exists and `<new>` does not, it performs the move and records the declaration; if
`<old>` is already gone and `<new>` exists (the ordinary flow of running a shell `mv` and only
afterward remembering to tell prikk), it records the declaration alone, touching no bytes; if both
or neither exist, it refuses. The declaration is durable and authored into the next `prikk commit`,
not immediately — `prikk worktree-status` shows every live declaration, in prose and in
`--format json` (also new). Renaming a node more than once before a commit collapses to its net
move (`mv a b` then `mv b c` authors one `RenamePath`, `a -> c`, never two hops); a declaration that
returns to its starting point is dropped, authoring nothing. A worktree that contradicts a live
declaration at commit time refuses the whole commit, naming the declaration, rather than silently
dropping it. This is the first sealed prikk history to ever contain a rename.

Every declaration that resolves to something other than the rename it asserted is now named, with
what it became: `prikk commit` prints one line per such declaration alongside the operations it
authors (a destination deleted or excluded by `.prikkignore` nets to a plain deletion; a source that
was never a tracked node has nothing to rename), and `prikk mv` itself discloses a round trip that
nets to no move, since that one is resolved immediately, not at the next commit. Every outcome was
already correct; only the silence about it was fixed.

`prikk commit` may now also print a hint for a plain shell move it was never told about: when a
commit's own delete and create exactly match one deleted path's content to one created path's
content, and the pairing is unambiguous, it prints `hint: looks like <old> moved to <new>; \`prikk
mv\` would have preserved its identity` — visually and grammatically distinct from a disclosure
line (unindented, conditional tense, no `declaration` prefix), printed after every operation and
disclosure, never interleaved with them. The hint authors nothing and never changes what a commit
authors; it may also be wrong, which costs a line of output, not a fact in history. A declared move
never reaches it (it already authored a `RenamePath`); an ambiguous set (more than one deleted or
created path sharing the same content) prints no hint rather than guess a pairing; more than five
unambiguous candidates in one commit print a single summary line instead of one per pair.

### Added — `rename-destination-conflict`: a thirteenth conflict witness, not yet reachable end to end

**`prikk merge-evidence` cannot produce this witness yet** — whole-sequence confluence checking, the
path that command uses, still defers on any operation sequence containing a rename before the new
classification runs (see the patch-algebra reference page). What changed is the algebra beneath it:
it now classifies the case only declared renames make reachable — two sides each renaming the
*same* node to two *disjoint* destinations. Distinct from the existing
`same-path-create` witness by design — one node with two competing paths, versus two nodes
competing for one path — and its resolution is the mirror image: *choose which destination wins*,
not *choose which node wins*. A rename onto a path a different node's own operation also claims
still classifies as `same-path-create`, unchanged. Classification only; prikk still refuses to
author a conflict resolution automatically.

## 0.37.0 — 2026-09-09

### Added — `prikk bundle preview --input <file>`: what a bundle would do to your repository

Until now, learning what a bundle would do to your repository required `bundle import` first —
admitting its objects into your object store before you could ask. `bundle preview` answers the
same question without writing anything: no objects, no refs, no received pointer, no trust state,
no cache. It reports, at the node level, which paths the bundle would create, delete, edit, or
change the permissions of; whether the bundle's history connects to yours at all (a bundle with no
shared ancestry is a legitimate, reported answer, not an error); whether applying it would conflict
(also a result, not a failure — a conflicting or disconnected bundle still exits `0`); and who
sealed it, with the same "continuity only, not a trust decision" disclaimer `bundle import` already
prints for recorded signer material. `--format json` makes all of this machine-branchable. Until
rename authoring lands, a moved path previews as a delete plus a create, and the output says so
rather than let a reader infer intent the repository does not yet record. A local ref that has
never been published — the state before your first `seal`, and the moment you most need to ask
what a bundle would do — now reports `no-local-history` with the bundle's whole content as
`created`, rather than the integrity error it returned before this line landed.

### Added — `prikk checkout --patch-plan --format json`: content at a replayed point

`checkout --patch-plan` already reconstructs every file's bytes to prove a replay, then discarded
them. `--format json --content-path <repo-relative path>` (repeatable) now reports that content for
exactly the paths named — never the whole tree by default, and a bare `--format json` with no
`--content-path` reports metadata only. A requested path the replay never resolved degrades to
`not_found`, not an error; an unsupported operation anywhere in the walked chain still fails the
whole call. Binary content is never emitted as bytes — a binary file reports its blob id and
declared size only. The report's `coverage` field names which operation kinds this replay actually
applied and that the walk is single-parent; it is not a completeness promise. This adds no
comparison surface: prikk still has no `diff`, and comparing two arbitrary points remains a
separate, not-yet-open question — the requesting side takes on that comparison itself.

### Added — measured scale limits, on the Current State page

The documentation's Current State page previously listed only missing *features* under "not a good
fit yet"; it now also says what happens as a repository grows. Sealing cost rises quadratically with
history depth, checkout and merge-evidence both cost roughly `depth^1.45`, and incremental commit
memory is flat to a few thousand tracked files before growing at roughly 1.7 KiB per file — so cost
tracks history depth rather than repository size: a wide, shallow repository is cheap to work with
and a deep one is not. Each figure is published with the evidence behind it, including which of them
rests on the thinnest run, and the deep-history projection is labelled a projection rather than a
measurement.

### Changed — the project's documentation and homepage move to `prikk.org`

`prikk.org` is now the project's permanent home: the landing page, the guide, and the crate's own
`homepage` field. The old `prikk-vcs.github.io/prikk/` address still redirects, so existing links
and bookmarks keep working.

## 0.36.0 — 2026-09-08

### Added — `prikk show`: what a block or patch changed

`prikk show <block-id|patch-id> [--format json]` renders what a sealed block or patch changed:
each operation, the path(s) it affects, and the content it changed. A text edit shows its exact
before and after span, never a synthesized line-oriented diff — prikk's edits are content-anchored
spans, not line ranges. There is no `prikk diff`: comparing two arbitrary points in history is a
separate, materially more expensive question this command does not answer. If a referenced blob is
simply absent from the object store — the ordinary case being a deleted file whose most recent edit
left its old identity unbacked by design, not repository damage — that one piece of content reports
as a named "unavailable" state rather than failing the command; every other operation still renders
and the command still exits `0`. A referenced blob the object store reports as damaged (a
content-hash mismatch, a stored type disagreement, a malformed payload, or a `SNAPSHOT`-kind blob
named where file content belongs) is different: `show` propagates it and the command fails.

## 0.35.0 — 2026-09-06

### Added — `prikk status --format json`, and which patches a seal will freeze

`prikk status` could say how many patches were queued, but not which ones. `--format json`
(`status-report-v1`) now carries everything the prose form does, plus the queue itself: for each
queued patch, in queue order, its patch id, its operations' kinds, and the paths those operations
affect. This is for knowing *which* patches a seal will freeze, not only how many — the same
information a seal ceremony needs to ask for informed consent before an irreversible act, not just
a count of how much is about to happen. Bare `prikk status` is unchanged and pays nothing new; the
queue is only resolved when `--format json` actually asks for it.

### Changed — six more refusals now say `precondition not met`, not `lock conflict`

A full active-patch queue, an active WAL owned by the wrong ref for a commit, an incomplete ref
publication, a non-empty active WAL at rollback-draft or seal-from-accepted time, and an active WAL
holding more than a rollback draft at verify time were all reported as `lock conflict: ...`. None
of them involve a lock: nothing is held, no other writer is racing, and waiting never made any of
them go away — only running `seal`, `doctor`, or retrying with the right ref did. They now read
`precondition not met: ...`, matching what was actually wrong and what actually fixes it.

## 0.34.0 — 2026-09-06

### Added — `prikk trust maintainer list` and `check`: asking what a repository trusts

Until now there was no way to ask which MAINTAINER keys a repository currently trusts — a caller
had to attempt a seal and see whether it failed. `prikk trust maintainer list [--format json]`
enumerates every currently adopted key, in adoption order; `prikk trust maintainer check --key-id
<ID> [--format json]` answers whether one specific id is adopted. Both read a policy already
loaded on every seal; neither adds any new state, and neither changes adoption, revocation, or
what `seal` requires.

**`check` exits `0` whichever way the question resolves.** *"Key X is not trusted"* is a successful
answer, not an operational failure — exiting `1` for a negative answer would file a successful
query as a refusal, the exact conflation a machine-readable exit code exists to avoid. `1` and `2`
keep their usual meanings: an unreadable policy is `1`, a missing or malformed `--key-id` is `2`.

### Changed — `trust maintainer add` and `setup` print a derived count, not a literal

Both used to print `policy: required=1` after adopting a key — a literal, from a policy that has
no such field (`MaintainerTrustPolicy` holds a `Vec` and nothing else; trust is any-of-N by
construction). They now print `adopted maintainer keys: <n>`, where `<n>` is read back from the
policy and changes as more keys are adopted. `prikk trust maintainer list` already states what
trust means; this line's job is only to say how many.

## 0.33.0 — 2026-09-06

### Added — `prikk setup` and `prikk key`: a first repository without inventing anything

**Before this release a new user could not reach a sealed commit at all.** It required two 32-byte
seeds invented by hand and a public key derived from one of them — and **no command derived a public
key**. There was no way to do it with prikk.

```console
$ prikk setup ./my-repo
initialized Prikk repository at ./my-repo/.prikk
trusted maintainer key: maintainer
policy: required=1

export these before committing:
  export PRIKK_AUTHOR_KEY_ID="author"
  export PRIKK_AUTHOR_SEED="..."
  export PRIKK_MAINTAINER_KEY_ID="maintainer"
  export PRIKK_MAINTAINER_SEED="..."
note: at least one seed above is now in your terminal scrollback -- treat it as a secret

next steps:
  prikk commit -m "<message>"
  prikk seal --allow-no-audit  # no audit trust policy is configured yet; see `prikk seal --help`
```

**Following that output word for word reaches a sealed, verified commit.** Measured on a clean path,
the number of unfamiliar steps drops from **eleven to five**.

- **`prikk setup [<path>] [--author-seed-out <path>] [--maintainer-seed-out <path>]`** — creates the
  repository, generates both keys, registers the maintainer key, and prints what you need next. It
  **shows the trust decision it makes**: registering a maintainer key is a trust act, and a one-shot
  flow that performed it invisibly would teach you that the step is a formality.
- **`prikk key generate [--out <path>]`** — a fresh seed from the OS CSPRNG, with its public key and
  the exact next commands.
- **`prikk key public --seed-env <NAME>`** — the public key for a seed you already hold.

### How prikk handles secrets, stated because it is a deliberate limit

**prikk never invents a location for key material, never reads one back, and never manages its
lifecycle.** It has no keystore and will not gain one.

- `--out` and `--*-seed-out` write a seed **only to a path you name**, mode `0600`, refusing to
  overwrite an existing file and refusing any path inside `.prikk/`.
- **When you give an output path the seed is never printed** — the printed `export` line reads
  `"$(cat <path>)"`, so the secret reaches neither your scrollback nor your shell history. Without a
  path the seed is printed and prikk says so plainly.
- **A seed is never accepted as a command-line argument**, only through an environment variable you
  name. `/proc/<pid>/cmdline` is world-readable on Linux and shells record arguments in history.
- **`--out` is refused on Windows**, because prikk cannot set restrictive permissions there without
  facilities it does not use. Use `prikk key generate` without a path and place the seed yourself.

Durable configuration for policy settings (`PRIKK_ACTIVE_PATCH_LIMIT` and the rest) is **deliberately
not part of this** — see [RFC 135](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/135-first-run-entrance-and-configuration.md).

### Changed — two error messages changed their classification prefix

Reported by an external front-end (stikk) matching on our error text: two refusals were reported
under the wrong error class, because each was built from whichever `PrikkError` variant happened to
be nearest the call site rather than one that describes the condition. Both refusals were already
correct; only the leading word was wrong, and a caller matching on it could reasonably conclude the
wrong thing about what to do next.

```
$ prikk commit --from-worktree --ref heads/other -m "x"
-error: lock conflict: active WAL is owned by heads/main; requested ref heads/other
+error: precondition not met: active WAL is owned by heads/main; requested ref heads/other

$ prikk commit --from-worktree -m "nothing changed"
-error: invalid name: worktree has no node-addressed changes to commit
+error: precondition not met: worktree has no node-addressed changes to commit
```

Neither was ever a lock (nothing is held, no other process is racing this one) or a name-validation
failure (no name is involved). Both are the caller asking for something the current state cannot
satisfy — waiting does not help, only changing the request does — the case `prikk-error`'s new
`Precondition` variant (added non-breaking, since `PrikkError` is `#[non_exhaustive]`) now names
directly. Anything matching on the old `lock conflict:` or `invalid name:` prefix for either of these
two specific messages needs to match `precondition not met:` instead. No other error site, no exit
code, and no command's control flow changed.

## 0.32.0 — 2026-09-05

**Read this before upgrading one machine and not another.** Nothing about using `prikk` changes —
no command, flag, or exit code differs from `0.31.1`. **But repositories written after this change
cannot be read by earlier releases.**

### Breaking change — repositories written here are not readable by `0.31.1` or earlier

`prikk commit -m <message>`'s message is no longer discarded: it is now signed, identity-bearing
evidence, recorded on every `Patch` at **schema 4**, which earlier releases do not admit. This
applies to **every commit**, not only ones that carry an unusual message — `-m` was already
mandatory, so every patch this release authors carries one.

An older `prikk` reads such a repository, or imports a bundle exported from one, and refuses:

```
error: integrity error: format-2 patch does not accept envelope schema 4 (accepted: [1, 2, 3])
```

A bundle offered directly (bypassing repository-level schema admission) refuses earlier still, at
decode:

```
error: malformed persisted data: invalid PatchPurpose canonical form: canonical encoding error: unknown PatchPayload field tag: 6
```

**It fails closed with an accurate message — nothing is corrupted and no history is lost** — but the
older build cannot proceed. Both the local case and the `bundle export` → `bundle import` case were
demonstrated against a `0.31.1` build, not assumed.

**Upgrade every machine that shares a repository before committing with this release.**

**This direction is the one the compatibility contract does not promise.** Repositories written
before this release keep working exactly as they did — a `0.31.1` build still reads and verifies
them cleanly, confirmed rather than assumed.

### Why the schema changed

`prikk commit -m <message>` validated the message, then dropped it — a repository whose whole claim
is that it is evidence could not answer "what was this change?" The message is now an optional,
identity-bearing field on `Patch` (tag 6), mirroring `TagPayload.message`, and is shown per patch
under each block in `prikk log`. A patch written before this change carries no message and shows no
message line — absence, not a placeholder.

Raised by the external architecture audit of 2026-08-31; designed and ruled on in
[RFC 123](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/123-commit-message-and-authorship-metadata.md).

### Changed

- `prikk commit`'s interim note that the message is "validated but not stored" is removed — it is
  stored now.
- `prikk log` prints one line per patch that carries a message, under its block.

## 0.31.1 — 2026-09-05

**Nothing about running `prikk` changes, and this release cannot strand a machine.** No command,
flag, exit code, message, object format or on-disk layout differs from `0.31.0`, and repositories
written by `0.31.1` are readable by `0.31.0` — the opposite of the previous release, which was
one-way. Not one line of runtime source changed between the two tags; only crate manifests and one
test file did.

### Why it exists — the published crates were miscatalogued

Every crate published up to and including `0.31.0` carried no categories, no keywords, and no
`documentation` URL. crates.io renders a crate's page from its **published** manifest, so the fix had
to travel in a release; there was no other way to deliver it.

All eight published crates now carry:

- **categories** — `development-tools`, `command-line-utilities` (checked against the live crates.io
  category list; there is no version-control category anywhere in it)
- **keywords** — `vcs`, `version-control`, `dvcs`, `patch`, `merge`
- **documentation** — a per-crate `https://docs.rs/<crate>` link
- **homepage** — previously set on one crate of eight

### The project has a front page

`https://prikk-vcs.github.io/prikk/` now serves a landing page, and the documentation moved to
`https://prikk-vcs.github.io/prikk/docs/`. **Existing links to the book need `/docs/` inserting**;
links to the site root still work and now land on the front page.

This shipped when it was published, not with this tag — it is noted here because the URL moved.

### Also in this release

- `CONTRIBUTING.md`, describing how work is actually reviewed in this repository.
- A [Git → prikk mapping page](https://prikk-vcs.github.io/prikk/docs/reference/git-mapping.html), a
  command-correspondence table plus the four places the two models genuinely differ.
- A slimmer `README.md`, with the command surface, current state and repository layout routed into
  the book.
- The documentation-currency gate now reads HTML code context, so the landing page's commands are
  checked against the live command registry like every other declared page.

## 0.31.0 — 2026-09-04

**Read this before upgrading one machine and not another.** Nothing about using `prikk` changes —
no command, flag, exit code, or message differs from `0.30.0`. **But repositories written by this
release cannot be read by older ones.**

### Breaking change — repositories written by 0.31.0 are not readable by 0.30.0 or earlier

Every patch this release authors is recorded at **`Patch` schema 3**, which earlier releases do not
admit. This applies to **every commit**, not only ones that edit text.

An older `prikk` reads such a repository, or imports a bundle exported from one, and refuses:

```
error: integrity error: format-2 patch does not accept envelope schema 3 (accepted: [1, 2])
```

**It fails closed with an accurate message — nothing is corrupted and no history is lost** — but the
older build cannot proceed. Both the local case and the `bundle export` → `bundle import` case were
demonstrated against a `0.30.0` build, not assumed.

**Upgrade every machine that shares a repository before committing with `0.31.0`.**

**This direction is the one the compatibility contract does not promise.** `prikk` guarantees that
any release can read every object any prior release wrote, and that identity and signatures never
require migration — **backward, not forward.** Repositories written before this release keep working
exactly as they did, and the operations inside them still resolve through their original scheme,
permanently.

### Why the schema changed

A text edit records *which* span it replaces. Until now that identity included the span's **position
among identical occurrences**, recomputed against whatever the file looked like at the time — so an
edit to one of two identical passages could renumber the other. Schema 3 replaces that with an
identity derived only from content and surrounding context, and guarantees uniqueness when the edit
is authored rather than guessing at replay.

**The old behaviour was not reachable through ordinary use** — every edit is authored against the
result of the ones before it, which kept the numbering consistent — and it was recorded as a known
limitation in `0.30.0`. This release removes the fragility rather than continuing to rely on that
invariant holding.

Found by this project's own patch-algebra property tests
([RFC 134](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/134-text-span-identity-under-composition.md)).

### Changed

- A sequence of operations that cannot compose is now refused as *"sequence operations do not compose
  against a shared baseline"* rather than reported as malformed evidence — the same refusal, named for
  what it is.

## 0.30.0 — 2026-09-04

**A verification and library-surface release.** Nothing about using `prikk` from the command line
changes except two help lines. The one breaking change is in the library API, and it is the kind that
makes *future* releases less disruptive rather than more.

### Breaking change — `PrikkError` (library only, no CLI effect)

- **`PrikkError` is now `#[non_exhaustive]`.** Code that matches on it must carry a wildcard arm.
  **This is the change worth having**: from here, adding a new error variant is no longer a breaking
  change for anyone matching on the type.
- **`Io(String)` became `Io { kind: Option<std::io::ErrorKind>, context: String }`**, and
  `From<std::io::Error>` now preserves the operating system's error kind instead of discarding it.
  `kind` is `None` at the sites that construct the variant by hand — which is most of them today, and
  is recorded honestly rather than filled with a placeholder.

**No message text and no exit code changed.** `Display` renders exactly as before, and the CLI
converts every error to a string before it reaches you, so this release is invisible from the command
line except for the help text below.

Crate source APIs remain an explicitly unstable compatibility surface
([release compatibility](https://prikk-vcs.github.io/prikk/docs/reference/release-compatibility.html)).

### Changed

- `prikk commit --help` and `prikk worktree-status --help` now mention `.prikkignore`, which shipped
  in `0.29.0` without appearing in either.

### Verification

None of this changes behaviour; it changes what the project can catch.

- **Ten doctests on the kernel entry points** — compiler-verified examples on `ObjectId`,
  `CanonicalWriter`, path validation, the Ed25519 surface, and `RefStore::publish`. The workspace
  previously ran **zero**.
- **Property tests for the patch algebra**, covering classifier conservatism and the difference
  between pairwise and full-order replay.
- **A `prikk-benchmarks` workspace member** carrying criterion, outside `default-members` so it
  reaches no product crate's manifest and no shipped dependency graph.

### Documentation

- **`.prikkignore` now has a guide page** describing its syntax, the two commands that consult it,
  the surfaces it deliberately does not affect, and its limits.
- **The architecture reference's `verify` cost section was corrected.** It described `verify` as
  roughly O(N³) — about 34 seconds at 160 blocks — which stopped being true on 2026-08-18. `verify`
  is linear: **27.04 ms at 160 blocks**, and the property is held by a gate.
- **A latent fragility is recorded**, in the architecture reference's known-costs table. A text
  span's identity includes its position among textually- and contextually-identical occurrences,
  recomputed against the buffer in front of it — so a sequence of edits authored against a *shared*
  baseline, rather than each against its predecessor's result, does not replay. It was found by this
  release's own new property tests. Tracked as RFC 134.

  **Corrected 2026-09-04, same day, before any wider claim rested on it:** this bullet first said
  *"`merge` can reach it"*. **It cannot.** Every `EditText` is authored against the text its
  predecessors produced, so the operations `merge` composes carry indices consistent with the replay
  that reproduces them — verified by building the exact two-commit case and replaying it, not by
  reading. **No user-facing path reaches this**; what remains is an unstated invariant that nothing
  checks, and a refusal reported as malformed evidence rather than as what it is.

## 0.29.0 — 2026-09-03

**Generated files can be kept out of a commit.** Until now `commit` scanned and signed everything in
the worktree — a limitation this project's own README named as a reason not to use it. `.prikkignore`
closes that gap. The rest of this release finishes the corrective program for the 2026-08-31 external
audit, **including a correction to something `0.28.0`'s own notes claimed and only half delivered.**

### Added

- **`.prikkignore` — a worktree ignore mechanism.** A file at the repository root, one rule per line,
  each a literal repo-relative path prefix. `target` matches `target` and everything under it, never
  `target2` or `targetfoo` — whole path components, so a rule cannot over-match a differently-named
  sibling. **There is no globbing, no negation, no comments, and no per-directory files**, and that
  is a stated limit rather than a first step: an ignore syntax that nearly matched gitignore's
  semantics would be worse than one that plainly does not attempt to.

  It binds at **discovery only** — `commit`'s worktree walk and `worktree-status`'s, and nothing
  else. Applying, replaying, verifying, and materializing history ignore it entirely, so two
  repositories with different ignore files can never disagree about the same signed history.

  **A rule can never hide a path that is already tracked**, or one under an already-tracked path, so
  adding a line cannot make `commit` see an existing file as deleted. **A malformed `.prikkignore` is
  refused rather than treated as empty.** No file at all means no rules: every existing repository
  behaves exactly as it did before.

  `.prikkignore` is an ordinary tracked file, not configuration — it is committed, signed, and
  travels through `bundle` and `sync` like anything else.

### Fixed

- **A full-disk stdout *and* stderr together no longer exits `101`.** `0.28.0`'s notes announced this
  fix and delivered half of it: the single-stream case was corrected, while
  `prikk verify >/dev/full 2>/dev/full` still panicked, because the error report itself panicked on
  its own failed write. The exit code now stays inside the ruled `0`/`1`/`2` vocabulary even when the
  message cannot be delivered anywhere.
- **`seal` and `merge` now report a bad flag as the usage error it is.** Both acquired the maintainer
  signing key before parsing their arguments, so `prikk seal --nonsense` with no key configured
  reported "maintainer signing is required" and exited `1`. It now exits `2` with the argument error,
  matching `commit` and `rollback-draft`.

### Changed

- A worktree path that is not valid UTF-8 is now reported as an invalid name rather than as a
  repository integrity failure. An empty path remains an integrity failure.
- Three internal `update_seq` increments now refuse rather than wrap on overflow — unreachable in
  ordinary use, changed for consistency with the ref-log's existing checked arithmetic.
- The workspace now denies `unwrap`, `expect`, and direct indexing in product code at build time
  rather than warning, the patch algebra gained oracle-backed property tests, and the three
  check-only CI workflows declare `contents: read` explicitly.

## 0.28.0 — 2026-09-02

**The command line now behaves the way a script expects.** An independent external architect audited
this project at `0.27.1` and found, among other things, that `prikk verify | head` panicked, that
`worktree-status` failed on every repository this tool can create, that a repeated flag silently took
the last value, and that a required commit message was validated and then discarded. This release is
the corrective program for that audit's first two bands. **Almost everything below is a behaviour
change, and each one is named here because `prikk` is pre-1.0 and this file is where such changes are
declared.**

### Breaking change — exit codes

`prikk` previously exited `0` or `1` and nothing else, so a flag typo, a dirty worktree, and a
corrupt repository were indistinguishable to a caller. The contract is now:

- **`0`** — the operation succeeded and did what was asked.
- **`1`** — operational failure: verification findings, integrity failure, a refusal, a dirty
  worktree.
- **`2`** — usage error: unknown argument, missing required flag, malformed value, duplicate flag,
  detected before any repository work begins.

A separate code for "findings" was considered and refused: `verify --format json` already carries
that verdict, structured and three-valued, and duplicating a lossy subset of it into an integer would
give two sources of truth for one question.

**Scripts that branch on the old two-code behaviour need review.** Specific movements:

- `prikk unlock` **declining to clear a lock now exits `1`, not `0`.** It previously reported success
  while the lock was still held — and because its confirmation prompt reads "no" from an empty
  stdin, a non-interactive `prikk unlock … && proceed` was told everything was fine when nothing had
  happened.
- **An unrecognized argument is now refused with `2` instead of being ignored.** `prikk status
  --nonsense` previously printed a normal report and exited `0`; `prikk init a b` silently discarded
  `b`.
- **A repeated flag is now refused with `2` instead of silently taking the last value.**
  `prikk bundle export --ref heads/main --ref heads/other` previously exported `heads/other` with no
  indication which ref it had chosen. Nearly every value-carrying flag in the tool had this shape.

### Breaking change — the format boundary refuses more

Two classes of input that the encoder would never produce are now rejected at decode as well as at
encode, so a hostile or corrupt artifact cannot enter through a path authoring would have refused:

- **File modes must be `0o100644` or `0o100755`.** Materialization applies recorded modes through
  `fchmod`, which would otherwise honour setuid, setgid and sticky bits; the only guard was a check
  in a different subsystem at seal time.
- **Repository paths are length-capped** — 255 bytes per component (`NAME_MAX`) and 1024 bytes total
  (macOS `PATH_MAX`, the strictest total the tool can guarantee). Longer paths previously entered
  signed history and then failed to materialize with a raw OS error. Windows' legacy 260-character
  limit is *not* claimed: it depends on where the worktree root sits and cannot be bounded from a
  repository-relative length.

Repeated singular fields in a decoded record are also refused now rather than silently taking the
last one. **Verified against real committed repository fixtures, including one written by `0.27.0`'s
own encoder: no existing history is refused by any of this.**

### Fixed

- **`prikk verify | head` no longer panics.** A closed reader is not a failure — the tool now exits
  `0` silently, on every platform rather than only where `SIGPIPE` exists. A genuine write failure
  (a full disk on a redirected stdout) still reports, and now exits `1` instead of aborting.
- **`prikk worktree-status` works.** It had been comparing against a snapshot baseline that no
  command has produced since the patch-replay migration, so it failed on *every* repository this CLI
  can create. It now shares `commit`'s own baseline derivation — one computation backing both, so
  they cannot drift apart again — and it says when the active queue belongs to a different ref, since
  an "untracked" file there may be committed-but-unsealed work.
- **`0.23.0`'s changelog entry is restored.** The `0.24.0` version bump replaced its heading instead
  of inserting above it, which attributed the entire `prikk sync` release to `0.24.0`. A gate now
  checks that *every* released tag still has exactly one entry, not just the tag being cut.

### Added

- **Per-command help.** `prikk <command> --help` prints that command's own usage, derived from the
  same table the top-level help renders from. It works before a repository is opened, and anywhere in
  the argument list.
- **Seven flags the help never mentioned** are now documented: `verify --format json`,
  `verify --stop-on-first-error`, `unlock --force` (an alias of `--yes`), `doctor --repair-main-ref`
  (recognized, and always refused, with the reason), and `--message` as the long form of `-m` in
  `commit`, `rollback-draft` and `tag create`.
- **`prikk commit` now says that it discards your message.** The message is still required and still
  validated; it is not stored, it does not appear in `prikk log`, and persisting it is a later
  increment. It was silently dropped before.
- **A security policy.** `SECURITY.md` states where to report a vulnerability privately, what this
  project does and does not promise, and — plainly — that release-signer verification of a `prikk`
  binary is not yet available.
- **Backup and restore documentation**, plus a self-describing bundle manifest, offline
  `bundle verify`, and atomic collision-safe writes on the `bundle export` and `sync` output
  surfaces.

### Changed

- **The project moved to `https://github.com/prikk-vcs/prikk`.** Documentation now lives at
  `https://prikk-vcs.github.io/prikk/`, and the installer downloads from the new location. The old
  repository URL redirects; the old documentation URL does not.
- CI gained a scheduled advisory-database audit, a rustdoc lint gate, and a book build on pull
  requests.

## 0.27.1 — 2026-08-31

**A one-command install, and a beginner's path through the docs.** No library or command behaviour
changed in this release — `prikk` itself is byte-for-byte the same tool as `0.27.0`. This cut exists
so the installer has a release to attach to.

### Added

- **A shell installer and uninstaller**, attached to this release as `install.sh` and `uninstall.sh`
  with their own checksums:

  ```sh
  curl -fsSL https://github.com/nabbisen/prikk/releases/latest/download/install.sh | sh
  ```

  It detects the platform, downloads the matching archive, **verifies its SHA-256 and refuses to
  install if verification fails or if no checksum tool is present**, and puts the binary on `PATH`.
  `--version X.Y.Z` pins a release; `--prefix DIR` chooses the location. The download-then-inspect
  form is documented beside the pipe form. `uninstall.sh` removes the binary and the one marked
  `PATH` block it added, and nothing else. Linux (`x86_64`/`aarch64`) and macOS (Apple Silicon);
  Windows is refused with a message pointing at `cargo install` or the `.zip`.

  **What a passing checksum proves is integrity of transport, not authority of origin** —
  `release-signers.toml` is still empty and fail-closed, so no release yet satisfies the DC-35
  signer-authority audit. The installer says so itself when it finishes.

- **A beginner's on-ramp in the documentation** — a [tutorial](https://nabbisen.github.io/prikk/guide/tutorial.html),
  a [troubleshooting page](https://nabbisen.github.io/prikk/guide/troubleshooting.html), and an
  [FAQ](https://nabbisen.github.io/prikk/guide/faq.html), placed before the signing setup a reader
  previously met first. **The tutorial's commands are run by a test on every change**, so a release
  that breaks them fails CI rather than the reader.

- **Build-from-source guidance for platforms without a prebuilt binary** — other Linux architectures
  build with no reduction in capability; the BSDs compile but are read-only, since repository
  mutation is refused off Linux, macOS, and Windows.

### Fixed

Nothing — no behaviour changed.

## 0.27.0 — 2026-08-28

**`prikk unlock` no longer fails on the repository it exists to recover.** On `0.26.0`, a repository
missing `.prikk/refs/locks` — one directory — defeated the command you run precisely when a lock is
wedged. It is fixed, and `doctor` now names damage of that kind instead of leaving it to be inferred
from a command that died. Everything else is smaller.

### Fixed

- **`prikk unlock` tolerates a missing `refs/locks`.** It previously returned
  `i/o error: directory is absent: refs/locks` and cleared nothing.
- **`prikk verify` tolerates a missing `refs/tmp`.** It previously failed its `Refs` stage and, under
  `--stop-on-first-error`, halted every downstream stage — so one missing directory reduced the whole
  report to almost nothing.
- **`prikk doctor --repair-wal-tail`'s refusal no longer blames `verify`.** It said *"repository
  verification has errors"* for checks that are not part of `verify` at all, sending readers to look
  at output that had nothing to say.

### Added

- **`prikk doctor` reports every required directory that is missing or occupied by the wrong kind of
  entry**, sourced from the same inventory `init` creates from. Nothing checked these after a
  repository was created; the old failure was an accident of a directory read, not a check.
- **`prikk verify` states how many active sessions exist and that it covers the default one only, by
  construction** — in prose, and as an `active_sessions` object in `--format json`, so a CI gate can
  act on `count` versus `verified_count` rather than parse text. Existing keys are unchanged.
- **`prikk doctor` reports each active session's write-ahead-log and reference-name metadata state.**
- **`prikk doctor --repair-wal-tail` repairs each active session independently**, naming any it
  skipped and why, and exiting non-zero when it skipped one. Damage in one session no longer prevents
  recovering another.

### Changed — library API

`prikk-store`'s `DoctorIssue` gained an `active_session` field and `DoctorRepairReport` gained an
`active_repairs` field. **Both structs have public fields and no `#[non_exhaustive]`, so code
constructing either with a struct literal must be updated.** Reading them is unaffected.

### Groundwork, not yet usable

Most of this release is the mechanism for concurrent workspaces (RFC 108): the active-session
directory is fully general, every diagnostic surface handles more than one, and recovery is
independent. **No command creates a second workspace, and none of this is reachable yet** — naming,
command surface, and whether a workspace can be shared are all still undecided.

## 0.26.0 — 2026-08-26

**crates.io still serves `prikk`'s description as `"Prikk CLI initial scaffold."`, and
`prikk-store`'s as `"Prikk storage crate scaffold."`** — both fixed in the repository since, but a
published description only updates on the next publish, and the seven crate READMEs that render
beneath it carry the same stale text. This cut exists to correct that. Everything else is smaller.

### Added

- **Conflict witnesses now report a path.** Ten sites previously discarded one that was available
  — merge evidence now says *where* a conflict is, not only that one exists. This is the one
  behaviour change in this release, and it is why the version moves rather than the patch level.
- **An install guide**, which the documentation site never had: checksum verification per
  platform, `PATH` setup, confirming the install, and uninstalling.
- **Corrected crate metadata.** All eight published descriptions and seven library-crate READMEs
  rewritten to reflect current posture instead of scaffold-era placeholder text.

### Fixed

Nothing user-facing beyond the additions above.

**Everything else is documentation currency and test-gate hardening.**

## 0.25.0 — 2026-08-26

**`prikk verify --format json` is this release — the first machine-readable output the tool has
ever had, and the thing that makes a CI publication gate possible without grepping prose.**
Everything else is smaller.

### Added

- **`prikk verify --format json`.** Emits a `verify-report-v1` document: a schema version, a
  verdict (`ok` plus every currently-failing condition, derived from the same nine-condition
  declaration `verify`'s exit code reads — the two cannot disagree), and one entry per
  verification stage. A CI job can finally assert on `verify` without grepping prose, and the
  prose output is unchanged.
- **Conflict witness kinds now reach merge evidence.** Twelve typed kinds that previously never
  left `patch_algebra`'s internals — why two operations conflict, not only that they do — now
  appear on `MergeEvidenceDisplayItem` (see Breaking change, below).
- **An MSRV rise policy, documented and gated.** MSRV rises only when a dependency or language
  requirement forces it, never for convenience, and a rise is a minor-version event whose
  `CHANGELOG.md` entry names the requirement that forced it. Six live transcriptions (the
  toolchain pin, the CI job name, and prose/gate-command lines in two reference pages and
  `rfcs/EXECUTION-ORDER.md`) are now bound to `Cargo.toml`'s declared version and checked by
  `reference-check`. **MSRV stays at `1.85`** — this release does not raise it.

### Fixed

Nothing user-facing beyond the additions above.

### Breaking change

**All three changes below are API breaks. None is a format break — no repository written by any
prior release becomes unreadable by `0.25.0`, and `0.25.0` reads every prior repository exactly as
before.** They matter to code that links against `prikk-object`/`prikk-store` as libraries, not to
repository content.

1. **`ObjectType::ProjectGenesis` removed** (`prikk-object`). `ObjectType::from_code(0x0A)` now
   returns a retirement error (`"object type code 10 is retired (formerly project-genesis) and
   must never be reused"`) instead of `Ok(ProjectGenesis)`. Breaks any downstream exhaustive
   `match` on `ObjectType`, and anything naming the variant directly. **No repository can contain
   a `0x0A` object** — no code path in any released version ever constructed one — so nothing on
   disk is affected.
2. **`RepositoryVerification::has_blocking_defect()` removed** (`prikk-store`). It reported only
   two of the nine conditions under which `prikk verify` actually refuses a repository, and its
   own documentation invited callers to treat it as the complete answer. **The remedy**: read
   `prikk verify --format json`'s `verdict.ok`/`verdict.failed_conditions` (new this release,
   above), or `prikk verify`'s own exit code, either of which reflects all nine conditions.
3. **`MergeEvidenceDisplayItem` gained three public fields** (`prikk-store`): `witness_kind`,
   `witness_path`, and `witness_node_id`. The struct is not `#[non_exhaustive]`, so any downstream
   construction of one by struct literal (rather than by reading a `prepare_merge_evidence` result)
   breaks.

## 0.24.0 — 2026-08-25

**A trust gap open since DC-63 is closed: an untrusted signer is now refused at `tag create`,
`branch create`, and `branch close`, not just at `seal`.** `verify` gained one more coverage
surface — locally-published `Tag` signatures, checked against the same maintainer trust policy
those commands themselves enforce. `Patch`'s schema drops `parent_patch_ids`, a field every
construction site has written as inert since it shipped.

**Everything else is gates, tests, and documentation.** This is predominantly an assurance
release — internal command/documentation binding, a release-compatibility gate, and a reduced
release-policy tool — not new product capability, and it is described that way rather than
inflated.

**Read the breaking change below before you author a patch under `0.24.0` and then try to read the
repository with `0.23.0`.**

### Added

- **`verify` gained a `LocalTagTrust` stage.** A locally-published `Tag`'s own MAINTAINER
  signature is now checked against the repository-local trust policy, the same one `053e442`
  (below) gates `tag create`/`branch create`/`branch close` on. **A received, not-yet-adopted tag
  is deliberately exempt** — its signature is the sender's, under a key this repository has not
  adopted; `sync adopt-tag` creates a local, receiver-signed tag from it, which then *is* checked.
  This is a different trust model from `verify`'s author-signature check (below): an explicit,
  operator-managed policy (`add_trusted_maintainer`/`remove_trusted_maintainer`), not
  trust-on-first-use pinning.

### Fixed

- **`tag create`, `branch create`, and `branch close` now refuse an untrusted signer.** DC-11
  required this and DC-63 adopted it in words, but not in code — the gap has been open since
  DC-63. `seal` already enforced it; these three commands now match.
- **`doctor --repair-main-ref`'s refusal message no longer names a stale version or an
  unreachable format-1 scenario.**

### Breaking change

**A repository written by `0.24.0` cannot be read by `0.23.0`. The reverse is not true —
`0.24.0` reads a `0.23.0` repository fine.**

`0.23.0` admits `Patch` envelope schema `1` only. `0.24.0` admits schemas `1` and `2`, and
**every patch `0.24.0` authors is written at schema `2`** — `parent_patch_ids`, inert since it
shipped (`Vec::new()` at every construction site, read nowhere), is retired outright rather than
carried forward as dead weight. A `0.23.0` binary reading a schema-2 `Patch` fails with
`format-2 patch does not accept envelope schema 2 (accepted: [1])`.

**There is no in-repository remedy.** Do not downgrade to `0.23.0` after authoring patches under
`0.24.0` — there is no repair path back, the same as `0.23.0`'s own `Tag` break one release ago.
If you need a repository `0.23.0` can still read, keep authoring it under `0.23.0`.

### Known limitation

- **`verify`'s author-signature check remains trust-on-first-use**: it proves the same author
  signed as last time, not who that author is on first contact. Unchanged by this release, and a
  different trust model from `LocalTagTrust` above.
- **`verify`'s `LocalTagTrust` stage covers locally-published tags only.** A received, unadopted
  tag is exempt by design.
- **No prikk release passes the DC-35 signer audit** — `release-signers.toml` is empty and
  fail-closed. Unchanged by this release.
- **"Two machines" is exercised as two repositories**, not two hosts — file-based and
  channel-agnostic, but no cross-host test exists yet.
- **Negotiation is branch-scoped.** `remotes/*` is excluded structurally; a tag's deletion and
  movement do not travel — only its creation and adoption do.
- **No discovery, remote identity, or remote-tracking semantics.**
- **Tag adoption resolves by scanning local blocks**, measured superlinear — 12.6 ms over 500
  blocks, 86 ms over 2000.
- **`seal`'s cost is unchanged: O(N) reads per call**, so building N commits remains O(N²) in
  total reads.

### Why

This release closes a real enforcement gap (`tag create`/`branch create`/`branch close` trusting
an unchecked signer since DC-63) and extends `verify`'s coverage to a surface that was previously
unchecked (`LocalTagTrust`), rather than adding product capability. `Patch` schema 2 removes a
field that has never done anything since it shipped, verified rather than assumed to be inert
(`f1528b8`'s completeness guard, below). No `MILESTONES.md` status-claim criterion changes state
in this release.

### Verified rather than assumed

- **RFC 114 Gate A's completeness guard was corrected from `ObjectType`-granular to
  pair-granular** (`f1528b8`) — a second schema on an already-vectored type previously passed
  silently, which is exactly the shape `(Patch, 2)` above takes. **Observed failing in both
  directions**: an unvectored pair added, and a vectored pair removed.
- **RFC 119 track C's release-compatibility gate (G1) was demonstrated with reverted code-path
  mutations**, not a one-time measurement — the declared-break path and the undeclared-break
  path were each shown to fail for the real reason before being reverted.
- **The five trust-gated surfaces this release did not touch — `seal`, `merge`, and `sync`
  `build`/`seal`/`adopt-tag` — gained their own caller-level negative controls** (`aa1b25d`),
  closing the last gap in trust-gate coverage: every trust-gated surface now has a control proving
  an untrusted signer is refused at that exact call site, not just somewhere upstream.
- **RFC 118's command-documentation join gate is bidirectional**: it fails if a real CLI command
  has no matching documentation, and fails if documentation describes a command that does not
  exist.

## 0.23.0 — 2026-08-23

**History moves between repositories.** `prikk sync` negotiates what one repository has that another
doesn't, moves it as an artifact file over whatever channel the operator already has, and the receiver
verifies every byte on arrival. Tags travel in that artifact and are adopted under the receiver's own
key. Anyone who upgrades also gets two things that were silently missing before: `verify` now checks
every AUTHOR signature in the whole repository, not just the ones it happened to look at, and `verify`'s
cost stopped growing faster than the repository does.

**Read the breaking change below before you tag anything on 0.22.1 and then upgrade.**

### Added

- **`prikk sync`** — the full negotiation loop: `summary` → `compare` → `have` → `build` → `accept` →
  `pending` → `seal`, over `PSYNCSU1`/`PSYNCHV1` negotiation artifacts and the `PEXCH002` exchange
  artifact. Nine subcommands in total, including `tags` and `adopt-tag <name>` (below). **Prikk does not
  move the bytes itself** — the artifact is a plain file; the operator's own channel carries it, and
  confidentiality is that channel's property, not prikk's.
- **Tags travel and are adopted under the receiver's own key** — `sync tags` lists what a repository has
  received; `sync adopt-tag <name>` creates a local, receiver-signed tag from one. A tag names a **patch
  set**, not a block, because blocks diverge by design even when two repositories hold the same history.
- **`verify` checks every reachable Patch's AUTHOR signature, repository-wide** — including history
  received from another party, which previously read `Unverifiable` forever. This is
  **trust-on-first-use**: it proves the same author signed as last time, not who that author is on first
  contact — see the [trust and threat model](./docs/src/reference/trust-threat-model.md).
- **The repository-format contract is explicit and gated.** What is frozen forever (the object-id
  preimage, each shipped `(object_type, schema_version)` pair's canonical encoding, the signature
  preimage, the algorithm identifiers) versus what may still change behind a documented, tested migration
  path is now a real contract, held by CI rather than by intention — a `CURRENT_FORMAT_VERSION` bump
  cannot pass without migration coverage.

### Changed

- **`verify` is linear in history length**, not superlinear — 27.04 ms at 160 blocks, ×1.97 per doubling,
  down from ×3.51 before this release. Held by a gate that fails if the cost regresses, not by a
  one-time measurement.
- **Bundle import accepts both `PBNDL001` and `PBNDL002`** on import, restoring a format-migration path
  an interim change had briefly severed.

### Fixed

- **Bundle export previously failed on a tag ref**, with a misleading message. It now resolves the ref's
  second hop correctly and exports the `Tag` object itself.
- **Bundle import validates the whole transported author-key set atomically**, before recording any of
  it — closes a partial-write hazard where a hostile or malformed import could leave some keys recorded
  and others not.
- **`import_bundle` and `accept_exchange_artifact` both validate closure completeness before any
  write** — an import naming an object it never actually ships (a missing blob, a missing block parent)
  is now refused outright, instead of being accepted with a dangling reference invisible to `verify`.
- **`merge-plan`'s `ConfluentSubset` action text no longer says merge execution is unimplemented** —
  DC-74 shipped it in 0.19.0; the message had never caught up.

### Breaking change

**A `Tag` written by 0.22.1 will not decode under 0.23.0, and the reverse is also true.** `TagPayload`
gained two fields — `patch_set_digest` and `patch_count` (RFC 117) — added **in place at
`schema_version` 1**, not as a new schema version. `0.23.0` reading a `0.22.1` tag fails with
`Tag missing patch_set_digest`; `0.22.1` reading a `0.23.0` tag fails with `unknown Tag field tag: 6`.

**This surfaces in `prikk verify`, not only `prikk tag list`.** A repository written by 0.22.1 that
contains any tag will not verify under 0.23.0, and the error reads as malformed data rather than a
version mismatch, because that is what the decoder's own refusal says.

Prikk has not yet been used to hold production history. On that basis the owner ruled `Tag`'s schema
window closed rather than adding a second live schema, the same way `RefState` carries two (open and
DC-61-closed).

**A repository written by 0.22.1 that already holds a tag cannot be repaired under 0.23.0.** There is no
`prikk tag delete`, and `prikk tag create` refuses outright when a tag ref of that name already exists —
so the old, now-unverifiable tag cannot be removed to make way for a new one in the same repository.
**If you have a repository from 0.22.1 with a tag in it and intend to keep using it, keep using 0.22.1
for that repository, or start a fresh repository under 0.23.0.** There is no in-place remediation today.

### Known limitation

- **No prikk release passes the DC-35 signer audit** — `release-signers.toml` is empty and fail-closed.
  Unchanged by this release.
- **"Two machines" is exercised as two repositories**, not two hosts — file-based and channel-agnostic,
  but no cross-host test exists yet.
- **Negotiation is branch-scoped.** `remotes/*` is excluded structurally; a tag's deletion and movement
  do not travel — only its creation and adoption do.
- **No discovery, remote identity, or remote-tracking semantics.**
- **Tag adoption resolves by scanning local blocks**, measured superlinear — 12.6 ms over 500 blocks,
  86 ms over 2000.
- **`seal`'s cost is unchanged: O(N) reads per call**, so building N commits remains O(N²) in total
  reads. `verify` is linear now; `seal` is a different command and this release does not touch it.

### Why

Four of the six status-claim criteria (`MILESTONES.md`) were met since 0.22.1 and none had shipped:
sync exists (criterion 1), the format-stability question is answered (criterion 2), `verify` is not
superlinear (criterion 3), and `verify` checks author signatures repository-wide (criterion 5). This is
the release that makes those real for anyone who installs it, not only for anyone reading `main`.

### Verified rather than assumed

- **Twenty-one distinct security refusals were recorded across RFC 115 and RFC 116's own stage reviews,
  each with an observed-failing negative control** — trust never expands on receipt (no artifact can
  cause a maintainer key to be adopted), every byte is verified on arrival, and the receiver seals only
  under its own key.
- **`crates/prikk-cli/tests/rfc116_sync_cli.rs` drives the whole sync loop through the binary alone**,
  asserting by reading the receiver's own ref tip back rather than trusting an in-process return value,
  and additionally runs `prikk verify` on both repositories after every sync and asserts both pass — the
  load-bearing claim, with its own negative control (planting a defect in material the sync itself
  created makes the assertion fail).
- **A dedicated gate fails if `verify`'s cost ever regresses to superlinear again**, rather than relying
  on a one-time measurement staying true.

## 0.22.1 — 2026-08-17

**Downloadable binaries for macOS and Windows, and a release page that tells you what changed.** No
product code changed in this release — only how it is distributed and described.

### Added

- **Prebuilt binaries for macOS (`aarch64-apple-darwin`) and Windows (`x86_64-pc-windows-msvc`)**,
  alongside the two Linux targets. Windows ships as a `.zip` containing `prikk.exe`; macOS and Linux as
  `.tar.gz`. Each archive still carries its own `.sha256` checksum and a `.build-info.txt` recording the
  exact toolchain and command that produced it.

  These are the two triples this project's CI actually exercises. `x86_64-apple-darwin` is deliberately
  **not** published: no build of it has ever run here, and shipping a binary whose platform has never been
  tested would be a claim we cannot support.

- **`cargo binstall prikk` now has something to fetch on all three platforms**, rather than falling back
  to a source build off Linux.

### Fixed

- **Release pages describe their release.** Every prikk release until now published the same static
  page, so a visitor could not tell what a version contained. Each release page is now assembled at
  publish time from that version's own `CHANGELOG.md` entry.
- **A false statement is gone from the release pages.** The static page claimed *"repository mutation is
  Linux-only project-wide"* — true when it was written, and false from 0.21.0 onward, which is to say it
  was wrong on the two release pages whose entire content was making mutation work on Windows. The
  platform list is now derived from the artifacts actually published, so that class of drift cannot
  recur.
- **The documentation badge** in `README.md` points at the documentation site.

### Known limitation

**macOS binaries are unsigned.** Gatekeeper will warn on first run; right-click and choose Open, or clear
the quarantine attribute with `xattr -d com.apple.quarantine <path>`. Notarization needs an Apple
Developer identity and is a stated gap for a future increment, not an oversight. This sits alongside the
release-authority position already published with every release: **no prikk release passes the signer
audit**, and a checksum proves integrity of transport, not authority of origin.

## 0.22.0 — 2026-08-17

**Windows catches up on two capabilities, and the durability claims made for it in 0.21.0 are now
tested rather than argued.** No repository format change, no command-surface change, nothing to do
before upgrading.

**If you use Linux or macOS, this release changes nothing you can observe.** Everything below is either
Windows-specific or internal. What you get is a more thoroughly tested implementation of what 0.21.0
already shipped — the Windows suite grew from 909 tests to 956 — and that is worth saying plainly rather
than dressing up.

### Changed

- **`prikk unlock` now reports real process liveness on Windows.** It previously returned *unknown* for
  every recorded process id, so recovering a wedged repository there meant deciding alone whether the
  process that left the lock was gone. It now answers, using the same asymmetry the other platforms
  use: a process that exists but cannot be queried is reported as **running**, because that is what
  "the operating system found something to check permissions against" means. **The check remains
  advisory** — trusted to refuse, never to authorise. A negative result is still not proof a lock is
  safe to clear.
- **Anchor identity uses Windows' 128-bit file identifier where the filesystem provides one.** The
  64-bit identifier this previously relied on is documented by Microsoft as *not unique on ReFS*, which
  is the filesystem behind Windows 11's Dev Drive — a location Microsoft recommends for source
  repositories. Filesystems that do not support the 128-bit form fall back to the previous behaviour,
  and the two forms cannot be compared against each other.

### Verified rather than assumed

None of this changes behaviour. It changes what is known about behaviour.

- **Crash-safety on Windows is demonstrated.** 0.21.0 shipped a durability implementation whose
  crash-safety rested on the code being correct. Fault injection is now wired into the Windows
  durability path, and every one of the nine controls has been observed to fail when the guarantee it
  covers is removed — which is the only way to show a durability test is testing durability.
- **All nine durability guarantees are classified per platform**, with either a demonstrated control or
  a specific stated reason none can exist. Two of them have no Windows equivalent at all, and the
  reference documentation now says which and why.
- **Two guards that protected nothing observable are now proven.** Both were found the same way: by
  disabling them and watching the test suite stay green.
- **Two internal methods were retired** — unreachable from any command, weaker on Windows than
  elsewhere, and therefore a documented guarantee nothing could rely on.

### Why

0.21.0 made Windows a mutating platform. That release named four places where Windows offered a
narrower guarantee than Linux or macOS. This release closes two of them and replaces argument with
evidence for the rest. **The remaining two are still named** in the
[platform support reference](./docs/src/reference/platform-support.md), including the resolution race
Windows cannot close by construction — it has no `openat` equivalent — and prikk does not claim
otherwise.

## 0.21.0 — 2026-08-16

**Windows becomes a mutating platform.** Until now prikk could only read a repository on Windows;
authoring, committing, and checking out required Linux or macOS. No repository format change, and no
action required for existing repositories.

### Added

- **Repository mutation on Windows.** The full `DurabilityContract` is implemented for Windows, so
  commands that write now run there. This is demonstrated rather than asserted: CI authors a repository
  on Linux, mutates it on Windows, and verifies it back on Linux, requiring the resulting object ids to
  be **byte-identical** to those produced by the same mutation on Linux. Prikk's central promise — that
  anyone can verify anyone's history — now holds across platforms by test, not by assumption.
- **A Windows test suite.** `cargo test --workspace` runs on Windows in CI for the first time. Windows
  had previously only been compiled, never exercised.
- **`prikk-ffi`**, an eighth published crate. It holds prikk's only `unsafe` code — two Win32 calls used
  to identify and follow a repository's anchor directory. Every other crate in the workspace forbids
  `unsafe` outright, and a release gate enforces that exactly one crate may hold the exemption.

### Changed

- **A replaced repository directory no longer redirects reads and writes.** If a repository's directory
  is renamed aside and a fresh one created at the same path, prikk previously followed the new directory
  on Windows — silently reading and writing an impostor while reporting success, including for objects,
  refs, and the write-ahead log. Prikk now retains a handle to the directory it validated and works
  against that object, so `.prikk` being moved is followed rather than lost, and replacing the worktree
  root is refused by the operating system outright. Linux and macOS were never affected.

### Windows limitations, stated rather than left to be found

`docs/src/reference/platform-support.md` carries these in full, with the reason for each.

- **`prikk unlock` gives no positive liveness signal on Windows.** Every stale-lock decision there rests
  on the operator. The check never authorises clearing, so this is safe — but it is materially weaker
  than on Linux and macOS.
- **A repository's directory cannot be renamed or moved while a prikk command holds it open.** This is
  the mechanism above, seen from the other side; it lasts one command, and prikk has no daemon.
- **Path resolution has a narrower guarantee.** Windows offers no way to open a child relative to a
  directory handle, so a concurrent process substituting a reparse point mid-walk is not provably
  defeated, as it is on Linux and macOS. An already-planted one is caught on every platform.
- **On ReFS — including Windows 11 Dev Drive — the 64-bit file index is not unique**, per Microsoft's own
  documentation. Prikk uses it as a confirming check rather than as the mechanism that decides where a
  write goes, which is why this weakens the diagnosis and not the guarantee.

### Why

Windows support was blocked for a year by a durability problem, not a porting one: prikk's
content-addressed storage created a new filename per object, and Windows has no equivalent of `fsync` on
a directory to make a new name durable. 0.20.0's container storage removed the requirement by making
appends to existing names the only durability-bearing write. This release is the port that became
possible once that was true.

## 0.20.0 — 2026-08-16

Storage rebuilt. **Every repository created by 0.19.0 or earlier must be re-imported.**

### Breaking

- **Repository format 6. Formats 1 through 5 are rejected at open**, each with an error naming the format
  found. There is no in-place migration and no read-only fallback for a superseded format. To carry work
  across: `prikk bundle export` on a version that still opens the old repository, then `prikk bundle
  import` into a new one. Editing `FORMAT` does not change the on-disk shape it describes.
- The format moved five times inside this release — 2→3 (object containers), 3→4 (ref containers), 4→5
  (trust containers, received-ref index, active ref metadata), 5→6 (compaction slots and generation
  logs). Prikk is early implementation software and has not committed to format stability; each bump was
  a deliberate decision that older repositories become unopenable.

### Added

- **`prikk compact`** reclaims dead records from the three containers that accumulate them — the ref
  pointer index, the received-ref index, and the trust policy container. `--plan-only` reports what a
  run would reclaim without writing. Targets are explicit; a bare invocation refuses rather than
  compacting everything. **Compaction refuses outright on a container with any damaged record** rather
  than compacting around it, because a refusal is recoverable and a deletion is not.
- **`prikk unlock`** recovers a repository wedged by a lock file left behind by a crash. A bare
  invocation lists what is held, with a best-effort liveness check of each recorded process id. Clearing
  requires naming one lock and confirming, or `--yes` for scripting. **The liveness check is advisory
  only**: it is trusted to refuse, never to authorise — a process that appears absent may not be, and
  clearing a lock that is still genuinely held would put two writers on one container.
- **`prikk trust maintainer remove`** revokes an adopted maintainer key. Revocation previously existed
  only as an undocumented hand-edit of `trust/policy.toml`; it now has a supported interface.

### Changed

- **Durability-bearing repository state lives in containers.** Objects, ref pointers, ref logs,
  received-ref pointers and trust material are now checksum-framed records appended into a fixed set of
  files, every name created at `init`. Nothing creates a name afterwards.
- **Corruption is isolated to the records it damaged.** A damaged record is named at its byte offset and
  the scan continues, rather than failing the whole container.
- **No durability-bearing write uses an atomic rename.** The two exceptions are rebuildable caches whose
  absence or corruption changes no result, and both are asserted by tests rather than described.

### Why

An object's identity is its content hash, so every write created a new directory entry — and making a
new directory entry durable requires an fsync on the parent directory, which POSIX provides and Windows
does not. Appending to a file that already has a name needs only content durability, which every
supported platform provides. This release moves durability-bearing state onto that footing so the
guarantee can be stated as a property rather than as a list of platforms that happen to pass.

**It does not enable Windows mutation.** It removes the obstacle. Windows remains read-only, unchanged
from 0.19.0; the implementation is targeted at 0.21.0.

## 0.19.0 — 2026-08-08

Merge. New CLI surface, additive object-format change, no existing object id moves.

**Added**

- **`prikk merge` executes a merge.** `merge-evidence` and `merge-plan` could report on one since
  DC-21/DC-25; nothing could apply one. `prikk merge --baseline-block ID --into REF --from REF` seals the
  other side's patches onto the target when `patch_algebra` proves the two sides confluent from a common
  baseline, and refuses cleanly — no object, WAL, or ref write — when it cannot.
- **A merge authors nothing.** Adopted patches are sealed **verbatim**: same canonical bytes, same
  `ObjectId`, same author signature. Nothing decodes, re-derives, or re-signs a patch. This works because
  prikk's operations are context-free — every operation names a stable `NodeId`, and `EditText` identifies
  its span by content anchors rather than position — so a patch from a divergent branch transports without
  transformation and its author's signature keeps covering it. The maintainer seals; nobody re-authors.
- **Merges are recorded structurally.** A merge seals as `BlockKind::Merge` naming both parents, a
  mainline pointer, and the baseline confluence was proven against. `prikk verify` **re-derives** rather
  than trusts that baseline, walking both parents' ancestries, and reports a divergence finding when the
  recorded baseline is not a genuine common ancestor of both.
- **Merging the same two branches more than once works.** The second merge's baseline is reachable only
  through the first merge's secondary parent, and its candidate set excludes patches the baseline already
  carries by adoption.

**Changed**

- `BlockPayload` gains two optional fields (canonical tags 6 and 7), written only when present, following
  `snapshot_blob_ref`'s established shape. **No existing object id moves** — every `Root` and `Normal`
  block encodes byte-identically to 0.18.4, and the DC-41 hash vectors and DC-55 reference implementation
  are unchanged.

**Known limitations**

- **Merge-base discovery is manual.** `--baseline-block` is explicit; nothing computes it for you. A
  baseline older than the true merge base is refused rather than mis-merged, though the message reports a
  conflict rather than naming the stale baseline.
- **`verify` confirms the recorded baseline is *a* common ancestor of both parents, not the *lowest*, and
  does not re-run confluence** — a merge is trusted on the maintainer's signature exactly as every other
  sealed decision is.
- **Conflict resolution does not exist.** Conflicts are detected and refused; resolving one is a separate
  capability, and a resolution is itself a patch somebody must sign.
- **`prikk verify` cost grows steeply with history length** — roughly cubic in sealed block count,
  measured at 34 s for 160 blocks. Pre-existing, not introduced here, and tracked in `FINDINGS.md`.
- Mutation remains Linux-only (read-only commands run on macOS and Windows).

## 0.18.4 — 2026-08-04

Correctness. No CLI surface change, no library API change, no format change.

**Fixed**

- **Every file rebuilt from sealed history came back at mode `0600`**, regardless of the mode it was
  authored with. `patch_replay`'s materialization pipeline carried no mode field at all — `apply.rs`
  discarded it outright and every write landed at `open_new_regular`'s hardcoded default. Long-standing;
  found by DC-73 while wiring `ChangePerm`, and fixed for `CreateFile` as a prerequisite rather than as
  adjacent cleanup, since both share one write path.
- **`checkout --patch-materialize` now replays `ReplaceBinary` and `ChangePerm`**, and **`rollback-draft`
  now inverts them.** Both were rejected outright before, so any rollback spanning a binary replacement or
  a mode change refused, and binary edits could not be reconstructed by independent replay.
- **Case-insensitive name collisions are rejected at creation** on branch refs, tag refs, and maintainer
  trust key ids (NFR-SEC-03). The trust-key case is the sharpest: on a case-insensitive filesystem,
  `Dev-Maintainer.pub` and `dev-maintainer.pub` collided, so a `required=N` maintainer threshold could
  silently drop to N-1 with no error anywhere.
- **Windows reserved names are rejected for maintainer trust key ids** — `CON`, `PRN`, `AUX`, `NUL`,
  `COM1`-`9`, `LPT1`-`9`.

**Recorded limitations**

- Collision folding is **ASCII-only**. Unicode NFC/NFD equivalence and locale-dependent case rules are
  **not** covered — `café` in two encodings still collides on macOS without being rejected. No Unicode
  dependency is permitted in `prikk-store` under DC-51's placement gate.
- Pre-existing collisions are **not** retroactively detected; the check applies at creation.
- Repository-path collisions are rejected at `seal`, not at `commit`, so a colliding `commit` appears to
  succeed and is refused later.
- `RenamePath` and `CreateSymlink` remain unimplemented — because **nothing authors them**, not because of
  the node model. Their deferral markers now say so.

**Release authority — unchanged from 0.18.1 through 0.18.3**

- Does **not** pass the DC-35 signer-authority audit and does not claim to. `release-signers.toml` is
  empty; no authority transaction was performed. The tag's OpenPGP signature is the maintainer's ordinary
  key, not allowlisted signer authority.
- Release evidence still does not describe the published binaries. Verify them by their published
  checksums and `.build-info.txt`.
- Prebuilt binaries remain Linux-only; mutation remains Linux-only (DC-37). Read-only commands are
  CI-verified on macOS and Windows.

## 0.18.3 — 2026-08-04

Portability, CI conformance, and documentation. No CLI behaviour change.

**prikk now runs off Linux — verified, not asserted**

- **Read-only commands execute on macOS and Windows** (DC-71). `verify`, `log`, `doctor`, `status`,
  `branch list`, `tag list`, and `checkout --plan-only` are exercised on both platforms by CI against a
  real repository, every push. `prikk-store` previously did not compile off Linux at all — inconsistent
  `#[cfg(target_os = "linux")]` gating on imports whose definitions were gated — contradicting DC-37's own
  design. 0.18.2's README claimed the opposite of what worked; that claim is now true.
- **Repository mutation remains Linux-only** (DC-37, unchanged). The full traced command set — 25 commands,
  15 read-only — is published at `docs/src/reference/platform-support.md`.
- Non-Linux I/O errors now name the path they were attempting, instead of a bare errno.

**Documentation**

- `branch create/close`, `tag create/list`, and multi-commit queuing are documented in the README for the
  first time, having shipped across DC-60, DC-61, DC-63, and DC-66.
- **`PRIKK_ACTIVE_PATCH_WARN` and `PRIKK_ACTIVE_PATCH_LIMIT`** (DC-57) are documented for the first time
  anywhere user-facing.
- Seven reference pages and DC-37 itself carried a "Linux is the only platform exercised by project gates"
  claim that was about mutation but read as blanket; all corrected.
- Every documented command in this release was verified by running it.

**Known gaps**

- `--help` documents `branch create <name>` and `tag create <name>`, but both require a **fully-qualified**
  ref (`heads/topic`, `tags/v1`). A bare name is rejected.
- `worktree-status` cannot run against an ordinarily-authored repository — it requires snapshot state no
  CLI command produces.
- Prebuilt binaries remain Linux-only (`x86_64`, `aarch64`); `x86_64-pc-windows-gnu` does not compile.
- NFR-PERF-01 remains unmet; commit cost still grows with repository size.

**Release authority — unchanged from 0.18.1/0.18.2**

- Does **not** pass the DC-35 signer-authority audit and does not claim to. `release-signers.toml` is
  empty; no authority transaction was performed. The tag's OpenPGP signature is the maintainer's ordinary
  key, not allowlisted signer authority.
- Release evidence still does not describe the published binaries — blocked behind DC-45's frozen baseline
  until the 0.19.0 cutover. Verify binaries by their published checksums and `.build-info.txt`.

## 0.18.2 — 2026-08-03

Packaging and documentation. No CLI behaviour, no library API change.

**Fixed**

- **The Quick Start failed at its first command.** `prikk init <path>` does not create the target
  directory, so the documented first-run sequence errored immediately with `i/o error: No such file or
  directory` and produced nothing. Corrected to `mkdir -p ./sample-repo && cd ./sample-repo` then
  `prikk init .`, and the whole sequence — commit, seal, log, verify, doctor — is now executed end to end
  rather than proofread. `prikk init`'s own behaviour is unchanged and tracked as a separate finding.

**Added**

- **Install without cloning.** `cargo install prikk` is now the documented install path; building from a
  clone is documented as the path for working *on* prikk.
- **Prebuilt Linux binaries** (DC-70). A tag-triggered workflow builds `x86_64-unknown-linux-gnu` and
  `aarch64-unknown-linux-gnu`, attaching a checksummed tarball plus a `.build-info.txt` per target
  recording the exact `rustc -vV`, commit, tag, and build command for third-party reproduction.
- **`cargo binstall prikk`** support via `[package.metadata.binstall]`.

**Release authority — unchanged from 0.18.1**

- This release does **not** pass the DC-35 signer-authority audit and does not claim to.
  `release-signers.toml` remains empty; no authority transaction was performed. The tag carries the
  maintainer's ordinary OpenPGP signature, which is **not** allowlisted signer authority.
- **Release evidence does not yet describe the published binaries.** The evidence schema models a single
  archive; extending it to N artifacts is blocked behind DC-45's frozen differential baseline until the
  0.19.0 cutover. Verify binaries by their published checksums and `.build-info.txt`.
- Both published targets are Linux. Repository *mutation* is Linux-only (DC-37).

**Known gaps** — unchanged from 0.18.1, plus:

- `x86_64-pc-windows-gnu` does not compile, contradicting DC-37's stated intent.
- The release workflow above runs for the first time with this tag.

## 0.18.1 — 2026-08-03

Packaging fix. **0.18.0 was tagged but never published to crates.io**; this is the version that ships.

- **Internal crate dependencies are now pinned.** `[workspace.dependencies]` declared each sibling crate as
  `version = "0"`, which `^0` resolves — so a published `prikk 0.18.0` would have accepted `prikk-store`
  **0.17.7**, allowing a 0.18 CLI to be assembled against a 0.17 store. Found by a `cargo publish --dry-run`
  pass before first publication: verification strips `path` and was compiling 0.18.0 sources against the
  previously published 0.17.7 crates, producing unresolved-import errors for symbols that exist locally.
- Sibling dependencies now pin `0.18.1` exactly. No source change; no behaviour change from 0.18.0.
- Pre-existing since the workspace was split — 0.17.x published the same way. It becomes consequential at
  0.18, the first release where the crates genuinely diverge (`RefState` schema 2, new store and object
  APIs).

The 0.18.0 tag is left in place and points at the release it described. It was never published.

## 0.18.0 — 2026-08-03

First release since 0.17.7 (129 commits). **Cut because 0.17.7 cannot edit the same text file twice** — a
long-standing defect in the core workflow, fixed here by DC-65. Minor rather than patch because the release
also adds new command surface and a new envelope schema.

**Correctness**

- **DC-65: repeated text edits.** Editing the same text file across two or more sealed commits failed with
  `baseline content Blob ... is missing`. `plan_edit_text` read a node's `blob_id` as a stored object, but
  `EditText` records a diff and never writes its derived content as a blob. Establishes the invariant that a
  node's `blob_id` is a **content identity, not necessarily a stored object**, and materializes on demand
  through replay — matching what the replay side always did. Verified across six sealed generations.
- **DC-61 N1: `branch close` fail-open.** A non-empty active WAL with missing or malformed ownership
  metadata permitted closure where every other publisher stops on the same integrity error.

**New surface**

- **DC-60, DC-61: branches.** `prikk branch create`, `prikk branch close`, and `prikk branch list --all`.
  Closure is not deletion — the pointer, history, and objects all stay, and reopening is an ordinary CAS
  update. Adds `RefState` envelope **schema 2** carrying a `closed` marker, emitted only when set so every
  existing ref-state ObjectId is unchanged.
- **DC-63: tags.** `prikk tag create` and `prikk tag list`, with kind-aware ref publication and verification.
- **DC-66: multi-commit queuing.** The active session holds N unsealed patches; `commit` no longer refuses
  on a non-empty WAL; `seal` batches the queue into one block. Queued commits chain — each authors against
  the previous one's state — so node identity holds across a queue.
- **DC-57: active-patch thresholds.** Warns at 800 queued patches and blocks at 1000, configurable via
  `PRIKK_ACTIVE_PATCH_WARN` and `PRIKK_ACTIVE_PATCH_LIMIT`. The block fires before any write, so a blocked
  commit leaves no partial state.

**Performance**

- **DC-56: changed-path index.** Commit no longer reads every file's contents; a cache under `cache_dir()`
  records per-path stat and content hash so unchanged files are skipped. Content-read cost fell ~20%.
- **DC-64: incremental baseline cache.** Commit no longer replays the whole lineage; it applies one block
  onto a cached predecessor, cutting the dominant cost (~370 ms to ~2.6 ms at 10,000 files). Both caches are
  rebuildable, never authoritative, and `verify` reports divergence for either.
- **NFR-PERF-01 is still not met.** Commit cost no longer tracks operations replayed, but `load`, `persist`,
  and `from_replay` remain proportional to live node count, so cost still grows with repository size.

**Assurance**

- **DC-55: SHA-256.** The first-party implementation is replaced by `sha2`, with the outgoing implementation
  retained test-only as a permanent differential reference.
- **DC-67: ordinary-use conformance suite.** Sequences of ordinary operations at N ≥ 3 through the compiled
  binary, each ending by rebuilding the worktree from sealed history and asserting byte-exact content — the
  axis the existing adversarial and structural coverage did not reach.
- **DC-59, DC-62: commit benchmark**, with wall-clock, memory, and consecutive-cycle axes.

**Release authority — read before relying on this release**

- **This release does not pass the DC-35 signer-authority audit, and does not claim to.** The committed
  release-signer set (`release-signers.toml`) is empty and fail-closed, so no release currently satisfies
  that gate. The tag carries the maintainer's ordinary OpenPGP signature, as tags through 0.17.7 do, but
  **that key is not an allowlisted release signer** — signature presence is not signer authority. Unlike
  those tags, 0.18.0 does not predate the policy, so this is stated here explicitly rather than covered by
  the pre-policy exemption in `docs/src/reference/release-compatibility.md`.
- No authority transaction was performed. Populating the signer set requires two distinct natural persons
  (`DC-35:219`) and remains outstanding.
- The default branch was not under observed branch-protection controls when this release was prepared.
- Accordingly: **verify what you obtain by content, not by release authority.** The repository's own
  object identity and `prikk verify` are the checkable properties here; the release channel is not.

**Known gaps**

- `checkout --patch-materialize` cannot replay `ReplaceBinary` or `ChangePerm`.
- No working-directory branch switch; every command resolves `--ref` explicitly.
- Repository format remains unstable. No compatibility is promised.
- NFR-PERF-01 (commit cost bounded independently of repository size) remains unmet — see Performance.

## 0.17.7 — 2026-07-13

DC-33: concurrency and locking reference.

- Adds `docs/src/reference/concurrency-locking.md`, an authoritative current-state reference for
  active-session locking, ref-specific publication locks, compare-and-swap behavior, narrow ref repair
  locking, stale-lock limits, and current concurrency non-goals.
- Documents the current lock primitive: local lock files created with exclusive file creation,
  fail-closed `LockConflict` behavior, fsynced lock files, best-effort parent-directory sync, and
  best-effort release on drop.
- Distinguishes lock-file conflicts from ref CAS baseline mismatches, including the different operator
  responses for `lock already exists` versus `ref CAS mismatch` failures.
- Connects the ref publication crash window between RefUpdate log append and pointer promotion to the
  narrow `doctor --repair-main-ref` missing-pointer repair path.
- Links the data-model, durability/recovery, integrity/recovery, repository-layout, and path-safety
  references to the new concurrency and locking reference.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, repository format,
  lock behavior, seal behavior, commit behavior, verification behavior, doctor behavior, trust policy,
  repair behavior, or release semantics are changed.

## 0.17.6 — 2026-07-13

DC-32: path and worktree safety reference.

- Adds `docs/src/reference/path-safety.md`, an authoritative current-state reference for repository
  path validation, snapshot manifest paths, checkout/worktree materialization safety, patch deletion
  safety, and worktree authoring safety.
- Documents the exact current `RepoPath` rejection set, including ASCII-only paths, top-level `.prikk`
  rejection, control-byte rejection, Windows reserved component basenames, duplicate paths, and
  case-insensitive collisions.
- Documents that materialization safety is conservative but check-then-write: it uses lexical
  root-containment and symlink-aware parent/target checks, but does not claim `openat`/`O_NOFOLLOW`,
  canonical realpath proof, or race-free protection under concurrent worktree mutation.
- Links README, checkout/worktree guides, the repository-layout reference, and the trust/threat
  reference to the path-safety reference.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, repository format,
  path validator behavior, checkout behavior, materialization behavior, worktree authoring behavior,
  trust policy, verification behavior, repair behavior, or release semantics are changed.

## 0.17.5 — 2026-07-13

DC-31: repository layout and authority reference.

- Adds `docs/src/reference/repository-layout.md`, an authoritative current-state reference for the
  initialized `.prikk/` layout, `.prikk/FORMAT`, persistent object directories, ref pointer/log paths,
  active-session paths, trust-store paths, and authority-vs-pointer/cache boundaries.
- Documents that `.prikk/FORMAT` is a current format gate, not a stable-format or migration guarantee.
- Documents that `cache/` and `quarantine/` are initialized but not current roots of trust, that
  `gc/` is not a current initialized directory, and that runtime files such as WALs, ref pointers, ref
  logs, trust policies, and maintainer keys are written by later operations rather than bare init.
- Links README, the data-model reference, durability/recovery reference, trust/threat reference, and
  signing setup guide to the layout reference.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, repository format,
  trust policy, verification behavior, repair behavior, or release semantics are changed.

## 0.17.4 — 2026-07-13

DC-30: key management and signing setup guide.

- Adds `docs/src/guide/security-setup.md`, a current-state operator guide for AUTHOR and MAINTAINER
  signing setup, environment key inputs, repository-local maintainer trust, and sensitive seed
  handling.
- Documents that Prikk currently has no key-generation or public-key-derivation command, that
  operators must obtain matched Ed25519 seed/public-key material externally, and that published sample
  seeds and keys are unsafe for real signing.
- Links README, the trust/threat reference, and the integrity/recovery diagnostics reference to the
  setup guide.
- Cleans public README and ROADMAP wording so durable docs do not direct readers to local scratch
  paths.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, repository format,
  signing behavior, trust policy, verify behavior, seal behavior, or release semantics are changed.

## 0.17.3 — 2026-07-13

DC-29: verify and doctor integrity/recovery reference.

- Adds `docs/src/reference/integrity-recovery.md`, an authoritative current-state reference for
  repository verification and doctor diagnostics.
- Documents what `prikk verify` checks, what it does not prove, verify output and failure behavior,
  all six active WAL metadata states, the current doctor issue catalog, and narrow doctor repair
  boundaries.
- Links the data-model, trust/threat, durability/recovery, and rollback-draft verification pages to
  the integrity and recovery diagnostics reference.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, repository format,
  trust policy, verify behavior, doctor behavior, or repair behavior is changed.

## 0.17.2 — 2026-07-12

DC-28: durability and crash-recovery reference.

- Adds `docs/src/reference/durability-recovery.md`, an authoritative current-state reference for
  active-WAL persistence, WAL replay/tail handling, active ref metadata, seal publication flow,
  ref-pointer recovery, doctor repair limits, stale-lock limits, and deferred crash/platform
  evidence.
- Links the data-model and trust/threat reference pages to the durability and crash-recovery
  reference.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, repository format,
  WAL/ref/seal/verify/doctor behavior, or release semantics are changed.

## 0.17.1 — 2026-07-12

DC-27: patch algebra and merge-evidence concepts reference.

- Adds `docs/src/reference/patch-algebra.md`, an authoritative current-state reference for patch
  algebra, commutation, flat confluence, merge-evidence outcomes, reason codes, proof phases, and
  merge-plan status mapping.
- Links the `merge-evidence` and `merge-plan` guide pages to the concept reference, so command output
  terms such as `op_seq`, `pair_conflict`, `classification`, `Confluent`, and `ConfluentSubset` have a
  reviewed explanation.
- Adds visible claim-to-source anchors tying the reference to released DCs and implementation paths.
- Keeps the release documentation-only: no Rust code, CLI behavior, object schema, merge execution,
  merge-base discovery, persisted proof/witness objects, JSON output, or public Rust API stability is
  changed.

## 0.17.0 — 2026-07-11

DC-25: merge planning surface.

- Adds `prikk merge-plan`, a read-only planning classification over the existing explicit-input
  merge evidence path.
- Requires explicit `--baseline-block` plus exactly one left selector and one right selector from
  `--left-block` / `--left-ref` and `--right-block` / `--right-ref`.
- Shows submitted selectors, resolved target Blocks, operation counts, plan status, underlying
  evidence outcome/reason, action text, evidence item counts, and evidence items.
- Maps `Confluent` evidence to `ConfluentSubset`, explicitly avoiding a whole-merge or executable
  merge claim.
- Maps blocked evidence outcomes to `BlockedConflict`, `BlockedOrderedDependency`,
  `BlockedUnsupported`, `BlockedDeferred`, `BlockedNotConfluent`, `BlockedEvidenceFailure`, and
  `BlockedInvalidCandidate`.
- Keeps process success separate from plan status: a valid blocked plan is still a successfully
  produced plan, while invalid arguments, selector failures, ancestry failures, object failures, and
  ref failures remain command failures.
- Preserves the existing `prikk merge-evidence` command and shares its read-only selector/evidence
  boundary without changing its diagnostic meaning.
- Removes the temporary 0.16.1 FDD-00/FDD-04 compatibility pointer files after their contents moved to
  the authoritative mdBook reference pages in 0.16.1.

Still deferred: `prikk merge`, merge execution, automatic merge-base discovery, branch merge
semantics, branch publication, merge commits, multi-parent Blocks, active-WAL merge drafts, worktree
conflict materialization, conflict resolution UI, persisted plan/proof/witness/evidence objects,
path-scoped merge analysis, display-path filtering, JSON output, schema changes, patch-algebra crate
extraction, and public `prikk-replay` API stabilization.

## 0.16.1 — 2026-07-11

DC-26: documentation home correction.

- Moves the authoritative current-state data model reference into
  `docs/src/reference/data-model.md`, with the full reference body, public caveats, provenance, and
  visible claim-to-source anchor table rendered in the mdBook.
- Moves the authoritative current-state trust/threat reference into
  `docs/src/reference/trust-threat-model.md`, including the security-claim review discipline required
  for trust, threat, verification, signature, key-management, durability, platform-support, and
  production-readiness claims.
- Replaces `rfcs/fdds/FDD-00-DATA-MODEL.md` and
  `rfcs/fdds/FDD-04-TRUST-THREAT-MODEL.md` with temporary compatibility pointers for 0.16.1. They are
  scheduled for removal in 0.17.0 unless a later review extends the window.
- Updates README, ROADMAP, RFC index, and implementation status references so current-state
  architecture/concept documentation is book-owned, while `rfcs/` remains for design-process and
  genuine gating FDD material.
- Updates the documentation-reference backlog homes for TASK-06/07/08/10/12 to the DC-26
  `docs/src/reference/` model.
- Keeps the release documentation-only: no repository format, object schema, trust policy,
  verification, CLI behavior, or RFC lifecycle policy change is introduced.

## 0.16.0 — 2026-07-11

DC-23: public merge evidence UX stabilization, plus reviewed pre-release documentation work.

- Stabilizes `prikk merge-evidence` text output so selector summaries, resolved target Blocks,
  operation counts, full-report outcome, reason code, and item counts are easier to scan.
- Renders cross-side evidence items as explicit `cross:` blocks with separate `left[...]` and
  `right[...]` operation lines, replacing the ambiguous one-line `<->` form.
- Renders report-level items as `report:` without a fake operation label such as `report report`.
- Shows item counts as `items: N displayed of N`. DC-23 does not add display filtering, so the two
  counts are equal in this release.
- Preserves DC-22 command shape, selector semantics, exit-status behavior, read-only behavior, and
  privacy/redaction rules.
- Adds store-level coverage that pins distinct left and right operation summaries for cross-side
  evidence display items.
- Reorganizes the mdBook navigation and source tree by function, adds the `merge-evidence` command
  page, and adds a GitHub Pages workflow with mdBook metadata and ignored generated output.
- Adds current-state FDD and mdBook reference entries for Prikk's data model and trust/threat model,
  with inline public caveats and claim-to-source anchor tables.
- Keeps the DC-24 documentation scope non-behavioral: no repository format, object schema, trust
  policy, verification, CLI behavior, or release semantics change is introduced.

Still deferred: `prikk merge`, merge execution, automatic merge-base discovery, branch merge
semantics, branch publication, merge commits, multi-parent Blocks, active-WAL merge drafts, persisted
proof/witness/merge-evidence objects, display-path filtering, scoped/path-limited merge analysis,
worktree conflict materialization, JSON output, schema changes, trust-store enforcement changes,
patch-algebra crate extraction, and public `prikk-replay` API stabilization.

## 0.15.0 — 2026-07-11

DC-22: public merge evidence UX boundary.

- Adds `prikk merge-evidence`, a read-only public display over the DC-21 merge/conflict evidence
  report contract.
- Requires explicit `--baseline-block` plus exactly one left and one right selector from `--left-block`
  / `--left-ref` and `--right-block` / `--right-ref`.
- Resolves ref selectors through current local branch RefState validation and shows both submitted
  selectors and resolved target Block ids.
- Derives sealed candidate sequences by walking single-parent target ancestry back to the explicit
  baseline, failing closed on missing ancestry, multi-parent chains, cycles, or unreadable evidence.
- Keeps the surface non-mutating: no object writes, ref updates, WAL writes, merge commits, or
  worktree changes.
- Adds CLI regression coverage for stdout/stderr privacy and read-only behavior on both successful and
  failing command paths.

Still deferred: `prikk merge`, merge execution, automatic merge-base discovery, branch merge
semantics, branch publication, merge commits, multi-parent Blocks, active-WAL merge drafts, persisted
proof/witness/merge-evidence objects, worktree conflict materialization, JSON output, schema changes,
patch-algebra crate extraction, and public `prikk-replay` API stabilization.

## 0.14.0 — 2026-07-05

DC-21: merge conflict evidence contract.

**Release scope.** This release adds an internal, read-only merge/conflict evidence report contract
over the existing patch-algebra commutation and flat confluence analyzers. It introduces
reviewable outcome categories, release-stable diagnostic reason codes for tests/future display, required
baseline identity, sequence summaries, evidence-scope mapping, and privacy-preserving report entries.
Reason codes are diagnostic vocabulary, not persisted object schema.

- **Evidence report vocabulary.** Adds internal report types for `Confluent`, `Conflict`,
  `OrderedDependency`, `Unsupported`, `Deferred`, `NotConfluent`, `EvidenceFailure`, and
  `InvalidCandidate`.
- **Analyzer adapters.** Adds read-only adapters from pair commutation and flat two-sequence confluence
  results into merge evidence reports, preserving required sealed evidence failures and optional
  unsealed candidate failures as distinct public outcomes.
- **Privacy and determinism tests.** Adds focused tests for baseline identity, outcome mapping,
  evidence-scope mapping, deterministic item ordering, narrow `NotConfluent` handling, and report debug
  output that avoids raw text spans, replacement text, blob bytes, and absolute host paths.
- **Flatness diagnostic scope.** Current flatness violations use `SequenceInternalDependencyDeferred`
  as the release-stable diagnostic reason; a separate `flatness_required` reason code remains deferred
  until a later DC introduces a broader flatness-reporting surface.
- **Replay crate publication note.** `prikk-replay` remains internally scoped and non-stable as an
  external Rust API, but its manifest no longer disables crate publication so workspace release
  packaging can publish it consistently with the rest of the crates.

Still deferred: merge execution, `prikk merge`, branch merge, multi-parent Blocks, persisted
proof/witness objects, schema changes, worktree conflict materialization, public conflict UX,
patch-algebra crate extraction, and public `prikk-replay` API stabilization.

## 0.13.0 — 2026-07-05

DC-20: replay boundary stabilization.

**Release scope.** This release stabilizes the post-DC-19 `prikk-replay` boundary without adding CLI
behavior, object schema changes, repository layout changes, or new public APIs. `prikk-replay` remains
internally scoped; at the time of 0.13.0 it was publication-disabled in the manifest, and 0.14.0 later
removed that packaging block without stabilizing its external API. `prikk-store` remains the repository
integration crate.

- **Boundary documentation.** Updates `prikk-replay` docs to use version-neutral
  replay-boundary-stabilization wording and keeps public Rust items explicitly non-stable for external
  API purposes.
- **Compatibility-wrapper inventory.** Records `crates/prikk-store/src/node_lifecycle.rs` and
  `crates/prikk-store/src/path.rs` as retained compatibility surfaces, with semantic ownership in
  `prikk-replay` and no duplicate lifecycle/path implementation in the wrappers.
- **Lexical path boundary.** Keeps `RepoPath` lexical in `prikk-replay`; root joining for
  worktree/repository materialization is owned by `prikk-store`.
- **Focused tests.** Adds direct replay path tests and a store-owned root-joining test, keeping tests
  outside implementation files.

Still deferred: `text_span` extraction, patch-algebra extraction, store-backed resolver movement,
lifecycle-cache persistence movement, worktree extraction, public `prikk-replay` API stabilization,
and public merge, confluence, and conflict surfaces.

## 0.12.0 — 2026-07-05

DC-19: replay/lifecycle crate boundary.

**Release scope.** This release introduces `prikk-replay` as a workspace-internal semantic
replay/lifecycle crate and moves the node lifecycle substrate plus its direct tests out of
`prikk-store`. `RepoPath` moves with the lifecycle substrate as the minimal lexical path leaf required
by `NodeLifecycleState`, while `prikk-store` keeps compatibility wrappers and continues to own
repository layout, refs, WAL, active sessions, lifecycle-cache persistence, verification, doctor,
object storage, and store-backed resolver construction. This release still does **not** add CLI
behavior, object schema changes, text-span extraction, patch-algebra extraction, worktree extraction,
merge execution, public confluence APIs, persisted proof/conflict-witness objects, rollback refs,
rollback authorization, branch switching, key lifecycle, or sync behavior.

- **Workspace-internal replay crate.** Adds `crates/prikk-replay` as an internal/experimental crate
  during DC-19. It was initially publication-disabled in the manifest; 0.14.0 later removed that
  packaging block without stabilizing its external API. Its dependency tree is limited to
  `prikk-error`, `prikk-hash`, and `prikk-object`, with no `prikk-store` dependency.
- **Lifecycle substrate extraction.** Moves `NodeLifecycleState`, `LiveNode`, `NodeContent`,
  `Tombstone`, lifecycle validation helpers, and direct lifecycle tests into `prikk-replay`, preserving
  existing behavior and structured lifecycle errors.
- **Path leaf extraction.** Moves repository-relative lexical `RepoPath` validation into
  `prikk-replay` because lifecycle state stores paths. Filesystem layout, materialization policy, and
  worktree ownership remain in `prikk-store`.
- **Store compatibility bridge.** Keeps `crates/prikk-store/src/node_lifecycle.rs` and
  `crates/prikk-store/src/path.rs` as compatibility import modules so existing internal call sites and
  the public `prikk_store::RepoPath` surface continue through the new crate boundary.
- **Test and file-layout cleanup.** Moves the direct lifecycle tests into `prikk-replay` and splits
  them into focused test submodules under the project line-count and test-placement guidelines.

## 0.11.0 — 2026-07-05

DC-18: patch algebra commutation and confluence contract.

**Release scope.** This release adds an internal, library/test-only commutation and flat confluence
contract for the DC-16/DC-17 patch-algebra subset. Pair commutation now requires classifier
independence plus replay-both-orders proof, and two flat candidate sequences can be proven confluent
only when all cross-pairs commute and composed replay reaches the same authoritative lifecycle state.
This release still does **not** add CLI behavior, merge execution, persisted proof or conflict-witness
objects, object schema changes, public confluence APIs, rollback refs, rollback authorization,
multi-parent publication, semantic merge, or user-facing conflict resolution.

- **Replay-backed commutation.** Adds an internal replay oracle for candidate operation pairs, so
  `Independent` becomes usable commutation evidence only when both operation orders replay from the
  common baseline and yield identical lifecycle state without rewriting operation identity.
- **Flat confluence contract.** Adds internal two-sequence confluence analysis for flat candidate
  sequences, preserving sequence order, rejecting concrete ordered dependencies/conflicts, and proving
  final-state equality through composed replay.
- **Candidate evidence validation.** Validates candidate blob evidence needed by replay, including
  `CreateFile.blob_id` and `ReplaceBinary.new_blob_id`, with required sealed evidence failures surfaced
  as `EvidenceError` and optional unsealed candidate gaps remaining fail-closed as `Unknown`.
- **Evidence precedence hardening.** Confluence scans candidate sequences so sealed-candidate evidence
  errors are not hidden by earlier algebraic `Unknown` or deferred-operation results.
- **Module/test split.** Keeps patch-algebra implementation and tests split across focused files under
  the project line-count discipline.

## 0.10.0 — 2026-07-05

DC-17: patch algebra evidence contract.

**Release scope.** This release turns the internal patch-algebra classifier's evidence boundary into an
explicit store-backed contract. Classification now separates ordinary unsupported algebra from
repository evidence failures, carries required-vs-optional evidence scope through resolver calls, and
keeps conflict witnesses as internal diagnostics rather than public schema. This release still does
**not** add CLI behavior, merge execution, persisted conflict-witness objects, object schema changes,
production confluence checks, rollback refs, rollback authorization, semantic merge, or user-facing
conflict resolution.

- **Scoped evidence contract.** Adds internal `EvidenceScope`, evidence state, and evidence-error
  types so sealed-baseline and sealed-candidate facts fail as integrity errors while explicitly
  optional unsealed-candidate evidence can remain fail-closed as `Unknown`.
- **Store-backed resolver boundary.** Adds a read-only resolver that derives baseline text and create
  blob text from replay/lifecycle state plus validated object-store blob evidence, with no
  default-empty fallback for sealed baselines.
- **Classifier result split.** Pair classification now returns an integrity-aware result surface,
  preserving `Unknown` for unsupported or intentionally deferred algebra while surfacing corrupt,
  missing, malformed, or unreadable required evidence separately.
- **Evidence-backed relation coverage.** Pins same-node `CreateFile -> ChangePerm` ordering/conflict
  behavior and keeps create-before-content-mutation decisions tied to validated blob kind/text
  evidence.
- **Internal witness policy.** Removes loose expected/actual witness fields and keeps witness facts
  deterministic, internal, and test-stable without adding persisted or public diagnostic schema.

## 0.9.0 — 2026-07-05

DC-16: patch algebra foundation.

**Release scope.** This release adds an internal, library/test-only pair-classification foundation for
future patch algebra work. It defines and tests `Independent`, `OrderedDependency`, `Conflict`, and
`Unknown` pair classes, structured path effects including `required_free`, internal diagnostic witness
kinds, baseline preimage validation, and both-order replay oracles for every independent fixture. This
release still does **not** add a CLI surface, merge execution, persisted conflict-witness objects,
Patch/Block/RefState/RefUpdate schema changes, production confluence checks, rollback refs, rollback
authorization, semantic merge, or user-facing conflict resolution.

- **Internal pair classifier.** Adds the private `patch_algebra` module in `prikk-store` with
  crate-internal classifier types and fail-closed handling for unsupported/deferred cases. Rename,
  symlink, malformed, and insufficient-evidence cases classify as `Unknown` or `Conflict`, never
  silently skipped or treated as independent.
- **Baseline preimage validation.** `Independent` now requires each operation's own baseline preimage to
  be proven. The classifier checks baseline path occupancy for `CreateFile.required_free`, live
  path/kind/blob/mode for file deletion and mutation preimages, and text-span localization when text
  resolver evidence is required.
- **Create/mutate evidence boundary.** Same-node create-before-content-mutation ordering requires
  blob-kind/content evidence: `CreateFile -> ReplaceBinary` is ordered only when the created blob is
  proven binary, and `CreateFile -> EditText` is ordered only when the created blob is proven text and
  the edit localizes against that created content. Missing evidence remains `Unknown`.
- **Oracle-backed vectors.** Adds 24 patch-algebra vectors covering same-path create/delete
  dependencies, stale preimages, same-node mode/content interactions, same-node text non-independence,
  resolver-proven text cases, and operation-identity preservation without `op_seq` renumbering.
- **Release-gate cleanup.** Removes legacy test-only `unwrap()` usage that blocked
  `cargo clippy --workspace --all-targets -- -D warnings`.

## 0.8.0 — 2026-07-04

DC-15: active-session integrity and verification hardening.

**Release scope.** Repository health checks now report active-WAL ref metadata integrity explicitly,
rollback-draft append re-checks the target ref tip before writing, the ref publication primitive rejects
non-local branch refs at its own boundary, and signature key-id validation is shared by AUTHOR,
MAINTAINER, and trust-policy paths. This release still does **not** add rollback refs, rollback
authorization, AUTHOR trust-store enforcement, branch switching, multi-commit active sessions,
commutation, confluence, conflict witnesses, semantic merge, or new object schemas.

- **Active-WAL metadata diagnostics.** `verify` records whether active ref metadata is valid, missing,
  malformed, or stale relative to the active WAL. Non-empty WALs with missing or malformed metadata are
  active-session integrity issues; empty-WAL metadata debris is reported as a warning/local-debris
  condition.
- **Doctor diagnostics.** `doctor` surfaces non-empty-WAL metadata faults as error-severity issues and
  empty-WAL metadata debris as warning-severity issues without adding automatic metadata repair.
- **Rollback-draft freshness.** `rollback-draft --append-inverse` snapshots the published target tip
  before lock-free inverse planning, then re-reads the tip under the active lock and refuses to append if
  the ref changed during planning.
- **Ref publication boundary.** `RefStore::publish` now validates `heads/*` local branch refs directly,
  preventing lower-level publication of tags, remotes, rollback refs, or malformed branch names.
- **Shared signature input validation.** `Signature::signed_bytes` is fallible and validates key ids
  through the same ASCII/length policy used by AUTHOR signing, MAINTAINER signing, and maintainer trust
  policy loading.

## 0.7.0 — 2026-07-03

DC-14: arbitrary-span text direct inverse and rollback exposure.

**Release scope.** Supported arbitrary-span `EditText` operations now have
deterministic direct inverses. Inverse planning, rollback preview, rollback draft append, and rollback
draft verification can handle supported text edits by recomputing inverse anchors, duplicate index, and
`span_id` against post-forward text. This release still does **not** add rollback refs, rollback
authorization, AUTHOR trust-store enforcement, worktree rollback mutation, commutation, confluence, or
conflict witnesses.

- **Direct `EditText` inverse.** The shared text-span layer derives inverse records by localizing the
  forward span, applying the forward splice, selecting the exact replacement range in post-forward
  text, swapping old/replacement text, recomputing identity bytes, re-localizing the inverse to the
  exact intended range, and proving byte-exact recovery.
- **Rollback surfaces.** Existing `inverse-plan`, `rollback-preview`, `rollback-draft --append-inverse`,
  and `rollback-draft-verify` surfaces now accept supported arbitrary-span text edits while preserving
  fail-closed behavior for unsupported operations.
- **Rollback draft verification.** Verification compares canonical `PatchPayload` bytes, rejects
  normal-purpose drafts with byte-identical operations, rejects stale inverse identity bytes and
  generated presentation hints, rejects legacy marker-key rollback authority, and structurally requires
  rollback AUTHOR signatures to be Ed25519 records with 64-byte signature payloads. This remains a
  structural check, not AUTHOR trust-store or rollback-authorization enforcement.
- **Deep-review durability repairs.** Seal now refuses to create a Root publication when the target ref
  pointer is missing but ref-log history or a partial ref log exists. Retrying after a crash that
  published the current active WAL but did not drain active metadata now recognizes the already-published
  tip and drains the WAL/ref metadata without appending a duplicate ref update.
- **Pinned vectors.** Added replacement, insertion, deletion, repeated text, CRLF, UTF-8 widening,
  multi-hunk enclosing span, and same-node multi-edit reverse-order vectors.

## 0.6.0 — 2026-07-03

DC-13: non-default ref genesis.

**Release scope.** `prikk commit --ref heads/<branch>` can explicitly create an unborn local branch as
an independent Root history from the current worktree, and `prikk seal --ref heads/<branch>` publishes
that branch through the existing signed `RefState` / `RefUpdate` path. This release still does **not**
claim branch switching, branch copy/fork from an existing tip, merge-base semantics, branch deletion or
rename, tag or remote ref creation, rollback refs, multi-commit queued active sessions, or per-ref
active WALs.

- **Explicit unborn branch genesis.** A valid unpublished `heads/*` ref with no pointer and an absent or
  empty ref log authors against an empty baseline, so all worktree files become `CreateFile` records.
  Pointer absence plus non-empty, malformed, unreadable, or partial ref-log history remains
  recovery/corruption, not genesis.
- **Active-WAL ref ownership.** The active WAL now records `.prikk/active/default/ref-name` before the
  first WAL append. Non-empty WALs with missing, malformed, or mismatched ref metadata fail closed;
  empty WAL metadata debris is cleaned under the active lock. Seal holds the active lock through
  metadata validation, publication, WAL drain, and metadata removal.
- **`seal --ref`.** Seal can publish the queued active WAL to an explicit local branch ref and reports
  the actual advanced ref. A WAL authored for `heads/topic` cannot be sealed to `heads/main`.
- **Publication ordering.** Ref publication now journals the signed `RefUpdate` before pointer
  promotion under the ref lock, preserving recovery evidence for created refs.
- **Upgrade note.** Repositories upgraded with a pre-DC-13 non-empty active WAL that lacks ref metadata
  fail closed. Seal or clear active sessions before upgrading, or inspect the active WAL and metadata
  before retrying.

## 0.5.0 — 2026-07-03

DC-12: arbitrary-span text edits.

**Release scope.** Worktree text edits are authored and replayed as deterministic, content-anchored
arbitrary spans through the shared text-span identity primitives. This release still does **not** claim
commutation, confluence, conflict witnesses, multi-operation diff minimization, semantic merge,
rollback authorization, rollback refs, worktree rollback mutation, or arbitrary-span inverse/rollback.

- **Arbitrary-span authoring.** Modified existing `TextFile` nodes now author one deterministic
  enclosing `EditText` span instead of a whole-file span. Span selection uses byte LCP/LCS, widens to
  UTF-8 character boundaries, and derives anchors, `old_span_hash`, `dup_index`, and `span_id` through
  the shared `text_span` module.
- **Arbitrary-span replay/materialization.** Patch replay and patch materialization apply supported
  `EditText` records by resolving the live `node_id`, validating text preconditions, localizing with
  `locate_text_span`, and splicing with `splice_text`.
- **Pinned vectors.** Added DC-12 byte-level vectors for replacement, insertion, deletion, sub-character
  UTF-8 widening (`é` -> `è` and CJK), CRLF preservation, and multi-hunk enclosing spans.
- **Deferred inverse/rollback.** Inverse planning now fails closed on arbitrary-span `EditText` until
  the direct-inverse round-trip vector set lands.

## 0.4.0 — 2026-07-03

DC-11: publication signing and minimal trust store.

**Release scope.** Production publication objects (`Block`, `RefState`, and inline `RefUpdate`) now
carry real role-bound Ed25519 MAINTAINER signatures verified against a repository-local trust policy.
This is still not full PKI: no key rotation, revocation, expiration, thresholds above one, remote trust,
hardware signing, audit-plugin policy, or repository-format stability guarantee. Pre-DC-11 histories
sealed with `dev-placeholder-maintainer` are treated as pre-publication-trust artifacts and report
publication-trust failures under v0.4.0 verification.

- **Minimal trust store.** `init` creates `.prikk/trust/keys/maintainer/`; `prikk trust maintainer add`
  writes the single-key `required = 1` trust policy through the production path with strict validation.
- **Real MAINTAINER signing.** `seal --allow-no-audit` requires `PRIKK_MAINTAINER_KEY_ID` and
  `PRIKK_MAINTAINER_SEED`, verifies the signer key id and seed-derived public key against local trust
  before publication, and signs Block, RefState, and RefUpdate with role-bound Ed25519 signatures.
- **Publication trust verification.** `verify` checks trusted MAINTAINER signatures for reached Blocks,
  RefStates, and inline RefUpdates, reporting publication-trust failures separately from structural
  corruption. `doctor` diagnoses trust failures but does not auto-trust keys or repair signatures.
- **Compatibility.** `RefUpdatePayload.author_key_id` now records the real MAINTAINER key id, so new
  RefUpdate identities differ from placeholder-era output. Existing PATCH anchors are unchanged.

## 0.3.0 — 2026-07-02

DC-10: rollback-draft identity and AUTHOR signing.

**Release scope.** AUTHOR-role Patch signatures produced by production commands are real role-bound
Ed25519 signatures. Rollback drafts are identified by `PatchPurpose::RollbackDraft`, not by a reserved
AUTHOR key id, and `prikk rollback-draft --append-inverse` signs the draft Patch through the same
injected AUTHOR signer boundary used by worktree commits. This release still does **not** include
publication-grade MAINTAINER signing, trust-store enforcement, key management/rotation, rollback
authorization policy, or stable repository-format guarantees.

- **Rollback-draft purpose marker.** Adds an optional canonical Patch payload `purpose` field. The
  absent field decodes as normal Patch purpose, explicit default encoding is rejected, and
  `RollbackDraft` is pinned by a deterministic hard vector.
- **Real rollback-draft AUTHOR signatures.** `rollback-draft --append-inverse` now requires AUTHOR key
  material, marks the inverse payload as `PatchPurpose::RollbackDraft`, and signs the unsigned Patch
  object id with a real role-bound Ed25519 AUTHOR signature.
- **Purpose-based verification and history classification.** Active rollback-draft verification and
  sealed rollback history classification now inspect payload purpose, fail closed on malformed purpose
  encoding, and report the real AUTHOR key id instead of recognizing the old development marker.
- **Documentation and design records.** Adds DC-10 design and handoff updates, and updates rollback draft,
  sealed history, README, roadmap, and implementation-status documentation to describe the new release
  scope.

## 0.2.0 — 2026-07-02

DC-09 Phase 4.4: node-addressed worktree authoring, genesis first-commit, and role-bound Ed25519
`AUTHOR` signing.

**Release scope.** Node-addressed `prikk commit` patches are role-bound Ed25519 `AUTHOR`-signed. This
release does **not** include trust-store enforcement, key management, `MAINTAINER`/publication signing,
or publication-grade `rollback-draft` signing; symlink authoring is fail-closed; and whole-file reads are
subject to the current large-file limits. The repository format is not yet stable.

`prikk commit` consumes node-addressed worktree authoring (R1/R1R) and supports **genesis / first-commit**
on a fresh repository (4.4b): a never-published ref authors an empty baseline (all `CreateFile`), and seal
publishes a Root block, so `init → commit → seal` works end to end. The earlier layers (4.4-2c-*) remain
internal replay/cache plumbing. Identity anchors unchanged (empty-PATCH `510ab866…5157`, populated
`24031b48…c854`).

- **Release-prep — runtime PR-030-era string cleanup.** `prikk --version` now derives from
  `CARGO_PKG_VERSION` (was the stale `0.1.0-pr030` literal; now prints the crate version, e.g.
  `prikk 0.2.0`); the `checkout`
  mode-flag error and the `status` diagnostic line no longer reference PR-030; stale `PR-030`-prefixed
  module docs (CLI, store, rollback) reworded to describe current scope. No behavior change beyond the
  version string; PATCH-framing anchors unchanged.

- **4.4b P2-1 — CLI end-to-end genesis harness.** Adds `crates/prikk-cli/tests/genesis_end_to_end.rs`, a
  permanent integration test that drives the compiled `prikk` binary through `init → commit → seal → log →
  verify` on a fresh repository (asserts a two-operation genesis commit, a Root block at `update-seq: 1`,
  and clean verify). Guards the release-facing first-commit flow at the CLI boundary. Test-only; no behavior
  change; PATCH-framing anchors unchanged.

- **4.4b — genesis / first-commit authoring.** Enables `init → commit → seal` on a fresh repository.
  Worktree authoring now resolves its baseline through `resolve_worktree_baseline`: when the target ref is
  **published** it authors against replay-derived node lifecycle state (unchanged); when the ref has
  **never** been published it authors against an empty `NodeLifecycleState::new()` baseline, so every
  worktree file becomes a fresh node-addressed `CreateFile` (canonical order, CSPRNG-minted ids, normalized
  modes, real role-bound Ed25519 AUTHOR signature) — a baseline-selection change only, reusing the entire
  existing signed authoring path (review E3). Seal already publishes the first block as `BlockKind::Root`
  (empty parents, `update_seq = 1`, `previous_ref_state_id = None`); no seal change was needed. Genesis is
  selected **only** when the ref pointer is absent **and** the ref log is readable and empty; a missing
  pointer with any ref-log history — or an unreadable/partial log — is treated as corruption, fails closed,
  and points at `doctor` (never silently re-genesis; design §4, review E2). Genesis additionally requires an
  **empty active WAL** — no records **and** no trailing partial bytes (review E1 + 4.4bR P1b): a second
  `commit` before the first `seal` fails closed ("active WAL already contains patches on an unpublished ref;
  run `prikk seal`…"), and a trailing partial WAL tail fails closed pointing at `doctor --repair-wal-tail`,
  rather than authoring a duplicate or ambiguous genesis patch. Empty worktree, worktree symlinks/non-regular
  files, and invalid/non-UTF-8 paths remain fail-closed (genesis synthesizes no zero-operation patch).
  Genesis is **enforced** to the default `heads/main` ref (review Q2 + 4.4bR P1a): a first commit onto any
  other unpublished ref fails closed pending branch-creation design. The Root block inherits the existing
  `scaffold_state_root` pending the real state-Merkle design (review Q3). The active-WAL guard and the WAL
  append are held in **one critical section under the active-session lock** (4.4bR2): the whole
  `commit_worktree_changes_signed` path acquires `ActiveLock` before the guard and holds it through append,
  so concurrent commits cannot both pass the guard and append (the loser fails via lock conflict or the
  post-lock "seal first" guard). Seven new store tests (all-`CreateFile` genesis + real signature;
  empty-worktree, second-pre-seal-commit, missing-pointer-with-log, non-default-ref, and trailing-partial-WAL
  fail-closed; concurrent-genesis serializes to one WAL record); prikk-store 288→295. Identity-neutral to
  existing objects; PATCH-framing anchors unchanged.

- **4.4a-1 — production `NodeIdGenerator` (CSPRNG node-id minting).** Adds the fail-closed minting
  primitive that future worktree authoring will use to assign fresh node identities. A `NodeId` is an
  opaque 256-bit value drawn from the OS CSPRNG — never derived from path, content, operation
  position, timestamp, counter, or baseline state — because it must survive rename/edit/chmod/binary
  replacement and is part of the text `span_id` preimage. The entropy source and the trusted minter
  are deliberately separate (E1): a `NodeIdEntropySource` produces *candidate bytes* (production:
  `OsEntropySource` over `getrandom`), and `NodeIdGenerator` is the only minting authority — it
  constructs an id only through the canonical checked constructor `NodeId::try_from_bytes` (rejecting
  the reserved all-zero value) and rejects any candidate already in the baseline's complete known-id
  set via `NodeLifecycleState::contains_seen_node_id` (E2, over `seen_ids`). Retry is bounded (E3):
  on an all-zero or colliding draw it redraws exactly once, then fails closed with a structured
  `NodeIdMintError` (E4: `EntropyUnavailable` / `ZeroNodeIdDraw` / `NodeIdCollision`) — no weak/seeded
  fallback, no placeholder, no unbounded loop. **Dependency-map change:** `getrandom` is added to
  `prikk-store` only; `prikk-object` stays pure (no entropy/IO). Covered by seven generator tests
  (E5: deterministic nonzero emission; entropy failure; zero rejected-then-redrawn; repeated zero
  fails closed; baseline collision rejected-then-redrawn; repeated collision fails closed; minted id
  nonzero). Also folds in the 2c-4 carry-#1 `splice_text` invalid-range tests (E6: `start > end` and
  `end > text.len()` reject). Unwired: no command path or worktree authoring consumes the generator
  yet, and the four DEV-only worktree-authoring tests remain ignored pending the 4.4a-2 design pass.
  Identity-neutral; PATCH-framing anchors unchanged.

- **4.4a-2a — node-addressed worktree authoring (content operations).** Re-enables
  `commit_worktree_changes[_with_options]` to author node-addressed §9.3 content operations
  (`CreateFile`, `DeleteNode`, `EditText`, `ReplaceBinary`) against a baseline reconstructed from
  authoritative replay, replacing the prior fail-closed stub. Baseline policy is **Option A**: the
  baseline node lifecycle state comes only from `replay_derived_state` over the ref's node-addressed
  lineage (`resolve_node_lineage_bounds`); the snapshot manifest is never an identity authority, and a
  snapshot-only baseline (empty node state with a `snapshot_blob_ref`) **fails closed** (review E3).
  Existing paths resolve to their persisted `node_id` via the replay-derived `NodeLifecycleState`;
  existing-node `NodeKind` is **authoritative** — a `TextFile` modification authors a whole-file
  `EditText`, a `BinaryFile` modification authors kind-preserving `ReplaceBinary`, and a text↔binary
  transition fails closed (`UnsupportedKindTransition`, review E4). Fresh nodes are minted through the
  production `NodeIdGenerator` in **canonical create order** (candidates sorted by `RepoPath` bytes
  before minting, each inserted into a working `NodeLifecycleState` immediately so same-session draws
  cannot alias), making path→`node_id` assignment independent of filesystem traversal order (review
  E1). Operations are emitted in a **canonical order** (kind rank `DeleteNode` < `CreateFile` <
  `ChangePerm` < `ReplaceBinary` < `EditText`, then `RepoPath` bytes, then `node_id` bytes) before
  `op_seq` assignment, so patch identity does not depend on traversal/map iteration. All `EditText`
  span identity (anchors, `span_id`, splice, derived text blob id) is computed through the shared
  `prikk-store::text_span` module — no authoring-local span logic — so authoring and replay agree
  byte-for-byte (covered by an authoring↔replay symmetry test). Created files record a **normalized**
  canonical mode (4.4a-2aR): any executable bit set → `0o100755`, otherwise `0o100644`; symlink mode
  `0`; non-Unix defaults to `0o100644` (read/write/setuid/sticky bits and platform attributes are
  ignored). **Deferred to 4.4a-2b:** `ChangePerm` / mode-change detection for *existing* nodes (this
  increment preserves a modified file's baseline mode and emits no `ChangePerm`); the normalization
  rule it will reuse is the one ratified and landed here. Rename inference (moves author as
  delete+create) and symlink authoring (fails closed) also remain out of scope. The four previously
  DEV-only worktree-authoring tests are migrated to node-addressed `CreateFile` baselines and
  re-enabled (no `#[ignore]` remain), alongside witnesses for E1/E3/E4, deterministic patch identity,
  deletion, canonical mixed-operation `op_seq` ordering, created-file mode normalization (regular and
  executable), structured error classes, and authoring↔replay symmetry. The 4.4a-1 carry to remove
  `node_id_gen`'s module `#[allow(dead_code)]` is discharged now that the production path consumes the
  generator. Identity-neutral to existing objects; PATCH-framing anchors unchanged.

- **4.4a-2b — `ChangePerm` authoring (existing-node mode-change detection).** Completes node-addressed
  worktree authoring of the §9.3 mutation set by detecting permission changes on existing regular file
  nodes and emitting `ChangePerm`. Detection reuses the single `normalize_file_mode` rule landed in
  4.4a-2aR (no second normalization implementation): the worktree file's normalized canonical mode is
  compared against the replay-derived baseline node mode, and a difference emits exactly one
  `ChangePerm` with `old_mode` = baseline mode and `new_mode` = normalized worktree mode. Mode
  detection is independent of content, so a mode-only change authors a lone `ChangePerm`, while a
  content+mode change authors `ChangePerm` plus the content op; the existing canonical operation sort
  places `ChangePerm` before any `ReplaceBinary`/`EditText` (full kind order `DeleteNode` <
  `CreateFile` < `ChangePerm` < `ReplaceBinary` < `EditText`). Scope stays narrow (no rename
  inference, symlink authoring, or text↔binary transition): symlink nodes never reach mode detection
  (they live in the symlink baseline view and already fail closed), and symlink mode remains
  normatively `0`. New witnesses: mode-only → single `ChangePerm` with correct old/new modes;
  content+mode → `ChangePerm` before `EditText`; the mixed-operation ordering test extended to all
  five kinds (`[Delete, Create, ChangePerm, ReplaceBinary, EditText]`). Stale rustdoc/comments flagged
  in review (snapshot-baseline wording, "rule not yet ratified", `node_id_gen` "unwired") are
  cleaned. Identity-neutral to existing objects; PATCH-framing anchors unchanged.

- **4.4a release-prep (pass 1) — authoring path-handling hardening + threat-model delta.** Worktree
  enumeration now converts OS→repo paths **strictly**: a non-UTF-8 OS path fails closed at the
  conversion boundary (`to_str().ok_or(...)`) instead of being lossily replaced before
  `RepoPath::parse`, so identity-bearing paths never derive from lossy bytes (review N2). Added a
  binary content+mode witness (`ChangePerm` before `ReplaceBinary`, review N1) and a non-UTF-8
  path-rejection test. A threat-model delta for the `worktree → authoring → object-store blobs → WAL
  patch` data flow was produced against FDD-04 v1.3 (no new trust boundary or asset class; existing
  controls cover it; residuals flagged: author signing is a dev placeholder pending real AUTHOR-role
  signing, and symlink boundary-1 wiring when symlink authoring lands). Identity-neutral; PATCH-framing
  anchors unchanged.

- **4.4a R1 — role-bound Ed25519 AUTHOR patch signing.** Closes the release-prep-1 R1 residual: authored
  patches are no longer signed with a development placeholder. Signing goes through an injected
  `AuthorSigner` boundary (`author_signing.rs`): the authoring engine builds the role-bound preimage via
  `Signature::signed_bytes(Ed25519, Patch, <unsigned patch object id>, Author, <caller key id>)` and the
  provider returns the detached signature bytes. The production provider `Ed25519AuthorSigner` produces a
  real Ed25519 signature through `prikk-crypto`; the sole worktree-authoring production entry
  `commit_worktree_changes_signed` requires an injected signer, and tests use an explicit deterministic
  Ed25519 signer. (Scope: this covers the node-addressed worktree/commit AUTHOR path. The seal/publication
  MAINTAINER signing path is a separate role handled in a later phase and is not part of this claim.) A
  verification test proves the authored signature verifies against the signer's public key and fails if
  the object id, signer role, or key id changes (the algorithm negative is vacuous in v1 — `Ed25519` is
  the only `SignatureAlgorithm` — but the algorithm is bound in the preimage). Trust stores, key
  persistence/rotation, and signature policy remain out of scope (later phases). Identity-neutral to
  existing objects; PATCH-framing anchors unchanged.

- **4.4a R1R — remove the broken `commit --allow-empty` scaffold.** The `--allow-empty` empty-commit path
  built a **zero-operation** patch, which canonical encoding rejects ("patch operations must contain at
  least one operation") *before* signing — so it never produced a valid patch, and it was the last
  remaining AUTHOR placeholder-signature production path (`dev_author_signature` in `prikk-cli`). Because a
  zero-operation patch is not representable and cannot be signed, the scaffold could not be converted to
  real signing; it is removed instead (`empty_patch_envelope`, the `--allow-empty` flag, the `CommitMode`
  enum, and the placeholder helper are deleted). `prikk commit` now always authors a node-addressed patch
  from the worktree (`--from-worktree` accepted as a no-op for compatibility) with a real role-bound
  Ed25519 AUTHOR signature. This removes the AUTHOR placeholder from the `commit` path. (An AUTHOR-role
  marker remains on the rollback-draft path and is scoped in R1R2 below; the seal MAINTAINER placeholder is
  unaffected and remains a later-phase item.) Identity-neutral; PATCH-framing anchors unchanged.

- **4.4a R1R2 — rollback-draft AUTHOR signing scoped as internal (non-publishable).** Review R1R found a
  second AUTHOR-role placeholder: `rollback_draft.rs` signs the inverse Patch with
  `dev-placeholder-rollback-author` (a `SignerRole::Author` sha256 marker, not a real Ed25519 signature),
  on the `prikk rollback-draft --append-inverse` production path. Converting it to real AUTHOR signing is
  **design-blocked**: that key is a *load-bearing marker* — `rollback_verify` (`is_rollback_draft_envelope`,
  `verify_rollback_marker`) uses it to distinguish rollback-draft patches from ordinary authored patches in
  the active WAL. Signing with a real key would erase the marker and break rollback verification, and every
  clean replacement (a payload/precondition marker, an intent field — which the design mandates be
  advisory-only — or a WAL-record kind) is an identity-bearing/FDD-level decision, not a signature swap. So
  for this cut the rollback-draft path is **explicitly scoped as an internal development scaffold that is
  not publication-grade authoring**, per the review's accepted fallback. The accurate release-scope claim is
  therefore narrowed: node-addressed **worktree `commit`** patches are role-bound Ed25519 AUTHOR-signed;
  rollback-draft patches are an internal scaffold and are excluded from the publishable-authoring surface, as
  are MAINTAINER publication signing and trust-store enforcement. A proper fix (separate the rollback-draft
  marker from the author signature, then sign with the real key) is deferred to a design pass in the later
  crypto/policy phase. No code identity change; PATCH-framing anchors unchanged.

- **4.4-2c-4 — shared `text_span` module + public §5.1 golden vectors.** Promotes the
  identity-bearing §5.1 text-span primitives out of the replay module into a single shared
  `prikk-store::text_span` module — `TEXT_ANCHOR_WINDOW`, `anchor_hash`/`left_anchor`/`right_anchor`,
  `compute_span_id`, `occurrences`, `locate_text_span`, `text_blob_id`, the `TextSpanResolutionFailure`
  taxonomy, and a new **bounds-checked** `splice_text` (E1) — so authoritative replay and (later)
  worktree authoring compute the full `text → anchor-filtered localization → splice → BlobPayload(Text,
  new_text) id` chain through one implementation and cannot drift. Replay's `apply_edit_text` now calls
  the shared module; no §5.1 primitive remains in `replay.rs`. Lands public golden conformance vectors
  (`text_span/vectors.rs`, FDD-01 §5.1 naming) pinning literal anchor hashes, span ids, localized
  ranges, resulting text, and derived blob ids across boundary clamps, empty/zero-length insertion,
  overlapping occurrences, duplicate-raw/different-anchor and duplicate-anchor-filtered cases, plus
  `AnchorMismatch`/`NoMatchingSpanId` negatives. Pure move: existing replay EditText tests pass
  unchanged against the shared module; PATCH-framing anchors unchanged.
- **4.4-2c-3 — payload-retaining single-read lineage walk (E4).** The shared lineage walk now
  carries an associated `Block` type, so each lineage block is read **once** and the walk returns
  what it read: `ReaderLineage::Block = BlockPayload` (replay applies patches from the retained
  payload, no second `read_block`), `ResolverLineage::Block = Vec<ObjectId>` (provenance maps to ids
  for the window hash). This removes the prior ids-then-re-read double read, closing the file-backed
  concurrent-mutation hazard before any command-path consumer. The single shared walk rule
  (single-parent, cycle, terminus = horizon, apply order) and all acceptance/rejection behavior are
  unchanged. Witnessed by a counting-reader test (one read per lineage block) and a guard that
  panics on any second block read. Identity-neutral; anchors unchanged.
- **4.4-2c-2eR2 — baseline-mismatch classification (review erratum).** `certified_compared_cache`
  now binds the caller's intended baseline explicitly up front and returns
  `CacheCertificationError::BaselineMismatch` directly, symmetric with `HorizonMismatch`, instead of
  letting a caller/cache baseline mismatch fall through the validator as `CacheRejected`. Test
  updated to assert `BaselineMismatch`. Identity bytes unchanged.
- **4.4-2c-2eR — certification errata (review carry).** Folds review errata E1–E3 on the 2c-2e
  producers. E1: cache certification now returns a structured `CacheCertificationError`
  (`BaselineMismatch` / `HorizonMismatch` / `CacheRejected` / `ReplayUnavailable` / `ContentMismatch`)
  instead of flat integrity strings, so a future consumer can branch a droppable cache fault from
  authoritative-history unavailability; a `From<CacheCertificationError> for PrikkError` keeps the
  flattened boundary. E2: `certified_compared_cache` now binds the caller's intended
  `lineage_horizon_id` explicitly (fails closed up front) just as it binds the baseline. E3:
  documents that the compare certifies only the live/tombstone lifecycle state — `snapshot_blob_id`
  is **not** certified and must not back materialization acceleration without its own validation.
  Identity bytes unchanged; producers remain `pub(crate)` and unwired. E4 (double block-read
  stability before mutable file-backed command use) is carried as an explicit pre-command-path gate.
- **4.4-2c-2e — replay-derived state exposure + compared-cache wiring + unified lineage walk.**
  Adds the sanctioned producers `replay_derived_state` (rung 3: authoritative replay wrapped through
  `ReplayDerivedLifecycleState::from_replay`, which validates internal consistency before exposure)
  and `certified_compared_cache` (rung 4: validate → replay → full compare; the only cache-derived
  rung permitted to accelerate identity decisions, and only because it is proven equal to replay —
  never a root of trust). Unifies cache provenance and authoritative replay on a single lineage
  definition: both now walk via the shared `walk_single_parent_chain` over a `LineageBlockReader`
  seam (reader-backed for replay, parent-resolver-backed for provenance), so the two cannot drift on
  which blocks are in the window or in what order. Provenance's "genesis-before-horizon" and
  "horizon-not-genesis" failures now collapse to the single `HorizonNotInLineage` terminus rule
  (behavior identical — both still fail closed; only the message changed). Also folds review
  carry-forward C3 (symmetric saturating `right_anchor` arithmetic). Producers are `pub(crate)` and
  unwired by design.

- **4.4-2c-2d — EditText state effect (forward).** Replay now applies `EditText` exactly:
  materializes the node's current text (lazily, via a new blob-content resolver; cached per pass),
  localizes the span with the FDD-01 §5.1 64-byte anchor-filtered rule, splices in
  `replacement_text`, derives the new `BlobPayload(Text, new_text)` content id, and records it
  (`NodeLifecycleState::set_text_blob`), preserving `node_id`, path, and mode. Adds the structured
  `TextSpanResolutionFailed { node_id, span_id, reason }` class. **All** lifecycle-affecting
  operations now have exact effects — no operation maps to `UnsupportedLifecycleEffect`. The 64-byte
  anchor window is recorded in the FDD-01 §5.1 clarification note. Folds in the E1 carry-forward
  (`ReplaceBinary` old-side blob negatives).
- **4.4-2c-2c — ReplaceBinary state effect.** Replay now applies `ReplaceBinary` exactly: both
  `old_blob_id` and `new_blob_id` are resolved and required to be `BlobKind::Binary` (missing →
  fail-closed, non-binary → inconsistent), the live node must be a `BinaryFile` currently
  referencing `old_blob_id`, and its blob is swapped to `new_blob_id` with mode preserved (new
  `NodeLifecycleState::replace_file_blob`). Only `EditText` now remains fail-closed
  (`UnsupportedLifecycleEffect`).
- **4.4-2c-2bR — DeleteNode/RenamePath persisted old-state assertions.** Exact replay now verifies
  a `DeleteNode` record's full preimage (path, kind, blob/mode or symlink target) and a `RenamePath`
  record's `old_path` against the replayed live node before mutating — via new
  `NodeLifecycleState::delete_node_checked` / `rename_node_checked`. A record whose old-state
  assertion disagrees with replayed reality is rejected (`InconsistentLifecycleEffect`) rather than
  silently tombstoning/renaming from live state. Closes review P1-1/P1-2.
- **4.4-2c-2b — lifecycle state-effect interpreter (Create/CreateSymlink/Delete/Rename/ChangePerm).**
  Replay now applies exact existence/path/kind/mode effects into a `NodeLifecycleState`:
  `CreateFile` (node kind resolved from its blob via the real store-backed resolver — the explicit
  boundary where authoritative store access enters the trust ladder, E1), `CreateSymlink`,
  `DeleteNode` (tombstone recorded from the live node, so it carries post-mutation content/mode per
  O1), `RenamePath` (preserves `node_id`), and `ChangePerm` (new `NodeLifecycleState::change_file_mode`,
  exact mode, old-mode cross-checked). `EditText` and `ReplaceBinary` still fail closed
  (`UnsupportedLifecycleEffect`); node-lifecycle apply failures map to the new
  `InconsistentLifecycleEffect` class. The reconstructed state is still **not** exposed as
  `ReplayDerivedLifecycleState` and consumed by no caller (that is 2c-2e). Adds a malformed/wrong-type
  patch negative (E2).
- **4.4-2c-2a — authoritative lifecycle replay: lineage walker + dispatch skeleton.** Walks the
  v1 single-parent block lineage from a baseline back to a genesis horizon over the real object
  store, failing closed on missing/unreadable blocks, merge windows, cycles, and a genesis that is
  not the claimed horizon. Dispatches each block's patch operations; per the O1 ruling no state
  effect is implemented yet, so every operation fails closed (`UnsupportedLifecycleEffect`) and no
  `ReplayDerivedLifecycleState` is produced. Lands the structured replay error taxonomy (P2-3)
  ahead of any caller branching on it.
- **4.4-2c-1 — store-backed lifecycle resolvers.** Real implementations of the lifecycle-cache
  `BlockParentResolver` and `BlobKindResolver` over the object store (generic over
  `ObjectReader`). Closes P2-1: a missing or unreadable block is an error, never genesis — only
  a decoded `Block` with zero parents is genesis. A missing blob returns the fail-closed
  `Ok(None)` sentinel; a present-but-wrong-type object is an error. No replay, no cache use, no
  identity decision in this increment.

## 0.1.3 — Documentation / release hygiene

Documentation-only release. No source code change; identity anchors unchanged
(empty-PATCH `510ab866…5157`, populated `24031b48…c854`).

- Replaced `README.md` (maintainer-updated).
- Folded the v0.1.2 release-note errata: the four ignored `prikk-store` tests are now
  explained as DEV-ONLY worktree-authoring checkpoint tests, and worktree-authoring re-enable
  is added to the carry-forwards.
- CHANGELOG hygiene: removed a duplicate top heading and consolidated the v0.1.2 sub-slices
  under a single `0.1.2` release heading.

## 0.1.2 — DC-09 Phase 4.3 / 4.4 internal node-model groundwork

Internal/unwired node-model groundwork: store decode-model promotion, the node-lifecycle
substrate, and the lifecycle-cache trust ladder. Not consumed by any command path; identity
anchors unchanged.

### Phase 4.4 step 2b.2R-2 — create_node nonzero guard

Pre-threading substrate hardening from the 2b.2R review (P2): `NodeLifecycleState::create_node`
now rejects the reserved all-zero `node_id` at the central node-introduction boundary,
matching `seed_live_node` / `seed_tombstone`, instead of relying on decode/generator
correctness. Validation-only; both anchors unchanged. Test: `create_node_rejects_all_zero_node_id`
(restoration-equivalent re-create with a nonzero id continues to clear the tombstone).

### Phase 4.4 step 2b.2R — live/tombstone overlap closure

Closes a substrate P1 found in the steps 3–4 review: `NodeLifecycleState` could hold a node
as both live and tombstoned after delete → restoration-equivalent re-create, which violated
the cache's no-overlap invariant and would make replay-and-compare reject a correct post-
restore cache. Model/validation correction only; both anchors unchanged.

- `create_node` now clears any tombstone for the node on a restoration-equivalent
  reintroduction, so live and tombstone sets stay disjoint (no-op for a fresh node_id).
- `NodeLifecycleState::validate_internal_consistency` now rejects any node_id present in both
  the live and tombstone sets.
- `ReplayDerivedLifecycleState::from_replay` now returns `Result` and validates internal
  consistency, so the compared rung cannot certify against a malformed reference state.
- Tests: substrate `create -> delete -> restore` leaves the node live with no overlap and
  passes consistency; a post-restore baseline cache (node live, no tombstone) compares equal
  to the replayed state.

### Phase 4.4 step 2b.2-3/4 — replay-derived + compared rungs

Adds the top trust rungs and the decisive right-provenance/false-tombstone guarantee. Still a
private, unwired slice — no apply/seal/verify path consumes a cache. Additive; both anchors
unchanged.

- **`ReplayDerivedLifecycleState`** — an authoritative replay-derived `NodeLifecycleState`
  bound to a baseline. Must be produced only by authoritative replay over the walked chain;
  the real producer arrives with threading, so this slice constructs it via `from_replay`.
- **`ComparedLifecycleCache`** — a validated cache **proven equal** to authoritative replay
  for the same baseline. `from_validated_and_replay` checks the baseline matches, rebuilds a
  `NodeLifecycleState` from the validated cache, and requires it to equal the replayed state.
  This is the only cache-derived rung that may participate in restoration-equivalence /
  `node_id` reuse decisions once wired — and only because it equals replay.
- **Decisive guarantee:** a cache with correct provenance but false live/tombstone contents
  is rejected — the rebuilt state will not equal the replayed state (test:
  `compared_rejects_false_tombstone`).
- **P2-2 closed:** `ValidatedLifecycleCache::from_decoded_for_baseline` binds a cache to the
  caller's intended baseline, so a cache valid for one checkpoint cannot be accepted where a
  different baseline was meant.
- `NodeLifecycleState` now derives `PartialEq`/`Eq` for the replay comparison.
- Carry-forwards still open: P2-1 (real store resolver must distinguish a missing/unreadable
  block from genesis — applies when the real resolver lands in threading) and P2-3 (structured
  error taxonomy before recovery/doctor branches on classes).

### Phase 4.4 step 2b.2-2 — walked-chain provenance

Makes lifecycle-cache provenance **operational**: the `replay_window_hash` is recomputed
over the actually walked single-parent block chain, never over cache-supplied data. Still a
private, unwired slice — no apply/seal/verify identity decision uses a cache. Additive; both
anchors unchanged.

- **`BlockParentResolver`** — a `block_id -> Vec<ObjectId>` seam (parents in seal order;
  empty at genesis), mirroring `BlobKindResolver`. Keeps the walk testable without a store
  handle; the real `Block`-reading resolver arrives with threading.
- **`DecodedLifecycleCache::verify_window_against_chain`** — walks `baseline_block_id` back
  to `lineage_horizon_id` over single-parent links and recomputes the window hash from the
  walked chain. Fails closed on a merge (multi-parent) block, a cycle, reaching genesis
  before the claimed horizon, a horizon that is not repository genesis (v1 adequate-horizon
  rule), or a hash mismatch.
- **`ValidatedLifecycleCache::from_decoded`** now also runs provenance verification (it takes
  both a blob and a block-parent resolver), so the `Validated` rung means structural +
  operational-provenance + blob-kind verified — design-v3's definition — and cannot exist
  with merely syntactic provenance.
- Tests: matching walked chain accepted; window-hash mismatch, merge block, non-genesis
  horizon, cycle, and genesis-before-horizon each rejected.

### Phase 4.4 step 2b.2-1 — blob-kind verification + Validated rung

Opens 4.4-2b.2 proper with the first blob-kind verification step and the first trust rung.
Still a private, unwired codec/import slice — no apply/seal/verify identity decision uses a
cache. Additive; both anchors unchanged.

- **`BlobKindResolver`** — a small `blob_id -> Option<BlobKind>` trait. `Ok(None)` means the
  blob is absent/unreadable and fails closed. Keeps verification testable without a store
  handle; a real store resolver arrives with the threading slice.
- **`ValidatedLifecycleCache`** — the first trust rung: a `DecodedLifecycleCache` whose every
  file entry's `NodeKind` has been checked against the referenced blob's `BlobKind`, reusing
  the canonical `NodeKind::from_file_blob_kind` rule. `from_decoded` **re-runs structural
  validation itself** (the input is not trusted to have come from `decode`, since fields are
  `pub(crate)`), then verifies blob kinds; a missing blob, a kind disagreement, a `SNAPSHOT`
  blob, or a resolver error fails closed. It is documented and structured as **not authority**
  for `node_id` reuse or restoration-equivalence — there is no method that yields such a
  decision; those wait for the replay-derived / replay-compared rungs.
- Tests: structural-invalid input rejected even when blob kinds resolve; Text and Binary
  matches accepted; Text↔Binary disagreement rejected; `SNAPSHOT` blob rejected; missing blob
  rejected; tombstone blob-kind mismatch rejected; resolver error propagated fail-closed.
- Review follow-ups: added explicit tombstone kind/content production-encode negatives (N1);
  verified `read_enum_u16` guards the wire type exactly once — the apparent double was two
  distinct call sites (node_kind tag 3, parent_policy tag 4), nothing to remove (N2).

### Phase 4.4 step 2b.2 — lifecycle cache codec hardening

Corrective patch opening 4.4-2b.2, closing the 4.4-2b.1 review errata. Validation-only;
no new data, no wiring into replay; both anchors unchanged.

- **P1 — production `encode()` validates before writing.** `encode()` now runs the same
  structural/cross-set `validate()` as `decode()` before serializing, so an internal
  caller cannot persist a cache the importer would later reject. `validate()` is now
  **structurally equivalent** to the decode path: beyond schema/policy/sorting/uniqueness
  and `seen_ids == live ∪ tombstoned`, it rejects the reserved all-zero `node_id` in live,
  tombstone, and `seen_ids` sets and rejects any kind/content discriminator mismatch,
  reusing the substrate's `ensure_node_id_nonzero` and `validate_kind_content_shape`
  (promoted to `pub(crate)`) rather than a parallel rule. The raw serializer is private and
  reachable in production only through the validated `encode`; a `#[cfg(test)]`
  `encode_unchecked` is used to craft malformed fixtures for decode negatives. Production
  encode is proven to reject unsorted live entries, a `seen_ids` mismatch, merge policy,
  all-zero ids (live/tombstone/seen), and file↔symlink kind/content mismatches.
- **P2-1 — non-canonical TLV tag order rejected.** Decode now requires nondecreasing field
  tags at both the top level and inside each node record (repeated tag 10/11 entries still
  allowed in-region), so a persisted cache has one canonical byte form. Tests cover a
  header field and a node-record field presented out of order.
- **P2-2 (error taxonomy)** remains a message-class mapping for now, per the review — to be
  promoted to a structured class before any recovery/doctor path depends on the outcomes.

### Phase 4.4 step 2b.1 — lifecycle cache codec

Adds the persisted lifecycle-cache wire format and its decoder/importer
(`lifecycle_cache`), a derived, rebuildable accelerator for `NodeLifecycleState`. Per
design v3 §0 the decoded value is **not validation authority**: it cannot seed a
`node_id`-reuse decision. Additive and identity-neutral; not wired into replay; both
anchors unchanged.

- `DecodedLifecycleCache::{encode, decode}` over `PRIKK-NODE-LIFECYCLE-CACHE-v1\0` magic
  plus canonical `FieldRecord` TLV. Wrong/short magic is rejected before TLV decode;
  repeated live/tombstone entries use `record_list_item` (`0x21`) and an entry sent as a
  plain `record` (`0x20`) is rejected.
- Fail-closed structural + cross-set validation: unknown top-level/nested tags, duplicate
  singleton fields, file/symlink discriminator (files require `blob_id`+`normalized_mode`
  and forbid a target; symlinks require a target and forbid `blob_id`/field 5 even when
  zero), live entries strictly sorted by canonical `repo_path` with unique path and id,
  tombstones strictly sorted by raw `node_id`, `seen_ids` a multiple of 32 / strictly
  ascending / nonzero, no id both live and tombstoned, and `seen_ids == live ∪ tombstoned`.
- `compute_window_hash` fixes the exact `replay_window_hash` preimage
  (`PRIKK-LIFECYCLE-CACHE-WINDOW-v1 || u64be(count) || raw32(block_id)…`): deterministic,
  count-bearing, order-sensitive, domain-separated.
- Blob-kind verification, provenance-vs-baseline staleness, replay reconstruction, and
  replay-and-compare are deferred to the next slice; no `ValidatedLifecycleCache` /
  `ReplayDerivedLifecycleState` / `ComparedLifecycleCache` ladder is exposed yet, so no
  type here can be mistaken for replay-derived authority.

### Phase 4.4 step 2a — baseline seeding substrate

Adds the baseline-seeding API to `NodeLifecycleState` and closes the substrate-level
4.4-2 errata, so a baseline cache cannot inject node state an operation could not.
Additive and identity-neutral; both anchors unchanged.

- `seed_live_node` / `seed_tombstone` seed the live clean tree and the non-live
  lifecycle history (`seen_ids` + `latest_tombstone_by_id`) needed for
  restoration-equivalence across a snapshot boundary. Both reject the reserved
  all-zero `node_id` (erratum P1-3), validate the kind/content discriminator through a
  shared `validate_kind_content_shape` (erratum P2-2), and reject duplicate live ids,
  duplicate live paths, and tombstones for currently-live nodes.
- `validate_internal_consistency` now also requires every live and every tombstoned
  `node_id` to be recorded as seen (erratum P1-4, whole-state check).
- `rename_node` gains the same path-index lockstep guard as `delete_node` (erratum
  P2-1), failing closed rather than silently healing a desynchronised index.
- Tests raised to 24: cross-boundary restoration-equivalence accept and non-equivalent
  reject (the identity-resurrection case), all-zero seed rejection, duplicate id/path
  rejection, tombstone-for-live rejection, and seed kind/content rejection.
- Deferred to the next slice (cache format + threading): cache provenance/staleness
  binding (P1-1), the materialization-bytes vs lifecycle-identity payload split (P1-2),
  the symlink `normalized_mode == 0` parse check, and threading `NodeLifecycleState`
  through replay/inverse/rollback.

### Phase 4.4 step 1 — node lifecycle substrate

Introduces the node-aware replay substrate. Additive and identity-neutral: a new
isolated module with no changes to any object/payload/encode path; both identity
anchors are unchanged.

- Added `prikk-store::node_lifecycle`: a derived, rebuildable `NodeLifecycleState`
  (`live_by_id` / `path_to_id` / `latest_tombstone_by_id` / `seen_ids`) that is
  explicitly **not a root of trust** (FDD-02 §12). It centralises the node rules so
  replay/inverse/rollback cannot diverge on them: per-`CleanTree` live-node
  uniqueness, rejection of currently-live `node_id` reuse, restoration-equivalence of
  a non-live reintroduced `node_id` to its latest deletion preimage (kind, content
  payload, mode, path — non-liveness necessary but not sufficient, DC-09a §4), and
  `node_id` preservation across rename.
- Review errata: `create_node` fails closed on an inconsistent kind/content
  discriminator (symlink-as-file or file-as-symlink); the path index is keyed by the
  canonical `RepoPath`; `delete_node` enforces `live_by_id`/`path_to_id` lockstep; and
  a `validate_internal_consistency()` helper checks the live-node bijection (for
  assertions and the 4.6 deep-verify validator).
- 17 unit tests covering uniqueness, live-reuse rejection, restoration-equivalence
  (file accept plus blob/mode/path/kind-mismatch rejects; symlink target match and
  mismatch), kind/content discriminator rejection, rename id-preservation and
  occupied-target rejection, non-live delete/rename rejection, and the consistency
  helper.
- The module is `dead_code`-allowed at declaration: it is threaded into the replay
  pipeline in the next 4.4 step (which first settles how the clean-tree baseline
  carries node identity).

### Phase 4.3 — store decode-model promotion

Promotes the store patch decoder from a two-variant, path-keyed supported subset
into a typed node-addressed stream over all seven FDD-03 §9.3 operation kinds.
Identity-neutral: the empty-PATCH anchor and populated framing vector are unchanged.
Applies design-review errata P1 (decode success must not imply apply support) and P2
(retain validated `op_seq`).

- Replaced `SupportedPatchOperation` with `DecodedPatchOperation { op_seq, kind }` and a
  seven-variant `DecodedOperationKind` (plus a discriminated `DecodedDeletePreimage`),
  and renamed `decode_supported_patch_operations` -> `decode_patch_operations`. Every
  well-formed §9.3 kind now decodes into its typed variant; symlink `DeleteNode` and the
  four other node-addressed kinds are no longer rejected at decode time.
- Added `ensure_apply_supported` as the single apply-support gate (erratum P1): decode
  is structural, applicability is a separate decision. Audited all callers — `patch_replay`
  apply and `patch_inverse` derivation gate before matching; `rollback_verify` now gates
  each decoded operation explicitly rather than relying on decode success to prove
  replayability.
- Retained validated `op_seq` in the decoded wrapper (erratum P2).
- Migrated decode tests: malformed/oneof/all-zero-`node_id`/wrong-wire negatives remain
  decode errors; each of the seven well-formed kinds asserts its typed decoded variant
  **and full decoded field values** (review erratum E1, so 4.4 application can depend on
  them), and the not-yet-wired kinds assert `UnsupportedObjectType` at the apply gate.

## 0.1.1 Housekeeping

Repository structure and developer-ergonomics pass. No identity-byte or behavior
changes; the empty-PATCH anchor and populated framing vector are unchanged.

- Relocated `prikk-store` unit tests from the central `src/tests/` directory to the
  project-standard co-located layout: `src/<module>/tests.rs` (and
  `src/patch_replay/tests/` for the three patch-decode test modules). Shared fixtures
  and cross-module harnesses moved to a single `src/test_support.rs`.
- Added `rfcs/proposed/` with a node-model plan RFC capturing the deferred
  application work (4.3–4.6) and the tracked carry-forward items (symlink target
  validator, duplicate scalar-field rejection, preconditions).
- Aligned the workspace `Cargo.toml` version (`0.1.0` -> `0.1.1`) with the active
  CHANGELOG line.
- Made the worktree-patch test module pass the CI clippy gate
  (`cargo clippy --workspace --all-targets -- -D warnings`): targeted
  `#[allow(clippy::indexing_slicing)]` on the four DEV-ONLY authoring-checkpoint tests
  (deliberate `Vec` indexing in assertions) and removed a needless borrow on a
  byte-slice literal.

## 0.1.0 DC-09 Phase 4.2

Operation-record identity reconciliation to FDD-03 §9.3 (code reconciliation effort,
architect-ratified across increments 4.2a–4.2e). Identity/read-validation surface
only; application of node-addressed operations is deferred to the node model.

- Reconciled all seven operation-kind payloads to their FDD-03 §9.3 records: `CreateFile`,
  `DeleteNode` (was `DeleteFile`), `EditText` (node-addressed, span-anchored, 9-field),
  `ReplaceBinary` (node-addressed), `RenamePath`, `ChangePerm`, `CreateSymlink` — all
  node-bearing records reject an all-zero `node_id` on encode and decode.
- Enforced the FDD-03 §9.2 operation-kind oneof on the read path (a record with more
  than one kind field is rejected as malformed, not decoded last-wins).
- Enforced the FDD-03 §9.2.1 `op_seq` canonical invariant on the read path
  (one-based, contiguous, unique, physical order == ascending `op_seq`).
- Added the `ReplaceBinary` binary-only blob-kind enforcement primitive
  (`ensure_blob_kind_is_binary`); wiring into real application is deferred to the node model.
- Retired the pre-FDD full-file `EditText` apply/inverse path and its worktree generation.
- **Worktree patch authoring (`commit --from-worktree`) is fail-closed in this snapshot**
  for create/delete/modify/text changes: every §9.3 mutation operation is node-addressed
  and requires node-id tracking and minting (deferred to increments 4.4/4.4a). This release
  does not support worktree authoring; replay of node-addressed operations is likewise deferred.
- Byte-level `(tag, value_type)` layout tests and read-side validator negatives added for
  every operation record; empty-PATCH anchor and the populated framing vector held throughout.

## 0.1.0 PR-030

Sealed rollback block/history classification after normal seal.

- Added sealed rollback block classification after rollback drafts are sealed by the existing seal path.
- `load_ref_history()` now reports `rollback_patch_count` and `is_rollback_block` for each history entry.
- `prikk log` now displays rollback block classification for sealed history entries.
- `verify_repository()` now counts sealed rollback blocks and sealed rollback-marked Patch objects.
- `prikk verify` now displays sealed rollback block and rollback Patch counts in addition to active rollback draft WAL records.
- Shared rollback Patch payload validation between active WAL verification and sealed Block/history classification.
- Fixed an obvious duplicate-parameter transcription defect in inverse planning source while touching rollback-adjacent code.
- Kept rollback-specific ref publication, rollback authorization, worktree rollback writes, arbitrary-span rollback, commutation, confluence, audit plugins, and sync deferred.

## 0.1.0 PR-029

Active rollback draft verification before seal.

- Added active rollback draft verification for the supported patch-operation subset.
- Added `verify_active_rollback_draft()` and `RollbackDraftVerification`.
- Added CLI command `prikk rollback-draft-verify [path] [--ref REF]`.
- Rollback drafts now use a dedicated development signature marker key: `dev-placeholder-rollback-author`.
- Repository verification now counts rollback draft WAL records and validates that rollback draft payloads decode under the supported replay subset.
- Kept seal publication, rollback refs, rollback authorization, worktree mutation, arbitrary-span rollback, commutation, confluence, audit plugins, and sync deferred.

## 0.1.0 PR-028

Conservative rollback draft append to an empty active WAL.

- Added conservative rollback draft append for the supported patch-operation subset.
- Added `append_rollback_draft()` and `RollbackDraftReport`.
- Added CLI command `prikk rollback-draft --append-inverse [path] [--ref REF] -m <message>`.
- Requires an explicit `--append-inverse` flag, a non-empty message, an empty active WAL, and no partial WAL tail.
- Appends a signed inverse Patch envelope to the active WAL only; ref publication remains the existing `seal --allow-no-audit` path.
- Kept rollback ref policy, authorization, worktree mutation, arbitrary-span rollback, commutation, confluence, audit plugins, and sync deferred.

## 0.1.0 PR-027

Non-mutating rollback preview for the supported patch-operation subset.

- Added non-mutating rollback preview for the supported patch-operation subset.
- Added `prepare_rollback_preview()` and `RollbackPreviewPlan`.
- Added CLI command `prikk rollback-preview [path] [--ref REF]`.
- Combines unsigned inverse planning with supported patch replay validation.
- Compares the current replayed target state with the latest snapshot baseline and reports `would-create`, `would-delete`, and `would-replace` file-level changes.
- Kept rollback refs, authorization policy, worktree writes, commutation, confluence, arbitrary-span rollback, audit plugins, and sync deferred.

## 0.1.0 PR-026

Read-only inverse planning for the supported patch-operation subset.

- Added read-only inverse planning for the supported patch-operation subset.
- Added `prepare_patch_inverse_plan()` and `PatchInversePlan`.
- Added CLI command `prikk inverse-plan [path] [--ref REF]`.
- Derives unsigned inverse Patch payloads for supported `CreateFile`, `DeleteFile`, `ReplaceBinary`, and full-file `EditText` operations.
- Reports an unsigned inverse Patch ID hint without writing or publishing it.
- Kept rollback refs, authorization policy, conflict witnesses, commutation, confluence, arbitrary-span inverse handling, audit plugins, and sync deferred.

## 0.1.0 PR-025

Opt-in full-file `EditText` generation from UTF-8 worktree modifications.

- Added opt-in full-file `EditText` generation from snapshot-baseline worktree modifications.
- Added `WorktreePatchCommitOptions` and `commit_worktree_changes_with_options()`.
- Added CLI support for `prikk commit --from-worktree --text-edits -m <message>`.
- Kept default `commit --from-worktree` behavior compatible: modified tracked files still emit `ReplaceBinary` unless text mode is requested.
- Text mode emits `EditText` only when both baseline and current file bytes are valid UTF-8; binary or invalid UTF-8 modifications fall back to `ReplaceBinary`.
- Added worktree patch tests for text edit emission and binary fallback.
- Kept arbitrary span discovery, text diff minimization, inverse, commutation, conflict witnesses, audit plugins, and sync deferred.

## 0.1.0 PR-024

Conservative full-file `EditText` replay for exact-span replacements.

- Added conservative `EditText` replay for full-file exact-span replacements.
- Added canonical decode support for `EditText` patch operations in the supported patch replay decoder.
- Added `full-file` anchor replay validation: current file bytes must be valid UTF-8 and must hash to the recorded `old_span_hash`.
- Split supported patch-operation decoding into `patch_replay/decode.rs` to keep the replay module within the project file-size guidance.
- Added a patch replay test for full-file text edit replay.
- Kept worktree text diff generation, arbitrary span discovery, inverse, commutation, conflict witnesses, audit plugins, and sync deferred.

## 0.1.0 PR-023

Explicit patch deletion planning and opt-in removal of files deleted by supported patch replay.

- Added a content-anchored text edit payload validation scaffold.
- Added fixed `TEXT_SPAN_HASH_BYTES = 32` and `text_span_hash(bytes)`.
- Added `validate_text_anchor_id()` for v1 anchor identifier validation.
- Changed `EditText.old_span_hash` to a fixed 32-byte value.
- Added tests for anchor validation, stable span hashing, and invalid anchor rejection.
- Fixed a replay-source transcription defect in the supported `ReplaceBinary` branch.
- Kept worktree text diff generation, text replay, inverse, commutation, conflict witnesses, audit plugins, and sync deferred.

## 0.1.0 PR-022

Explicit patch deletion planning and opt-in removal of files deleted by supported patch replay.

- Added read-only explicit deletion planning via `prikk checkout --patch-delete-plan`.
- Added opt-in deletion during supported patch materialization via `prikk checkout --patch-materialize-delete`.
- Deletion is limited to files explicitly removed by replayed `DeleteFile` operations.
- Deletion is refused unless the current worktree file bytes still match the operation's old Blob bytes.
- Arbitrary untracked files and modified deleted files are never removed.
- Added deletion planning/materialization tests and documentation.
- Kept general destructive pruning, text edits, renames, chmod, symlinks, merge/conflict algebra, audit plugins, and sync deferred.

## 0.1.0 PR-021

Opt-in supported patch replay materialization without destructive removals.

- Added opt-in supported patch replay materialization via `prikk checkout --patch-materialize`.
- Added `materialize_patch_checkout()` and `PatchMaterializationReport`.
- Reuses the PR-020 supported replay subset: `CreateFile`, `DeleteFile`, and `ReplaceBinary`.
- Writes only validated replay-result files through the same conservative materializer used by snapshot checkout.
- Refuses conflicting existing files and never deletes extra worktree files.
- Keeps destructive removal, content-anchored text edit replay, renames, chmod, symlinks, merge/conflict algebra, audit plugins, and sync deferred.

## 0.1.0 PR-020

Minimal worktree-to-patch draft generation for missing, modified, and untracked files, still without patch replay or full algebra.

- Added read-only supported patch replay planning via `prikk checkout --patch-plan`.
- Added `prepare_patch_replay_plan()` and `PatchReplayPlan`.
- Replays single-parent block chains from oldest to newest.
- Loads snapshot Blob baselines and applies supported `CreateFile`, `DeleteFile`, and `ReplaceBinary` operations.
- Verifies `old_blob_id` preconditions for delete/replace operations.
- Keeps text-span edits, renames, chmod, symlinks, merge/conflict algebra, and worktree writes deferred.

## 0.1.0 PR-019

Minimal worktree-to-patch draft generation for missing, modified, and untracked files, still without patch replay or full algebra.

- Added minimal worktree-to-patch draft generation from snapshot-baseline changes.
- Added `prikk commit --from-worktree -m <message>`.
- Emits file-level `CreateFile`, `DeleteFile`, and `ReplaceBinary` operations only.
- Writes Blob objects referenced by generated operations before appending the Patch envelope to WAL.
- Keeps rename detection, content-anchored text-span edits, patch replay, audit plugins, and sync deferred.

## Earlier PRs

See `rfcs/IMPLEMENTATION-STATUS.md` and `rfcs/done/PR-*-HANDOFF.md` for earlier implementation history.
